//! Unified EventCore backend contract suite for Tiber's Git adapter.

use eventcore_fs::{FileCheckpointStore, FileProjectorCoordinator};
use eventcore_testing::contract::{backend_contract_tests, ContractTestEvent};
use eventcore_types::{
    collect_events, BatchSize, CheckpointStore, CommandStateSnapshot, CommandStateSnapshotId,
    EventFilter, EventPage, EventReader, EventStore, ProjectorCoordinator, StreamId,
    StreamPosition, StreamVersion, StreamWrites,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tiber_git::git_event_store::{GitEventStore, SynchronizeOutcome};

struct TempGitStore {
    _dir: TempDir,
    repository: PathBuf,
    origin: PathBuf,
    inner: GitEventStore,
}

impl eventcore_types::EventStore for TempGitStore {
    async fn read_stream<E: eventcore_types::Event>(
        &self,
        stream_id: eventcore_types::StreamId,
    ) -> Result<eventcore_types::EventStream<E>, eventcore_types::EventStoreError> {
        self.inner.read_stream(stream_id).await
    }

    async fn append_events(
        &self,
        writes: eventcore_types::StreamWrites,
    ) -> Result<eventcore_types::EventStreamSlice, eventcore_types::EventStoreError> {
        self.inner.append_events(writes).await
    }

    async fn load_command_state_snapshot(
        &self,
        snapshot_id: CommandStateSnapshotId,
    ) -> Result<Option<CommandStateSnapshot>, eventcore_types::EventStoreError> {
        self.inner.load_command_state_snapshot(snapshot_id).await
    }

    async fn save_command_state_snapshot(
        &self,
        snapshot_id: CommandStateSnapshotId,
        snapshot: CommandStateSnapshot,
    ) -> Result<(), eventcore_types::EventStoreError> {
        self.inner
            .save_command_state_snapshot(snapshot_id, snapshot)
            .await
    }
}

impl eventcore_types::EventReader for TempGitStore {
    type Error = eventcore_types::EventStoreError;

    async fn read_events<E: eventcore_types::Event>(
        &self,
        filter: eventcore_types::EventFilter,
        page: eventcore_types::EventPage,
    ) -> Result<Vec<(E, StreamPosition)>, Self::Error> {
        self.inner.read_events(filter, page).await
    }
}

struct TempCheckpoint {
    _dir: TempDir,
    inner: FileCheckpointStore,
}

impl CheckpointStore for TempCheckpoint {
    type Error = <FileCheckpointStore as CheckpointStore>::Error;

    async fn load(&self, name: &str) -> Result<Option<StreamPosition>, Self::Error> {
        self.inner.load(name).await
    }

    async fn save(&self, name: &str, position: StreamPosition) -> Result<(), Self::Error> {
        self.inner.save(name, position).await
    }
}

struct TempCoordinator {
    _dir: TempDir,
    inner: FileProjectorCoordinator,
}

impl ProjectorCoordinator for TempCoordinator {
    type Error = <FileProjectorCoordinator as ProjectorCoordinator>::Error;
    type Guard = <FileProjectorCoordinator as ProjectorCoordinator>::Guard;

    async fn try_acquire(&self, subscription_name: &str) -> Result<Self::Guard, Self::Error> {
        self.inner.try_acquire(subscription_name).await
    }
}

fn make_store() -> TempGitStore {
    let dir = TempDir::new().expect("create temporary Git event store");
    let repository = dir.path().join("repository");
    let origin = dir.path().join("origin.git");
    let signing_key = dir.path().join("signing-key");
    run(
        dir.path(),
        ["init", "--bare", origin.to_str().expect("UTF-8 origin")],
    );
    run(
        dir.path(),
        ["init", repository.to_str().expect("UTF-8 repository")],
    );
    let key_status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&signing_key)
        .status()
        .expect("run ssh-keygen");
    assert!(
        key_status.success(),
        "generate deterministic test signing setup"
    );
    run(&repository, ["config", "user.name", "Tiber Contract Test"]);
    run(
        &repository,
        ["config", "user.email", "tiber-contract@example.invalid"],
    );
    run(&repository, ["config", "gpg.format", "ssh"]);
    run(&repository, ["config", "commit.gpgsign", "true"]);
    run(
        &repository,
        [
            "config",
            "user.signingkey",
            signing_key.to_str().expect("UTF-8 signing key"),
        ],
    );
    run(
        &repository,
        [
            "remote",
            "add",
            "origin",
            origin.to_str().expect("UTF-8 origin"),
        ],
    );
    let inner = GitEventStore::open(&repository).expect("open Git event store");
    TempGitStore {
        _dir: dir,
        repository,
        origin,
        inner,
    }
}

