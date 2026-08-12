use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

pub fn codex_sandbox_setup() -> String {
    [
        "Tiber Codex sandbox setup preview",
        "",
        "Prefer the narrowest approval that can retry the same structured Tiber MCP operation.",
        "Do not run the whole Tiber MCP server outside the sandbox unless these narrow permissions are insufficient.",
        "",
        "If a signed commit fails with `Couldn't get agent socket?`, first verify the active Tiber MCP registration forwards SSH_AUTH_SOCK.",
        "Codex plugin MCP policy overlays do not change transport env; the plugin .mcp.json must declare env_vars = [\"SSH_AUTH_SOCK\"], or an already-installed plugin must be replaced by an equivalent top-level [mcp_servers.tiber] registration that preserves the absolute installed launcher and includes env_vars = [\"SSH_AUTH_SOCK\"].",
        "Never forward SSH_AUTH_SOCK to a PATH-resolved, repo-relative, or otherwise project-controlled MCP launcher.",
        "Do not try to fix SSH signing by enabling danger-full-access, unsandboxing the whole MCP server, or approving every git command.",
        "",
        "Request these approvals only when a Tiber MCP write/sync fails because Git cannot write refs, objects, signed commits, or push credentials from the sandbox:",
        "- case-by-case approval for prefix_rule [\"git\", \"hash-object\"] because it can write arbitrary host-readable file contents into Git objects",
        "- case-by-case approval for prefix_rule [\"git\", \"mktree\"] because it can construct arbitrary Git trees from stdin",
        "- case-by-case approval for prefix_rule [\"git\", \"commit-tree\"] because it can create commits, including signed commit-tree -S when commit.gpgsign=true",
        "- case-by-case approval for Tiber MCP calls that publish event transactions to origin/tiber",
        "",
        "Persist approval only when the harness can scope it to the exact Tiber-internal operation, not merely to a raw git prefix.",
        "Never persist a raw git, wildcard git, bash, sh, or whole-MCP-server permission for Tiber recovery.",
        "",
        "After the user approves the needed narrow permissions, retry the same structured Tiber MCP operation.",
        "Do not ask the user to rerun an equivalent tiber CLI command manually as the normal recovery path.",
        "",
    ]
    .join("\n")
}

pub fn run_stdio(input: impl BufRead, mut output: impl Write) -> Result<(), tiber_git::Error> {
    let fallback_session = format!(
        "tiber-mcp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut repository_root = None;
    for line in input.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                writeln!(
                    output,
                    "{}",
                    error_response(Value::Null, -32603, &error.to_string())
                )?;
                output.flush()?;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Value>(&line) {
            Ok(request) => request,
            Err(error) => {
                writeln!(
                    output,
                    "{}",
                    error_response(Value::Null, -32700, &format!("json_parse source={error}"))
                )?;
                output.flush()?;
                continue;
            }
        };
        if request.get("id").is_none() {
            continue;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let operation_result = match codex_sandbox_cwd(&request) {
            Ok(root) => {
                if root.is_some() {
                    repository_root = root;
                }
                tiber_git::with_mcp_ci_recovery_session(&fallback_session, || {
                    tiber_git::with_mcp_repository_root(repository_root.as_deref(), || {
                        handle_json_rpc(&request)
                    })
                })
            }
            Err(error) => Err(error),
        };
        let response = match operation_result {
            Ok(response) => response,
            Err(error) => {
                let message = error.to_string();
                if let Some(blocker) = error.workflow_blocker_data() {
                    blocking_error_response(id, &message, blocker)
                } else {
                    error_response(id, -32603, &message)
                }
            }
        };
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}

pub fn handle_json_rpc(request: &Value) -> Result<Value, tiber_git::Error> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Ok(error_response(id, -32600, "mcp_method_missing=true"));
    };

    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "tools": {},
                "resources": {},
                "experimental": {
                    "codex/sandbox-state-meta": {}
                }
            },
            "serverInfo": {
                "name": "tiber",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": "Mutating task tools publish an EventCore transaction to origin/tiber on success. For Codex sandbox write failures, call tiber.codex_sandbox_setup or read tasks://codex-sandbox before retrying the same structured Tiber MCP operation. Use case-by-case approval for raw Git prefixes; persist approval only when the harness can scope it to the exact Tiber-internal operation. Do not run the whole Tiber MCP server outside the sandbox unless narrow Git permissions are insufficient."
        }),
        "tools/list" => json!({ "tools": tools() }),
        "resources/list" => json!({ "resources": resources()? }),
        "tools/call" => {
            let Some(name) = request.pointer("/params/name").and_then(Value::as_str) else {
                return Ok(error_response(id, -32602, "mcp_tool_name_missing=true"));
            };
            let arguments = request
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            call_tool(name, &arguments)?
        }
        "resources/read" => {
            let Some(uri) = request.pointer("/params/uri").and_then(Value::as_str) else {
                return Ok(error_response(id, -32602, "mcp_resource_uri_missing=true"));
            };
            json!({
                "contents": [
                    {
                        "uri": uri,
                        "mimeType": "text/markdown",
                        "text": read_resource(uri)?
                    }
                ]
            })
        }
        _ => {
            return Ok(error_response(
                id,
                -32601,
                &format!("unsupported method: {method}"),
            ))
        }
    };

    Ok(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
}

