pub mod support;

use support::{assert_success, assert_success_ref, task_stem, TempRepo};

#[test]
fn link_and_unlink_maintain_reciprocal_dependencies() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Build API"]));
    assert_success(repo.tiber(["create", "Build UI"]));
    let api_stem = task_stem(&repo, "backlog", "build-api");
    let ui_stem = task_stem(&repo, "backlog", "build-ui");

    let link = repo.tiber(["link", "build-api", "blocks", "build-ui"]);

    assert_success(link);
    let api = repo.tiber(["show", "build-api"]);
    let ui = repo.tiber(["show", "build-ui"]);
    assert_success_ref(&api);
    assert_success_ref(&ui);
    assert!(String::from_utf8(api.stdout)
        .expect("api task should be utf8")
        .contains(&format!("blocks: [{ui_stem}]")));
    assert!(String::from_utf8(ui.stdout)
        .expect("ui task should be utf8")
        .contains(&format!("blocked_by: [{api_stem}]")));

    let unlink = repo.tiber(["unlink", "build-api", "blocks", "build-ui"]);

    assert_success(unlink);
    let api = repo.tiber(["show", "build-api"]);
    let ui = repo.tiber(["show", "build-ui"]);
    assert_success_ref(&api);
    assert_success_ref(&ui);
    assert!(String::from_utf8(api.stdout)
        .expect("api task should be utf8")
        .contains("blocks: []"));
    assert!(String::from_utf8(ui.stdout)
        .expect("ui task should be utf8")
        .contains("blocked_by: []"));
}
