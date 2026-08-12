use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

#[tokio::test]
async fn dashboard_routes_render_board_and_task_pages() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Render dashboard"]);
    let stem = repo.task_stem("backlog", "render-dashboard");

    let app = tiber_server::router_at(repo.path.clone());
    let board = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).expect("request"))
        .await
        .expect("board response");
    assert_eq!(board.status(), StatusCode::OK);
    let board = body_text(board).await;
    assert!(board.contains("Render dashboard"));
    assert!(board.contains(&stem));
    let ticket_id = &stem[..13];
    assert!(board.contains(&format!("data-copy-task-id=\"{ticket_id}\"")));
    assert!(board.contains(&format!("Copy ticket ID {ticket_id}")));

    let task = app
        .clone()
        .oneshot(
            Request::get(format!("/tasks/{stem}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("task response");
    assert_eq!(task.status(), StatusCode::OK);
    let task = body_text(task).await;
    assert!(task.contains("title: Render dashboard"));

    let traversal = app
        .oneshot(
            Request::get("/tasks/../render-dashboard.md")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("traversal response");
    assert_eq!(traversal.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboard_board_page_exposes_browser_smoke_controls() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Inspect dashboard"]);

    let app = tiber_server::router_at(repo.path.clone());
    let board = app
        .oneshot(Request::get("/").body(Body::empty()).expect("request"))
        .await
        .expect("board response");
    assert_eq!(board.status(), StatusCode::OK);
    let board = body_text(board).await;

    assert!(board.contains("data-dashboard-board"));
    assert!(board.contains("data-task-link"));
    assert!(board.contains("data-copy-task-id"));
    assert!(board.contains("data-copy-status"));
    assert!(board.contains("data-reorder-status"));
    assert!(board.contains("data-task-modal"));
    assert!(board.contains("data-modal-content"));
    assert!(board.contains("href=\"/docs\""));
    assert!(board.contains("data-external-link"));
    assert!(board.contains("data-link-intercept-status"));
    assert!(board.contains("data-backlog-draggable=\"true\""));
    assert!(board.contains("draggable=\"true\""));
    assert!(board.contains("x-tiber-dashboard-action"));
    assert!(board.contains("new EventSource(\"/events\")"));
}

#[tokio::test]
async fn dashboard_reprioritizes_backlog_cards_with_post_guard() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "First card"]);
    repo.tiber(["create", "Second card"]);
    let first = repo.task_stem("backlog", "first-card");
    let second = repo.task_stem("backlog", "second-card");
    assert_eq!(repo.order_entries(), vec![first.clone(), second.clone()]);

    let app = tiber_server::router_at(repo.path.clone());
    let forbidden = app
        .clone()
        .oneshot(
            Request::post(format!("/tasks/{second}/prioritize-before/{first}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("forbidden response");
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let response = app
        .oneshot(
            Request::post(format!("/tasks/{second}/prioritize-before/{first}"))
                .header("x-tiber-dashboard-action", "prioritize-before")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("prioritize response");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(repo.order_entries(), vec![second, first]);
}

#[tokio::test]
async fn dashboard_reprioritize_rejects_non_backlog_tasks() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Backlog card"]);
    repo.tiber(["create", "Started card"]);
    let backlog = repo.task_stem("backlog", "backlog-card");
    let started = repo.task_stem("backlog", "started-card");
    repo.move_task("backlog", "in-progress", &started);

    let response = tiber_server::router_at(repo.path.clone())
        .oneshot(
            Request::post(format!("/tasks/{started}/prioritize-before/{backlog}"))
                .header("x-tiber-dashboard-action", "prioritize-before")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("prioritize response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(body_text(response)
        .await
        .contains("dashboard_prioritize_scope status=backlog"));
}

#[tokio::test]
async fn dashboard_board_renders_while_tiber_writer_lock_exists() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Visible during write"]);
    repo.write_fresh_tiber_lock();

    let response = tiber_server::router_at(repo.path.clone())
        .oneshot(Request::get("/").body(Body::empty()).expect("request"))
        .await
        .expect("board response");

    assert_eq!(response.status(), StatusCode::OK);
    let board = body_text(response).await;
    assert!(board.contains("Visible during write"));
    assert!(!board.contains("tiber_lock_busy"));
}

#[tokio::test]
async fn dashboard_board_renders_course_columns_badges_dependencies_and_modal_content() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Build API"]);
    repo.tiber(["create", "Build UI"]);
    repo.tiber(["create", "Document release"]);
    let api = repo.task_stem("backlog", "build-api");
    let ui = repo.task_stem("backlog", "build-ui");
    let docs = repo.task_stem("backlog", "document-release");
    repo.move_task("backlog", "in-progress", &ui);
    repo.move_task("backlog", "done", &docs);
    tiber_git::link_blocks_at(&repo.path, &api, &ui).expect("link tasks");
    tiber_git::update_task_at(
        &repo.path,
        &api,
        tiber_git::TaskUpdate {
            title: None,
            summary: Some("API summary with `code` and [Draft](docs/missing.md)."),
            context: None,
            tags: Some(vec!["backend".into()]),
            pr_mr_url: None,
            pr_mr_status: None,
        },
    )
    .expect("update API task");
    tiber_git::update_task_at(
        &repo.path,
        &ui,
        tiber_git::TaskUpdate {
            title: None,
            summary: Some("UI summary."),
            context: None,
            tags: Some(vec!["frontend".into()]),
            pr_mr_url: None,
            pr_mr_status: None,
        },
    )
    .expect("update UI task");

    let board = tiber_server::router_at(repo.path.clone())
        .oneshot(Request::get("/").body(Body::empty()).expect("request"))
        .await
        .expect("board response");

    assert_eq!(board.status(), StatusCode::OK);
    let board = body_text(board).await;
    assert!(board.contains("data-column=\"backlog\""));
    assert!(board.contains("data-column=\"in-progress\""));
    assert!(board.contains("data-column=\"done\""));
    assert!(board.contains("data-rank-badge=\"#1\""));
    assert!(board.contains("data-recency-badge"));
    assert!(board.contains("data-dependent=\""));
    assert!(board.contains("data-dependency=\""));
    assert!(board.contains("<code>code</code>"));
    assert!(board.contains("Draft <span class=\"draft-marker\">(draft)</span>"));
    assert!(board.contains("data-modal-content"));
    assert!(board.contains("Acceptance criteria"));
    assert!(board.contains("Notes / Log"));
}

#[tokio::test]
async fn dashboard_in_progress_cards_show_pr_mr_status_badges() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Review badge"]);
    let stem = repo.task_stem("backlog", "review-badge");
    repo.move_task("backlog", "in-progress", &stem);
    tiber_git::update_task_at(
        &repo.path,
        &stem,
        tiber_git::TaskUpdate {
            title: None,
            summary: None,
            context: None,
            tags: None,
            pr_mr_url: Some("https://github.com/example/repo/pull/42"),
            pr_mr_status: Some("checks-failing"),
        },
    )
    .expect("update PR status");

    let response = tiber_server::router_at(repo.path.clone())
        .oneshot(Request::get("/").body(Body::empty()).expect("request"))
        .await
        .expect("board response");

    assert_eq!(response.status(), StatusCode::OK);
    let board = body_text(response).await;
    assert!(board.contains("data-pr-mr-status=\"checks-failing\""));
    assert!(board.contains("PR/MR checks failing"));
    assert!(board.contains("pr-status-checks-failing"));
}

#[tokio::test]
async fn dashboard_exposes_read_only_sse_events_route() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Stream dashboard"]);

    let response = tiber_server::router_at(repo.path.clone())
        .oneshot(
            Request::get("/events")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("events response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .expect("content type"),
        "text/event-stream"
    );
    let mut body = response.into_body();
    let body = next_body_frame(&mut body).await;
    assert!(body.starts_with("data: "));
    assert!(body.contains("Stream dashboard"));
}

#[tokio::test]
async fn dashboard_events_stream_board_changes_without_reconnecting() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);
    repo.tiber(["create", "Initial stream task"]);

    let response = tiber_server::router_at(repo.path.clone())
        .oneshot(
            Request::get("/events")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("events response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();

    let initial = next_body_frame(&mut body).await;
    assert!(initial.contains("Initial stream task"));

    repo.tiber(["create", "Second stream task"]);

    let changed = next_body_frame(&mut body).await;
    assert!(changed.contains("Second stream task"));
}

#[tokio::test]
async fn dashboard_does_not_expose_http_mcp_route() {
    let repo = TempRepo::initialized();
    repo.tiber(["init"]);

    let response = tiber_server::router_at(repo.path.clone())
        .oneshot(Request::get("/mcp").body(Body::empty()).expect("request"))
        .await
        .expect("mcp response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboard_routes_render_repo_docs_with_relative_paths() {
    let repo = TempRepo::initialized();
    fs::create_dir_all(repo.path.join("docs/guides")).expect("create docs directory");
    fs::write(
        repo.path.join("docs/guides/tiber.md"),
        "# Tiber guide\n\nDashboard docs stay read-only.\n\n[Draft](missing.md)\n",
    )
    .expect("write doc");

    let app = tiber_server::router_at(repo.path.clone());
    let docs = app
        .clone()
        .oneshot(Request::get("/docs").body(Body::empty()).expect("request"))
        .await
        .expect("docs response");
    assert_eq!(docs.status(), StatusCode::OK);
    let docs = body_text(docs).await;
    assert!(docs.contains("docs/guides/tiber.md"));
    assert!(docs.contains("/docs/guides/tiber.md"));

    let doc = app
        .clone()
        .oneshot(
            Request::get("/docs/guides/tiber.md")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("doc response");
    assert_eq!(doc.status(), StatusCode::OK);
    let doc = body_text(doc).await;
    assert!(doc.contains("<h1>Tiber guide</h1>"));
    assert!(doc.contains("Dashboard docs stay read-only."));
    assert!(doc.contains("Draft (draft)"));

    let traversal = app
        .oneshot(
            Request::get("/docs/../README.md")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("traversal response");
    assert_eq!(traversal.status(), StatusCode::NOT_FOUND);
}

async fn next_body_frame(body: &mut Body) -> String {
    let frame = tokio::time::timeout(Duration::from_secs(4), body.frame())
        .await
        .expect("timed out waiting for dashboard event")
        .expect("dashboard event stream ended")
        .expect("dashboard event frame should be readable");
    let bytes = frame
        .into_data()
        .expect("dashboard event frame should contain data");
    String::from_utf8(bytes.to_vec()).expect("dashboard event should be utf8")
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("body should be utf8")
}

struct TempRepo {
    path: std::path::PathBuf,
}

impl TempRepo {
    fn initialized() -> Self {
        static TEMP_REPO_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let sequence = TEMP_REPO_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tiber-server-test-{}-{unique}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("create temp repo");
        let repo = Self { path };
        repo.git(["init", "-b", "main"]);
        repo.git(["config", "user.email", "tiber@example.test"]);
        repo.git(["config", "user.name", "Tiber Test"]);
        repo.git(["config", "commit.gpgsign", "false"]);
        fs::write(repo.path.join("README.md"), "# test repo\n").expect("write readme");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "Initial commit"]);
        repo
    }

    fn tiber<I, S>(&self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let result = match args.as_slice() {
            [command] if command == "init" => tiber_git::init_repository_at(&self.path),
            [command, title] if command == "create" => {
                tiber_git::create_task_at(&self.path, title).map(|_| ())
            }
            _ => panic!("unsupported test tiber args: {args:?}"),
        };
        result.expect("tiber command should succeed");
    }

    fn git<I, S>(&self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        assert_success(
            Command::new("git")
                .args(args)
                .current_dir(&self.path)
                .output()
                .expect("run git"),
        );
    }

    fn move_task(&self, from_status: &str, to_status: &str, stem: &str) {
        assert!(tiber_git::list_tasks_by_status_at(&self.path, from_status)
            .expect("read task status")
            .iter()
            .any(|task| task.path.contains(stem)));
        tiber_git::transition_task_at(&self.path, stem, to_status)
            .expect("transition projected task");
    }

    fn order_entries(&self) -> Vec<String> {
        tiber_git::list_tasks_at(&self.path)
            .expect("read order")
            .into_iter()
            .map(|task| {
                std::path::Path::new(&task.path)
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn task_stem(&self, status: &str, nickname: &str) -> String {
        let mut matches = tiber_git::list_tasks_by_status_at(&self.path, status)
            .expect("list tasks")
            .into_iter()
            .filter_map(|task| {
                std::path::Path::new(&task.path)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .filter(|stem| stem.ends_with(&format!("-{nickname}")))
            .collect::<Vec<_>>();
        matches.sort();
        assert_eq!(matches.len(), 1, "expected one task matching {nickname}");
        matches.remove(0)
    }

    fn write_fresh_tiber_lock(&self) {
        let lock_dir = self.path.join(".git").join("tiber");
        fs::create_dir_all(&lock_dir).expect("create tiber lock directory");
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_secs();
        fs::write(
            lock_dir.join("tiber.lock"),
            format!("pid={}\ntimestamp={timestamp}\n", std::process::id()),
        )
        .expect("write tiber lock");
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "command failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