fn run<const N: usize>(repository: &Path, arguments: [&str; N]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("run Git command");
    assert!(
        output.status.success(),
        "Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_checkpoint_store() -> TempCheckpoint {
    let dir = TempDir::new().expect("create temporary checkpoint store");
    let inner = FileCheckpointStore::open(dir.path()).expect("open checkpoint store");
    TempCheckpoint { _dir: dir, inner }
}

fn make_coordinator() -> TempCoordinator {
    let dir = TempDir::new().expect("create temporary projector coordinator");
    let inner = FileProjectorCoordinator::open(dir.path()).expect("open projector coordinator");
    TempCoordinator { _dir: dir, inner }
}

backend_contract_tests! {
    suite = git_event_store,
    make_store = || { crate::make_store() },
    make_checkpoint_store = || { crate::make_checkpoint_store() },
    make_coordinator = || { crate::make_coordinator() },
}

#[tokio::test]
async fn first_append_creates_one_signed_authoritative_branch() {
    let fixture = make_store();
    let stream_id = StreamId::try_new("tiber:repository").expect("valid stream id");
    let writes = StreamWrites::new()
        .register_stream(stream_id.clone(), StreamVersion::new(0))
        .expect("register stream")
        .append(ContractTestEvent::new(stream_id))
        .expect("append event");

    fixture
        .append_events(writes)
        .await
        .expect("publish first event transaction");

    let candidate = git_output(
        fixture.origin.parent().expect("origin parent"),
        [
            "--git-dir",
            fixture.origin.to_str().expect("UTF-8 origin"),
            "rev-parse",
            "refs/heads/tiber",
        ],
    );
    let commit = git_output(
        &fixture.repository,
        ["cat-file", "commit", candidate.trim()],
    );
    assert!(commit.contains("gpgsig -----BEGIN SSH SIGNATURE-----"));
    assert_eq!(
        git_output(
            &fixture.repository,
            [
                "show",
                "--no-patch",
                "--no-show-signature",
                "--format=%an <%ae>|%cn <%ce>",
                candidate.trim(),
            ],
        )
        .trim(),
        "Tiber Contract Test <tiber-contract@example.invalid>|\
         Tiber Contract Test <tiber-contract@example.invalid>"
    );

    let refs = git_output(
        fixture.origin.parent().expect("origin parent"),
        [
            "--git-dir",
            fixture.origin.to_str().expect("UTF-8 origin"),
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
        ],
    );
    assert_eq!(refs.trim(), "refs/heads/tiber");
    let tree = git_output(
        &fixture.repository,
        ["ls-tree", "-r", "--name-only", candidate.trim()],
    );
    assert!(tree
        .lines()
        .all(|path| path.starts_with("eventstore/events/")));
}

#[tokio::test]
async fn corrupt_command_state_snapshot_is_discarded_and_rebuilt() {
    let fixture = make_store();
    let snapshot_id = CommandStateSnapshotId::try_new("contract:corrupt-recovery".to_owned())
        .expect("valid snapshot id");
    let snapshot = CommandStateSnapshot::new(
        serde_json::json!({"decision": "ready"}),
        std::collections::HashMap::new(),
    );
    fixture
        .inner
        .save_command_state_snapshot(snapshot_id.clone(), snapshot.clone())
        .await
        .expect("initial snapshot save");
    let directory = fixture
        .repository
        .join(".git/tiber/command-state-snapshots");
    let path = fs::read_dir(&directory)
        .expect("snapshot directory")
        .next()
        .expect("snapshot entry")
        .expect("read snapshot entry")
        .path();
    fs::write(&path, b"{truncated").expect("corrupt reconstructible snapshot");
    assert!(fixture
        .inner
        .load_command_state_snapshot(snapshot_id.clone())
        .await
        .expect("corrupt cache is not authoritative")
        .is_none());
    fixture
        .inner
        .save_command_state_snapshot(snapshot_id.clone(), snapshot)
        .await
        .expect("reconstructed snapshot replaces corruption");
    assert!(fixture
        .inner
        .load_command_state_snapshot(snapshot_id)
        .await
        .expect("rebuilt snapshot loads")
        .is_some());
}

#[tokio::test]
async fn two_writers_publish_one_candidate_and_report_one_version_conflict() {
    let fixture = make_store();
    let left = fixture.inner.clone();
    let right = GitEventStore::open(&fixture.repository).expect("open second writer");
    let stream_id = StreamId::try_new("tiber:board").expect("valid stream id");
    let make_writes = || {
        StreamWrites::new()
            .register_stream(stream_id.clone(), StreamVersion::new(0))
            .expect("register stream")
            .append(ContractTestEvent::new(stream_id.clone()))
            .expect("append event")
    };

    let (left_result, right_result) = tokio::join!(
        left.append_events(make_writes()),
        right.append_events(make_writes())
    );
    let results = [left_result, right_result];

    let errors = results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .collect::<Vec<_>>();
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "{errors:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(eventcore_types::EventStoreError::VersionConflict { .. })
            ))
            .count(),
        1,
        "{errors:?}"
    );
}

