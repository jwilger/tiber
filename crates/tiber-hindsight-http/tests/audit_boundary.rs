#![expect(
    clippy::absolute_paths,
    clippy::arithmetic_side_effects,
    clippy::default_numeric_fallback,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core,
    clippy::tests_outside_test_module,
    clippy::too_many_lines,
    reason = "the bounded black-box HTTP fixture fails loudly and directly inspects its deterministic wire transcript"
)]

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use tiber_hindsight_http::{HindsightEndpoint, HindsightHttp};
use tiber_integration_audit::{AuditReceiptId, MemoryAuditFact};
use tiber_memory_core::{
    AgentId, MemoryBackend as _, MemoryCancellation, MemoryDeadline, MemoryDocumentId, MemoryKind,
    MemoryRequestOptions, MemoryScope, MemoryText, OwnerId, RecallItemBudget, RecallRequest,
    RecallTokenBudget, RepositoryId, RetainRequest, SessionId, TaskId, TurnId,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc},
};

struct Response(Vec<u8>);

async fn fake_hindsight(
    responses: Vec<Response>,
) -> (
    HindsightEndpoint,
    mpsc::Receiver<(String, String)>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fake server binds loopback");
    let address = listener.local_addr().expect("fake server has address");
    let endpoint =
        HindsightEndpoint::parse(&format!("http://{address}/")).expect("loopback endpoint is safe");
    let (sender, receiver) = mpsc::channel(responses.len());
    let responses = Arc::new(Mutex::new(responses.into_iter()));
    let task = tokio::spawn(async move {
        loop {
            let Some(response) = responses.lock().await.next() else {
                break;
            };
            let (mut stream, _peer) = listener.accept().await.expect("fake server accepts");
            let request = read_request(&mut stream).await;
            sender.send(request).await.expect("request is recorded");
            write_response(&mut stream, &response.0).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(40), listener.accept())
                .await
                .is_err(),
            "adapter must not replay a request"
        );
    });
    (endpoint, receiver, task)
}

async fn read_request(stream: &mut TcpStream) -> (String, String) {
    let mut raw = Vec::new();
    let header_end = loop {
        let mut chunk = [u8::default(); 4096];
        let read = stream.read(&mut chunk).await.expect("request reads");
        assert_ne!(read, 0, "request headers complete");
        raw.extend_from_slice(&chunk[..read]);
        let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        break index + 4;
    };
    let head = std::str::from_utf8(&raw[..header_end]).expect("headers are UTF-8");
    let mut request_line = head
        .lines()
        .next()
        .expect("request line exists")
        .split_whitespace();
    let method = request_line.next().expect("method exists").to_owned();
    let path = request_line.next().expect("path exists").to_owned();
    (method, path)
}

async fn write_response(stream: &mut TcpStream, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .expect("response head writes");
    stream.write_all(body).await.expect("response body writes");
    stream.shutdown().await.expect("response closes");
}

fn response(value: Value) -> Response {
    Response(serde_json::to_vec(&value).expect("fixture response serializes"))
}

fn version() -> Response {
    response(json!({
        "api_version":"0.8.3",
        "features":{
            "audit_log":false,"bank_config_api":true,"bank_llm_health":true,
            "document_export_api":true,"document_import_api":true,"file_upload_api":true,
            "llm_trace":false,"mcp":false,"observations":true,
            "store_document_text":true,"worker":true
        }
    }))
}

fn parsed<T>(
    value: &str,
    parser: fn(&str) -> Result<T, tiber_memory_core::MemoryContractError>,
) -> T {
    parser(value).expect("fixture identity parses")
}

fn scope() -> MemoryScope {
    MemoryScope::repository(
        parsed("owner", OwnerId::parse),
        parsed("repo", RepositoryId::parse),
        parsed("agent", AgentId::parse),
        parsed("session", SessionId::parse),
        parsed("task", TaskId::parse),
        parsed("turn-summary", MemoryKind::parse),
    )
}

