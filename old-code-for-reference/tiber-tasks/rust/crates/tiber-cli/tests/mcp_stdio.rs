pub mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use support::{assert_success, task_stem, TempRepo};

#[test]
fn mcp_uses_codex_sandbox_metadata_when_started_from_an_installed_plugin_root() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Inherited repository root"]));
    let plugin_root = tempfile::tempdir().expect("create installed plugin root");
    assert_success(
        Command::new("git")
            .args(["init"])
            .current_dir(plugin_root.path())
            .output()
            .expect("initialize plugin-root repository"),
    );
    assert_success(
        Command::new(env!("CARGO_BIN_EXE_tiber"))
            .arg("init")
            .current_dir(plugin_root.path())
            .output()
            .expect("initialize plugin-root Tiber board"),
    );
    std::fs::create_dir_all(plugin_root.path().join(".codex-plugin"))
        .expect("create Codex plugin manifest directory");
    std::fs::write(
        plugin_root.path().join(".codex-plugin/plugin.json"),
        r#"{"name":"development-system"}"#,
    )
    .expect("write Codex plugin manifest");
    let mut child = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args(["mcp", "stdio"])
        .current_dir(plugin_root.path())
        .env("PWD", plugin_root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tiber MCP from installed plugin root");
    let mut stdin = child.stdin.take().expect("mcp stdin should be available");
    let mut stdout = BufReader::new(child.stdout.take().expect("mcp stdout should be available"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"codex","version":"0.147.0"}}}"#,
    );
    let initialize = read_json_message(&mut stdout);
    assert!(
        initialize["result"]["capabilities"]["experimental"]
            .get("codex/sandbox-state-meta")
            .is_some(),
        "Tiber must request Codex's per-call sandbox metadata"
    );

    let repository_uri = format!("file://{}", repo.path().display());
    write_message(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "tiber.list",
                "arguments": {"status": "backlog"},
                "_meta": {
                    "codex/sandbox-state-meta": {
                        "sandboxCwd": repository_uri
                    }
                }
            }
        })
        .to_string(),
    );
    let response = read_message(&mut stdout);

    assert!(response.contains("Inherited repository root"));
    assert!(!response.contains("tiber.repository_not_found"));
    drop(stdin);
    assert!(child.wait().expect("wait for mcp server").success());
}

