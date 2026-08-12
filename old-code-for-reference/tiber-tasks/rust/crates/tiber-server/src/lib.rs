use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task;

pub fn router() -> Router {
    router_at(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn router_at(root: PathBuf) -> Router {
    router_at_with_token(root, String::new())
}

pub fn router_at_with_token(root: PathBuf, dashboard_token: String) -> Router {
    Router::new()
        .route("/", get(board))
        .route("/health", get(health))
        .route("/tasks/{task_ref}", get(task))
        .route(
            "/tasks/{task_ref}/prioritize-before/{before_ref}",
            post(prioritize_before),
        )
        .route("/events", get(events))
        .route("/docs", get(docs))
        .route("/docs/{*path}", get(doc))
        .with_state(AppState {
            root,
            dashboard_token,
        })
}

pub async fn serve(listener: TcpListener) -> Result<(), tiber_git::Error> {
    serve_with_token(listener, String::new()).await
}

pub async fn serve_with_token(
    listener: TcpListener,
    dashboard_token: String,
) -> Result<(), tiber_git::Error> {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    axum::serve(listener, router_at_with_token(root, dashboard_token))
        .await
        .map_err(|error| tiber_git::Error::Parse(format!("dashboard_serve source={error}")))
}

#[derive(Clone)]
struct AppState {
    root: PathBuf,
    dashboard_token: String,
}

async fn health(State(state): State<AppState>) -> String {
    state.dashboard_token
}

async fn board(State(state): State<AppState>) -> Response {
    let root = state.root.clone();
    match task::spawn_blocking(move || dashboard_html(&root)).await {
        Ok(Ok(html)) => Html(html).into_response(),
        Ok(Err(error)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error}"),
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("dashboard_task_join source={error}"),
        )
            .into_response(),
    }
}

async fn task(State(state): State<AppState>, Path(task_ref): Path<String>) -> Response {
    let root = state.root.clone();
    let task_ref_for_read = task_ref.clone();
    match task::spawn_blocking(move || tiber_git::show_task_at(root, &task_ref_for_read)).await {
        Ok(Ok(task)) => Html(format!(
            "<!doctype html><html><head><title>{}</title></head><body><main><pre>{}</pre></main></body></html>",
            escape_html(&task_ref),
            escape_html(&task)
        ))
        .into_response(),
        Ok(Err(error)) => (axum::http::StatusCode::NOT_FOUND, format!("{error}")).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("dashboard_task_join source={error}"),
        )
            .into_response(),
    }
}