fn codex_sandbox_cwd(request: &Value) -> Result<Option<PathBuf>, tiber_git::Error> {
    let Some(value) = request
        .pointer("/params/_meta/codex~1sandbox-state-meta/sandboxCwd")
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let encoded_path = value.strip_prefix("file://").ok_or_else(|| {
        tiber_git::Error::Parse(
            "mcp_sandbox_cwd_invalid reason=local_file_uri_required".to_string(),
        )
    })?;
    if !encoded_path.starts_with('/') {
        return Err(tiber_git::Error::Parse(
            "mcp_sandbox_cwd_invalid reason=local_file_uri_required".to_string(),
        ));
    }
    let path = percent_encoding::percent_decode_str(encoded_path)
        .decode_utf8()
        .map_err(|error| {
            tiber_git::Error::Parse(format!(
                "mcp_sandbox_cwd_invalid reason=utf8_path_required source={error}"
            ))
        })?;
    Ok(Some(PathBuf::from(path.as_ref())))
}

fn call_tool(name: &str, arguments: &Value) -> Result<Value, tiber_git::Error> {
    validate_ci_recovery_arguments(name, arguments)?;
    match name {
        "tiber.init" => {
            tiber_git::init_repository()?;
            Ok(text_content("initialized tiber".to_string()))
        }
        "tiber.sync" => {
            tiber_git::sync_repository()?;
            Ok(text_content("synced tiber".to_string()))
        }
        "tiber.codex_sandbox_setup" => Ok(text_content(codex_sandbox_setup())),
        "tiber.create" => {
            let title = arguments
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    tiber_git::Error::Parse("mcp_tool_title_missing=true".to_string())
                })?;
            let created = tiber_git::create_task(title)?;
            Ok(text_content(format!("created {}", created.path)))
        }
        "tiber.list" => {
            let tasks = match optional_string_checked(arguments, "status")? {
                Some(status) => tiber_git::list_tasks_by_status(status)?,
                None => tiber_git::list_tasks()?,
            };
            Ok(text_content(
                tasks
                    .into_iter()
                    .map(|task| format!("{}\t{}\n", task.path, task.title))
                    .collect::<String>(),
            ))
        }
        "tiber.search" => {
            let results = serde_json::to_value(tiber_git::search_tasks(required_string(
                arguments, "query",
            )?)?)
            .map_err(|error| {
                tiber_git::Error::Parse(format!("search_json_invalid source={error}"))
            })?;
            Ok(search_content(results))
        }
        "tiber.show" => Ok(text_content(tiber_git::show_task(required_string(
            arguments, "ref",
        )?)?)),
        "tiber.metadata" => {
            let metadata = tiber_git::task_metadata(required_string(arguments, "ref")?)?;
            Ok(text_content(format!(
                "{}\t{}\tcommitted_at={}\n",
                metadata.path,
                metadata.title,
                metadata
                    .committed_at
                    .unwrap_or_else(|| "uncommitted".to_string())
            )))
        }
        "tiber.next" => Ok(text_content(
            tiber_git::next_task()?
                .map(|task| format!("{}\t{}\n", task.path, task.title))
                .unwrap_or_default(),
        )),
        "tiber.transition" => {
            let task_ref = required_string(arguments, "ref")?;
            let status = required_string(arguments, "status")?;
            let transitioned = tiber_git::transition_task(task_ref, status)?;
            Ok(text_content(format!("transitioned {}", transitioned.path)))
        }
        "tiber.prioritize" => {
            let task_ref = required_string(arguments, "ref")?;
            let before_ref = required_string(arguments, "before")?;
            tiber_git::prioritize_before(task_ref, before_ref)?;
            Ok(text_content(format!(
                "prioritized {task_ref} before {before_ref}"
            )))
        }
        "tiber.link" => {
            let from_ref = required_string(arguments, "from")?;
            let to_ref = required_string(arguments, "to")?;
            tiber_git::link_blocks(from_ref, to_ref)?;
            Ok(text_content(format!("linked {from_ref} blocks {to_ref}")))
        }
        "tiber.unlink" => {
            let from_ref = required_string(arguments, "from")?;
            let to_ref = required_string(arguments, "to")?;
            tiber_git::unlink_blocks(from_ref, to_ref)?;
            Ok(text_content(format!("unlinked {from_ref} blocks {to_ref}")))
        }
        "tiber.subtask.add" => {
            let task_ref = required_string(arguments, "ref")?;
            let title = required_string(arguments, "title")?;
            let after_refs = optional_string_array(arguments, "after")?.unwrap_or_default();
            tiber_git::add_subtask(task_ref, title, &after_refs)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.subtask.check" => {
            let task_ref = required_string(arguments, "ref")?;
            let index = required_string(arguments, "index")?;
            tiber_git::set_subtask_checked(task_ref, index, true)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.subtask.uncheck" => {
            let task_ref = required_string(arguments, "ref")?;
            let index = required_string(arguments, "index")?;
            tiber_git::set_subtask_checked(task_ref, index, false)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.update" => {
            let task_ref = required_string(arguments, "ref")?;
            tiber_git::update_task(
                task_ref,
                tiber_git::TaskUpdate {
                    title: optional_string(arguments, "title"),
                    summary: optional_string(arguments, "summary"),
                    context: optional_string(arguments, "context"),
                    tags: optional_tags(arguments)?,
                    pr_mr_url: optional_string(arguments, "pr_mr_url"),
                    pr_mr_status: optional_string(arguments, "pr_mr_status"),
                },
            )?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.acceptance.add" => {
            let task_ref = required_string(arguments, "ref")?;
            let criterion = required_string(arguments, "criterion")?;
            tiber_git::add_acceptance(task_ref, criterion)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.acceptance.check" => {
            let task_ref = required_string(arguments, "ref")?;
            let index = required_string(arguments, "index")?;
            tiber_git::set_acceptance_checked(task_ref, index, true)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.acceptance.uncheck" => {
            let task_ref = required_string(arguments, "ref")?;
            let index = required_string(arguments, "index")?;
            tiber_git::set_acceptance_checked(task_ref, index, false)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.acceptance.remove" => {
            let task_ref = required_string(arguments, "ref")?;
            let index = required_string(arguments, "index")?;
            tiber_git::remove_acceptance(task_ref, index)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.note.add" => {
            let task_ref = required_string(arguments, "ref")?;
            let note = required_string(arguments, "note")?;
            tiber_git::add_note(task_ref, note)?;
            Ok(text_content(format!("updated {task_ref}")))
        }
        "tiber.validate_fix" => Ok(text_content(
            tiber_git::validate_fix()?
                .into_iter()
                .map(|message| format!("{message}\n"))
                .collect::<String>(),
        )),
        "tiber.close_from_trailers" => Ok(text_content(
            tiber_git::close_from_trailers()?
                .into_iter()
                .map(|closed| format!("closed {closed}\n"))
                .collect::<String>(),
        )),
        "tiber.scaffold_repo_dry_run" => Ok(text_content(
            tiber_git::scaffold_repo(false, false)?
                .into_iter()
                .map(|message| format!("{message}\n"))
                .collect::<String>(),
        )),
        "tiber.scaffold_repo_apply" => {
            let replace_conflicts = arguments
                .get("replace_conflicts")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(text_content(
                tiber_git::scaffold_repo(true, replace_conflicts)?
                    .into_iter()
                    .map(|message| format!("{message}\n"))
                    .collect::<String>(),
            ))
        }
        "tiber.install_bin" => {
            let target_dir = required_string(arguments, "target_dir")?;
            let apply = arguments
                .get("apply")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let installed = tiber_git::install_bin(target_dir, apply)?;
            if apply {
                Ok(text_content(format!("installed {installed}")))
            } else {
                Ok(text_content(format!("would install {installed}")))
            }
        }
        "tiber.ci_recovery.claim" => structured_content(tiber_git::claim_ci_recovery(
            tiber_git::CiRecoveryTrigger {
                run_id: required_string(arguments, "run_id")?.to_string(),
                run_url: required_string(arguments, "run_url")?.to_string(),
                failed_sha: required_string(arguments, "failed_sha")?.to_string(),
                workflow: required_string(arguments, "workflow")?.to_string(),
                git_ref: required_string(arguments, "git_ref")?.to_string(),
            },
        )?),
        "tiber.ci_recovery.status" => structured_content(tiber_git::ci_recovery_status()?),
        "tiber.ci_recovery.assert_owner" => {
            structured_content(tiber_git::assert_ci_recovery_owner(
                required_string(arguments, "incident_id")?,
                required_u64(arguments, "epoch")?,
            )?)
        }
        "tiber.ci_recovery.heartbeat" => structured_content(tiber_git::heartbeat_ci_recovery(
            required_string(arguments, "incident_id")?,
            required_u64(arguments, "epoch")?,
        )?),
        "tiber.ci_recovery.transfer" => structured_content(tiber_git::transfer_ci_recovery(
            required_string(arguments, "incident_id")?,
            required_u64(arguments, "epoch")?,
            required_string(arguments, "to_host")?,
            required_string(arguments, "to_session")?,
        )?),
        "tiber.ci_recovery.takeover" => structured_content(tiber_git::takeover_ci_recovery(
            required_string(arguments, "incident_id")?,
            required_u64(arguments, "epoch")?,
        )?),
        "tiber.ci_recovery.assign" => structured_content(tiber_git::assign_ci_recovery(
            required_string(arguments, "incident_id")?,
            required_u64(arguments, "epoch")?,
            tiber_git::CiRecoveryAssignmentInput {
                to_host: required_string(arguments, "to_host")?.to_string(),
                to_session: required_string(arguments, "to_session")?.to_string(),
                capabilities: required_ci_recovery_capabilities(arguments)?.join(","),
                scope: required_string(arguments, "scope")?.to_string(),
            },
        )?),
        "tiber.ci_recovery.report" => structured_content(tiber_git::report_ci_recovery(
            required_string(arguments, "incident_id")?,
            required_string(arguments, "assignment_id")?,
            required_string(arguments, "summary")?,
            required_string(arguments, "evidence")?,
        )?),
        "tiber.ci_recovery.wait" => structured_content(tiber_git::wait_for_ci_recovery(
            required_string(arguments, "incident_id")?,
            required_u64(arguments, "epoch")?,
            required_u64(arguments, "timeout_seconds")?,
        )?),
        "tiber.ci_recovery.diagnose" => structured_content(tiber_git::diagnose_ci_recovery(
            required_string(arguments, "incident_id")?,
            required_u64(arguments, "epoch")?,
            tiber_git::CiRecoveryDiagnosisInput {
                job: required_string(arguments, "job")?.to_string(),
                step: required_string(arguments, "step")?.to_string(),
                log_evidence: required_string(arguments, "log_evidence")?.to_string(),
                cause: required_string(arguments, "cause")?.to_string(),
                classification: required_string(arguments, "classification")?.to_string(),
            },
        )?),
        "tiber.ci_recovery.choose_action" => {
            structured_content(tiber_git::choose_ci_recovery_action(
                required_string(arguments, "incident_id")?,
                required_u64(arguments, "epoch")?,
                required_string(arguments, "kind")?,
                required_string(arguments, "description")?,
            )?)
        }
        "tiber.ci_recovery.record_replacement" => {
            structured_content(tiber_git::record_ci_recovery_replacement(
                required_string(arguments, "incident_id")?,
                required_u64(arguments, "epoch")?,
                tiber_git::CiRecoveryReplacementInput {
                    run_id: required_string(arguments, "run_id")?.to_string(),
                    run_url: required_string(arguments, "run_url")?.to_string(),
                    sha: required_string(arguments, "sha")?.to_string(),
                    status: required_string(arguments, "status")?.to_string(),
                },
            )?)
        }
        "tiber.ci_recovery.resolve" => structured_content(tiber_git::resolve_ci_recovery(
            required_string(arguments, "incident_id")?,
            tiber_git::CiRecoveryReleaseInput {
                replacement_run_id: required_string(arguments, "replacement_run_id")?.to_string(),
                replacement_run_url: required_string(arguments, "replacement_run_url")?.to_string(),
                sha: required_string(arguments, "sha")?.to_string(),
                terminal_status: required_string(arguments, "terminal_status")?.to_string(),
            },
        )?),
        _ => Err(tiber_git::Error::Parse(format!(
            "unsupported_mcp_tool name={name}"
        ))),
    }
}