#[test]
fn mcp_admissions_return_the_shared_backlog_capacity_refusal() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Completed work"]));
    assert_success(repo.tiber(["transition", "completed-work", "done"]));
    fs::write(
        repo.path().join(".tiber.toml"),
        "[backlog]\nmax_queued = 1\n",
    )
    .expect("write tiber config");
    assert_success(repo.tiber(["create", "Queued work"]));
    let mut child = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args(["mcp", "stdio"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tiber mcp stdio");
    let mut stdin = child.stdin.take().expect("mcp stdin should be available");
    let stdout = child.stdout.take().expect("mcp stdout should be available");
    let mut stdout = BufReader::new(stdout);

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tiber.create","arguments":{"title":"Overflow through MCP"}}}"#,
    );
    let create = read_message(&mut stdout);

    assert!(create.contains(r#""id":1"#));
    assert!(create.contains("backlog_capacity_exceeded"));
    assert!(create.contains("queued=1"));
    assert!(create.contains("max_queued=1"));
    assert!(create.contains("replace"));
    assert!(create.contains("combine"));
    assert!(create.contains("reject"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tiber.transition","arguments":{"ref":"completed-work","status":"backlog"}}}"#,
    );
    let reopen = read_message(&mut stdout);
    assert!(reopen.contains(r#""id":2"#));
    assert!(reopen.contains("backlog_capacity_exceeded"));
    task_stem(&repo, "done", "completed-work");

    drop(stdin);
    assert!(child.wait().expect("wait for mcp server").success());
}

#[test]
fn mcp_stdio_exposes_tools_and_task_resources() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Expose MCP task"]));
    let expose_mcp_task = task_stem(&repo, "backlog", "expose-mcp-task");
    assert_success(repo.tiber(["create", "Completed MCP history"]));
    assert_success(repo.tiber([
        "update",
        "completed-mcp-history",
        "--summary",
        "Detect duplicate agent requests",
    ]));
    assert_success(repo.tiber(["transition", "completed-mcp-history", "done"]));
    let completed_mcp_history = task_stem(&repo, "done", "completed-mcp-history");
    let install_target_dir = repo.path().join("bin");
    let launcher = repo.path().join("plugin/bin/tiber");
    std::fs::create_dir_all(launcher.parent().expect("launcher parent"))
        .expect("create launcher dir");
    std::fs::write(&launcher, "#!/usr/bin/env bash\n").expect("write fake launcher");
    std::fs::create_dir_all(repo.path().join("docs/guides")).expect("create docs directory");
    std::fs::write(
        repo.path().join("docs/guides/tiber.md"),
        "# Tiber guide\n\nUse tiber mcp stdio.\n",
    )
    .expect("write doc");
    let mut child = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args(["mcp", "stdio"])
        .current_dir(repo.path())
        .env("TIBER_LAUNCHER_PATH", &launcher)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tiber mcp stdio");
    let mut stdin = child.stdin.take().expect("mcp stdin should be available");
    let stdout = child.stdout.take().expect("mcp stdout should be available");
    let mut stdout = BufReader::new(stdout);

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.0.0"}}}"#,
    );
    let initialize = read_json_message(&mut stdout);
    assert_eq!(initialize["id"], 1);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "tiber");
    assert_eq!(
        initialize["result"]["capabilities"]["tools"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize["result"]["capabilities"]["resources"],
        serde_json::json!({})
    );
    let instructions = initialize["result"]["instructions"]
        .as_str()
        .expect("initialize instructions should be a string");
    assert!(instructions.contains(
        "Mutating task tools publish an EventCore transaction to origin/tiber on success"
    ));
    assert!(instructions.contains("tiber.codex_sandbox_setup"));
    assert!(instructions.contains("case-by-case approval for raw Git prefixes"));
    assert!(instructions.contains("exact Tiber-internal operation"));
    assert!(instructions
        .to_lowercase()
        .contains("do not run the whole tiber mcp server outside the sandbox"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    let tools = read_json_message(&mut stdout);
    assert_eq!(tools["id"], 2);
    let listed_tools = tools["result"]["tools"]
        .as_array()
        .expect("tools/list should return an array");
    for name in [
        "tiber.codex_sandbox_setup",
        "tiber.create",
        "tiber.list",
        "tiber.search",
        "tiber.show",
        "tiber.metadata",
        "tiber.next",
        "tiber.transition",
        "tiber.prioritize",
        "tiber.link",
        "tiber.unlink",
        "tiber.subtask.add",
        "tiber.subtask.check",
        "tiber.subtask.uncheck",
        "tiber.update",
        "tiber.acceptance.add",
        "tiber.acceptance.check",
        "tiber.acceptance.uncheck",
        "tiber.acceptance.remove",
        "tiber.note.add",
        "tiber.validate_fix",
        "tiber.close_from_trailers",
        "tiber.scaffold_repo_dry_run",
        "tiber.install_bin",
    ] {
        assert!(listed_tools.iter().any(|tool| tool["name"] == name));
    }
    let list_tool = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.list")
        .expect("tiber.list should be advertised");
    assert_eq!(
        list_tool["inputSchema"]["properties"]["status"]["enum"],
        serde_json::json!(["backlog", "in-progress", "done", "abandoned"])
    );
    assert_eq!(list_tool["inputSchema"]["required"], serde_json::json!([]));
    let transition_tool = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.transition")
        .expect("tiber.transition should be advertised");
    assert_eq!(
        transition_tool["inputSchema"]["properties"]["status"]["enum"],
        serde_json::json!(["backlog", "in-progress", "done", "abandoned"])
    );
    let show_tool = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.show")
        .expect("tiber.show should be advertised");
    assert!(show_tool["inputSchema"]["properties"]["ref"]["description"]
        .as_str()
        .expect("task ref description")
        .contains("task ID, nickname, or full task stem"));
    let link_tool = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.link")
        .expect("tiber.link should be advertised");
    assert!(link_tool["description"]
        .as_str()
        .expect("link description")
        .contains("from is the blocker and to is the blocked task"));
    assert!(
        link_tool["inputSchema"]["properties"]["from"]["description"]
            .as_str()
            .expect("from description")
            .contains("Blocking task")
    );
    assert!(link_tool["inputSchema"]["properties"]["to"]["description"]
        .as_str()
        .expect("to description")
        .contains("blocked by from"));
    let subtask_check = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.subtask.check")
        .expect("tiber.subtask.check should be advertised");
    assert_eq!(
        subtask_check["inputSchema"]["properties"]["index"]["pattern"],
        "^[1-9][0-9]*$"
    );
    let sync_tool = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.sync")
        .expect("tiber.sync should be advertised");
    assert!(sync_tool["description"]
        .as_str()
        .expect("sync description")
        .contains("publishing any pending local transaction"));
    let validate_fix = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.validate_fix")
        .expect("tiber.validate_fix should be advertised");
    assert!(validate_fix["description"]
        .as_str()
        .expect("validate description")
        .contains("report dependency cycles that still require manual resolution"));
    let close_from_trailers = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.close_from_trailers")
        .expect("tiber.close_from_trailers should be advertised");
    assert!(close_from_trailers["description"]
        .as_str()
        .expect("closure description")
        .contains("current HEAD commit message only"));
    let search_tool = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.search")
        .expect("tiber.search should be advertised");
    assert_eq!(
        search_tool["inputSchema"]["required"],
        serde_json::json!(["query"])
    );
    assert_eq!(
        search_tool["outputSchema"]["properties"]["results"]["items"]["properties"]["status"]
            ["enum"],
        serde_json::json!(["backlog", "in-progress", "done", "abandoned"])
    );

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
    );
    let resources = read_message(&mut stdout);
    assert!(resources.contains(r#""id":3"#));
    assert!(resources.contains(r#""uri":"tasks://board""#));
    assert!(resources.contains(r#""uri":"tasks://codex-sandbox""#));
    assert!(resources.contains(&format!(r#""uri":"tasks://task/{expose_mcp_task}""#)));
    assert!(resources.contains(r#""uri":"tasks://docs/tree""#));
    assert!(resources.contains(r#""uri":"tasks://docs/guides/tiber.md""#));

    write_message(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{{"uri":"tasks://task/{expose_mcp_task}"}}}}"#
        ),
    );
    let task = read_message(&mut stdout);
    assert!(task.contains(r#""id":4"#));
    assert!(task.contains("title: Expose MCP task"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"tiber.create","arguments":{"title":"Created through MCP"}}}"#,
    );
    let create = read_message(&mut stdout);
    assert!(create.contains(r#""id":5"#));
    assert!(create.contains("-created-through-mcp"));
    let created_through_mcp = task_stem(&repo, "backlog", "created-through-mcp");

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"tiber.list","arguments":{}}}"#,
    );
    let list = read_message(&mut stdout);
    assert!(list.contains(r#""id":6"#));
    assert!(list.contains(&expose_mcp_task));
    assert!(list.contains(&created_through_mcp));
    assert!(!list.contains(&completed_mcp_history));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":60,"method":"tools/call","params":{"name":"tiber.list","arguments":{"status":"done"}}}"#,
    );
    let completed = read_json_message(&mut stdout);
    let completed_text = completed["result"]["content"][0]["text"]
        .as_str()
        .expect("completed task listing should be text");
    assert!(completed_text.contains(&completed_mcp_history));
    assert!(!completed_text.contains(&expose_mcp_task));
    assert!(!completed_text.contains(&created_through_mcp));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":61,"method":"tools/call","params":{"name":"tiber.list","arguments":{"status":1}}}"#,
    );
    let malformed_status = read_message(&mut stdout);
    assert!(malformed_status.contains("mcp_argument_invalid name=status"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":62,"method":"tools/call","params":{"name":"tiber.search","arguments":{"query":"duplicate AGENT"}}}"#,
    );
    let search = read_json_message(&mut stdout);
    assert_eq!(
        search["result"]["structuredContent"],
        serde_json::json!({
            "results": [{
                "id": completed_mcp_history,
                "status": "done",
                "title": "Completed MCP history",
                "summary": "Detect duplicate agent requests",
                "context": ""
            }]
        })
    );
    assert_eq!(search["result"]["content"][0]["type"], "text");
    let search_text: serde_json::Value = serde_json::from_str(
        search["result"]["content"][0]["text"]
            .as_str()
            .expect("search text should be JSON"),
    )
    .expect("search text should parse");
    assert_eq!(
        search_text,
        search["result"]["structuredContent"]["results"]
    );

    for (id, arguments, expected_error) in [
        (63, "{}", "mcp_argument_missing name=query"),
        (64, r#"{"query":1}"#, "mcp_argument_invalid name=query"),
    ] {
        write_message(
            &mut stdin,
            &format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"tiber.search","arguments":{arguments}}}}}"#
            ),
        );
        let invalid_search = read_message(&mut stdout);
        assert!(invalid_search.contains(expected_error));
    }

    write_message(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","id":7,"method":"resources/read","params":{{"uri":"tasks://task/{created_through_mcp}"}}}}"#
        ),
    );
    let created = read_message(&mut stdout);
    assert!(created.contains(r#""id":7"#));
    assert!(created.contains("title: Created through MCP"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":8,"method":"resources/read","params":{"uri":"tasks://docs/tree"}}"#,
    );
    let docs_tree = read_message(&mut stdout);
    assert!(docs_tree.contains(r#""id":8"#));
    assert!(docs_tree.contains("docs/guides/tiber.md"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":9,"method":"resources/read","params":{"uri":"tasks://docs/guides/tiber.md"}}"#,
    );
    let doc = read_message(&mut stdout);
    assert!(doc.contains(r#""id":9"#));
    assert!(doc.contains("# Tiber guide"));
    assert!(doc.contains("tiber mcp stdio"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":91,"method":"tools/call","params":{"name":"tiber.codex_sandbox_setup","arguments":{}}}"#,
    );
    let codex_setup_tool = read_message(&mut stdout);
    assert!(codex_setup_tool.contains(r#""id":91"#));
    assert!(codex_setup_tool.contains("Couldn't get agent socket?"));
    assert!(codex_setup_tool.contains("SSH_AUTH_SOCK"));
    assert!(codex_setup_tool.contains("env_vars = [\\\"SSH_AUTH_SOCK\\\"]"));
    assert!(codex_setup_tool.contains("preserves the absolute installed launcher"));
    assert!(codex_setup_tool.contains("Never forward SSH_AUTH_SOCK to a PATH-resolved"));
    assert!(codex_setup_tool.contains("publish event transactions to origin/tiber"));
    assert!(codex_setup_tool.contains(
        "Persist approval only when the harness can scope it to the exact Tiber-internal operation"
    ));
    assert!(codex_setup_tool.contains("Never persist a raw git"));
    assert!(codex_setup_tool.contains("retry the same structured Tiber MCP operation"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":92,"method":"resources/read","params":{"uri":"tasks://codex-sandbox"}}"#,
    );
    let codex_setup_resource = read_message(&mut stdout);
    assert!(codex_setup_resource.contains(r#""id":92"#));
    assert!(
        codex_setup_resource.contains("Do not run the whole Tiber MCP server outside the sandbox")
    );

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"tiber.show","arguments":{"ref":"expose-mcp-task"}}}"#,
    );
    let show = read_message(&mut stdout);
    assert!(show.contains(r#""id":10"#));
    assert!(show.contains("title: Expose MCP task"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"tiber.metadata","arguments":{"ref":"expose-mcp-task"}}}"#,
    );
    let metadata = read_message(&mut stdout);
    assert!(metadata.contains(r#""id":11"#));
    assert!(metadata.contains(&format!(
        "{expose_mcp_task}\\tExpose MCP task\\tcommitted_at=20"
    )));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"tiber.next","arguments":{}}}"#,
    );
    let next = read_message(&mut stdout);
    assert!(next.contains(r#""id":12"#));
    assert!(next.contains(&expose_mcp_task));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"tiber.subtask.add","arguments":{"ref":"created-through-mcp","title":"Write MCP mirror tests"}}}"#,
    );
    let subtask_add = read_message(&mut stdout);
    assert!(subtask_add.contains(r#""id":13"#));
    assert!(subtask_add.contains("updated created-through-mcp"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"tiber.subtask.check","arguments":{"ref":"created-through-mcp","index":"1"}}}"#,
    );
    let subtask_check = read_message(&mut stdout);
    assert!(subtask_check.contains(r#""id":14"#));
    assert!(subtask_check.contains("updated created-through-mcp"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":141,"method":"tools/call","params":{"name":"tiber.subtask.add","arguments":{"ref":"created-through-mcp","title":"Wire dependency","after":["s1"]}}}"#,
    );
    let dependent_subtask_add = read_message(&mut stdout);
    assert!(dependent_subtask_add.contains(r#""id":141"#));
    assert!(dependent_subtask_add.contains("updated created-through-mcp"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"tiber.transition","arguments":{"ref":"created-through-mcp","status":"in-progress"}}}"#,
    );
    let transition = read_message(&mut stdout);
    assert!(transition.contains(r#""id":15"#));
    assert!(transition.contains(&created_through_mcp));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"tiber.link","arguments":{"from":"created-through-mcp","to":"expose-mcp-task"}}}"#,
    );
    let link = read_message(&mut stdout);
    assert!(link.contains(r#""id":16"#));
    assert!(link.contains("linked created-through-mcp blocks expose-mcp-task"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":17,"method":"tools/call","params":{"name":"tiber.unlink","arguments":{"from":"created-through-mcp","to":"expose-mcp-task"}}}"#,
    );
    let unlink = read_message(&mut stdout);
    assert!(unlink.contains(r#""id":17"#));
    assert!(unlink.contains("unlinked created-through-mcp blocks expose-mcp-task"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":18,"method":"tools/call","params":{"name":"tiber.prioritize","arguments":{"ref":"created-through-mcp","before":"expose-mcp-task"}}}"#,
    );
    let prioritize = read_message(&mut stdout);
    assert!(prioritize.contains(r#""id":18"#));
    assert!(prioritize.contains("prioritized created-through-mcp before expose-mcp-task"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":181,"method":"tools/call","params":{"name":"tiber.update","arguments":{"ref":"created-through-mcp","summary":"MCP summary line one\nline two with literal \\\\n text","context":"MCP context line one\ncontext line two","tags":["mcp","structured"]}}}"#,
    );
    let update = read_message(&mut stdout);
    assert!(update.contains(r#""id":181"#));
    assert!(update.contains("updated created-through-mcp"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":182,"method":"tools/call","params":{"name":"tiber.acceptance.add","arguments":{"ref":"created-through-mcp","criterion":"MCP criterion"}}}"#,
    );
    let acceptance_add = read_message(&mut stdout);
    assert!(acceptance_add.contains(r#""id":182"#));
    assert!(acceptance_add.contains("updated created-through-mcp"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":183,"method":"tools/call","params":{"name":"tiber.acceptance.check","arguments":{"ref":"created-through-mcp","index":"1"}}}"#,
    );
    let acceptance_check = read_message(&mut stdout);
    assert!(acceptance_check.contains(r#""id":183"#));
    assert!(acceptance_check.contains("updated created-through-mcp"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":184,"method":"tools/call","params":{"name":"tiber.note.add","arguments":{"ref":"created-through-mcp","note":"MCP note"}}}"#,
    );
    let note_add = read_message(&mut stdout);
    assert!(note_add.contains(r#""id":184"#));
    assert!(note_add.contains("updated created-through-mcp"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":185,"method":"tools/call","params":{"name":"tiber.show","arguments":{"ref":"created-through-mcp"}}}"#,
    );
    let structured_show = read_message(&mut stdout);
    assert!(structured_show.contains(r#""id":185"#));
    assert!(structured_show.contains("MCP summary line one\\nline two with literal \\\\\\\\n text"));
    assert!(structured_show.contains("MCP context line one\\ncontext line two"));
    assert!(structured_show.contains("tags: [mcp, structured]"));
    assert!(structured_show.contains("- [x] MCP criterion"));
    assert!(structured_show.contains("- [ ] (s2) Wire dependency — after: s1"));
    assert!(structured_show.contains("MCP note"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":19,"method":"tools/call","params":{"name":"tiber.validate_fix","arguments":{}}}"#,
    );
    let validate = read_message(&mut stdout);
    assert!(validate.contains(r#""id":19"#));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"tiber.scaffold_repo_dry_run","arguments":{}}}"#,
    );
    let scaffold = read_message(&mut stdout);
    assert!(scaffold.contains(r#""id":20"#));

    write_message(&mut stdin, r#"{"jsonrpc":"2.0","id":21}"#);
    let missing_method = read_json_message(&mut stdout);
    assert_eq!(missing_method["id"], 21);
    assert_eq!(missing_method["error"]["code"], -32600);
    assert!(missing_method["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("mcp_method_missing"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"arguments":{}}}"#,
    );
    let missing_tool_name = read_json_message(&mut stdout);
    assert_eq!(missing_tool_name["id"], 22);
    assert_eq!(missing_tool_name["error"]["code"], -32602);
    assert!(missing_tool_name["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("mcp_tool_name_missing"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":23,"method":"resources/read","params":{}}"#,
    );
    let missing_resource_uri = read_json_message(&mut stdout);
    assert_eq!(missing_resource_uri["id"], 23);
    assert_eq!(missing_resource_uri["error"]["code"], -32602);
    assert!(missing_resource_uri["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("mcp_resource_uri_missing"));

    write_message(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{{"name":"tiber.install_bin","arguments":{{"target_dir":"{}","apply":false}}}}}}"#,
            install_target_dir.display()
        ),
    );
    let install_bin = read_message(&mut stdout);
    assert!(install_bin.contains(r#""id":24"#));
    assert!(install_bin.contains(&format!(
        "would install {} -> {}",
        install_target_dir.join("tiber").display(),
        launcher.display()
    )));
    assert!(!install_target_dir.join("tiber").exists());

    drop(stdin);
    let status = child.wait().expect("wait for mcp process");
    assert!(status.success());
}

#[test]
fn mcp_stdio_exposes_strict_structured_ci_recovery_tools() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let repo = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(repo.path())
            .output()
            .expect("add origin remote"),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args(["mcp", "stdio"])
        .current_dir(repo.path())
        .env("TIBER_CLAIM_HOST", "mcp-host")
        .env("TIBER_CLAIM_SESSION", "mcp-session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tiber mcp stdio");
    let mut stdin = child.stdin.take().expect("mcp stdin should be available");
    let stdout = child.stdout.take().expect("mcp stdout should be available");
    let mut stdout = BufReader::new(stdout);

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let tools = read_json_message(&mut stdout);
    let listed_tools = tools["result"]["tools"]
        .as_array()
        .expect("tools/list should return an array");
    for name in [
        "tiber.ci_recovery.claim",
        "tiber.ci_recovery.status",
        "tiber.ci_recovery.assert_owner",
        "tiber.ci_recovery.heartbeat",
        "tiber.ci_recovery.transfer",
        "tiber.ci_recovery.takeover",
        "tiber.ci_recovery.assign",
        "tiber.ci_recovery.report",
        "tiber.ci_recovery.wait",
        "tiber.ci_recovery.diagnose",
        "tiber.ci_recovery.choose_action",
        "tiber.ci_recovery.record_replacement",
        "tiber.ci_recovery.resolve",
    ] {
        let tool = listed_tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} should be advertised"));
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["outputSchema"]["type"], "object");
        assert_eq!(tool["outputSchema"]["additionalProperties"], false);
    }
    let assign = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.ci_recovery.assign")
        .expect("tiber.ci_recovery.assign should be advertised");
    assert_eq!(
        assign["inputSchema"]["properties"]["capabilities"]["items"]["enum"],
        serde_json::json!(["inspect", "reproduce", "edit", "test"])
    );
    assert_eq!(
        assign["inputSchema"]["properties"]["capabilities"]["type"],
        "array"
    );
    assert_eq!(
        assign["inputSchema"]["properties"]["capabilities"]["uniqueItems"],
        true
    );

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tiber.ci_recovery.claim","arguments":{"run_id":"123","run_url":"https://forge.example.invalid/runs/123","failed_sha":"abcdef0123456789","workflow":"CI","git_ref":"refs/heads/main","unexpected":true}}}"#,
    );
    let unexpected_argument = read_json_message(&mut stdout);
    assert!(unexpected_argument["error"]["message"]
        .as_str()
        .expect("unexpected argument error should be text")
        .contains("mcp_argument_unexpected name=unexpected"));

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tiber.ci_recovery.claim","arguments":{"run_id":"123","run_url":"https://forge.example.invalid/runs/123","failed_sha":"abcdef0123456789","workflow":"CI","git_ref":"refs/heads/main"}}}"#,
    );
    let claim = read_json_message(&mut stdout);
    assert_eq!(claim["result"]["structuredContent"]["role"], "owner");
    assert_eq!(
        claim["result"]["structuredContent"]["incident_id"],
        "ci-123"
    );
    assert_eq!(claim["result"]["content"][0]["type"], "text");

    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tiber.ci_recovery.status","arguments":{}}}"#,
    );
    let status = read_json_message(&mut stdout);
    assert_eq!(
        status["result"]["structuredContent"]["incident_id"],
        "ci-123"
    );
    assert_eq!(
        status["result"]["structuredContent"]["hold_released"],
        false
    );
    assert_eq!(status["result"]["structuredContent"]["trigger_count"], 1);
    assert_eq!(
        status["result"]["structuredContent"]["owner"]["session"],
        "mcp-session"
    );
    let status_tool = listed_tools
        .iter()
        .find(|tool| tool["name"] == "tiber.ci_recovery.status")
        .expect("tiber.ci_recovery.status should be advertised");
    assert_eq!(
        status_tool["outputSchema"]["properties"]["trigger_count"]["type"],
        "integer"
    );
    assert_eq!(
        status_tool["outputSchema"]["properties"]["owner"]["additionalProperties"],
        false
    );
    assert_eq!(
        status_tool["outputSchema"]["properties"]["assignments"]["items"]["properties"]["assignee"]
            ["additionalProperties"],
        false
    );
    assert_eq!(
        status_tool["outputSchema"]["properties"]["release_proof"]["anyOf"][1]
            ["additionalProperties"],
        false
    );

    drop(stdin);
    assert!(child.wait().expect("wait for mcp process").success());
}

#[test]
fn mcp_stdio_generates_a_process_stable_ci_identity_when_harness_identity_is_absent() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let repo = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(repo.path())
            .output()
            .expect("add origin remote"),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args(["mcp", "stdio"])
        .current_dir(repo.path())
        .env("TIBER_CLAIM_HOST", "mcp-host")
        .env_remove("TIBER_CLAIM_SESSION")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CLAUDE_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tiber mcp stdio");
    let mut stdin = child.stdin.take().expect("mcp stdin");
    let stdout = child.stdout.take().expect("mcp stdout");
    let mut stdout = BufReader::new(stdout);

    let claim = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tiber.ci_recovery.claim","arguments":{"run_id":"generated","run_url":"https://example.invalid/runs/generated","failed_sha":"abcdef0123456789","workflow":"CI","git_ref":"refs/heads/main"}}}"#;
    write_message(&mut stdin, claim);
    let first = read_json_message(&mut stdout);
    assert_eq!(first["result"]["structuredContent"]["role"], "owner");
    write_message(&mut stdin, &claim.replace(r#""id":1"#, r#""id":2"#));
    let second = read_json_message(&mut stdout);
    assert_eq!(second["result"]["structuredContent"]["role"], "owner");
    assert_eq!(
        first["result"]["structuredContent"]["incident_id"],
        second["result"]["structuredContent"]["incident_id"]
    );
    write_message(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tiber.ci_recovery.status","arguments":{}}}"#,
    );
    let status = read_json_message(&mut stdout);
    assert!(status["result"]["structuredContent"]["owner"]["session"]
        .as_str()
        .expect("generated session")
        .starts_with("tiber-mcp-"));

    drop(stdin);
    assert!(child.wait().expect("wait for mcp server").success());
}

#[test]
fn failed_mcp_claim_returns_structured_blocker_and_successful_retry_clears_it() {
    let repo = TempRepo::initialized();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args(["mcp", "stdio"])
        .current_dir(repo.path())
        .env_remove("TIBER_CLAIM_SESSION")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CLAUDE_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tiber mcp stdio");
    let mut stdin = child.stdin.take().expect("mcp stdin");
    let stdout = child.stdout.take().expect("mcp stdout");
    let mut stdout = BufReader::new(stdout);
    let claim = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tiber.ci_recovery.claim","arguments":{"run_id":"blocked","run_url":"https://example.invalid/runs/blocked","failed_sha":"abcdef0123456789","workflow":"CI","git_ref":"refs/heads/main"}}}"#;

    write_message(&mut stdin, claim);
    let failed = read_json_message(&mut stdout);
    assert_eq!(failed["error"]["data"]["workflow_blocked"], true);
    assert_eq!(
        failed["error"]["data"]["error_code"],
        "tiber.ci_recovery_claim_failed"
    );
    assert!(repo
        .path()
        .join(".git/tiber/workflow-blocker.json")
        .is_file());

    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(repo.path())
            .output()
            .expect("add origin"),
    );
    write_message(&mut stdin, &claim.replace(r#""id":1"#, r#""id":2"#));
    let retried = read_json_message(&mut stdout);
    assert_eq!(retried["result"]["structuredContent"]["role"], "owner");
    assert!(!repo
        .path()
        .join(".git/tiber/workflow-blocker.json")
        .exists());

    drop(stdin);
    assert!(child.wait().expect("wait for mcp server").success());
}

fn write_message(stdin: &mut impl Write, message: &str) {
    writeln!(stdin, "{message}").expect("write mcp message");
    stdin.flush().expect("flush mcp message");
}

fn read_message(stdout: &mut impl BufRead) -> String {
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read mcp response");
    assert!(!line.is_empty(), "expected MCP response line");
    let parsed: serde_json::Value =
        serde_json::from_str(&line).expect("mcp response should be valid json");
    assert_eq!(parsed["jsonrpc"], "2.0");
    line
}

fn read_json_message(stdout: &mut impl BufRead) -> serde_json::Value {
    let line = read_message(stdout);
    serde_json::from_str(&line).expect("mcp response should be valid json")
}

#[test]
fn mcp_stdio_ignores_json_rpc_notifications() {
    let input = std::io::Cursor::new(
        r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
"#,
    );
    let mut output = Vec::new();

    tiber_mcp::run_stdio(std::io::BufReader::new(input), &mut output).expect("run stdio");

    assert_eq!(output, b"");
}