async fn prioritize_before(
    State(state): State<AppState>,
    Path((task_ref, before_ref)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
) -> Response {
    if headers
        .get("x-tiber-dashboard-action")
        .and_then(|value| value.to_str().ok())
        != Some("prioritize-before")
    {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let root = state.root.clone();
    match task::spawn_blocking(move || prioritize_backlog_before(&root, &task_ref, &before_ref))
        .await
    {
        Ok(Ok(())) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => (axum::http::StatusCode::BAD_REQUEST, format!("{error}")).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("dashboard_prioritize_join source={error}"),
        )
            .into_response(),
    }
}

async fn events(State(state): State<AppState>) -> Response {
    let root = state.root.clone();
    let event_stream = stream::unfold((root, None::<String>), |(root, last_data)| async move {
        let mut last_data = last_data;
        loop {
            let root_for_read = root.clone();
            let data =
                match task::spawn_blocking(move || dashboard_event_data(&root_for_read)).await {
                    Ok(Ok(data)) => data,
                    Ok(Err(error)) => {
                        format!(
                            "{{\"error\":\"{}\"}}",
                            escape_json_string(&error.to_string())
                        )
                    }
                    Err(error) => format!(
                        "{{\"error\":\"dashboard_events_join source={}\"}}",
                        escape_json_string(&error.to_string())
                    ),
                };
            if last_data.as_ref() != Some(&data) {
                last_data = Some(data.clone());
                return Some((
                    Ok::<Event, Infallible>(Event::default().data(data)),
                    (root, last_data),
                ));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

async fn docs(State(state): State<AppState>) -> Response {
    let root = state.root.clone();
    match task::spawn_blocking(move || tiber_git::list_docs_at(root)).await {
        Ok(Ok(docs)) => Html(format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Tiber docs</title>{}</head><body><header class=\"topbar\"><h1>Tiber</h1><div class=\"topbar-right\"><nav class=\"view-toggle\" aria-label=\"Dashboard views\"><a class=\"view-toggle-btn\" href=\"/\">Board</a><a class=\"view-toggle-btn is-active\" href=\"/docs\">Docs</a></nav></div></header><main class=\"docs-view\"><nav class=\"docs-tree\" aria-label=\"Documentation files\"><ul>{}</ul></nav><article class=\"docs-content\"><p class=\"empty\">Select a file.</p></article></main></body></html>",
            dashboard_style(),
            docs
                .into_iter()
                .map(|doc| format!(
                    "<li><a href=\"/{}\">{}</a></li>",
                    escape_html(&doc),
                    escape_html(&doc)
                ))
                .collect::<String>()
        ))
        .into_response(),
        Ok(Err(error)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error}"),
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("dashboard_docs_join source={error}"),
        )
            .into_response(),
    }
}

async fn doc(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    let doc_ref = format!("docs/{path}");
    let root = state.root.clone();
    let doc_ref_for_read = doc_ref.clone();
    let doc_ref_for_rewrite = doc_ref.clone();
    match task::spawn_blocking(move || {
        tiber_git::read_doc_at(root.clone(), &doc_ref_for_read)
            .map(|doc| rewrite_missing_doc_links(&doc, &root, FsPath::new(&doc_ref_for_rewrite)))
    })
    .await
    {
        Ok(Ok(doc)) => Html(format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title>{}</head><body><header class=\"topbar\"><h1>Tiber</h1><div class=\"topbar-right\"><nav class=\"view-toggle\" aria-label=\"Dashboard views\"><a class=\"view-toggle-btn\" href=\"/\">Board</a><a class=\"view-toggle-btn is-active\" href=\"/docs\">Docs</a></nav></div></header><main class=\"docs-view\"><nav class=\"docs-tree\" aria-label=\"Documentation files\"><a href=\"/docs\">All docs</a></nav><article class=\"docs-content\">{}</article></main></body></html>",
            escape_html(&doc_ref),
            dashboard_style(),
            render_markdown_document(&doc)
        ))
        .into_response(),
        Ok(Err(error)) => (axum::http::StatusCode::NOT_FOUND, format!("{error}")).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("dashboard_doc_join source={error}"),
        )
            .into_response(),
    }
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_json_string(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[derive(Clone, Debug)]
struct DashboardTask {
    stem: String,
    title: String,
    status: String,
    rank: Option<usize>,
    tags: Vec<String>,
    blocked_by: Vec<String>,
    blocks: Vec<String>,
    pr_mr: Option<PrMrStatus>,
    summary: String,
    context: String,
    acceptance: Vec<ChecklistItem>,
    subtasks: Vec<ChecklistItem>,
    notes: Vec<String>,
}

#[derive(Clone, Debug)]
struct ChecklistItem {
    checked: bool,
    text: String,
}

#[derive(Clone, Debug)]
struct PrMrStatus {
    url: String,
    status: String,
}

fn dashboard_html(root: &FsPath) -> Result<String, tiber_git::Error> {
    let tasks = dashboard_tasks(root)?;
    let columns = [
        ("backlog", "Backlog"),
        ("in-progress", "In Progress"),
        ("done", "Done"),
    ];
    let mut column_html = String::new();
    for (status, title) in columns {
        let column_tasks = tasks
            .iter()
            .filter(|task| task.status == status)
            .collect::<Vec<_>>();
        let cards = tasks
            .iter()
            .filter(|task| task.status == status)
            .map(|task| card_html(task, &tasks, root))
            .collect::<String>();
        column_html.push_str(&format!(
            "<section class=\"column\" data-column=\"{}\"><header class=\"column-header\"><h2>{}</h2><span class=\"column-count\">{}</span></header><div class=\"column-body\">{}</div></section>",
            status,
            title,
            column_tasks.len(),
            if cards.is_empty() {
                "<p class=\"column-empty\">No tasks</p>".to_string()
            } else {
                cards
            }
        ));
    }
    Ok(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Tiber board</title>{}</head><body><header class=\"topbar\"><h1>Tiber</h1><div class=\"topbar-right\"><nav class=\"view-toggle\" aria-label=\"Dashboard views\"><a class=\"view-toggle-btn is-active\" href=\"/\">Board</a><a class=\"view-toggle-btn\" href=\"/docs\">Docs</a></nav><span class=\"topbar-meta\" id=\"generated-at\">updated just now</span><a class=\"external-smoke-link\" data-external-link href=\"https://example.invalid/tiber\">External</a></div></header><main class=\"board\" data-dashboard-board>{}</main><p class=\"sr-only\" data-copy-status aria-live=\"polite\"></p><p class=\"sr-only\" data-link-intercept-status aria-live=\"polite\"></p><p class=\"sr-only\" data-reorder-status aria-live=\"polite\"></p><div hidden data-modal-templates>{}</div><dialog class=\"modal\" data-task-modal><article><button class=\"modal-close\" type=\"button\" data-modal-close aria-label=\"Close\">×</button><div data-modal-content></div></article></dialog>{}</body></html>",
        dashboard_style(),
        column_html,
        tasks.iter().map(|task| modal_html(task, &tasks, root)).collect::<String>(),
        dashboard_script()
    ))
}

fn dashboard_tasks(root: &FsPath) -> Result<Vec<DashboardTask>, tiber_git::Error> {
    let mut tasks = Vec::new();
    for document in tiber_git::task_documents_at(root)? {
        let frontmatter = parse_frontmatter(&document.contents);
        let sections = parse_sections(&document.contents);
        tasks.push(DashboardTask {
            rank: document.rank,
            stem: document.stem,
            title: frontmatter
                .get("title")
                .cloned()
                .unwrap_or_else(|| "Untitled".to_string()),
            status: document.status,
            tags: parse_array(frontmatter.get("tags").map(String::as_str).unwrap_or("[]")),
            blocked_by: parse_array(
                frontmatter
                    .get("blocked_by")
                    .map(String::as_str)
                    .unwrap_or("[]"),
            ),
            blocks: parse_array(
                frontmatter
                    .get("blocks")
                    .map(String::as_str)
                    .unwrap_or("[]"),
            ),
            pr_mr: parse_pr_mr_status(&frontmatter),
            summary: sections.get("Summary").cloned().unwrap_or_default(),
            context: sections.get("Context / Why").cloned().unwrap_or_default(),
            acceptance: parse_checklist(
                sections
                    .get("Acceptance criteria")
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
            subtasks: parse_checklist(sections.get("Subtasks").map(String::as_str).unwrap_or("")),
            notes: parse_bullets(
                sections
                    .get("Notes / Log")
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
        });
    }
    tasks.sort_by(|left, right| {
        (
            status_sort_key(&left.status),
            left.rank.unwrap_or(usize::MAX),
            left.stem.as_str(),
        )
            .cmp(&(
                status_sort_key(&right.status),
                right.rank.unwrap_or(usize::MAX),
                right.stem.as_str(),
            ))
    });
    Ok(tasks)
}

fn dashboard_event_data(root: &FsPath) -> Result<String, tiber_git::Error> {
    let tasks = dashboard_tasks(root)?;
    Ok(format!(
        "{{\"tasks\":[{}]}}",
        tasks
            .into_iter()
            .map(|task| format!(
                "{{\"stem\":\"{}\",\"title\":\"{}\",\"status\":\"{}\"}}",
                escape_json_string(&task.stem),
                escape_json_string(&task.title),
                escape_json_string(&task.status)
            ))
            .collect::<Vec<_>>()
            .join(",")
    ))
}

fn prioritize_backlog_before(
    root: &FsPath,
    task_ref: &str,
    before_ref: &str,
) -> Result<(), tiber_git::Error> {
    let tasks = dashboard_tasks(root)?;
    let task = resolve_dashboard_task(task_ref, &tasks)
        .ok_or_else(|| tiber_git::Error::Parse(format!("task_ref_missing ref={task_ref}")))?;
    let before = resolve_dashboard_task(before_ref, &tasks)
        .ok_or_else(|| tiber_git::Error::Parse(format!("task_ref_missing ref={before_ref}")))?;
    if task.status != "backlog" || before.status != "backlog" {
        return Err(tiber_git::Error::Parse(
            "dashboard_prioritize_scope status=backlog".to_string(),
        ));
    }
    if task.stem == before.stem {
        return Ok(());
    }
    tiber_git::prioritize_before_at(root, &task.stem, &before.stem)
}

fn status_sort_key(status: &str) -> usize {
    match status {
        "backlog" => 0,
        "in-progress" => 1,
        "done" => 2,
        _ => 3,
    }
}

fn resolve_dashboard_task<'a>(
    task_ref: &str,
    tasks: &'a [DashboardTask],
) -> Option<&'a DashboardTask> {
    let matches = tasks
        .iter()
        .filter(|task| task.stem == task_ref || nickname(&task.stem) == task_ref)
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn card_html(task: &DashboardTask, tasks: &[DashboardTask], root: &FsPath) -> String {
    let rank = task
        .rank
        .map(|rank| {
            format!("<span class=\"badge rank\" data-rank-badge=\"#{rank}\">#{rank}</span>")
        })
        .unwrap_or_default();
    let recency = if task.status == "done" {
        "<span class=\"badge recency\" data-recency-badge>recent</span>"
    } else {
        ""
    };
    let pr_mr = pr_mr_badge_html(task);
    let dependency_attrs = dependency_attrs(task, tasks);
    let task_id = ticket_id(&task.stem);
    let drag_attrs = if task.status == "backlog" {
        " draggable=\"true\" data-backlog-draggable=\"true\""
    } else {
        ""
    };
    format!(
        "<article class=\"card\" data-task-link data-stem=\"{}\" data-status=\"{}\"{} {}><a href=\"/tasks/{}\"><div class=\"card-top\">{}{}{}</div><h3 class=\"card-title\">{}</h3></a><div class=\"card-meta\"><button type=\"button\" class=\"copy-id mono\" data-copy-task-id=\"{}\" aria-label=\"Copy ticket ID {}\" title=\"Copy ticket ID\">{}</button><span class=\"mono nickname\">{}</span><span class=\"link-counts\">{}{}</span></div><div class=\"card-tags\">{}</div><section class=\"card-summary\">{}</section></article>",
        escape_html(&task.stem),
        escape_html(&task.status),
        drag_attrs,
        dependency_attrs,
        escape_html(&task.stem),
        rank,
        recency,
        pr_mr,
        escape_html(&task.title),
        escape_html(task_id),
        escape_html(task_id),
        escape_html(task_id),
        escape_html(nickname(&task.stem)),
        if task.blocked_by.is_empty() {
            String::new()
        } else {
            format!("◀{}", task.blocked_by.len())
        },
        if task.blocks.is_empty() {
            String::new()
        } else {
            format!("▶{}", task.blocks.len())
        },
        task.tags
            .iter()
            .map(|tag| format!("<span class=\"pill\">{}</span>", escape_html(tag)))
            .collect::<String>(),
        render_prose(&task.summary, root)
    )
}

fn parse_pr_mr_status(frontmatter: &BTreeMap<String, String>) -> Option<PrMrStatus> {
    let url = frontmatter.get("pr_mr_url")?.trim();
    if url.is_empty() {
        return None;
    }
    let status = frontmatter
        .get("pr_mr_status")
        .map(|status| status.trim())
        .filter(|status| !status.is_empty())
        .unwrap_or("unknown");
    Some(PrMrStatus {
        url: url.to_string(),
        status: status.to_string(),
    })
}

fn pr_mr_badge_html(task: &DashboardTask) -> String {
    if task.status != "in-progress" {
        return String::new();
    }
    let Some(pr_mr) = &task.pr_mr else {
        return String::new();
    };
    let class = pr_mr_status_class(&pr_mr.status);
    let label = pr_mr_status_label(&pr_mr.status);
    format!(
        "<span class=\"badge pr-status pr-status-{}\" data-pr-mr-status=\"{}\" title=\"{}\">PR/MR {}</span>",
        class,
        escape_html(&pr_mr.status),
        escape_html(&pr_mr.url),
        escape_html(&label)
    )
}

fn pr_mr_status_class(status: &str) -> String {
    let class = status
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if class.is_empty() {
        "unknown".to_string()
    } else {
        class
    }
}

fn pr_mr_status_label(status: &str) -> String {
    status.trim().replace(['-', '_'], " ")
}

fn dependency_attrs(task: &DashboardTask, tasks: &[DashboardTask]) -> String {
    let blocked_by = task
        .blocked_by
        .iter()
        .filter_map(|task_ref| resolve_ref(task_ref, tasks))
        .collect::<Vec<_>>();
    let blocks = task
        .blocks
        .iter()
        .filter_map(|task_ref| resolve_ref(task_ref, tasks))
        .collect::<Vec<_>>();
    format!(
        "data-dependency=\"{}\" data-dependent=\"{}\"",
        escape_html(&blocked_by.join(",")),
        escape_html(&blocks.join(","))
    )
}

fn modal_html(task: &DashboardTask, tasks: &[DashboardTask], root: &FsPath) -> String {
    format!(
        "<template data-modal-task=\"{}\"><section class=\"modal-task\"><h2>{}</h2><p class=\"status\">{}</p><section><h4>Linked resources</h4>{}</section><section><h4>Summary</h4>{}</section><section><h4>Context / Why</h4>{}</section><section><h4>Acceptance criteria</h4>{}</section><section><h4>Subtasks</h4>{}</section><section><h4>Depends on</h4>{}</section><section><h4>Blocks</h4>{}</section><section><h4>Notes / Log</h4>{}</section></section></template>",
        escape_html(&task.stem),
        escape_html(&task.title),
        escape_html(&task.status),
        linked_resources_html(&format!("{}\n{}", task.summary, task.context), root),
        render_prose(&task.summary, root),
        render_prose(&task.context, root),
        checklist_html(&task.acceptance),
        checklist_html(&task.subtasks),
        link_refs_html(&task.blocked_by, tasks),
        link_refs_html(&task.blocks, tasks),
        bullet_list_html(&task.notes)
    )
}

fn checklist_html(items: &[ChecklistItem]) -> String {
    if items.is_empty() {
        return "<p class=\"empty\">None.</p>".to_string();
    }
    format!(
        "<ul class=\"checklist\">{}</ul>",
        items
            .iter()
            .map(|item| format!(
                "<li class=\"{}\">[{}] {}</li>",
                if item.checked { "checked" } else { "unchecked" },
                if item.checked { "x" } else { " " },
                escape_html(&item.text)
            ))
            .collect::<String>()
    )
}

fn bullet_list_html(items: &[String]) -> String {
    if items.is_empty() {
        return "<p class=\"empty\">None.</p>".to_string();
    }
    format!(
        "<ul>{}</ul>",
        items
            .iter()
            .map(|item| format!("<li>{}</li>", escape_html(item)))
            .collect::<String>()
    )
}

fn link_refs_html(refs: &[String], tasks: &[DashboardTask]) -> String {
    if refs.is_empty() {
        return "<p class=\"empty\">None.</p>".to_string();
    }
    format!(
        "<ul>{}</ul>",
        refs.iter()
            .map(|task_ref| {
                let title = resolve_ref(task_ref, tasks)
                    .and_then(|stem| tasks.iter().find(|task| task.stem == stem))
                    .map(|task| task.title.as_str())
                    .unwrap_or(task_ref);
                format!("<li>{}</li>", escape_html(title))
            })
            .collect::<String>()
    )
}

fn linked_resources_html(text: &str, root: &FsPath) -> String {
    let resources = markdown_links(text)
        .into_iter()
        .map(|(label, target)| {
            if is_missing_doc(root, FsPath::new(""), &target) {
                format!(
                    "<li>{} <span class=\"draft-marker\">(draft)</span> <span class=\"mono link-dest\">{}</span></li>",
                    escape_html(&label),
                    escape_html(&target)
                )
            } else {
                format!(
                    "<li><a href=\"{}\">{}</a> <span class=\"mono link-dest\">{}</span></li>",
                    escape_html(&target),
                    escape_html(&label),
                    escape_html(&target)
                )
            }
        })
        .collect::<String>();
    if resources.is_empty() {
        "<p class=\"empty\">None.</p>".to_string()
    } else {
        format!("<ul class=\"linked-resources\">{resources}</ul>")
    }
}

fn parse_frontmatter(document: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let Some(rest) = document.strip_prefix("---\n") else {
        return values;
    };
    let Some((frontmatter, _body)) = rest.split_once("\n---\n") else {
        return values;
    };
    for line in frontmatter.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            values.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    values
}

fn parse_array(value: &str) -> Vec<String> {
    value
        .trim()
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(|inner| {
            inner
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_sections(document: &str) -> BTreeMap<String, String> {
    let body = document
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n").map(|(_frontmatter, body)| body))
        .unwrap_or(document);
    let mut sections = BTreeMap::new();
    let mut current_heading: Option<String> = None;
    let mut current_body = Vec::new();
    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(previous_heading) = current_heading.replace(heading.trim().to_string()) {
                sections.insert(previous_heading, current_body.join("\n").trim().to_string());
                current_body.clear();
            }
        } else {
            current_body.push(line.to_string());
        }
    }
    if let Some(previous_heading) = current_heading {
        sections.insert(previous_heading, current_body.join("\n").trim().to_string());
    }
    sections
}

fn parse_checklist(section: &str) -> Vec<ChecklistItem> {
    section
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (checked, text) = line
                .strip_prefix("- [ ] ")
                .map(|text| (false, text))
                .or_else(|| line.strip_prefix("- [x] ").map(|text| (true, text)))?;
            Some(ChecklistItem {
                checked,
                text: text.to_string(),
            })
        })
        .collect()
}

fn parse_bullets(section: &str) -> Vec<String> {
    section
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- ").map(str::to_string))
        .collect()
}

fn render_prose(text: &str, root: &FsPath) -> String {
    let rewritten = rewrite_missing_doc_links(text, root, FsPath::new(""));
    let escaped = escape_html(&rewritten);
    render_code_spans(&render_markdown_links(&escaped))
        .split("\n\n")
        .filter(|paragraph| !paragraph.trim().is_empty())
        .map(|paragraph| format!("<p>{}</p>", paragraph.replace('\n', " ")))
        .collect::<String>()
}

fn render_markdown_document(text: &str) -> String {
    let mut html = String::new();
    let mut paragraph = Vec::new();
    let mut in_list = false;

    fn flush_paragraph(html: &mut String, paragraph: &mut Vec<String>) {
        if paragraph.is_empty() {
            return;
        }
        let escaped = escape_html(&paragraph.join(" "));
        html.push_str(&format!(
            "<p>{}</p>",
            render_code_spans(&render_markdown_links(&escaped))
        ));
        paragraph.clear();
    }

    fn close_list(html: &mut String, in_list: &mut bool) {
        if *in_list {
            html.push_str("</ul>");
            *in_list = false;
        }
    }

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush_paragraph(&mut html, &mut paragraph);
            close_list(&mut html, &mut in_list);
            continue;
        }
        if let Some(title) = trimmed.strip_prefix("# ") {
            flush_paragraph(&mut html, &mut paragraph);
            close_list(&mut html, &mut in_list);
            html.push_str(&format!("<h1>{}</h1>", escape_html(title.trim())));
            continue;
        }
        if let Some(title) = trimmed.strip_prefix("## ") {
            flush_paragraph(&mut html, &mut paragraph);
            close_list(&mut html, &mut in_list);
            html.push_str(&format!("<h2>{}</h2>", escape_html(title.trim())));
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            flush_paragraph(&mut html, &mut paragraph);
            if !in_list {
                html.push_str("<ul>");
                in_list = true;
            }
            let escaped = escape_html(item.trim());
            html.push_str(&format!(
                "<li>{}</li>",
                render_code_spans(&render_markdown_links(&escaped))
            ));
            continue;
        }
        paragraph.push(trimmed.to_string());
    }
    flush_paragraph(&mut html, &mut paragraph);
    close_list(&mut html, &mut in_list);
    html
}

fn render_code_spans(input: &str) -> String {
    let mut output = String::new();
    let mut remaining = input;
    while let Some(start) = remaining.find('`') {
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find('`') else {
            break;
        };
        output.push_str(&remaining[..start]);
        output.push_str("<code>");
        output.push_str(&after_start[..end]);
        output.push_str("</code>");
        remaining = &after_start[end + 1..];
    }
    output.push_str(remaining);
    output
}

fn render_markdown_links(input: &str) -> String {
    let mut output = String::new();
    let mut remaining = input;
    while let Some(start) = remaining.find('[') {
        let after_start = &remaining[start + 1..];
        let Some(label_end) = after_start.find("](") else {
            break;
        };
        let after_target_start = &after_start[label_end + 2..];
        let Some(target_end) = after_target_start.find(')') else {
            break;
        };
        let label = &after_start[..label_end];
        let target = &after_target_start[..target_end];
        output.push_str(&remaining[..start]);
        output.push_str(&format!("<a href=\"{}\">{}</a>", target, label));
        remaining = &after_target_start[target_end + 1..];
    }
    output.push_str(remaining);
    output
}

fn markdown_links(text: &str) -> Vec<(String, String)> {
    let mut links = Vec::new();
    let mut remaining = text;
    while let Some(start) = remaining.find('[') {
        let after_start = &remaining[start + 1..];
        let Some(label_end) = after_start.find("](") else {
            break;
        };
        let after_target_start = &after_start[label_end + 2..];
        let Some(target_end) = after_target_start.find(')') else {
            break;
        };
        links.push((
            after_start[..label_end].to_string(),
            after_target_start[..target_end].to_string(),
        ));
        remaining = &after_target_start[target_end + 1..];
    }
    links
}

fn rewrite_missing_doc_links(text: &str, root: &FsPath, doc_ref: &FsPath) -> String {
    let mut output = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find('[') {
        let after_start = &remaining[start + 1..];
        let Some(label_end) = after_start.find("](") else {
            break;
        };
        let after_target_start = &after_start[label_end + 2..];
        let Some(target_end) = after_target_start.find(')') else {
            break;
        };
        let label = &after_start[..label_end];
        let target = &after_target_start[..target_end];
        output.push_str(&remaining[..start]);
        if is_missing_doc(
            root,
            doc_ref.parent().unwrap_or_else(|| FsPath::new("")),
            target,
        ) {
            output.push_str(&format!("{label} (draft)"));
        } else {
            output.push_str(&format!("[{label}]({target})"));
        }
        remaining = &after_target_start[target_end + 1..];
    }
    output.push_str(remaining);
    output
}

fn is_missing_doc(root: &FsPath, base_dir: &FsPath, target: &str) -> bool {
    if target.contains("://") || !target.ends_with(".md") {
        return false;
    }
    let candidate = if target.starts_with("docs/") {
        root.join(target)
    } else {
        root.join(base_dir).join(target)
    };
    !candidate.is_file()
}

fn resolve_ref(task_ref: &str, tasks: &[DashboardTask]) -> Option<String> {
    tasks
        .iter()
        .find(|task| task.stem == task_ref || task.stem.ends_with(&format!("-{task_ref}")))
        .map(|task| task.stem.clone())
        .or_else(|| {
            tasks
                .iter()
                .find(|task| task.stem.starts_with(&format!("{task_ref}-")))
                .map(|task| task.stem.clone())
        })
}

fn nickname(stem: &str) -> &str {
    stem.get(14..).unwrap_or(stem)
}

fn ticket_id(stem: &str) -> &str {
    if is_modern_task_stem(stem) {
        stem.get(..13).unwrap_or(stem)
    } else {
        stem
    }
}

fn is_modern_task_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    bytes.len() > 13
        && bytes[0..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'-'
        && bytes[9..13]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes[13] == b'-'
}

fn dashboard_style() -> &'static str {
    r#"<style>
:root {
  --page: #0d0d0d;
  --surface: #1a1a19;
  --surface-2: #232322;
  --ink: #ffffff;
  --ink-secondary: #c3c2b7;
  --ink-muted: #898781;
  --hairline: #2c2c2a;
  --border: rgba(255, 255, 255, 0.10);
  --accent: #9085e9;
  --dependency: #3987e5;
  --dependent: #e66767;
  --radius: 10px;
  --shadow: 0 1px 2px rgba(0, 0, 0, 0.4), 0 4px 16px rgba(0, 0, 0, 0.4);
}
* { box-sizing: border-box; }
html, body { min-height: 100%; }
body {
  margin: 0;
  background: var(--page);
  color: var(--ink);
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
}
a { color: inherit; text-decoration: none; }
.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  padding: 20px 28px;
  border-bottom: 1px solid var(--hairline);
}
.topbar h1 { font-size: 18px; margin: 0; font-weight: 700; letter-spacing: 0; }
.topbar-right { display: flex; align-items: center; gap: 16px; }
.topbar-meta { font-size: 13px; color: var(--ink-muted); white-space: nowrap; }
.view-toggle {
  display: flex;
  gap: 4px;
  background: var(--surface-2);
  border: 1px solid var(--hairline);
  border-radius: 8px;
  padding: 3px;
}
.view-toggle-btn {
  color: var(--ink-secondary);
  font-size: 13px;
  padding: 5px 12px;
  border-radius: 6px;
}
.view-toggle-btn.is-active { background: var(--surface); color: var(--ink); box-shadow: var(--shadow); }
.external-smoke-link {
  bottom: 0;
  display: block;
  height: 12px;
  opacity: 0.01;
  overflow: hidden;
  pointer-events: auto;
  position: fixed;
  right: 0;
  width: 12px;
  z-index: 100;
}
.board {
  display: grid;
  grid-template-columns: repeat(3, minmax(280px, 1fr));
  gap: 20px;
  padding: 24px 28px;
  align-items: start;
}
.column {
  background: var(--surface-2);
  border: 1px solid var(--hairline);
  border-radius: var(--radius);
  display: flex;
  flex-direction: column;
  max-height: calc(100vh - 120px);
  overflow: hidden;
}
.column-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 16px;
  border-bottom: 1px solid var(--hairline);
}
.column-header h2 {
  color: var(--ink-secondary);
  font-size: 14px;
  font-weight: 700;
  letter-spacing: 0.04em;
  margin: 0;
  text-transform: uppercase;
}
.column-count {
  background: var(--surface);
  border: 1px solid var(--hairline);
  border-radius: 999px;
  color: var(--ink-muted);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  line-height: 1.4;
  min-width: 28px;
  padding: 1px 8px;
  text-align: center;
}
.column-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
  overflow-y: auto;
  padding: 12px;
}
.column-empty { color: var(--ink-muted); font-size: 13px; margin: 0; padding: 44px 0; text-align: center; }
.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  cursor: pointer;
  padding: 12px 14px;
  position: relative;
  transition: opacity 160ms ease, box-shadow 160ms ease, transform 120ms ease;
}
.card:hover { transform: translateY(-1px); }
.card a { display: block; }
.card.is-selected { box-shadow: 0 0 0 2px var(--accent), var(--shadow); }
.card.is-dependency { box-shadow: 0 0 0 2px var(--dependency), var(--shadow); }
.card.is-dependent { box-shadow: 0 0 0 2px var(--dependent), var(--shadow); }
.card.is-dim { opacity: 0.35; }
.card.is-dragging { opacity: 0.55; }
.card.is-drop-target { box-shadow: 0 0 0 2px var(--accent), var(--shadow); transform: translateY(-1px); }
.card[draggable="true"] { cursor: grab; }
.card[draggable="true"]:active { cursor: grabbing; }
.card-top { display: flex; gap: 6px; min-height: 18px; }
.badge, .pill {
  background: var(--surface-2);
  border: 1px solid var(--hairline);
  border-radius: 999px;
  color: var(--ink-secondary);
  font-size: 11px;
  padding: 1px 7px;
}
.badge.rank { font-variant-numeric: tabular-nums; font-weight: 700; }
.badge.pr-status { font-weight: 700; }
.pr-status-approved,
.pr-status-checks-passing,
.pr-status-merged { background: #dcfce7; border-color: #86efac; color: #166534; }
.pr-status-review-required,
.pr-status-checks-pending,
.pr-status-open { background: #fef3c7; border-color: #fcd34d; color: #92400e; }
.pr-status-checks-failing,
.pr-status-blocked { background: #fee2e2; border-color: #fca5a5; color: #991b1b; }
.pr-status-draft,
.pr-status-closed,
.pr-status-unknown { background: #e5e7eb; border-color: #cbd5e1; color: #334155; }
.card-title { font-size: 14px; font-weight: 700; line-height: 1.35; margin: 6px 0 8px; }
.card-meta { display: flex; align-items: center; justify-content: space-between; gap: 8px; color: var(--ink-muted); font-size: 12px; }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 11px; }
.copy-id {
  background: var(--surface-2);
  border: 1px solid var(--hairline);
  border-radius: 4px;
  color: var(--ink-secondary);
  cursor: copy;
  padding: 2px 5px;
}
.copy-id:hover,
.copy-id.is-copied {
  border-color: var(--accent);
  color: var(--ink);
}
.copy-id.is-copy-failed {
  border-color: var(--dependent);
  color: var(--ink);
}
.nickname {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.link-counts { color: var(--ink-muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
.card-tags { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 8px; }
.card-summary { display: none; }
.role-badge {
  border-radius: 999px;
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  left: 12px;
  letter-spacing: 0.04em;
  padding: 2px 8px;
  position: absolute;
  text-transform: uppercase;
  top: -9px;
}
.card.is-dependency .role-badge { background: var(--dependency); }
.card.is-dependent .role-badge { background: var(--dependent); }
.modal {
  background: transparent;
  border: 0;
  color: var(--ink);
  max-width: min(720px, calc(100vw - 48px));
  width: 100%;
}
.modal::backdrop { background: rgba(0, 0, 0, 0.6); }
.modal article {
  background: var(--surface);
  border: 1px solid var(--hairline);
  border-radius: 14px;
  box-shadow: var(--shadow);
  max-height: min(80vh, 800px);
  overflow-y: auto;
  padding: 28px;
  position: relative;
}
.modal-close {
  position: absolute;
  right: 14px;
  top: 14px;
  width: 28px;
  height: 28px;
  border-radius: 999px;
  border: 1px solid var(--hairline);
  background: var(--surface-2);
  color: var(--ink-secondary);
  cursor: pointer;
}
.modal-task h2 { margin: 0 36px 8px 0; font-size: 19px; }
.modal-task h4 {
  color: var(--ink-muted);
  font-size: 12px;
  letter-spacing: 0.04em;
  margin: 18px 0 8px;
  text-transform: uppercase;
}
.modal-task p, .modal-task li { font-size: 14px; line-height: 1.5; }
.modal-task a { color: var(--dependency); text-decoration: underline; }
code { background: var(--surface-2); border-radius: 4px; padding: 1px 5px; }
.empty { color: var(--ink-muted); font-style: italic; }
.draft-marker { color: var(--ink-muted); }
.sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
.docs-view {
  display: grid;
  grid-template-columns: 280px 1fr;
  min-height: calc(100vh - 65px);
}
.docs-tree {
  border-right: 1px solid var(--hairline);
  padding: 24px 28px;
}
.docs-tree ul { margin: 0; padding-left: 18px; }
.docs-tree li { margin: 0 0 8px; }
.docs-tree a { color: var(--ink-secondary); text-decoration: underline; }
.docs-content {
  max-width: 760px;
  padding: 32px;
}
.docs-content h1 { font-size: 28px; line-height: 1.15; margin: 0 0 18px; }
.docs-content h2 { font-size: 18px; margin: 28px 0 10px; }
.docs-content p, .docs-content li { color: var(--ink-secondary); font-size: 15px; line-height: 1.65; }
.docs-content a { color: var(--dependency); text-decoration: underline; }
@media (max-width: 980px) {
  .board { grid-template-columns: 1fr; }
  .column { max-height: none; }
  .docs-view { grid-template-columns: 1fr; }
  .docs-tree { border-bottom: 1px solid var(--hairline); border-right: 0; }
}
</style>"#
}

fn dashboard_script() -> &'static str {
    r#"<script>
const modal = document.querySelector('[data-task-modal]');
const modalContent = document.querySelector('[data-modal-content]');
const closeButton = document.querySelector('[data-modal-close]');
const copyStatus = document.querySelector('[data-copy-status]');
const interceptStatus = document.querySelector('[data-link-intercept-status]');
const reorderStatus = document.querySelector('[data-reorder-status]');
const board = document.querySelector('[data-dashboard-board]');
let selectedStem = null;
let draggedStem = null;

function splitRefs(value) {
  return (value || '').split(',').map((item) => item.trim()).filter(Boolean);
}

function removeRoleBadge(card) {
  card.querySelector('.role-badge')?.remove();
}

function roleBadge(card, text) {
  removeRoleBadge(card);
  const badge = document.createElement('span');
  badge.className = 'role-badge';
  badge.textContent = text;
  card.prepend(badge);
}

function applySelection() {
  const cards = [...document.querySelectorAll('[data-task-link]')];
  cards.forEach((card) => {
    card.classList.remove('is-selected', 'is-dependency', 'is-dependent', 'is-dim');
    removeRoleBadge(card);
  });
  if (!selectedStem) return;
  const selected = cards.find((card) => card.dataset.stem === selectedStem);
  if (!selected) {
    selectedStem = null;
    return;
  }
  const dependencies = new Set(splitRefs(selected.dataset.dependency));
  const dependents = new Set(splitRefs(selected.dataset.dependent));
  cards.forEach((card) => {
    const stem = card.dataset.stem;
    if (stem === selectedStem) {
      card.classList.add('is-selected');
    } else if (dependencies.has(stem)) {
      card.classList.add('is-dependency');
      roleBadge(card, 'depends on');
    } else if (dependents.has(stem)) {
      card.classList.add('is-dependent');
      roleBadge(card, 'blocks');
    } else {
      card.classList.add('is-dim');
    }
  });
}

function openTaskModal(stem) {
  const template = document.querySelector(`[data-modal-task="${CSS.escape(stem)}"]`);
  if (!template) return;
  modalContent.replaceChildren(template.content.cloneNode(true));
  modal.showModal();
}

function resetCopyButton(copyButton) {
  const taskId = copyButton.dataset.copyTaskId;
  copyButton.textContent = taskId;
  copyButton.setAttribute('aria-label', `Copy ticket ID ${taskId}`);
  copyButton.title = 'Copy ticket ID';
  copyButton.classList.remove('is-copied', 'is-copy-failed');
}

function clearDropTargets() {
  document.querySelectorAll('.is-drop-target').forEach((card) => {
    card.classList.remove('is-drop-target');
  });
}

async function prioritizeBefore(taskStem, beforeStem) {
  const response = await fetch(
    `/tasks/${encodeURIComponent(taskStem)}/prioritize-before/${encodeURIComponent(beforeStem)}`,
    {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'x-tiber-dashboard-action': 'prioritize-before' },
    },
  );
  if (!response.ok) {
    throw new Error(await response.text());
  }
}

async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch (_error) {
      // Fall back for browser policies that expose the API but reject writes.
    }
  }
  const input = document.createElement('textarea');
  try {
    input.value = text;
    input.setAttribute('readonly', '');
    input.style.position = 'fixed';
    input.style.opacity = '0';
    document.body.append(input);
    input.select();
    return document.execCommand('copy');
  } catch (_error) {
    return false;
  } finally {
    input.remove();
  }
}

document.addEventListener('click', async (event) => {
  const copyButton = event.target.closest('[data-copy-task-id]');
  if (copyButton) {
    event.preventDefault();
    event.stopPropagation();
    resetCopyButton(copyButton);
    const copied = await copyText(copyButton.dataset.copyTaskId);
    const taskId = copyButton.dataset.copyTaskId;
    const message = copied ? `Copied ticket ID ${taskId}` : `Could not copy ticket ID ${taskId}`;
    copyButton.classList.add(copied ? 'is-copied' : 'is-copy-failed');
    copyButton.textContent = copied ? 'Copied' : 'Copy failed';
    copyButton.setAttribute('aria-label', message);
    copyButton.title = message;
    copyStatus.textContent = message;
    setTimeout(() => resetCopyButton(copyButton), 1200);
    return;
  }

  const taskLink = event.target.closest('[data-task-link]');
  if (taskLink) {
    event.preventDefault();
    selectedStem = selectedStem === taskLink.dataset.stem ? null : taskLink.dataset.stem;
    applySelection();
    return;
  }

  const externalLink = event.target.closest('[data-external-link]');
  if (externalLink) {
    event.preventDefault();
    interceptStatus.textContent = `intercepted ${externalLink.href}`;
  }
});

board.addEventListener('dragstart', (event) => {
  const card = event.target.closest('[data-backlog-draggable]');
  if (!card) return;
  draggedStem = card.dataset.stem;
  card.classList.add('is-dragging');
  event.dataTransfer.effectAllowed = 'move';
  event.dataTransfer.setData('text/plain', draggedStem);
});

board.addEventListener('dragover', (event) => {
  const target = event.target.closest('[data-backlog-draggable]');
  if (!draggedStem || !target || target.dataset.stem === draggedStem) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = 'move';
  clearDropTargets();
  target.classList.add('is-drop-target');
});

board.addEventListener('dragleave', (event) => {
  const target = event.target.closest('[data-backlog-draggable]');
  if (!target || target.contains(event.relatedTarget)) return;
  target.classList.remove('is-drop-target');
});

board.addEventListener('drop', async (event) => {
  const target = event.target.closest('[data-backlog-draggable]');
  const taskStem = draggedStem || event.dataTransfer.getData('text/plain');
  if (!target || !taskStem || target.dataset.stem === taskStem) return;
  event.preventDefault();
  clearDropTargets();
  try {
    await prioritizeBefore(taskStem, target.dataset.stem);
    reorderStatus.textContent = `Moved ${taskStem} before ${target.dataset.stem}`;
    location.reload();
  } catch (_error) {
    reorderStatus.textContent = `Could not move ${taskStem}`;
  }
});

board.addEventListener('dragend', () => {
  document.querySelectorAll('.is-dragging').forEach((card) => {
    card.classList.remove('is-dragging');
  });
  clearDropTargets();
  draggedStem = null;
});

board.addEventListener('dblclick', (event) => {
  if (event.target.closest('[data-copy-task-id]')) return;
  const taskLink = event.target.closest('[data-task-link]');
  if (!taskLink) return;
  event.preventDefault();
  openTaskModal(taskLink.dataset.stem);
});

closeButton.addEventListener('click', () => {
  modal.close();
  modalContent.replaceChildren();
});
let seenInitialEvent = false;
new EventSource("/events").onmessage = () => {
  if (seenInitialEvent) {
    location.reload();
    return;
  }
  seenInitialEvent = true;
};
</script>"#
}