fn validate_ci_recovery_arguments(name: &str, arguments: &Value) -> Result<(), tiber_git::Error> {
    let allowed = match name {
        "tiber.ci_recovery.claim" => {
            &["run_id", "run_url", "failed_sha", "workflow", "git_ref"][..]
        }
        "tiber.ci_recovery.status" => &[][..],
        "tiber.ci_recovery.assert_owner"
        | "tiber.ci_recovery.heartbeat"
        | "tiber.ci_recovery.takeover" => &["incident_id", "epoch"][..],
        "tiber.ci_recovery.transfer" => &["incident_id", "epoch", "to_host", "to_session"][..],
        "tiber.ci_recovery.assign" => &[
            "incident_id",
            "epoch",
            "to_host",
            "to_session",
            "capabilities",
            "scope",
        ][..],
        "tiber.ci_recovery.report" => &["incident_id", "assignment_id", "summary", "evidence"][..],
        "tiber.ci_recovery.wait" => &["incident_id", "epoch", "timeout_seconds"][..],
        "tiber.ci_recovery.diagnose" => &[
            "incident_id",
            "epoch",
            "job",
            "step",
            "log_evidence",
            "cause",
            "classification",
        ][..],
        "tiber.ci_recovery.choose_action" => &["incident_id", "epoch", "kind", "description"][..],
        "tiber.ci_recovery.record_replacement" => {
            &["incident_id", "epoch", "run_id", "run_url", "sha", "status"][..]
        }
        "tiber.ci_recovery.resolve" => &[
            "incident_id",
            "replacement_run_id",
            "replacement_run_url",
            "sha",
            "terminal_status",
        ][..],
        _ => return Ok(()),
    };
    let arguments = arguments.as_object().ok_or_else(|| {
        tiber_git::Error::Parse("mcp_argument_invalid name=arguments".to_string())
    })?;
    if let Some(unexpected) = arguments
        .keys()
        .find(|name| !allowed.contains(&name.as_str()))
    {
        return Err(tiber_git::Error::Parse(format!(
            "mcp_argument_unexpected name={unexpected}"
        )));
    }
    Ok(())
}

