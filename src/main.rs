use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, Write};

const PROTOCOL_VERSION: u16 = 1;
const MAX_LINE_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
struct Request {
    protocol_version: u16,
    correlation_id: String,
    #[serde(flatten)]
    operation: Operation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Operation {
    Negotiate {
        supported_versions: Vec<u16>,
    },
    Doctor,
    AuthorizeToolCall {
        tool_name: String,
        input: Value,
    },
    ResolveRole {
        role: Role,
        catalog: Vec<ModelCapability>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Role {
    BoundedHelper,
    SubstantiveWorker,
    IndependentReviewer,
    Verifier,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ModelCapability {
    provider: String,
    model: String,
    reasoning: bool,
    input: Vec<String>,
    authenticated: bool,
}

#[derive(Debug, Serialize)]
struct Response<T: Serialize> {
    protocol_version: u16,
    correlation_id: String,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ProtocolError>,
}

#[derive(Debug, Serialize)]
struct ProtocolError {
    code: &'static str,
    class: &'static str,
    message: String,
    retryable: bool,
}

fn reject<T: Serialize>(
    id: String,
    code: &'static str,
    class: &'static str,
    message: impl Into<String>,
) -> Response<T> {
    Response {
        protocol_version: PROTOCOL_VERSION,
        correlation_id: id,
        outcome: "error",
        result: None,
        error: Some(ProtocolError {
            code,
            class,
            message: message.into(),
            retryable: false,
        }),
    }
}

fn handle(request: Request) -> Response<Value> {
    let id = request.correlation_id;
    if request.protocol_version != PROTOCOL_VERSION {
        return reject(
            id,
            "protocol.incompatible",
            "configuration",
            format!(
                "protocol {} is unsupported; required {PROTOCOL_VERSION}",
                request.protocol_version
            ),
        );
    }
    let result = match request.operation {
        Operation::Negotiate { supported_versions } => {
            if !supported_versions.contains(&PROTOCOL_VERSION) {
                return reject(
                    id,
                    "protocol.no_common_version",
                    "configuration",
                    "no common protocol version",
                );
            }
            serde_json::json!({"selected_version": PROTOCOL_VERSION, "executable": env!("CARGO_PKG_NAME"), "version": env!("CARGO_PKG_VERSION"), "max_line_bytes": MAX_LINE_BYTES})
        }
        Operation::Doctor => {
            serde_json::json!({"status": "ok", "executable": env!("CARGO_PKG_NAME"), "version": env!("CARGO_PKG_VERSION"), "protocol_version": PROTOCOL_VERSION})
        }
        Operation::AuthorizeToolCall { tool_name, input } => {
            let command = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let delivery_command = tool_name == "bash"
                && ["git commit", "git push"]
                    .iter()
                    .any(|needle| command.contains(needle));
            serde_json::json!({
                "authorized": !delivery_command,
                "reason": delivery_command.then_some("Direct commit/push is blocked until a Rust-authorized delivery workflow supplies current verification evidence."),
                "policy": "delivery-gate-v1"
            })
        }
        Operation::ResolveRole { role, catalog } => {
            let selected = catalog.into_iter().find(|model| {
                model.authenticated
                    && match role {
                        Role::BoundedHelper => true,
                        Role::SubstantiveWorker | Role::IndependentReviewer | Role::Verifier => {
                            model.reasoning
                        }
                    }
            });
            let Some(selected) = selected else {
                return reject(
                    id,
                    "routing.no_compatible_model",
                    "domain_rejection",
                    format!("no authenticated compatible model for role {role:?}"),
                );
            };
            serde_json::json!({"role": role, "selection": selected, "fallback_used": false, "policy": "first-compatible-v1"})
        }
    };
    Response {
        protocol_version: PROTOCOL_VERSION,
        correlation_id: id,
        outcome: "ok",
        result: Some(result),
        error: None,
    }
}

fn service_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let response = if line.len() > MAX_LINE_BYTES {
            reject::<Value>(
                "unknown".into(),
                "protocol.request_too_large",
                "configuration",
                format!("request exceeds {MAX_LINE_BYTES} bytes"),
            )
        } else {
            match serde_json::from_str::<Request>(&line) {
                Ok(request) => handle(request),
                Err(error) => reject(
                    "unknown".into(),
                    "protocol.malformed_request",
                    "configuration",
                    format!("malformed request: {error}"),
                ),
            }
        };
        serde_json::to_writer(&mut stdout, &response)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [service, transport] if service == "service" && transport == "stdio" => service_stdio(),
        [doctor] if doctor == "doctor" => {
            println!(
                "tiber {} protocol {}",
                env!("CARGO_PKG_VERSION"),
                PROTOCOL_VERSION
            );
            Ok(())
        }
        _ => {
            eprintln!("usage: tiber service stdio | doctor");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("tiber: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: Operation) -> Request {
        Request {
            protocol_version: 1,
            correlation_id: "test-1".into(),
            operation,
        }
    }

    #[test]
    fn blocks_direct_delivery_commands() {
        let response = handle(request(Operation::AuthorizeToolCall {
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "git push origin main"}),
        }));
        assert_eq!(response.result.unwrap()["authorized"], false);
    }

    #[test]
    fn strong_roles_require_reasoning_capability() {
        let response = handle(request(Operation::ResolveRole {
            role: Role::Verifier,
            catalog: vec![ModelCapability {
                provider: "local".into(),
                model: "small".into(),
                reasoning: false,
                input: vec!["text".into()],
                authenticated: true,
            }],
        }));
        assert_eq!(response.error.unwrap().code, "routing.no_compatible_model");
    }
}
