#![expect(
    clippy::absolute_paths,
    clippy::default_numeric_fallback,
    clippy::panic,
    clippy::shadow_reuse,
    clippy::std_instead_of_core,
    clippy::tests_outside_test_module,
    clippy::unnested_or_patterns,
    clippy::unused_trait_names,
    reason = "black-box async peer fixtures favor explicit standard paths, readable identities, and scenario-local names"
)]

use std::path::Path;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tempfile::tempdir;
use tiber_codex_gateway::{EffectResponse, Gateway, GatewayConfig, serve_one};
use tiber_codex_gateway_core::GatewayPolicy;
use tokio::{
    net::{UnixListener, UnixStream},
    sync::mpsc,
};
use tokio_tungstenite::{accept_async, client_async, tungstenite::Message};

#[tokio::test]
async fn real_unix_websocket_peers_receive_rewritten_authority_and_exact_presentation() {
    let directory = tempdir().expect("socket directory should be created");
    let gateway_path = directory.path().join("gateway.sock");
    let upstream_path = directory.path().join("upstream.sock");
    let upstream_listener = UnixListener::bind(&upstream_path).expect("upstream should bind");
    let upstream = tokio::spawn(async move {
        let (stream, _) = upstream_listener
            .accept()
            .await
            .expect("upstream should accept");
        let mut peer = accept_async(stream)
            .await
            .expect("upstream websocket should accept");
        let request = peer
            .next()
            .await
            .expect("thread/start should arrive")
            .expect("frame should be valid");
        let Message::Text(request) = request else {
            panic!("thread/start must be text")
        };
        let request: Value = serde_json::from_str(&request).expect("thread/start should be JSON");
        assert_eq!(
            request.pointer("/params/approvalPolicy"),
            Some(&json!("never"))
        );
        assert_eq!(
            request.pointer("/params/approvalsReviewer"),
            Some(&json!("user"))
        );
        assert_eq!(
            request.pointer("/params/sandbox"),
            Some(&json!("read-only"))
        );
        peer.send(Message::Text(r#"{"id":7,"result":{"approvalPolicy":"never","approvalsReviewer":"user","sandbox":"read-only","thread":{"id":"thread-1"}}}"#.into())).await.expect("response should send");
        let presentation =
            r#"{"jsonrpc":"2.0", "method":"item/agentMessage/delta", "params":{"delta":"hello"}}"#;
        peer.send(Message::Text(presentation.into()))
            .await
            .expect("presentation should send");
        peer.close(None)
            .await
            .expect("upstream should close cleanly");
        presentation.to_owned()
    });
    let policy = GatewayPolicy::new("developer", vec![]).expect("policy should be valid");
    let (effects, _effect_receiver) = mpsc::channel(1);
    let server_path = gateway_path.clone();
    let gateway = tokio::spawn(async move {
        serve_one(
            GatewayConfig::new(server_path, upstream_path, policy),
            effects,
        )
        .await
    });
    wait_for_socket(&gateway_path).await;
    let stream = UnixStream::connect(&gateway_path)
        .await
        .expect("TUI should connect");
    let (mut tui, _) = client_async("ws://localhost/", stream)
        .await
        .expect("TUI websocket should connect");
    tui.send(Message::Text(r#"{"jsonrpc":"2.0","id":7,"method":"thread/start","params":{"approvalPolicy":"untrusted","approvalsReviewer":"model","sandbox":{"type":"dangerFullAccess","networkAccess":true}}}"#.into())).await.expect("thread/start should send");
    let response = tui
        .next()
        .await
        .expect("response should arrive")
        .expect("response frame should be valid");
    assert!(matches!(response, Message::Text(_)));
    let presentation = tui
        .next()
        .await
        .expect("presentation should arrive")
        .expect("presentation frame should be valid");
    let Message::Text(presentation) = presentation else {
        panic!("presentation must remain text")
    };
    assert_eq!(
        presentation.as_str(),
        upstream.await.expect("upstream should finish")
    );
    drop(tui);
    gateway
        .await
        .expect("gateway task should join")
        .expect("gateway should stop cleanly");
}

#[tokio::test]
async fn reviewed_authentication_refresh_round_trips_only_between_codex_peers() {
    let directory = tempdir().expect("socket directory should be created");
    let gateway_path = directory.path().join("gateway.sock");
    let upstream_path = directory.path().join("upstream.sock");
    let upstream_listener = UnixListener::bind(&upstream_path).expect("upstream should bind");
    let upstream = tokio::spawn(async move {
        let (stream, _) = upstream_listener
            .accept()
            .await
            .expect("upstream should accept");
        let mut peer = accept_async(stream)
            .await
            .expect("upstream websocket should accept");
        let request = r#"{"id":"auth-1","method":"account/chatgptAuthTokens/refresh","params":{"reason":"expired"}}"#;
        peer.send(Message::Text(request.into()))
            .await
            .expect("refresh should send");
        let response = peer
            .next()
            .await
            .expect("refresh response should arrive")
            .expect("refresh response should be valid");
        let Message::Text(response) = response else {
            panic!("refresh response must be text")
        };
        assert_eq!(
            response.as_str(),
            r#"{"id":"auth-1","result":{"accessToken":"bounded-secret","accountId":"account-1"}}"#
        );
        peer.close(None).await.expect("upstream should close");
    });
    let policy = GatewayPolicy::new("developer", vec![]).expect("policy should be valid");
    let (effects, mut effect_receiver) = mpsc::channel(1);
    let server_path = gateway_path.clone();
    let gateway = tokio::spawn(async move {
        serve_one(
            GatewayConfig::new(server_path, upstream_path, policy),
            effects,
        )
        .await
    });
    wait_for_socket(&gateway_path).await;
    let stream = UnixStream::connect(&gateway_path)
        .await
        .expect("TUI should connect");
    let (mut tui, _) = client_async("ws://localhost/", stream)
        .await
        .expect("TUI websocket should connect");
    let request = tui
        .next()
        .await
        .expect("refresh should reach the reviewed TUI")
        .expect("refresh should be valid");
    let Message::Text(request) = request else {
        panic!("refresh must be text")
    };
    assert_eq!(
        request.as_str(),
        r#"{"id":"auth-1","method":"account/chatgptAuthTokens/refresh","params":{"reason":"expired"}}"#
    );
    effect_receiver
        .try_recv()
        .expect_err("credential refresh must not enter the application effect channel");
    tui.send(Message::Text(
        r#"{"id":"auth-1","result":{"accessToken":"bounded-secret","accountId":"account-1"}}"#
            .into(),
    ))
    .await
    .expect("reviewed TUI refresh response should send");
    upstream.await.expect("upstream should finish");
    let close = tui
        .next()
        .await
        .expect("upstream close should reach the TUI")
        .expect("close frame should be valid");
    assert!(close.is_close());
    drop(tui);
    gateway
        .await
        .expect("gateway should join")
        .expect("gateway should stop cleanly");
}

#[tokio::test]
async fn backend_effect_is_completed_only_by_the_application_and_never_reaches_the_tui() {
    let directory = tempdir().expect("socket directory should be created");
    let gateway_path = directory.path().join("gateway.sock");
    let upstream_path = directory.path().join("upstream.sock");
    let upstream_listener = UnixListener::bind(&upstream_path).expect("upstream should bind");
    let upstream = tokio::spawn(async move {
        let (stream, _) = upstream_listener
            .accept()
            .await
            .expect("upstream should accept");
        let mut peer = accept_async(stream)
            .await
            .expect("upstream websocket should accept");
        peer.send(Message::Text(r#"{"jsonrpc":"2.0","id":"effect-1","method":"item/tool/call","params":{"tool":"tiber_task","arguments":{"id":"task-1"}}}"#.into())).await.expect("effect should send");
        let response = peer
            .next()
            .await
            .expect("application response should arrive")
            .expect("response should be valid");
        let Message::Text(response) = response else {
            panic!("effect response must be text")
        };
        let response: Value =
            serde_json::from_str(&response).expect("effect response should be JSON");
        assert_eq!(
            response,
            json!({"id":"effect-1","result":{"accepted":true}})
        );
        peer.send(Message::Text(r#"{"jsonrpc":"2.0","id":"effect-2","method":"item/tool/call","params":{"tool":"tiber_task","arguments":{}}}"#.into())).await.expect("second effect should send");
        let failure = peer
            .next()
            .await
            .expect("failure should arrive")
            .expect("failure should be valid");
        let Message::Text(failure) = failure else {
            panic!("failure must be text")
        };
        let failure: Value = serde_json::from_str(&failure).expect("failure should be JSON");
        assert_eq!(
            failure,
            json!({"id":"effect-2","error":{"code":-32001,"message":"denied","data":{"reason":"policy"}}})
        );
        peer.close(None)
            .await
            .expect("upstream should close cleanly");
    });
    let policy = GatewayPolicy::new("developer", vec![]).expect("policy should be valid");
    let (effect_sender, mut effects) = mpsc::channel(1);
    let gateway = Gateway::bind(
        GatewayConfig::new(&gateway_path, upstream_path, policy),
        effect_sender,
    )
    .expect("gateway should bind before Codex launches");
    let (_shutdown_sender, shutdown) = tokio::sync::watch::channel(false);
    let gateway_task = tokio::spawn(gateway.serve_one(shutdown));
    let stream = UnixStream::connect(&gateway_path)
        .await
        .expect("TUI should connect");
    let (mut tui, _) = client_async("ws://localhost/", stream)
        .await
        .expect("TUI websocket should connect");
    let effect = effects
        .recv()
        .await
        .expect("application should receive typed effect");
    assert_eq!(effect.request().method(), "item/tool/call");
    effect
        .complete(
            EffectResponse::dynamic_tool_call(json!({"accepted": true}))
                .expect("result should be bounded"),
        )
        .expect("live gateway should accept completion");
    let second = effects.recv().await.expect("second effect should arrive");
    second
        .complete(
            EffectResponse::failure(-32001, "denied", Some(json!({"reason":"policy"})))
                .expect("failure should be bounded"),
        )
        .expect("live gateway should accept failure");
    let tui_frame = tokio::time::timeout(std::time::Duration::from_millis(50), tui.next()).await;
    assert!(
        matches!(
            tui_frame,
            Err(_) | Ok(None) | Ok(Some(Ok(Message::Close(_))))
        ),
        "effect request and response must remain hidden from the TUI"
    );
    upstream.await.expect("upstream should finish");
    gateway_task
        .await
        .expect("gateway task should join")
        .expect("gateway should stop cleanly");
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_tui_handshake_and_removes_the_private_socket() {
    let directory = tempdir().expect("socket directory should be created");
    let gateway_path = directory.path().join("gateway.sock");
    let upstream_path = directory.path().join("upstream.sock");
    let policy = GatewayPolicy::new("developer", vec![]).expect("policy should be valid");
    let (effects, _effect_receiver) = mpsc::channel(1);
    let gateway = Gateway::bind(
        GatewayConfig::new(&gateway_path, upstream_path, policy),
        effects,
    )
    .expect("gateway should bind");
    let (shutdown_sender, shutdown) = tokio::sync::watch::channel(false);
    let gateway_task = tokio::spawn(gateway.serve_one(shutdown));
    let _stalled_client = UnixStream::connect(&gateway_path)
        .await
        .expect("raw TUI socket should connect");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    shutdown_sender
        .send(true)
        .expect("cancellation should publish");
    tokio::time::timeout(std::time::Duration::from_millis(250), gateway_task)
        .await
        .expect("cancellation must interrupt the handshake")
        .expect("gateway task should join")
        .expect("cancellation should be clean");
    assert!(
        !gateway_path.exists(),
        "clean cancellation must remove the socket path"
    );
}

#[tokio::test]
async fn bind_recovers_a_stale_socket_left_by_an_interrupted_owner() {
    let directory = tempdir().expect("socket directory should be created");
    let gateway_path = directory.path().join("gateway.sock");
    let stale =
        std::os::unix::net::UnixListener::bind(&gateway_path).expect("stale owner should bind");
    drop(stale);
    assert!(gateway_path.exists(), "fixture must leave a stale pathname");
    let policy = GatewayPolicy::new("developer", vec![]).expect("policy should be valid");
    let (effects, _effect_receiver) = mpsc::channel(1);
    let gateway = Gateway::bind(
        GatewayConfig::new(
            &gateway_path,
            directory.path().join("upstream.sock"),
            policy,
        ),
        effects,
    )
    .expect("a socket with no live listener should be recovered");
    drop(gateway);
    assert!(
        !gateway_path.exists(),
        "recovered gateway should own cleanup"
    );
}

#[tokio::test]
async fn unknown_backend_request_fails_closed_without_reaching_the_tui() {
    let directory = tempdir().expect("socket directory should be created");
    let gateway_path = directory.path().join("gateway.sock");
    let upstream_path = directory.path().join("upstream.sock");
    let upstream_listener = UnixListener::bind(&upstream_path).expect("upstream should bind");
    let upstream = tokio::spawn(async move {
        let (stream, _) = upstream_listener
            .accept()
            .await
            .expect("upstream should accept");
        let mut peer = accept_async(stream)
            .await
            .expect("upstream websocket should accept");
        peer.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":9,"method":"item/model/executeAnything","params":{}}"#.into(),
        ))
        .await
        .expect("unknown request should send");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });
    let policy = GatewayPolicy::new("developer", vec![]).expect("policy should be valid");
    let (effects, _effect_receiver) = mpsc::channel(1);
    let gateway = Gateway::bind(
        GatewayConfig::new(&gateway_path, upstream_path, policy),
        effects,
    )
    .expect("gateway should bind");
    let (_shutdown_sender, shutdown) = tokio::sync::watch::channel(false);
    let gateway_task = tokio::spawn(gateway.serve_one(shutdown));
    let stream = UnixStream::connect(&gateway_path)
        .await
        .expect("TUI should connect");
    let (mut tui, _) = client_async("ws://localhost/", stream)
        .await
        .expect("TUI websocket should connect");
    let error = gateway_task
        .await
        .expect("gateway task should join")
        .expect_err("unknown request must stop the gateway");
    assert_eq!(error.code(), "codex_gateway_unknown_effect_request");
    let tui_frame = tokio::time::timeout(std::time::Duration::from_millis(50), tui.next()).await;
    assert!(
        !matches!(
            tui_frame,
            Ok(Some(Ok(Message::Text(_)))) | Ok(Some(Ok(Message::Binary(_))))
        ),
        "unknown effect-bearing data must not reach the TUI"
    );
    upstream.await.expect("upstream should finish");
}

#[tokio::test]
async fn thread_start_without_a_correlatable_id_fails_before_reaching_upstream() {
    let directory = tempdir().expect("socket directory should be created");
    let gateway_path = directory.path().join("gateway.sock");
    let upstream_path = directory.path().join("upstream.sock");
    let upstream_listener = UnixListener::bind(&upstream_path).expect("upstream should bind");
    let upstream = tokio::spawn(async move {
        let (stream, _) = upstream_listener
            .accept()
            .await
            .expect("upstream should accept");
        let mut peer = accept_async(stream)
            .await
            .expect("upstream websocket should accept");
        let frame = tokio::time::timeout(std::time::Duration::from_millis(50), peer.next()).await;
        assert!(
            !matches!(
                frame,
                Ok(Some(Ok(Message::Text(_)))) | Ok(Some(Ok(Message::Binary(_))))
            ),
            "uncorrelatable thread/start data must not reach upstream"
        );
    });
    let policy = GatewayPolicy::new("developer", vec![]).expect("policy should be valid");
    let (effects, _effect_receiver) = mpsc::channel(1);
    let gateway = Gateway::bind(
        GatewayConfig::new(&gateway_path, upstream_path, policy),
        effects,
    )
    .expect("gateway should bind");
    let (_shutdown_sender, shutdown) = tokio::sync::watch::channel(false);
    let gateway_task = tokio::spawn(gateway.serve_one(shutdown));
    let stream = UnixStream::connect(&gateway_path)
        .await
        .expect("TUI should connect");
    let (mut tui, _) = client_async("ws://localhost/", stream)
        .await
        .expect("TUI websocket should connect");
    tui.send(Message::Text(
        r#"{"jsonrpc":"2.0","method":"thread/start","params":{}}"#.into(),
    ))
    .await
    .expect("fixture should send");
    let error = gateway_task
        .await
        .expect("gateway task should join")
        .expect_err("missing id must fail closed");
    assert_eq!(error.code(), "codex_gateway_thread_start_id_invalid");
    upstream.await.expect("upstream should finish");
}

async fn wait_for_socket(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("gateway socket was not created");
}