fn required_string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, tiber_git::Error> {
    arguments
        .get(name)
        .ok_or_else(|| tiber_git::Error::Parse(format!("mcp_argument_missing name={name}")))?
        .as_str()
        .ok_or_else(|| tiber_git::Error::Parse(format!("mcp_argument_invalid name={name}")))
}

fn required_u64(arguments: &Value, name: &str) -> Result<u64, tiber_git::Error> {
    arguments
        .get(name)
        .ok_or_else(|| tiber_git::Error::Parse(format!("mcp_argument_missing name={name}")))?
        .as_u64()
        .ok_or_else(|| tiber_git::Error::Parse(format!("mcp_argument_invalid name={name}")))
}

fn required_ci_recovery_capabilities(arguments: &Value) -> Result<Vec<&str>, tiber_git::Error> {
    let values = arguments
        .get("capabilities")
        .ok_or_else(|| {
            tiber_git::Error::Parse("mcp_argument_missing name=capabilities".to_string())
        })?
        .as_array()
        .ok_or_else(|| {
            tiber_git::Error::Parse("mcp_argument_invalid name=capabilities".to_string())
        })?;
    if values.is_empty() {
        return Err(tiber_git::Error::Parse(
            "mcp_argument_invalid name=capabilities".to_string(),
        ));
    }
    let capabilities = values
        .iter()
        .map(|value| {
            value.as_str().ok_or_else(|| {
                tiber_git::Error::Parse("mcp_argument_invalid name=capabilities".to_string())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if capabilities
        .iter()
        .any(|capability| !matches!(*capability, "inspect" | "reproduce" | "edit" | "test"))
        || capabilities.iter().enumerate().any(|(index, capability)| {
            capabilities
                .iter()
                .skip(index + 1)
                .any(|other| capability == other)
        })
    {
        return Err(tiber_git::Error::Parse(
            "mcp_argument_invalid name=capabilities".to_string(),
        ));
    }
    Ok(capabilities)
}

fn optional_string<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

fn optional_string_checked<'a>(
    arguments: &'a Value,
    name: &str,
) -> Result<Option<&'a str>, tiber_git::Error> {
    match arguments.get(name) {
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| tiber_git::Error::Parse(format!("mcp_argument_invalid name={name}"))),
        None => Ok(None),
    }
}

fn optional_tags(arguments: &Value) -> Result<Option<Vec<String>>, tiber_git::Error> {
    optional_string_array(arguments, "tags")
}

fn optional_string_array(
    arguments: &Value,
    name: &str,
) -> Result<Option<Vec<String>>, tiber_git::Error> {
    let Some(values) = arguments.get(name) else {
        return Ok(None);
    };
    let values = values
        .as_array()
        .ok_or_else(|| tiber_git::Error::Parse(format!("mcp_argument_invalid name={name}")))?;
    Ok(Some(
        values
            .iter()
            .map(|value| {
                value.as_str().map(str::to_string).ok_or_else(|| {
                    tiber_git::Error::Parse(format!("mcp_argument_invalid name={name}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

fn task_ref_schema(role: &str) -> Value {
    json!({
        "type": "string",
        "description": format!("{role} Accepts a task ID, nickname, or full task stem.")
    })
}

fn one_based_index_schema(item: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^[1-9][0-9]*$",
        "description": format!("One-based {item} index encoded as a decimal string.")
    })
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "tiber.init",
            "Initialize tiber",
            "Initialize tiber in the current repository.",
            json!({}),
            vec![],
        ),
        tool(
            "tiber.sync",
            "Sync tiber",
            "Fetch and reconcile the Git-backed EventCore store, publishing any pending local transaction. If unpublished work is discarded during reconciliation, return a workflow blocker requiring the operation to be reissued.",
            json!({}),
            vec![],
        ),
        tool(
            "tiber.codex_sandbox_setup",
            "Preview Codex sandbox setup",
            "Preview the narrow Codex approval guidance for Tiber-owned Git write and sync operations.",
            json!({}),
            vec![],
        ),
        tool(
            "tiber.create",
            "Create task",
            "Create a tiber task in backlog status.",
            json!({ "title": { "type": "string" } }),
            vec!["title"],
        ),
        tool(
            "tiber.list",
            "List tasks",
            "List open tiber tasks in board order or tasks in one status.",
            json!({
                "status": {
                    "type": "string",
                    "enum": ["backlog", "in-progress", "done", "abandoned"]
                }
            }),
            vec![],
        ),
        search_tool(),
        tool(
            "tiber.show",
            "Show task",
            "Read a task by task reference.",
            json!({ "ref": task_ref_schema("Task to read.") }),
            vec!["ref"],
        ),
        tool(
            "tiber.metadata",
            "Read task metadata",
            "Read task path, title, and tasks-branch commit time by task reference.",
            json!({ "ref": task_ref_schema("Task whose metadata is requested.") }),
            vec!["ref"],
        ),
        tool(
            "tiber.next",
            "Next task",
            "Read the next task in board order.",
            json!({}),
            vec![],
        ),
        tool(
            "tiber.transition",
            "Transition task",
            "Move a task to another status.",
            json!({
                "ref": task_ref_schema("Task to transition."),
                "status": {
                    "type": "string",
                    "enum": ["backlog", "in-progress", "done", "abandoned"]
                }
            }),
            vec!["ref", "status"],
        ),
        tool(
            "tiber.prioritize",
            "Prioritize task",
            "Move a task before another task in board order.",
            json!({
                "ref": task_ref_schema("Task to move."),
                "before": task_ref_schema("Task that will immediately follow the moved task.")
            }),
            vec!["ref", "before"],
        ),
        tool(
            "tiber.link",
            "Link task dependency",
            "Add a dependency where from is the blocker and to is the blocked task.",
            json!({
                "from": task_ref_schema("Blocking task."),
                "to": task_ref_schema("Task blocked by from.")
            }),
            vec!["from", "to"],
        ),
        tool(
            "tiber.unlink",
            "Unlink task dependency",
            "Remove a dependency where from is the blocker and to is the blocked task.",
            json!({
                "from": task_ref_schema("Blocking task."),
                "to": task_ref_schema("Task blocked by from.")
            }),
            vec!["from", "to"],
        ),
        tool(
            "tiber.subtask.add",
            "Add subtask",
            "Add a checklist subtask to a task.",
            json!({
                "ref": task_ref_schema("Task receiving the subtask."),
                "title": { "type": "string" },
                "after": { "type": "array", "items": { "type": "string" } }
            }),
            vec!["ref", "title"],
        ),
        tool(
            "tiber.subtask.check",
            "Check subtask",
            "Mark a subtask checked by one-based index.",
            json!({ "ref": task_ref_schema("Task containing the subtask."), "index": one_based_index_schema("subtask") }),
            vec!["ref", "index"],
        ),
        tool(
            "tiber.subtask.uncheck",
            "Uncheck subtask",
            "Mark a subtask unchecked by one-based index.",
            json!({ "ref": task_ref_schema("Task containing the subtask."), "index": one_based_index_schema("subtask") }),
            vec!["ref", "index"],
        ),
        tool(
            "tiber.update",
            "Update task",
            "Update task title, summary, context, tags, or PR/MR tracking fields.",
            json!({
                "ref": task_ref_schema("Task to update."),
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "context": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "pr_mr_url": { "type": "string" },
                "pr_mr_status": { "type": "string" }
            }),
            vec!["ref"],
        ),
        tool(
            "tiber.acceptance.add",
            "Add acceptance criterion",
            "Add an acceptance criterion to a task.",
            json!({ "ref": task_ref_schema("Task receiving the acceptance criterion."), "criterion": { "type": "string" } }),
            vec!["ref", "criterion"],
        ),
        tool(
            "tiber.acceptance.check",
            "Check acceptance criterion",
            "Mark an acceptance criterion checked by one-based index.",
            json!({ "ref": task_ref_schema("Task containing the acceptance criterion."), "index": one_based_index_schema("acceptance-criterion") }),
            vec!["ref", "index"],
        ),
        tool(
            "tiber.acceptance.uncheck",
            "Uncheck acceptance criterion",
            "Mark an acceptance criterion unchecked by one-based index.",
            json!({ "ref": task_ref_schema("Task containing the acceptance criterion."), "index": one_based_index_schema("acceptance-criterion") }),
            vec!["ref", "index"],
        ),
        tool(
            "tiber.acceptance.remove",
            "Remove acceptance criterion",
            "Remove an acceptance criterion by one-based index.",
            json!({ "ref": task_ref_schema("Task containing the acceptance criterion."), "index": one_based_index_schema("acceptance-criterion") }),
            vec!["ref", "index"],
        ),
        tool(
            "tiber.note.add",
            "Add note",
            "Append a dated note to a task.",
            json!({ "ref": task_ref_schema("Task receiving the note."), "note": { "type": "string" } }),
            vec!["ref", "note"],
        ),
        tool(
            "tiber.validate_fix",
            "Validate and safely fix",
            "Validate the task projection; repair reciprocal typed links and board-order membership, and report dependency cycles that still require manual resolution.",
            json!({}),
            vec![],
        ),
        tool(
            "tiber.close_from_trailers",
            "Close from trailers",
            "Close tasks referenced by Closes trailers in the current HEAD commit message only; older commit trailers are ignored.",
            json!({}),
            vec![],
        ),
        tool(
            "tiber.scaffold_repo_dry_run",
            "Preview repository scaffold",
            "Preview repository files tiber can scaffold.",
            json!({}),
            vec![],
        ),
        tool(
            "tiber.scaffold_repo_apply",
            "Apply repository scaffold",
            "Write repository files tiber scaffolds.",
            json!({
                "replace_conflicts": {
                    "type": "boolean",
                    "default": false,
                    "description": "When false, preserve conflicting existing files; when true, replace conflicts that the scaffold operation reports as replaceable."
                }
            }),
            vec![],
        ),
        tool(
            "tiber.install_bin",
            "Install tiber launcher",
            "Preview or install the bundled tiber launcher into a target directory.",
            json!({
                "target_dir": { "type": "string" },
                "apply": {
                    "type": "boolean",
                    "default": false,
                    "description": "False previews the launcher path; true writes the launcher and fails if the target already exists."
                }
            }),
            vec!["target_dir"],
        ),
    ]
    .into_iter()
    .chain(ci_recovery_tools())
    .collect()
}

fn ci_recovery_tools() -> Vec<Value> {
    vec![
        ci_recovery_tool(
            "tiber.ci_recovery.claim",
            "Claim CI recovery",
            "Claim a terminal failed pushed-CI run or join its recovery as a waiter.",
            json!({
                "run_id": { "type": "string" },
                "run_url": { "type": "string" },
                "failed_sha": { "type": "string" },
                "workflow": { "type": "string" },
                "git_ref": { "type": "string" }
            }),
            vec!["run_id", "run_url", "failed_sha", "workflow", "git_ref"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.status",
            "Read CI recovery status",
            "Read the authoritative repository-wide CI recovery state.",
            json!({}),
            vec![],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.assert_owner",
            "Assert CI recovery ownership",
            "Verify this session owns the active fenced recovery lease.",
            ci_recovery_owner_properties(),
            vec!["incident_id", "epoch"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.heartbeat",
            "Renew CI recovery lease",
            "Renew the active owner's recovery lease.",
            ci_recovery_owner_properties(),
            vec!["incident_id", "epoch"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.transfer",
            "Transfer CI recovery ownership",
            "Transfer recovery ownership to a joined session.",
            json!({
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "to_host": { "type": "string" },
                "to_session": { "type": "string" }
            }),
            vec!["incident_id", "epoch", "to_host", "to_session"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.takeover",
            "Take over expired CI recovery",
            "Take over a recovery lease only after it expires.",
            ci_recovery_owner_properties(),
            vec!["incident_id", "epoch"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.assign",
            "Assign CI recovery helper",
            "Assign bounded recovery work to a joined helper session.",
            json!({
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "to_host": { "type": "string" },
                "to_session": { "type": "string" },
                "capabilities": {
                    "type": "array",
                    "minItems": 1,
                    "uniqueItems": true,
                    "description": "Bounded helper capabilities; matches the preferred Development Discipline CI-recovery representation.",
                    "items": { "type": "string", "enum": ["inspect", "reproduce", "edit", "test"] }
                },
                "scope": { "type": "string", "description": "Exact bounded files, commands, or diagnostic responsibility delegated to the helper." }
            }),
            vec![
                "incident_id",
                "epoch",
                "to_host",
                "to_session",
                "capabilities",
                "scope",
            ],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.report",
            "Report CI recovery assignment",
            "Report the result of an assigned recovery task.",
            json!({
                "incident_id": { "type": "string" },
                "assignment_id": { "type": "string" },
                "summary": { "type": "string" },
                "evidence": { "type": "string" }
            }),
            vec!["incident_id", "assignment_id", "summary", "evidence"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.wait",
            "Wait for CI recovery event",
            "Wait up to sixty seconds for an assignment, epoch change, or resolution.",
            json!({
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "timeout_seconds": { "type": "integer", "minimum": 0, "maximum": 60 }
            }),
            vec!["incident_id", "epoch", "timeout_seconds"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.diagnose",
            "Record CI recovery diagnosis",
            "Record the exact failed job, step, log evidence, and whether the failure was caused by the pushed SHA, unrelated, or transient.",
            json!({
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "job": { "type": "string" },
                "step": { "type": "string" },
                "log_evidence": { "type": "string" },
                "cause": { "type": "string" },
                "classification": { "type": "string", "enum": ["caused", "unrelated", "transient"] }
            }),
            vec![
                "incident_id",
                "epoch",
                "job",
                "step",
                "log_evidence",
                "cause",
                "classification",
            ],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.choose_action",
            "Choose CI recovery action",
            "Select exactly one diagnosis-compatible action: repair for a caused failure, or unchanged-SHA rerun for an unrelated or transient failure.",
            json!({
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "kind": { "type": "string", "enum": ["repair", "rerun"] },
                "description": { "type": "string" }
            }),
            vec!["incident_id", "epoch", "kind", "description"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.record_replacement",
            "Record CI replacement",
            "Record a queued, running, or failed replacement CI run.",
            json!({
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "run_id": { "type": "string" },
                "run_url": { "type": "string" },
                "sha": { "type": "string" },
                "status": { "type": "string", "enum": ["queued", "running", "failed"] }
            }),
            vec!["incident_id", "epoch", "run_id", "run_url", "sha", "status"],
        ),
        ci_recovery_tool(
            "tiber.ci_recovery.resolve",
            "Resolve CI recovery",
            "Release the hold only with terminal-success proof whose run identity and SHA match the recorded replacement.",
            json!({
                "incident_id": { "type": "string" },
                "replacement_run_id": { "type": "string" },
                "replacement_run_url": { "type": "string" },
                "sha": { "type": "string" },
                "terminal_status": { "type": "string", "enum": ["success"] }
            }),
            vec![
                "incident_id",
                "replacement_run_id",
                "replacement_run_url",
                "sha",
                "terminal_status",
            ],
        ),
    ]
}

fn ci_recovery_owner_properties() -> Value {
    json!({
        "incident_id": { "type": "string" },
        "epoch": {
            "type": "integer",
            "minimum": 0,
            "description": "Fenced ownership epoch returned by the current incident state; stale epochs are rejected."
        }
    })
}

fn ci_recovery_tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Value,
    required: Vec<&str>,
) -> Value {
    let mut value = tool(name, title, description, properties, required);
    value["inputSchema"]["additionalProperties"] = Value::Bool(false);
    value["outputSchema"] = ci_recovery_output_schema(name);
    value
}

fn ci_recovery_output_schema(name: &str) -> Value {
    let (properties, required) = match name {
        "tiber.ci_recovery.claim" => (
            json!({
                "incident_id": { "type": "string" },
                "state": { "type": "string" },
                "role": { "type": "string", "enum": ["owner", "waiting"] },
                "epoch": { "type": "integer", "minimum": 0 },
                "lease_expires_at": { "type": "integer", "minimum": 0 }
            }),
            json!(["incident_id", "state", "role", "epoch", "lease_expires_at"]),
        ),
        "tiber.ci_recovery.assert_owner" | "tiber.ci_recovery.heartbeat" => (
            json!({
                "allowed": { "type": "boolean" },
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "lease_expires_at": { "type": "integer", "minimum": 0 }
            }),
            json!(["allowed", "incident_id", "epoch", "lease_expires_at"]),
        ),
        "tiber.ci_recovery.transfer" | "tiber.ci_recovery.takeover" => (
            json!({
                "incident_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "lease_expires_at": { "type": "integer", "minimum": 0 }
            }),
            json!(["incident_id", "epoch", "lease_expires_at"]),
        ),
        "tiber.ci_recovery.assign" | "tiber.ci_recovery.report" => (
            json!({
                "incident_id": { "type": "string" },
                "assignment_id": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 }
            }),
            json!(["incident_id", "assignment_id", "epoch"]),
        ),
        "tiber.ci_recovery.wait" => (
            json!({
                "incident_id": { "type": "string" },
                "state": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "wake_reason": { "type": "string", "enum": ["assignment", "epoch-changed", "resolved", "timeout"] },
                "assignment_id": { "type": ["string", "null"] }
            }),
            json!([
                "incident_id",
                "state",
                "epoch",
                "wake_reason",
                "assignment_id"
            ]),
        ),
        _ => (
            json!({
                "schema_version": { "type": "integer", "minimum": 1 },
                "incident_id": { "type": "string" },
                "state": { "type": "string" },
                "epoch": { "type": "integer", "minimum": 0 },
                "lease_expires_at": { "type": "integer", "minimum": 0 },
                "hold_released": { "type": "boolean" },
                "trigger_count": { "type": "integer", "minimum": 0 },
                "trigger": ci_recovery_trigger_output_schema(),
                "triggers": { "type": "array", "items": ci_recovery_trigger_output_schema() },
                "owner": ci_recovery_participant_output_schema(),
                "participants": { "type": "array", "items": ci_recovery_participant_output_schema() },
                "assignments": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "owner_epoch": { "type": "integer", "minimum": 0 },
                            "assignee": ci_recovery_participant_output_schema(),
                            "capabilities": { "type": "array", "items": { "type": "string" } },
                            "scope": { "type": "string" },
                            "report": {
                                "anyOf": [
                                    { "type": "null" },
                                    {
                                        "type": "object",
                                        "properties": {
                                            "summary": { "type": "string" },
                                            "evidence": { "type": "string" }
                                        },
                                        "required": ["summary", "evidence"],
                                        "additionalProperties": false
                                    }
                                ]
                            }
                        },
                        "required": ["id", "owner_epoch", "assignee", "capabilities", "scope", "report"],
                        "additionalProperties": false
                    }
                },
                "failure_record": nullable_ci_recovery_object_schema(json!({
                    "job": { "type": "string" },
                    "step": { "type": "string" },
                    "log_evidence": { "type": "string" }
                }), json!(["job", "step", "log_evidence"])),
                "diagnosis": nullable_ci_recovery_object_schema(json!({
                    "cause": { "type": "string" },
                    "classification": { "type": "string" }
                }), json!(["cause", "classification"])),
                "next_action": nullable_ci_recovery_object_schema(json!({
                    "kind": { "type": "string" },
                    "description": { "type": "string" }
                }), json!(["kind", "description"])),
                "replacement": nullable_ci_recovery_object_schema(json!({
                    "run_id": { "type": "string" },
                    "run_url": { "type": "string" },
                    "sha": { "type": "string" },
                    "status": { "type": "string" }
                }), json!(["run_id", "run_url", "sha", "status"])),
                "release_proof": nullable_ci_recovery_object_schema(json!({
                    "replacement_run_id": { "type": "string" },
                    "replacement_run_url": { "type": "string" },
                    "sha": { "type": "string" },
                    "terminal_status": { "type": "string" }
                }), json!(["replacement_run_id", "replacement_run_url", "sha", "terminal_status"]))
            }),
            json!([
                "schema_version",
                "incident_id",
                "state",
                "epoch",
                "lease_expires_at",
                "hold_released",
                "trigger_count",
                "trigger",
                "triggers",
                "owner",
                "participants",
                "assignments",
                "failure_record",
                "diagnosis",
                "next_action",
                "replacement",
                "release_proof"
            ]),
        ),
    };
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn ci_recovery_trigger_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "run_id": { "type": "string" },
            "run_url": { "type": "string" },
            "failed_sha": { "type": "string" },
            "workflow": { "type": "string" },
            "git_ref": { "type": "string" }
        },
        "required": ["run_id", "run_url", "failed_sha", "workflow", "git_ref"],
        "additionalProperties": false
    })
}

fn ci_recovery_participant_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "host": { "type": "string" },
            "session": { "type": "string" }
        },
        "required": ["host", "session"],
        "additionalProperties": false
    })
}

fn nullable_ci_recovery_object_schema(properties: Value, required: Value) -> Value {
    json!({
        "anyOf": [
            { "type": "null" },
            {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false
            }
        ]
    })
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Value,
    required: Vec<&str>,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required
        }
    })
}

