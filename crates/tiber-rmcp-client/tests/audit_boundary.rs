#[cfg(test)]
#[expect(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    clippy::pattern_type_mismatch,
    clippy::shadow_unrelated,
    clippy::similar_names,
    clippy::unreachable,
    reason = "the bounded local protocol fixture fails loudly and directly asserts its closed transcript"
)]
mod tests {
    use core::{iter::empty, str, time::Duration};

    use serde_json::{Value, json};
    use tiber_external_tools_core::{
        AgentRole, AssignmentId, AuthorizationContext, ConfiguredTool, ExternalToolCapability,
        ExternalToolError, IdempotencyKey, IntegrationId, LoopbackEndpoint, McpIntegration,
        McpTransport, OwnerApprovalId, PermissionGrant, PolicyDecisionId, PolicyIntersection,
        ReconciliationOutcome, ScopedPermission, SessionId, ToolArguments,
        ToolCallAuthorizationDecision, ToolCallProposal, ToolClass, ToolName, WorkflowMode,
        authorize_tool_call, decide_tool_call,
    };
    use tiber_integration_audit::{
        AuditReceiptId, ExternalFailureCode, ExternalToolAuditFact, ExternalToolFailure,
        ExternalToolRetryability,
    };
    use tiber_rmcp_client::{RequestOptions, RmcpClient};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        time::timeout,
    };
    use tokio_util::sync::CancellationToken;

    const MAX_FIXTURE_REQUEST_BYTES: usize = 128 * 1024;

    struct HttpRequest {
        body: Value,
    }

    fn text<T>(parse: impl FnOnce(&str) -> Result<T, ExternalToolError>, value: &str) -> T {
        parse(value).expect("fixture semantic identity is valid")
    }

    fn tool(value: &str) -> ToolName {
        text(ToolName::parse, value)
    }

    fn context() -> AuthorizationContext {
        AuthorizationContext::new(
            text(WorkflowMode::parse, "review"),
            text(AgentRole::parse, "reviewer"),
            text(SessionId::parse, "session-audit"),
            text(AssignmentId::parse, "assignment-audit"),
            text(PolicyDecisionId::parse, "policy-audit"),
        )
    }

    fn integration(endpoint: &str) -> McpIntegration {
        McpIntegration::new(
            text(IntegrationId::parse, "fixture-tools"),
            McpTransport::StreamableHttp {
                endpoint: LoopbackEndpoint::parse(endpoint).expect("fixture endpoint is loopback"),
            },
            [
                ConfiguredTool::new(tool("inspect"), ToolClass::Observe),
                ConfiguredTool::new(tool("mutate"), ToolClass::Mutate),
                ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
            ],
            Some(tool("mutation_status")),
        )
        .expect("fixture integration is valid")
    }

    fn policy(integration: &McpIntegration, allowed: bool) -> PolicyIntersection {
        let grant = if allowed {
            PermissionGrant::new(
                [tool("inspect"), tool("mutate"), tool("mutation_status")],
                [
                    ExternalToolCapability::InvokeTools,
                    ExternalToolCapability::ReconcileMutations,
                ],
            )
        } else {
            PermissionGrant::new(empty::<ToolName>(), empty::<ExternalToolCapability>())
        };
        PolicyIntersection::new(
            integration,
            grant.clone(),
            ScopedPermission::new(text(WorkflowMode::parse, "review"), grant.clone()),
            ScopedPermission::new(text(AgentRole::parse, "reviewer"), grant.clone()),
            ScopedPermission::new(text(SessionId::parse, "session-audit"), grant.clone()),
            ScopedPermission::new(text(AssignmentId::parse, "assignment-audit"), grant.clone()),
            ScopedPermission::new(text(PolicyDecisionId::parse, "policy-audit"), grant),
        )
    }

    fn proposal(name: &str, class: ToolClass) -> ToolCallProposal {
        ToolCallProposal::new(
            tool(name),
            ToolArguments::parse(r#"{"secret_argument":"do-not-audit"}"#)
                .expect("fixture arguments are valid JSON"),
            (class == ToolClass::Mutate).then(|| text(IdempotencyKey::parse, "mutation-audit-1")),
        )
    }

    fn authorized_call(
        integration: &McpIntegration,
        name: &str,
        class: ToolClass,
    ) -> tiber_external_tools_core::AuthorizedToolCall {
        authorize_tool_call(
            integration,
            &policy(integration, true),
            &context(),
            proposal(name, class),
            (class == ToolClass::Mutate).then(|| text(OwnerApprovalId::parse, "approval-audit-1")),
        )
        .expect("fixture call is authorized")
    }

    fn options() -> RequestOptions {
        RequestOptions::new(Duration::from_secs(2), CancellationToken::new())
    }

    fn response(request: &Value, result: Value) -> Value {
        json!({"jsonrpc":"2.0", "id":request["id"].clone(), "result":result})
    }

    fn json_body(value: Value) -> Vec<u8> {
        serde_json::to_vec(&value).expect("fixture serializes JSON")
    }

    async fn read_request(stream: &mut TcpStream) -> HttpRequest {
        let mut raw = Vec::new();
        let header_end = loop {
            let mut chunk = [0; 4096];
            let read = stream
                .read(&mut chunk)
                .await
                .expect("fixture reads HTTP request");
            assert_ne!(read, 0, "fixture receives a complete HTTP request");
            raw.extend_from_slice(&chunk[..read]);
            assert!(
                raw.len() <= MAX_FIXTURE_REQUEST_BYTES,
                "fixture request remains bounded"
            );
            if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let head = str::from_utf8(&raw[..header_end]).expect("HTTP request head is UTF-8");
        let content_length = head
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _value)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_name, value)| {
                value
                    .trim()
                    .parse::<usize>()
                    .expect("content length is valid")
            });
        while raw.len().saturating_sub(header_end) < content_length {
            let mut chunk = [0; 4096];
            let read = stream
                .read(&mut chunk)
                .await
                .expect("fixture reads HTTP body");
            assert_ne!(read, 0, "fixture receives the complete HTTP body");
            raw.extend_from_slice(&chunk[..read]);
        }
        HttpRequest {
            body: serde_json::from_slice(&raw[header_end..header_end + content_length])
                .expect("fixture body is JSON-RPC"),
        }
    }

    async fn write_response(stream: &mut TcpStream, status: &str, body: &[u8]) {
        let content_type = if body.is_empty() {
            ""
        } else {
            "Content-Type: application/json\r\n"
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\n{content_type}Content-Length: {}\r\n\r\n",
            body.len()
        );
        stream
            .write_all(head.as_bytes())
            .await
            .expect("fixture writes response head");
        stream
            .write_all(body)
            .await
            .expect("fixture writes response body");
        stream.shutdown().await.expect("fixture closes response");
    }

    async fn serve_initialization(listener: &TcpListener) {
        let (mut initialize_stream, _peer) =
            listener.accept().await.expect("fixture accepts initialize");
        let initialize = read_request(&mut initialize_stream).await;
        assert_eq!(initialize.body["method"], "initialize");
        let body = json_body(response(
            &initialize.body,
            json!({
                "protocolVersion":"2025-11-25",
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"audit-fixture","version":"0.0.0"}
            }),
        ));
        write_response(&mut initialize_stream, "200 OK", &body).await;

        let (mut initialized_stream, _peer) = listener
            .accept()
            .await
            .expect("fixture accepts initialized");
        let initialized = read_request(&mut initialized_stream).await;
        assert_eq!(initialized.body["method"], "notifications/initialized");
        write_response(&mut initialized_stream, "202 Accepted", &[]).await;
    }

    #[tokio::test]
    async fn policy_denial_emits_a_sanitized_fact_without_fake_server_io() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds loopback");
        let endpoint = format!(
            "http://{}/mcp",
            listener.local_addr().expect("fixture has an address")
        );
        let integration = integration(&endpoint);
        let decision = decide_tool_call(
            &integration,
            &policy(&integration, false),
            &context(),
            proposal("inspect", ToolClass::Observe),
            None,
        );
        let ToolCallAuthorizationDecision::Denied(denial) = decision else {
            unreachable!("deny-by-default policy refuses the call before transport");
        };
        let fact = ExternalToolAuditFact::denied(
            AuditReceiptId::parse("audit-denied").expect("valid audit identity"),
            &denial,
        );
        let serialized = serde_json::to_string(&fact).expect("audit fact serializes");

        assert!(serialized.contains("external_tools_tool_denied"));
        assert!(!serialized.contains("do-not-audit"));
        assert!(!serialized.contains(&endpoint));
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err(),
            "policy denial performs zero fake-server I/O"
        );
    }

    #[tokio::test]
    async fn pre_dispatch_failure_consumes_authority_and_emits_only_safe_provenance() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds loopback");
        let endpoint = format!(
            "http://{}/mcp",
            listener.local_addr().expect("fixture has an address")
        );
        let server = tokio::spawn(async move {
            serve_initialization(&listener).await;
            assert!(
                timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err(),
                "pre-dispatch cancellation performs no tool-call I/O"
            );
        });
        let integration = integration(&endpoint);
        let call = authorized_call(&integration, "mutate", ToolClass::Mutate);
        let mut client = RmcpClient::connect_for_call(&call, options())
            .await
            .expect("authorized client initializes");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = match client
            .call(
                call,
                RequestOptions::new(Duration::from_secs(1), cancellation),
            )
            .await
        {
            Ok(_outcome) => unreachable!("cancelled call cannot produce an outcome"),
            Err(error) => error,
        };
        let fact = ExternalToolAuditFact::failed(
            AuditReceiptId::parse("audit-failed").expect("valid audit identity"),
            error.provenance(),
            ExternalToolFailure::new(
                ExternalFailureCode::parse(error.error().code())
                    .expect("adapter code is a safe semantic code"),
                ExternalToolRetryability::Permanent,
            ),
        );
        let serialized = serde_json::to_string(&fact).expect("failure fact serializes");

        assert!(serialized.contains("rmcp_client_cancelled"));
        assert!(serialized.contains(r#""operation":"invoke""#));
        assert!(serialized.contains(r#""retryability":"permanent""#));
        assert!(!serialized.contains("do-not-audit"));
        assert!(!serialized.contains(&endpoint));
        client.close().await;
        server.await.expect("fixture transcript completes");
    }

    #[tokio::test]
    async fn permitted_observation_emits_only_a_digest_and_trusted_provenance() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds loopback");
        let endpoint = format!(
            "http://{}/mcp",
            listener.local_addr().expect("fixture has an address")
        );
        let server = tokio::spawn(async move {
            serve_initialization(&listener).await;
            let (mut stream, _peer) = listener.accept().await.expect("fixture accepts tool call");
            let request = read_request(&mut stream).await;
            assert_eq!(request.body["method"], "tools/call");
            assert_eq!(request.body["params"]["name"], "inspect");
            let body = json_body(response(
                &request.body,
                json!({
                    "content":[{"type":"text","text":"secret-server-payload"}],
                    "isError":false
                }),
            ));
            write_response(&mut stream, "200 OK", &body).await;
        });
        let integration = integration(&endpoint);
        let call = authorized_call(&integration, "inspect", ToolClass::Observe);
        let mut client = RmcpClient::connect_for_call(&call, options())
            .await
            .expect("authorized client initializes");
        let bound_outcome = client
            .call(call, options())
            .await
            .expect("authorized observation succeeds");
        let fact = ExternalToolAuditFact::completed(
            AuditReceiptId::parse("audit-observed").expect("valid audit identity"),
            &bound_outcome,
        );
        let serialized = serde_json::to_value(&fact).expect("audit fact serializes");

        assert_eq!(serialized["outcome"], "observed");
        assert_eq!(serialized["integration_id"], "fixture-tools");
        assert_eq!(serialized["tool"], "inspect");
        assert_eq!(
            serialized["payload_sha256"].as_str().map(str::len),
            Some(64)
        );
        let audit_text = serialized.to_string();
        assert!(!audit_text.contains("secret-server-payload"));
        assert!(!audit_text.contains("do-not-audit"));
        assert!(!audit_text.contains(&endpoint));
        client.close().await;
        server.await.expect("fixture transcript completes");
    }

    #[tokio::test]
    async fn dispatched_mutation_emits_ambiguity_and_reconciliation_facts_without_secrets() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds loopback");
        let endpoint = format!(
            "http://{}/mcp",
            listener.local_addr().expect("fixture has an address")
        );
        let server = tokio::spawn(async move {
            serve_initialization(&listener).await;
            let (mut mutation_stream, _peer) =
                listener.accept().await.expect("fixture accepts mutation");
            let mutation = read_request(&mut mutation_stream).await;
            assert_eq!(mutation.body["method"], "tools/call");
            assert_eq!(mutation.body["params"]["name"], "mutate");
            assert_eq!(
                mutation.body["params"]["arguments"]["idempotencyKey"],
                "mutation-audit-1"
            );
            drop(mutation_stream);

            serve_initialization(&listener).await;
            let (mut status_stream, _peer) =
                listener.accept().await.expect("fixture accepts status");
            let status = read_request(&mut status_stream).await;
            assert_eq!(status.body["method"], "tools/call");
            assert_eq!(status.body["params"]["name"], "mutation_status");
            assert_eq!(
                status.body["params"]["arguments"],
                json!({"idempotencyKey":"mutation-audit-1"})
            );
            let body = json_body(response(
                &status.body,
                json!({
                    "content":[],
                    "structuredContent":{"status":"committed"},
                    "isError":false
                }),
            ));
            write_response(&mut status_stream, "200 OK", &body).await;
        });
        let integration = integration(&endpoint);
        let call = authorized_call(&integration, "mutate", ToolClass::Mutate);
        let mut client = RmcpClient::connect_for_call(&call, options())
            .await
            .expect("authorized mutation client initializes");
        let bound_outcome = client
            .call(call, options())
            .await
            .expect("a dispatched transport loss becomes explicit ambiguity");
        let unknown_fact = ExternalToolAuditFact::completed(
            AuditReceiptId::parse("audit-unknown").expect("valid audit identity"),
            &bound_outcome,
        );
        let reconciliation = bound_outcome
            .into_reconciliation()
            .expect("the exact dispatched mutation carries recovery authority");

        let mut status_client = RmcpClient::connect_for_reconciliation(&reconciliation, options())
            .await
            .expect("authorized reconciliation client initializes");
        let state = status_client
            .reconcile(&reconciliation, options())
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(state.outcome(), ReconciliationOutcome::Committed);
        let reconciled_fact = ExternalToolAuditFact::reconciled(
            AuditReceiptId::parse("audit-reconciled").expect("valid audit identity"),
            &state,
        );

        let unknown_text = serde_json::to_string(&unknown_fact).expect("unknown fact serializes");
        let reconciled_text =
            serde_json::to_string(&reconciled_fact).expect("reconciled fact serializes");
        for audit_text in [&unknown_text, &reconciled_text] {
            assert!(audit_text.contains("mutation-audit-1"));
            assert!(!audit_text.contains("do-not-audit"));
            assert!(!audit_text.contains("secret-server-payload"));
            assert!(!audit_text.contains(&endpoint));
        }
        assert!(unknown_text.contains(r#""outcome":"unknown""#));
        assert!(unknown_text.contains(r#""reconciliation_tool":"mutation_status""#));
        assert!(reconciled_text.contains(r#""outcome":"reconciled""#));
        assert!(reconciled_text.contains(r#""state":"Committed""#));
        status_client.close().await;
        server.await.expect("fixture transcript completes");
    }
}