#[tokio::test]
async fn disjoint_stream_ref_race_is_truthful_and_succeeds_after_rebase() {
    let fixture = make_store();
    let left = fixture.inner.clone();
    let right = GitEventStore::open(&fixture.repository).expect("open second writer");
    let left_stream = StreamId::try_new("tiber:task:left").unwrap();
    let right_stream = StreamId::try_new("tiber:task:right").unwrap();
    let (left_result, right_result) = tokio::join!(
        left.append_events(writes_at(left_stream.as_ref(), 0)),
        right.append_events(writes_at(right_stream.as_ref(), 0))
    );
    left_result.expect("left disjoint transaction");
    right_result.expect("right disjoint transaction rebased by immutable union");
    let reopened = GitEventStore::open(&fixture.repository).unwrap();
    assert_eq!(
        collect_events(
            reopened
                .read_stream::<ContractTestEvent>(left_stream)
                .await
                .unwrap()
        )
        .await
        .unwrap()
        .len(),
        1
    );
    assert_eq!(
        collect_events(
            reopened
                .read_stream::<ContractTestEvent>(right_stream)
                .await
                .unwrap()
        )
        .await
        .unwrap()
        .len(),
        1
    );
    let ordered = reopened
        .read_events::<ContractTestEvent>(EventFilter::all(), EventPage::first(BatchSize::new(10)))
        .await
        .unwrap();
    assert_eq!(
        ordered.len(),
        2,
        "immutable transaction union is exactly once"
    );
    assert!(ordered[0].1 < ordered[1].1, "global read order is stable");
}

#[tokio::test]
async fn rejected_push_is_retried_as_the_same_candidate_by_synchronize() {
    let fixture = make_store();
    install_rejecting_hook(&fixture.origin);
    let error = fixture.inner.append_events(writes("tiber:pending")).await;
    assert!(error.is_err());
    let marker = pending_marker(&fixture.repository);
    let before = fs::read_to_string(&marker).expect("pending marker");
    let workflow_marker = fixture.repository.join(".git/tiber/workflow-blocker.json");
    let workflow: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&workflow_marker).expect("workflow blocker"))
            .expect("workflow blocker JSON");
    assert_eq!(workflow["error_code"], "tiber.publication_failed");

    fs::remove_file(fixture.origin.join("hooks/pre-receive")).expect("remove hook");
    assert_eq!(
        fixture
            .inner
            .synchronize()
            .await
            .expect("recover publication"),
        SynchronizeOutcome::PublishedPending
    );
    assert!(!marker.exists());
    assert!(!workflow_marker.exists());
    let candidate: serde_json::Value = serde_json::from_str(&before).expect("marker JSON");
    assert_eq!(
        git_output(
            &fixture.repository,
            ["rev-parse", "refs/remotes/origin/tiber"]
        )
        .trim(),
        candidate["candidate"].as_str().expect("candidate")
    );
}

#[tokio::test]
async fn synchronize_recognizes_a_candidate_published_after_a_lost_response() {
    let fixture = make_store();
    install_rejecting_hook(&fixture.origin);
    assert!(fixture
        .inner
        .append_events(writes("tiber:lost"))
        .await
        .is_err());
    let marker: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(pending_marker(&fixture.repository)).expect("pending marker"),
    )
    .expect("marker JSON");
    fs::remove_file(fixture.origin.join("hooks/pre-receive")).expect("remove hook");
    run(
        &fixture.repository,
        [
            "push",
            "origin",
            &format!("{}:refs/heads/tiber", marker["candidate"].as_str().unwrap()),
        ],
    );

    assert_eq!(
        fixture.inner.synchronize().await.unwrap(),
        SynchronizeOutcome::PublishedPending
    );
}

#[tokio::test]
async fn synchronize_discards_a_candidate_excluded_by_remote_advance() {
    let fixture = make_store();
    fixture
        .inner
        .append_events(writes("tiber:base"))
        .await
        .unwrap();
    install_rejecting_hook(&fixture.origin);
    assert!(fixture
        .inner
        .append_events(writes_at("tiber:pending", 0))
        .await
        .is_err());
    fs::remove_file(fixture.origin.join("hooks/pre-receive")).expect("remove hook");
    let base = git_output(
        &fixture.repository,
        ["rev-parse", "refs/remotes/origin/tiber"],
    );
    let tree = git_output(
        &fixture.repository,
        ["rev-parse", &format!("{}^{{tree}}", base.trim())],
    );
    let rival = git_output_dynamic(
        &fixture.repository,
        &[
            "commit-tree",
            tree.trim(),
            "-S",
            "-p",
            base.trim(),
            "-m",
            "rival advance",
        ],
    );
    run(
        &fixture.repository,
        [
            "push",
            "origin",
            &format!("{}:refs/heads/tiber", rival.trim()),
        ],
    );

    assert_eq!(
        fixture.inner.synchronize().await.unwrap(),
        SynchronizeOutcome::DiscardedUnpublished
    );
    assert!(!pending_marker(&fixture.repository).exists());
    assert!(!fixture
        .repository
        .join(".git/tiber/workflow-blocker.json")
        .exists());
}