fn search_tool() -> Value {
    json!({
        "name": "tiber.search",
        "title": "Search task history",
        "description": "Search task titles, summaries, and context across every status.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "results": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "status": {
                                "type": "string",
                                "enum": ["backlog", "in-progress", "done", "abandoned"]
                            },
                            "title": { "type": "string" },
                            "summary": { "type": "string" },
                            "context": { "type": "string" }
                        },
                        "required": ["id", "status", "title", "summary", "context"]
                    }
                }
            },
            "required": ["results"]
        }
    })
}

fn search_content(results: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": results.to_string()
        }],
        "structuredContent": {
            "results": results
        }
    })
}

fn structured_content(value: impl Serialize) -> Result<Value, tiber_git::Error> {
    let value = serde_json::to_value(value).map_err(|error| {
        tiber_git::Error::Parse(format!("mcp_structured_content_invalid source={error}"))
    })?;
    Ok(json!({
        "content": [{
            "type": "text",
            "text": value.to_string()
        }],
        "structuredContent": value
    }))
}

fn text_content(text: String) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ]
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn blocking_error_response(
    id: Value,
    message: &str,
    blocker: tiber_git::WorkflowBlockerData,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32603,
            "message": message,
            "data": {
                "error_code": blocker.error_code,
                "workflow_blocked": true,
                "required_action": blocker.required_action,
                "prohibited_actions": ["diagnose", "edit", "test", "rerun", "push", "unrelated-work"]
            }
        }
    })
}