fn options() -> MemoryRequestOptions {
    MemoryRequestOptions::new(
        MemoryDeadline::new(Duration::from_secs(2)).expect("deadline is valid"),
        MemoryCancellation::default(),
    )
}

#[tokio::test]
async fn backend_results_form_redacted_audit_facts_without_request_replay() {
    let memory_scope = scope();
    let retained = RetainRequest::new(
        memory_scope.clone(),
        parsed("document-retained", MemoryDocumentId::parse),
        parsed("turn-retained", TurnId::parse),
        parsed("raw-retain-content", MemoryText::parse),
        parsed("raw-retain-context", MemoryText::parse),
    );
    let current_document = parsed("document-current", MemoryDocumentId::parse);
    let prior_document =
        memory_scope.backend_document_id(&parsed("document-prior", MemoryDocumentId::parse));
    let recalled = RecallRequest::new(
        memory_scope,
        parsed("turn-current", TurnId::parse),
        current_document,
        parsed("raw-recall-query", MemoryText::parse),
        RecallItemBudget::new(3).expect("item budget is valid"),
        RecallTokenBudget::new(64).expect("token budget is valid"),
    )
    .expect("recall request is valid");
    let (endpoint, mut requests, task) = fake_hindsight(vec![
        version(),
        response(json!({
            "success":true,"bank_id":"tiber-repository-repo","items_count":1,
            "async":true,"operation_id":"operation-1"
        })),
        version(),
        response(json!({
            "server_message":"raw-server-error",
            "results":[{
                "id":"memory-prior","type":"experience","text":"raw-recalled-result",
                "document_id":prior_document.as_str(),
                "tags":["owner:owner","repository:repo","agent:agent","session:session",
                    "task:task","kind:turn-summary","turn:turn-prior"]
            }]
        })),
    ])
    .await;
    let backend = HindsightHttp::new(endpoint).expect("backend builds");

    let retain_outcome = backend
        .retain(&retained, &options())
        .await
        .expect("retain is accepted");
    let recall_result = backend
        .recall(&recalled, &options())
        .await
        .expect("recall succeeds");
    let retain_fact = MemoryAuditFact::retain_accepted(
        AuditReceiptId::parse("audit-retain").expect("receipt is valid"),
        &retained,
        &retain_outcome,
    );
    let recall_fact = MemoryAuditFact::recall(
        AuditReceiptId::parse("audit-recall").expect("receipt is valid"),
        &recalled,
        &recall_result,
    );
    let audit_json = format!(
        "{}{}",
        serde_json::to_string(&retain_fact).expect("retain fact serializes"),
        serde_json::to_string(&recall_fact).expect("recall fact serializes")
    );
    let audit_debug = format!("{retain_fact:?}{recall_fact:?}");

    assert!(audit_json.contains("operation-1"));
    assert!(audit_json.contains(retained.expected_evidence().as_str()));
    assert!(audit_json.contains("document-current"));
    assert!(audit_json.contains("turn-current"));
    assert!(audit_json.contains("\"admitted_count\":1"));
    for prohibited in [
        "raw-retain-content",
        "raw-retain-context",
        "raw-recall-query",
        "raw-recalled-result",
        "raw-server-error",
    ] {
        assert!(!audit_json.contains(prohibited));
        assert!(!audit_debug.contains(prohibited));
    }

    assert_eq!(
        requests.recv().await,
        Some(("GET".to_owned(), "/version".to_owned()))
    );
    assert_eq!(
        requests.recv().await,
        Some((
            "POST".to_owned(),
            "/v1/default/banks/tiber-repository-repo/memories".to_owned()
        ))
    );
    assert_eq!(
        requests.recv().await,
        Some(("GET".to_owned(), "/version".to_owned()))
    );
    assert_eq!(
        requests.recv().await,
        Some((
            "POST".to_owned(),
            "/v1/default/banks/tiber-repository-repo/memories/recall".to_owned()
        ))
    );
    task.await.expect("fake server completes without replay");
}