#[tokio::test]
async fn unavailable_remote_and_malformed_marker_remain_blocking() {
    let fixture = make_store();
    install_rejecting_hook(&fixture.origin);
    assert!(fixture
        .inner
        .append_events(writes("tiber:pending"))
        .await
        .is_err());
    let unavailable = fixture.origin.with_extension("offline");
    fs::rename(&fixture.origin, &unavailable).expect("make origin unavailable");
    assert!(fixture.inner.synchronize().await.is_err());
    assert!(pending_marker(&fixture.repository).exists());
    fs::rename(&unavailable, &fixture.origin).expect("restore origin");

    fs::write(pending_marker(&fixture.repository), "not-json\n").expect("corrupt marker");
    assert!(fixture.inner.synchronize().await.is_err());
    assert!(pending_marker(&fixture.repository).exists());
}

#[tokio::test]
async fn missing_pending_candidate_remains_blocking() {
    let fixture = make_store();
    let marker = pending_marker(&fixture.repository);
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(
        &marker,
        r#"{"version":1,"candidate":"0000000000000000000000000000000000000000","base":null,"authority":"origin"}"#,
    )
    .unwrap();
    assert!(fixture.inner.synchronize().await.is_err());
    assert!(marker.exists());
}

#[tokio::test]
async fn replica_identity_is_stable_across_refreshes() {
    let fixture = make_store();
    let replica = fixture.repository.join(".git/tiber/replica-id");
    let before = fs::read_to_string(&replica).expect("replica identity");
    fixture
        .inner
        .append_events(writes("tiber:first"))
        .await
        .unwrap();
    fixture.inner.synchronize().await.unwrap();
    let reopened = GitEventStore::open(&fixture.repository).expect("reopen store");
    reopened
        .append_events(writes("tiber:second"))
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(replica).unwrap(), before);
}

#[tokio::test]
async fn local_only_synchronize_reports_current() {
    let fixture = make_store();
    run(&fixture.repository, ["remote", "remove", "origin"]);
    let store = GitEventStore::open(&fixture.repository).expect("local store");
    store.append_events(writes("tiber:local")).await.unwrap();
    assert_eq!(
        store.synchronize().await.unwrap(),
        SynchronizeOutcome::Current
    );
}

#[tokio::test]
async fn local_only_synchronize_publishes_the_exact_pending_candidate() {
    let fixture = make_store();
    fixture
        .inner
        .append_events(writes("tiber:base"))
        .await
        .unwrap();
    let base = git_output(
        &fixture.repository,
        ["rev-parse", "refs/remotes/origin/tiber"],
    );
    install_rejecting_hook(&fixture.origin);
    assert!(fixture
        .inner
        .append_events(writes("tiber:pending"))
        .await
        .is_err());
    let marker_path = pending_marker(&fixture.repository);
    let mut marker: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
    marker["authority"] = serde_json::Value::String("local".into());
    fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
    run(
        &fixture.repository,
        ["update-ref", "refs/heads/tiber", base.trim()],
    );
    run(&fixture.repository, ["remote", "remove", "origin"]);
    let store = GitEventStore::open(&fixture.repository).expect("local store");
    assert_eq!(
        store.synchronize().await.unwrap(),
        SynchronizeOutcome::PublishedPending
    );
    assert_eq!(
        git_output(&fixture.repository, ["rev-parse", "refs/heads/tiber"]).trim(),
        marker["candidate"].as_str().unwrap()
    );
}

fn writes(stream: &str) -> StreamWrites {
    writes_at(stream, 0)
}

fn writes_at(stream: &str, version: usize) -> StreamWrites {
    let stream_id = StreamId::try_new(stream).expect("valid stream id");
    StreamWrites::new()
        .register_stream(stream_id.clone(), StreamVersion::new(version))
        .unwrap()
        .append(ContractTestEvent::new(stream_id))
        .unwrap()
}

fn pending_marker(repository: &Path) -> PathBuf {
    repository.join(".git/tiber/pending-publication")
}

fn install_rejecting_hook(origin: &Path) {
    let hook = origin.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).expect("chmod hook");
    }
}

fn git_output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("run Git command");
    assert!(
        output.status.success(),
        "Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn git_output_dynamic(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