fn resources() -> Result<Vec<Value>, tiber_git::Error> {
    let mut resources = vec![
        json!({
            "uri": "tasks://board",
            "name": "Tiber board",
            "mimeType": "text/markdown"
        }),
        json!({
            "uri": "tasks://codex-sandbox",
            "name": "Codex sandbox setup",
            "mimeType": "text/markdown"
        }),
        json!({
            "uri": "tasks://docs/tree",
            "name": "Tiber docs tree",
            "mimeType": "text/markdown"
        }),
    ];
    for task in tiber_git::list_tasks()? {
        resources.push(json!({
            "uri": format!("tasks://task/{}", task.path),
            "name": task.title,
            "mimeType": "text/markdown"
        }));
    }
    for doc in tiber_git::list_docs()? {
        resources.push(json!({
            "uri": format!("tasks://{doc}"),
            "name": doc,
            "mimeType": "text/markdown"
        }));
    }
    Ok(resources)
}

fn read_resource(uri: &str) -> Result<String, tiber_git::Error> {
    if uri == "tasks://board" {
        return tiber_git::list_tasks().map(|tasks| {
            tasks
                .into_iter()
                .map(|task| format!("{}\t{}\n", task.path, task.title))
                .collect::<String>()
        });
    }
    if uri == "tasks://codex-sandbox" {
        return Ok(codex_sandbox_setup());
    }
    if uri == "tasks://docs/tree" {
        return tiber_git::list_docs().map(|docs| {
            docs.into_iter()
                .map(|doc| format!("{doc}\n"))
                .collect::<String>()
        });
    }
    if let Some(task_ref) = uri.strip_prefix("tasks://task/") {
        return tiber_git::show_task(task_ref);
    }
    if let Some(doc_ref) = uri.strip_prefix("tasks://docs/") {
        return tiber_git::read_doc(&format!("docs/{doc_ref}"));
    }
    Err(tiber_git::Error::Parse(format!(
        "unsupported_resource uri={uri}"
    )))
}

#[cfg(test)]
mod blocker_response_tests {
    use super::*;

    #[test]
    fn structured_blocker_uses_the_errors_actual_code_and_recovery() {
        let response = blocking_error_response(
            json!(7),
            "tiber.publication_failed workflow_blocked=true",
            tiber_git::WorkflowBlockerData {
                error_code: "tiber.publication_failed",
                required_action: "run Tiber sync until authoritative publication is resolved",
            },
        );
        assert_eq!(
            response["error"]["data"]["error_code"],
            "tiber.publication_failed"
        );
        assert_eq!(
            response["error"]["data"]["required_action"],
            "run Tiber sync until authoritative publication is resolved"
        );
    }
}
