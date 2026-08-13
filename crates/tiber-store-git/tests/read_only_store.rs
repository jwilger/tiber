#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::process::ExitStatus;
    use std::{
        env, fs,
        path::{Path, PathBuf},
        process::Command,
    };

    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use eventcore_fs::FileEventStore;
    use eventcore_types::{
        BatchSize, Event as _, EventFilter, EventPage, EventStore as _, EventStoreError, StreamId,
        StreamPattern, StreamVersion, StreamWrites,
    };
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;
    use tiber_store_git::{
        GitCommandFailureKind, GitOperation, Retryability, TIBER_REF, TiberEventStore,
        TransactionHistoryError,
    };

    const COUNTED_EVENT_COUNT: usize = 3;
    const SUBTREE_MATERIALIZATION_MARKER: &str = "subtree materialization fixture event";
    const UNRELATED_ARTIFACT_SIZE_BYTES: usize = 3 * 1024 * 1024;
    static COUNTED_EVENT_DESERIALIZATIONS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct FixtureEvent {
        stream_id: StreamId,
        text: String,
    }

    impl eventcore_types::Event for FixtureEvent {
        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression type name"
        )]
        fn event_type_name() -> &'static str {
            "tiber.store_git.fixture_event"
        }

        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression field accessor"
        )]
        fn stream_id(&self) -> &StreamId {
            &self.stream_id
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq, Serialize)]
    struct CountedFixtureEvent {
        stream_id: StreamId,
        text: String,
    }

    #[derive(Deserialize)]
    struct CountedFixtureEventWire {
        stream_id: StreamId,
        text: String,
    }

    #[expect(
        clippy::missing_trait_methods,
        reason = "the test event needs only ordinary owned deserialization to count public reader decode work"
    )]
    impl<'de> Deserialize<'de> for CountedFixtureEvent {
        #[expect(
            clippy::implicit_return,
            reason = "the counted event returns its reconstructed typed value after recording one successful decode"
        )]
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let wire = match CountedFixtureEventWire::deserialize(deserializer) {
                Ok(wire) => wire,
                Err(error) => return Err(error),
            };
            COUNTED_EVENT_DESERIALIZATIONS.fetch_add(1, Ordering::Relaxed);
            Ok(Self {
                stream_id: wire.stream_id,
                text: wire.text,
            })
        }
    }

    impl eventcore_types::Event for CountedFixtureEvent {
        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression type name"
        )]
        fn event_type_name() -> &'static str {
            "tiber.store_git.counted_fixture_event"
        }

        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression field accessor"
        )]
        fn stream_id(&self) -> &StreamId {
            &self.stream_id
        }
    }

    /// A valid `EventCore` envelope deliberately incompatible with `FixtureEvent`.
    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct IncompatibleFixtureEvent {
        stream_id: StreamId,
        unexpected: String,
    }

    impl eventcore_types::Event for IncompatibleFixtureEvent {
        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression type name"
        )]
        fn event_type_name() -> &'static str {
            FixtureEvent::event_type_name()
        }

        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression field accessor"
        )]
        fn stream_id(&self) -> &StreamId {
            &self.stream_id
        }
    }

    /// A valid persisted event whose payload claims a different stream identity.
    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct StreamMismatchedFixtureEvent {
        envelope_stream_id: StreamId,
        #[serde(rename = "stream_id")]
        payload_stream_id: StreamId,
        text: String,
    }

    impl eventcore_types::Event for StreamMismatchedFixtureEvent {
        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression type name"
        )]
        fn event_type_name() -> &'static str {
            FixtureEvent::event_type_name()
        }

        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression field accessor"
        )]
        fn stream_id(&self) -> &StreamId {
            &self.envelope_stream_id
        }
    }

    /// A future workflow envelope that shares a stream but not a domain schema.
    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct UnrelatedFixtureEvent {
        context: String,
        stream_id: StreamId,
    }

    impl eventcore_types::Event for UnrelatedFixtureEvent {
        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression type name"
        )]
        fn event_type_name() -> &'static str {
            "tiber.store_git.unrelated_fixture_event"
        }

        #[expect(
            clippy::implicit_return,
            reason = "the fixture event trait method is intentionally a one-expression field accessor"
        )]
        fn stream_id(&self) -> &StreamId {
            &self.stream_id
        }
    }

    struct GitFixture {
        directory: TempDir,
        object_format: FixtureObjectFormat,
        repository: PathBuf,
        revision: String,
        stream: StreamId,
    }

    #[derive(Clone, Copy)]
    enum FixtureObjectFormat {
        Sha1,
        Sha256,
    }

    impl FixtureObjectFormat {
        #[expect(
            clippy::implicit_return,
            reason = "the closed fixture format mapping reads most clearly as its final expression"
        )]
        const fn git_init_argument(self) -> &'static str {
            match self {
                Self::Sha1 => "--object-format=sha1",
                Self::Sha256 => "--object-format=sha256",
            }
        }

        #[expect(
            clippy::implicit_return,
            reason = "the fixture format’s expected storage spelling reads most clearly as its final expression"
        )]
        const fn storage_name(self) -> &'static str {
            match self {
                Self::Sha1 => "sha1",
                Self::Sha256 => "sha256",
            }
        }
    }

    impl GitFixture {
        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the fixture keeps origin setup adjacent to the signed history it publishes"
        )]
        fn add_origin(&self, publish_tiber_history: bool) -> PathBuf {
            let origin = self.directory.path().join("origin.git");
            git(
                self.directory.path(),
                [
                    "init",
                    "--bare",
                    self.object_format.git_init_argument(),
                    origin.to_str().expect("UTF-8 origin path"),
                ],
            );
            git(
                &self.repository,
                [
                    "remote",
                    "add",
                    "origin",
                    origin.to_str().expect("UTF-8 origin path"),
                ],
            );
            if publish_tiber_history {
                git(
                    &self.repository,
                    ["push", "origin", "refs/heads/tiber:refs/heads/tiber"],
                );
            }
            origin
        }

        fn commit_event_changes(&self, subject: &str) {
            git(&self.repository, ["add", "-A", "eventstore"]);
            let _revision = commit_and_point_tiber(&self.repository, subject);
        }

        #[expect(
            clippy::implicit_return,
            reason = "the adversarial fixture deliberately constructs one unsigned commit with fail-fast Git assertions"
        )]
        fn point_tiber_at_unsigned_child(&self) -> String {
            let tree = git_output(&self.repository, ["rev-parse", "HEAD^{tree}"]);
            let revision = git_output(
                &self.repository,
                [
                    "commit-tree",
                    tree.as_str(),
                    "-p",
                    self.revision.as_str(),
                    "-m",
                    "unsigned authority child",
                ],
            );
            git(
                &self.repository,
                ["update-ref", TIBER_REF, revision.as_str()],
            );
            revision
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the fixture keeps a remote-only caller and its signature-verification configuration adjacent to the published authority"
        )]
        fn remote_only_consumer(&self, origin: &Path) -> PathBuf {
            let consumer = self.directory.path().join("remote-only-consumer");
            git(
                self.directory.path(),
                [
                    "init",
                    self.object_format.git_init_argument(),
                    consumer.to_str().expect("UTF-8 consumer path"),
                ],
            );
            git(
                &consumer,
                [
                    "remote",
                    "add",
                    "origin",
                    origin.to_str().expect("UTF-8 origin path"),
                ],
            );
            git(&consumer, ["config", "gpg.format", "ssh"]);
            git(
                &consumer,
                [
                    "config",
                    "gpg.ssh.allowedSignersFile",
                    self.directory
                        .path()
                        .join("allowed-signers")
                        .to_str()
                        .expect("UTF-8 allowed-signers path"),
                ],
            );
            consumer
        }

        #[expect(
            clippy::implicit_return,
            reason = "the signed fixture default delegates directly to the SHA-1 object-format fixture"
        )]
        async fn signed_history() -> Self {
            Self::signed_history_in_object_format(FixtureObjectFormat::Sha1).await
        }

        #[expect(
            clippy::implicit_return,
            clippy::expect_used,
            reason = "the signed fixture constructor intentionally fails fast while creating independent Git input in one selected object format"
        )]
        async fn signed_history_in_object_format(object_format: FixtureObjectFormat) -> Self {
            let directory = TempDir::new().expect("fixture directory should be created");
            let repository = directory.path().join("repository");
            let signing_key = directory.path().join("fixture-signing-key");
            git(
                directory.path(),
                [
                    "init",
                    object_format.git_init_argument(),
                    repository.to_str().expect("UTF-8 path"),
                ],
            );
            git(&repository, ["config", "user.name", "Tiber Store Fixture"]);
            git(
                &repository,
                [
                    "config",
                    "user.email",
                    "tiber-store-fixture@example.invalid",
                ],
            );
            create_ssh_signing_configuration(&repository, &signing_key, directory.path());

            let stream = StreamId::try_new("tiber:task:fixture".to_owned())
                .expect("fixture stream should be valid");
            let store = FileEventStore::open(repository.join("eventstore"))
                .expect("fixture event store should initialize");
            append_fixture_event(&store, stream.clone(), 0, "replay this fact").await;
            drop(store);
            git(&repository, ["add", "eventstore/events"]);
            let revision = commit_and_point_tiber(&repository, "fixture event history");

            Self {
                directory,
                object_format,
                repository,
                revision,
                stream,
            }
        }
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the black-box fixture test intentionally fails fast on setup and replay assertions"
    )]
    #[tokio::test]
    async fn replays_an_exact_signed_local_tiber_revision() {
        let fixture = GitFixture::signed_history().await;

        let store = TiberEventStore::open(&fixture.repository)
            .expect("a signed local Tiber history should open");

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert_eq!(store.stream_ids(), [fixture.stream.clone()].as_slice());
        let reader = store
            .verified_reader::<FixtureEvent>(EventFilter::all())
            .expect("known event history should verify");
        assert_eq!(
            reader
                .read_page(EventPage::first(BatchSize::new(64)))
                .await
                .expect("known event page should read")
                .into_iter()
                .map(|(event, _position)| event)
                .collect::<Vec<_>>(),
            vec![FixtureEvent {
                stream_id: StreamId::try_new("tiber:task:fixture".to_owned())
                    .expect("fixture stream should be valid"),
                text: "replay this fact".to_owned(),
            }],
        );
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the public store regression fixture deliberately fails fast while proving immutable transaction order wins over a committed reversed replica-local cursor"
    )]
    #[tokio::test]
    async fn transaction_reader_replays_cross_stream_history_in_transaction_order_not_ingestion_order()
     {
        let fixture = GitFixture::signed_history().await;
        let later_stream = StreamId::try_new("tiber:task:later-fixture".to_owned())
            .expect("later fixture stream should be valid");
        let writable = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture event store should reopen");
        append_fixture_event(&writable, later_stream, 0, "causally later fact").await;
        drop(writable);
        reverse_ingestion_log(&fixture.repository);
        git(
            &fixture.repository,
            [
                "add",
                "-f",
                "eventstore/events",
                "eventstore/index/ingestion.log",
            ],
        );
        let _revision = commit_and_point_tiber(
            &fixture.repository,
            "transaction order with reversed ingestion cursor",
        );

        let store = TiberEventStore::open(&fixture.repository).expect(
            "signed task authority should open with its intentionally reversed local cursor",
        );
        let reader = store
            .verified_transaction_reader::<FixtureEvent>(&[task_namespace_stream_pattern()])
            .expect("the selected task transaction chain should be unambiguous");
        let mut page = tiber_store_git::TransactionEventPage::first(BatchSize::new(1));
        let mut facts = Vec::new();
        loop {
            let events = reader
                .read_page(page)
                .expect("validated transaction page should decode");
            let Some(next_page) = page.next_from_results(&events) else {
                break;
            };
            facts.extend(events);
            page = next_page;
        }

        assert_eq!(
            facts
                .into_iter()
                .map(|event| event.text)
                .collect::<Vec<_>>(),
            ["replay this fact", "causally later fact"],
            "immutable transaction ancestry and per-transaction event order must win over EventCore's replica-local cursor"
        );
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the black-box fixture test intentionally fails fast while proving a signed authority snapshot contains only EventCore history"
    )]
    #[tokio::test]
    async fn materializes_only_eventstore_from_a_signed_authority_revision() {
        let fixture = GitFixture::signed_history().await;
        let writable = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture history should reopen for the snapshot marker");
        append_fixture_event(
            &writable,
            fixture.stream.clone(),
            1,
            SUBTREE_MATERIALIZATION_MARKER,
        )
        .await;
        drop(writable);

        let artifact = fixture.repository.join("unrelated-large-artifact.bin");
        fs::write(&artifact, vec![0; UNRELATED_ARTIFACT_SIZE_BYTES])
            .expect("large unrelated artifact should write");
        git(&fixture.repository, ["add", "unrelated-large-artifact.bin"]);
        fixture.commit_event_changes("signed authority with unrelated large artifact");

        let store = TiberEventStore::open(&fixture.repository)
            .expect("signed EventCore history should remain readable");
        let reader = store
            .verified_reader::<FixtureEvent>(EventFilter::all())
            .expect("known signed EventCore history should verify");
        let events = reader
            .read_page(EventPage::first(BatchSize::new(64)))
            .await
            .expect("known signed EventCore history should read")
            .into_iter()
            .map(|(event, _position)| event)
            .collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| event.text == SUBTREE_MATERIALIZATION_MARKER)
        );

        let snapshots = snapshots_containing_event_text(SUBTREE_MATERIALIZATION_MARKER);
        assert!(
            !snapshots.is_empty(),
            "the public store must retain its temporary snapshot while the reader exists",
        );
        for snapshot in snapshots {
            assert!(
                !snapshot.join("unrelated-large-artifact.bin").exists(),
                "the disposable snapshot must not materialize an unrelated tracked artifact: {}",
                snapshot.display(),
            );
        }
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the black-box fixture test intentionally fails fast while measuring caller-visible typed decode work across pages"
    )]
    #[tokio::test]
    async fn verified_paging_validates_selected_history_once() {
        let fixture = GitFixture::signed_history().await;
        let writable = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture history should reopen for counted envelopes");
        append_counted_fixture_events(&writable).await;
        drop(writable);
        fixture.commit_event_changes("counted paged event history");

        let store = TiberEventStore::open(&fixture.repository)
            .expect("the counted signed EventCore history should open");
        COUNTED_EVENT_DESERIALIZATIONS.store(0, Ordering::Relaxed);
        let reader = store
            .verified_reader::<CountedFixtureEvent>(EventFilter::all())
            .expect("the complete counted event history should verify once");
        let mut page = EventPage::first(BatchSize::new(1));
        let mut events = Vec::new();
        loop {
            let results = reader
                .read_page(page)
                .await
                .expect("each counted event page should read");
            let Some(next_page) = page.next_from_results(&results) else {
                break;
            };
            events.extend(results.into_iter().map(|(event, _position)| event));
            page = next_page;
        }

        assert_eq!(events.len(), COUNTED_EVENT_COUNT);
        assert_eq!(
            COUNTED_EVENT_DESERIALIZATIONS.load(Ordering::Relaxed),
            COUNTED_EVENT_COUNT * 2,
            "the selected history should be decoded once for verification and once while its pages are materialized",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while establishing a signed but schema-incompatible EventCore envelope"
    )]
    #[tokio::test]
    async fn rejects_a_signed_known_event_type_that_cannot_decode() {
        let fixture = GitFixture::signed_history().await;
        let writable = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture history should reopen for an incompatible envelope");
        append_incompatible_fixture_event(&writable, fixture.stream.clone(), 1).await;
        drop(writable);
        fixture.commit_event_changes("incompatible known event payload");

        let store = TiberEventStore::open(&fixture.repository)
            .expect("a structurally valid signed EventCore history should open");
        let error = store
            .verified_transaction_reader::<FixtureEvent>(&[fixture_stream_pattern()])
            .expect_err(
                "typed transaction replay must not silently omit an incompatible known event",
            );

        assert!(matches!(
            error,
            TransactionHistoryError::EventStore(EventStoreError::DeserializationFailed { stream_id, .. }) if stream_id == fixture.stream
        ));
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while establishing a signed integrity-valid envelope with inconsistent application stream identity"
    )]
    #[tokio::test]
    async fn rejects_a_signed_integrity_valid_payload_with_mismatched_stream_identity() {
        let fixture = GitFixture::signed_history().await;
        let payload_stream = StreamId::try_new("tiber:task:payload-mismatch".to_owned())
            .expect("payload stream should be valid");
        let writable = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture history should reopen for a mismatched envelope");
        append_stream_mismatched_fixture_event(
            &writable,
            fixture.stream.clone(),
            payload_stream,
            1,
        )
        .await;
        drop(writable);
        fixture.commit_event_changes("mismatched application stream identity");

        let store = TiberEventStore::open(&fixture.repository)
            .expect("the signed integrity-valid EventCore history should open");
        let error = store
            .verified_transaction_reader::<FixtureEvent>(&[fixture_stream_pattern()])
            .expect_err(
                "a selected transaction payload must retain its persisted envelope stream identity",
            );

        assert!(matches!(
            error,
            TransactionHistoryError::EventStore(EventStoreError::DeserializationFailed { stream_id, .. }) if stream_id == fixture.stream
        ));
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while establishing a signed unrelated future workflow envelope"
    )]
    #[tokio::test]
    async fn accepts_an_unrelated_event_type_when_verifying_known_events() {
        let fixture = GitFixture::signed_history().await;
        let writable = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture history should reopen for an unrelated envelope");
        append_unrelated_fixture_event(&writable, fixture.stream.clone(), 1).await;
        drop(writable);
        fixture.commit_event_changes("unrelated future workflow event");

        let store = TiberEventStore::open(&fixture.repository)
            .expect("a structurally valid signed EventCore history should open");

        store
            .verified_reader::<FixtureEvent>(EventFilter::all())
            .expect("typed verification must ignore a differently tagged event");
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while establishing a signed payload selected only by an explicit EventCore event-type filter"
    )]
    #[tokio::test]
    async fn rejects_an_explicitly_selected_event_type_that_cannot_decode() {
        let fixture = GitFixture::signed_history().await;
        let writable = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture history should reopen for an explicitly selected envelope");
        append_unrelated_fixture_event(&writable, fixture.stream.clone(), 1).await;
        drop(writable);
        fixture.commit_event_changes("explicitly selected incompatible event");

        let store = TiberEventStore::open(&fixture.repository)
            .expect("a structurally valid signed EventCore history should open");
        let filter =
            EventFilter::all().with_event_type(UnrelatedFixtureEvent::event_type_name().to_owned());
        let error = store
            .verified_reader::<FixtureEvent>(filter)
            .expect_err("a selected incompatible payload must not be silently omitted");

        assert!(matches!(
            error,
            EventStoreError::DeserializationFailed { stream_id, .. } if stream_id == fixture.stream
        ));
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on setup and ref-observation assertions"
    )]
    #[tokio::test]
    async fn materializes_a_remote_authority_without_populating_the_caller_object_database_or_refs()
    {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&consumer)
            .expect("the signed origin authority should be materialized");

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a remote query must not create, move, or delete caller refs",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box transport fixture intentionally fails fast while proving caller-local SSH configuration crosses only into disposable authority storage"
    )]
    #[cfg(unix)]
    #[tokio::test]
    async fn materializes_a_remote_authority_with_caller_local_ssh_transport_configuration() {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        let helper = fixture.directory.path().join("fixture-ssh-transport");
        let marker = fixture.directory.path().join("fixture-ssh-invocations");
        create_git_upload_pack_ssh_helper(&helper);
        let ssh_command = format!(
            "{} {} {}",
            helper.to_str().expect("UTF-8 helper path"),
            marker.to_str().expect("UTF-8 marker path"),
            origin.to_str().expect("UTF-8 origin path"),
        );
        git(&consumer, ["config", "core.sshCommand", &ssh_command]);
        git(&consumer, ["config", "ssh.variant", "simple"]);
        git(
            &consumer,
            [
                "remote",
                "set-url",
                "origin",
                "ssh://fixture.invalid/tiber.git",
            ],
        );
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        let objects_before = git_output(&consumer, ["count-objects", "-v"]);
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&consumer)
            .expect("the disposable fetch should retain caller-local SSH transport configuration");

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert!(
            fs::read_to_string(marker)
                .expect("the fixture SSH invocation marker should read")
                .lines()
                .count()
                >= 2,
            "both caller ls-remote and disposable fetch must use the caller-local SSH command",
        );
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "an SSH-backed remote query must not create, move, or delete caller refs",
        );
        assert_eq!(
            git_output(&consumer, ["count-objects", "-v"]),
            objects_before,
            "an SSH-backed remote query must not add caller objects",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box relative transport fixture intentionally fails fast while proving disposable Git storage does not change the caller's SSH command working directory"
    )]
    #[cfg(unix)]
    #[tokio::test]
    async fn materializes_a_remote_authority_with_a_relative_caller_local_ssh_command() {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        let helper = consumer.join("fixture-relative-ssh-transport");
        let marker = fixture
            .directory
            .path()
            .join("fixture-relative-ssh-invocations");
        create_git_upload_pack_ssh_helper(&helper);
        let ssh_command = format!(
            "./fixture-relative-ssh-transport {} {}",
            marker.to_str().expect("UTF-8 marker path"),
            origin.to_str().expect("UTF-8 origin path"),
        );
        git(&consumer, ["config", "core.sshCommand", &ssh_command]);
        git(&consumer, ["config", "ssh.variant", "simple"]);
        git(
            &consumer,
            [
                "remote",
                "set-url",
                "origin",
                "ssh://fixture.invalid/tiber.git",
            ],
        );
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        let objects_before = git_output(&consumer, ["count-objects", "-v"]);
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&consumer).expect(
            "the disposable fetch should run a relative SSH command from the caller worktree root",
        );

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert!(
            fs::read_to_string(marker)
                .expect("the relative SSH invocation marker should read")
                .lines()
                .count()
                >= 2,
            "both caller ls-remote and disposable fetch must run the relative SSH command",
        );
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a relative SSH command query must not create, move, or delete caller refs",
        );
        assert_eq!(
            git_output(&consumer, ["count-objects", "-v"]),
            objects_before,
            "a relative SSH command query must not add caller objects",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box relative-origin fixture intentionally fails fast on caller-isolation assertions"
    )]
    #[tokio::test]
    async fn materializes_a_relative_filesystem_origin_from_the_caller_repository() {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        git(&consumer, ["remote", "set-url", "origin", "../origin.git"]);
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        let objects_before = git_output(&consumer, ["count-objects", "-v"]);
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&consumer).expect(
            "a caller-relative filesystem origin should remain stable in disposable storage",
        );

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a relative-origin query must not create, move, or delete caller refs",
        );
        assert_eq!(
            git_output(&consumer, ["count-objects", "-v"]),
            objects_before,
            "a relative-origin query must not add caller objects",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box subdirectory fixture intentionally fails fast while proving Git's repository-root URL semantics survive disposable authority setup"
    )]
    #[tokio::test]
    async fn materializes_a_relative_filesystem_origin_when_opened_from_a_subdirectory() {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        let subdirectory = consumer.join("nested/caller");
        fs::create_dir_all(&subdirectory).expect("fixture caller subdirectory should create");
        git(&consumer, ["remote", "set-url", "origin", "../origin.git"]);
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        let objects_before = git_output(&consumer, ["count-objects", "-v"]);
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&subdirectory).expect(
            "a relative filesystem origin should remain based at the worktree root for subdirectory callers",
        );

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a subdirectory-origin query must not create, move, or delete caller refs",
        );
        assert_eq!(
            git_output(&consumer, ["count-objects", "-v"]),
            objects_before,
            "a subdirectory-origin query must not add caller objects",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box SHA-256 fixture test intentionally fails fast on setup and caller-isolation assertions"
    )]
    #[tokio::test]
    async fn materializes_a_remote_sha256_authority_without_changing_caller_objects_or_refs() {
        let fixture =
            GitFixture::signed_history_in_object_format(FixtureObjectFormat::Sha256).await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        assert_eq!(
            git_output(&consumer, ["rev-parse", "--show-object-format=storage"]),
            fixture.object_format.storage_name(),
            "the SHA-256 consumer must drive the disposable authority format",
        );
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&consumer)
            .expect("the signed SHA-256 origin authority should be materialized");

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a SHA-256 remote query must not create, move, or delete caller refs",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on setup and ref-observation assertions"
    )]
    #[tokio::test]
    async fn materializes_a_remote_signed_authority_with_relative_local_ssh_verification_files() {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        fs::write(fixture.directory.path().join("revoked-signers"), "")
            .expect("fixture revocation file should write");
        git(
            &consumer,
            ["config", "gpg.ssh.allowedSignersFile", "../allowed-signers"],
        );
        git(
            &consumer,
            ["config", "gpg.ssh.revocationFile", "../revoked-signers"],
        );
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&consumer).expect(
            "a signed origin authority should honor caller-relative SSH verification files",
        );

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a remote query must not create, move, or delete caller refs",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on setup and caller-isolation assertions"
    )]
    #[tokio::test]
    async fn materializes_a_remote_signed_authority_with_optional_relative_ssh_verification_files()
    {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let consumer = fixture.remote_only_consumer(&origin);
        fs::write(fixture.directory.path().join("revoked-signers"), "")
            .expect("fixture revocation file should write");
        git(
            &consumer,
            [
                "config",
                "gpg.ssh.allowedSignersFile",
                ":(optional)../allowed-signers",
            ],
        );
        git(
            &consumer,
            [
                "config",
                "gpg.ssh.revocationFile",
                ":(optional)../revoked-signers",
            ],
        );
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        let objects_before = git_output(&consumer, ["count-objects", "-v"]);
        assert_authority_object_is_absent(&consumer, &fixture.revision);

        let store = TiberEventStore::open(&consumer).expect(
            "a signed origin authority should preserve optional caller-relative SSH verification files",
        );

        assert_eq!(store.revision().as_str(), fixture.revision);
        assert_authority_object_is_absent(&consumer, &fixture.revision);
        assert_eq!(
            git_output(&consumer, ["config", "--get", "gpg.ssh.allowedSignersFile"]),
            ":(optional)../allowed-signers",
            "a remote query must retain the caller-local allowed-signers pathname",
        );
        assert_eq!(
            git_output(&consumer, ["config", "--get", "gpg.ssh.revocationFile"]),
            ":(optional)../revoked-signers",
            "a remote query must retain the caller-local revocation pathname",
        );
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a remote query must not create, move, or delete caller refs",
        );
        assert_eq!(
            git_output(&consumer, ["count-objects", "-v"]),
            objects_before,
            "a remote query must not add objects to the caller object database",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on setup and authority-selection assertions"
    )]
    #[tokio::test]
    async fn refuses_an_absent_origin_tiber_ref_without_falling_back_to_a_stale_local_ref() {
        let fixture = GitFixture::signed_history().await;
        let _origin = fixture.add_origin(false);

        let error = TiberEventStore::open(&fixture.repository)
            .expect_err("an absent origin authority must not use the stale local ref");

        assert_eq!(error.code(), "tiber_git_refresh_origin_tiber_ref_failed");
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            fixture.revision,
            "the existing local ref remains untouched and is not treated as authority",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on signed-authority setup and rejection assertions"
    )]
    #[tokio::test]
    async fn refuses_an_unsigned_exact_origin_authority_revision() {
        let fixture = GitFixture::signed_history().await;
        let origin = fixture.add_origin(true);
        let unsigned = fixture.point_tiber_at_unsigned_child();
        git(
            &fixture.repository,
            [
                "push",
                "--force",
                "origin",
                "refs/heads/tiber:refs/heads/tiber",
            ],
        );
        let consumer = fixture.remote_only_consumer(&origin);
        let refs_before = git_output(
            &consumer,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        assert_authority_object_is_absent(&consumer, &unsigned);

        let error = TiberEventStore::open(&consumer)
            .expect_err("an unsigned remote authority must not be replayed");

        assert_eq!(error.code(), "tiber_git_verify_tiber_signature_failed");
        assert_eq!(error.to_string(), "tiber_git_verify_tiber_signature_failed");
        assert_eq!(error.retryability(), Retryability::Permanent);
        let failure = error
            .git_command_failure()
            .expect("signature rejection must retain Git process context");
        assert_eq!(failure.operation(), GitOperation::VerifyTiberSignature);
        assert_eq!(failure.kind(), GitCommandFailureKind::NonZeroExit);
        let signature_failure_exit_code: i32 = 1;
        assert_eq!(failure.exit_code(), Some(signature_failure_exit_code));
        assert!(failure.io_source().is_none());
        assert_ne!(unsigned, fixture.revision);
        assert_authority_object_is_absent(&consumer, &unsigned);
        assert_eq!(
            git_output(
                &consumer,
                ["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            refs_before,
            "a rejected remote revision must not create, move, or delete caller refs",
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on signed snapshot setup and rejection assertions"
    )]
    #[tokio::test]
    async fn refuses_a_signed_revision_without_eventstore_events() {
        let fixture = GitFixture::signed_history().await;
        fs::remove_dir_all(fixture.repository.join("eventstore/events"))
            .expect("fixture events should remove");
        fixture.commit_event_changes("remove event history");

        let error = TiberEventStore::open(&fixture.repository)
            .expect_err("a resolved authority must retain eventstore/events");

        assert_eq!(error.code(), "tiber_store_snapshot_events_missing");
    }

    #[cfg(unix)]
    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while constructing a signed malformed snapshot"
    )]
    #[tokio::test]
    async fn refuses_a_signed_symlinked_eventstore_without_writing_outside_the_snapshot() {
        let fixture = GitFixture::signed_history().await;
        let outside = fixture.directory.path().join("outside-eventstore");
        fs::create_dir_all(outside.join("events"))
            .expect("outside sentinel event directory should create");
        fs::remove_dir_all(fixture.repository.join("eventstore"))
            .expect("fixture eventstore should remove");
        symlink(&outside, fixture.repository.join("eventstore"))
            .expect("fixture eventstore symlink should create");
        fixture.commit_event_changes("symlink event store outside snapshot");
        let sentinel_before = direct_directory_entries(&outside);

        let result = TiberEventStore::open(&fixture.repository);
        let sentinel_after = direct_directory_entries(&outside);

        assert_eq!(
            sentinel_after, sentinel_before,
            "opening malformed signed authority must not initialize the symlink target",
        );
        let error = result.expect_err("a symlinked eventstore must be rejected");
        assert_eq!(error.code(), "tiber_store_event_history_invalid");
    }

    #[cfg(unix)]
    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while constructing a signed malformed snapshot"
    )]
    #[tokio::test]
    async fn refuses_signed_symlinked_events_without_touching_the_external_directory() {
        let fixture = GitFixture::signed_history().await;
        let outside = fixture.directory.path().join("outside-events");
        fs::create_dir_all(&outside).expect("outside sentinel directory should create");
        fs::remove_dir_all(fixture.repository.join("eventstore/events"))
            .expect("fixture events should remove");
        symlink(&outside, fixture.repository.join("eventstore/events"))
            .expect("fixture events symlink should create");
        fixture.commit_event_changes("symlink events outside snapshot");
        let sentinel_before = direct_directory_entries(&outside);

        let result = TiberEventStore::open(&fixture.repository);
        let sentinel_after = direct_directory_entries(&outside);

        assert_eq!(
            sentinel_after, sentinel_before,
            "opening malformed signed authority must leave the external events directory untouched",
        );
        let error = result.expect_err("a symlinked events directory must be rejected");
        assert_eq!(error.code(), "tiber_store_event_history_invalid");
    }

    #[cfg(unix)]
    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture intentionally fails fast while proving EventCore derived state cannot escape a signed disposable snapshot"
    )]
    #[tokio::test]
    async fn refuses_a_signed_symlinked_eventcore_state_directory_before_opening_the_store() {
        let fixture = GitFixture::signed_history().await;
        let outside = fixture.directory.path().join("outside-eventcore-state");
        fs::create_dir_all(&outside).expect("outside sentinel directory should create");
        fs::write(outside.join("sentinel"), "unchanged").expect("outside sentinel should write");
        fs::remove_dir_all(fixture.repository.join("eventstore/.eventcore"))
            .expect("fixture EventCore state should remove");
        symlink(&outside, fixture.repository.join("eventstore/.eventcore"))
            .expect("fixture EventCore state symlink should create");
        git(&fixture.repository, ["add", "-f", "eventstore/.eventcore"]);
        let _revision = commit_and_point_tiber(
            &fixture.repository,
            "symlink EventCore state outside snapshot",
        );
        let sentinel_before = direct_directory_entries(&outside);

        let result = TiberEventStore::open(&fixture.repository);
        let sentinel_after = direct_directory_entries(&outside);

        assert_eq!(
            sentinel_after, sentinel_before,
            "opening malformed signed authority must not initialize external EventCore state",
        );
        let error = result.expect_err("a symlinked EventCore state directory must be rejected");
        assert_eq!(error.code(), "tiber_store_event_history_invalid");
    }

    #[cfg(unix)]
    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while constructing a signed malformed transaction path"
    )]
    #[tokio::test]
    async fn refuses_a_signed_symlinked_transaction_without_touching_the_external_file() {
        let fixture = GitFixture::signed_history().await;
        let transaction = transaction_files(&fixture.repository.join("eventstore/events"))
            .into_iter()
            .next()
            .expect("fixture should have an event file");
        let outside = fixture.directory.path().join("outside-transaction.jsonl");
        fs::rename(&transaction, &outside).expect("fixture transaction should move outside");
        symlink(&outside, &transaction).expect("fixture transaction symlink should create");
        fixture.commit_event_changes("symlink transaction outside snapshot");
        let sentinel_before = fs::read(&outside).expect("outside transaction should read");

        let result = TiberEventStore::open(&fixture.repository);
        let sentinel_after =
            fs::read(&outside).expect("outside transaction should remain readable");

        assert_eq!(
            sentinel_after, sentinel_before,
            "opening malformed signed authority must leave the external transaction untouched",
        );
        let error = result.expect_err("a symlinked transaction file must be rejected");
        assert_eq!(error.code(), "tiber_store_event_history_invalid");
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on event-history tampering setup and rejection assertions"
    )]
    #[tokio::test]
    async fn refuses_eventcore_integrity_failures() {
        let fixture = GitFixture::signed_history().await;
        let event_file = transaction_files(&fixture.repository.join("eventstore/events"))
            .into_iter()
            .next()
            .expect("fixture should have an event file");
        let original = fs::read_to_string(&event_file).expect("fixture event file should read");
        let changed = original.replacen("replay this fact", "tampered fixture fact", 1);
        fs::write(event_file, changed).expect("fixture event file should change");
        fixture.commit_event_changes("tamper event history");

        let error = TiberEventStore::open(&fixture.repository)
            .expect_err("EventCore integrity failure must reject the snapshot");

        assert_eq!(error.code(), "tiber_store_event_history_integrity_failed");
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast on dangling-history setup and rejection assertions"
    )]
    #[tokio::test]
    async fn refuses_eventcore_dangling_transactions() {
        let fixture = GitFixture::signed_history().await;
        let event_file = transaction_files(&fixture.repository.join("eventstore/events"))
            .into_iter()
            .next()
            .expect("fixture should have an event file");
        replace_header_field(
            &event_file,
            "parent_transaction_ids",
            serde_json::json!(["00000000-0000-7000-8000-000000000001"]),
        );
        fixture.commit_event_changes("dangling event history");

        let error = TiberEventStore::open(&fixture.repository)
            .expect_err("EventCore dangling transaction must reject the snapshot");

        assert_eq!(
            error.code(),
            "tiber_store_event_history_dangling_transaction"
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while constructing concurrent writer replicas"
    )]
    #[tokio::test]
    async fn refuses_two_concurrent_writers_to_the_same_stream() {
        let fixture = GitFixture::signed_history().await;
        let first_replica = fixture.directory.path().join("first-writer-replica");
        let second_replica = fixture.directory.path().join("second-writer-replica");
        let source = fixture.repository.join("eventstore/events");
        copy_transaction_files(&source, &first_replica.join("eventstore/events"));
        copy_transaction_files(&source, &second_replica.join("eventstore/events"));

        let first = FileEventStore::open(first_replica.join("eventstore"))
            .expect("first writer replica should open");
        append_fixture_event(&first, fixture.stream.clone(), 1, "first writer").await;
        let second = FileEventStore::open(second_replica.join("eventstore"))
            .expect("second writer replica should open");
        append_fixture_event(&second, fixture.stream.clone(), 1, "second writer").await;
        copy_transaction_files(&first_replica.join("eventstore/events"), &source);
        copy_transaction_files(&second_replica.join("eventstore/events"), &source);
        fixture.commit_event_changes("concurrent stream writers");

        let error = TiberEventStore::open(&fixture.repository)
            .expect_err("two EventCore writers to a shared base must be rejected");

        assert_eq!(error.code(), "tiber_store_event_history_fork_detected");
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while constructing legacy read-context replicas"
    )]
    #[tokio::test]
    async fn accepts_concurrent_legacy_read_contexts_with_no_writer_to_the_disputed_stream() {
        let fixture = GitFixture::signed_history().await;
        let first_replica = fixture.directory.path().join("first-reader-replica");
        let second_replica = fixture.directory.path().join("second-reader-replica");
        let source = fixture.repository.join("eventstore/events");
        copy_transaction_files(&source, &first_replica.join("eventstore/events"));
        copy_transaction_files(&source, &second_replica.join("eventstore/events"));
        let first_output = StreamId::try_new("tiber:task:first-output".to_owned())
            .expect("first output stream should be valid");
        let second_output = StreamId::try_new("tiber:task:second-output".to_owned())
            .expect("second output stream should be valid");

        let first = FileEventStore::open(first_replica.join("eventstore"))
            .expect("first reader replica should open");
        append_fixture_event_with_read_context(
            &first,
            fixture.stream.clone(),
            1,
            first_output.clone(),
            "first reader output",
        )
        .await;
        let second = FileEventStore::open(second_replica.join("eventstore"))
            .expect("second reader replica should open");
        append_fixture_event_with_read_context(
            &second,
            fixture.stream.clone(),
            1,
            second_output.clone(),
            "second reader output",
        )
        .await;
        copy_transaction_files(&first_replica.join("eventstore/events"), &source);
        copy_transaction_files(&second_replica.join("eventstore/events"), &source);
        fixture.commit_event_changes("concurrent read contexts");

        let store = TiberEventStore::open(&fixture.repository)
            .expect("zero-writer legacy read contexts must remain replayable");

        assert_eq!(
            store.stream_ids(),
            [first_output, fixture.stream, second_output].as_slice(),
        );
        let error = store
            .verified_transaction_reader::<FixtureEvent>(&[task_namespace_stream_pattern()])
            .expect_err(
                "two concurrent selected task transactions must not receive an invented filename or cursor order",
            );
        assert!(matches!(
            error,
            TransactionHistoryError::AmbiguousTransactionOrder
        ));
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while constructing a causal read-before-write history"
    )]
    #[tokio::test]
    async fn accepts_one_writer_after_every_read_context_candidate() {
        let fixture = GitFixture::signed_history().await;
        let output = StreamId::try_new("tiber:task:causal-reader-output".to_owned())
            .expect("output stream should be valid");
        let store = FileEventStore::open(fixture.repository.join("eventstore"))
            .expect("fixture history should reopen");
        append_fixture_event_with_read_context(
            &store,
            fixture.stream.clone(),
            1,
            output.clone(),
            "causal reader output",
        )
        .await;
        append_fixture_event(&store, fixture.stream.clone(), 1, "causally later writer").await;
        drop(store);
        fixture.commit_event_changes("causal reader then writer");

        let replay = TiberEventStore::open(&fixture.repository)
            .expect("a writer descending from every reader is causally valid");

        assert_eq!(replay.stream_ids(), [output, fixture.stream].as_slice());
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box fixture test intentionally fails fast while constructing a stale concurrent read context"
    )]
    #[tokio::test]
    async fn refuses_one_writer_with_a_concurrent_stale_read_context() {
        let fixture = GitFixture::signed_history().await;
        let writer_replica = fixture.directory.path().join("writer-replica");
        let reader_replica = fixture.directory.path().join("reader-replica");
        let source = fixture.repository.join("eventstore/events");
        copy_transaction_files(&source, &writer_replica.join("eventstore/events"));
        copy_transaction_files(&source, &reader_replica.join("eventstore/events"));
        let reader_output = StreamId::try_new("tiber:task:reader-output".to_owned())
            .expect("reader output stream should be valid");

        let writer = FileEventStore::open(writer_replica.join("eventstore"))
            .expect("writer replica should open");
        append_fixture_event(&writer, fixture.stream.clone(), 1, "concurrent writer").await;
        let reader = FileEventStore::open(reader_replica.join("eventstore"))
            .expect("reader replica should open");
        append_fixture_event_with_read_context(
            &reader,
            fixture.stream.clone(),
            1,
            reader_output,
            "concurrent reader output",
        )
        .await;
        copy_transaction_files(&writer_replica.join("eventstore/events"), &source);
        copy_transaction_files(&reader_replica.join("eventstore/events"), &source);
        fixture.commit_event_changes("writer with stale reader context");

        let error = TiberEventStore::open(&fixture.repository)
            .expect_err("a writer must descend from every candidate at its base");

        assert_eq!(error.code(), "tiber_store_event_history_fork_detected");
    }

    #[expect(
        clippy::expect_used,
        reason = "the black-box boundary test intentionally fails fast when temporary setup cannot begin"
    )]
    #[test]
    fn sanitizes_non_git_open_failures() {
        let directory = TempDir::new().expect("fixture directory should be created");

        let error = TiberEventStore::open(directory.path())
            .expect_err("a non-Git directory cannot provide Tiber history");

        assert_eq!(error.code(), "tiber_git_resolve_tiber_ref_failed");
        assert!(!error.to_string().contains("fatal:"));
    }

    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        reason = "the isolated signing fixture must fail fast if its ephemeral Git trust configuration cannot be established"
    )]
    fn create_ssh_signing_configuration(repository: &Path, key: &Path, root: &Path) {
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(key)
            .status()
            .expect("fixture signing-key creation should start");
        assert!(
            status.success(),
            "fixture signing-key creation should succeed"
        );
        let allowed_signers = root.join("allowed-signers");
        let public_key =
            fs::read_to_string(key.with_extension("pub")).expect("fixture public key should read");
        fs::write(
            &allowed_signers,
            format!("tiber-store-fixture@example.invalid {public_key}"),
        )
        .expect("fixture allowed signers should write");
        git(repository, ["config", "gpg.format", "ssh"]);
        git(repository, ["config", "commit.gpgsign", "true"]);
        git(
            repository,
            [
                "config",
                "user.signingkey",
                key.to_str().expect("UTF-8 key path"),
            ],
        );
        git(
            repository,
            [
                "config",
                "gpg.ssh.allowedSignersFile",
                allowed_signers.to_str().expect("UTF-8 signer path"),
            ],
        );
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fixture deliberately fails fast if it cannot make a signed authority commit"
    )]
    fn commit_and_point_tiber(repository: &Path, subject: &str) -> String {
        git(repository, ["commit", "-m", subject]);
        let revision = git_output(repository, ["rev-parse", "HEAD"]);
        let commit = git_output(repository, ["cat-file", "commit", revision.as_str()]);
        assert!(commit.contains("-----BEGIN SSH SIGNATURE-----"));
        git(repository, ["update-ref", TIBER_REF, revision.as_str()]);
        revision
    }

    #[expect(
        clippy::expect_used,
        reason = "the fixture event writer deliberately fails fast when EventCore cannot construct an independent history"
    )]
    async fn append_fixture_event(
        store: &FileEventStore,
        stream_id: StreamId,
        expected_version: usize,
        text: &str,
    ) {
        let writes = StreamWrites::new()
            .register_stream(stream_id.clone(), StreamVersion::new(expected_version))
            .expect("fixture stream should register")
            .append(FixtureEvent {
                stream_id,
                text: text.to_owned(),
            })
            .expect("fixture event should append");
        let _slice = store
            .append_events(writes)
            .await
            .expect("fixture event transaction should append");
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the store fixture's closed task-stream selection is static and must fail fast if EventCore changes its accepted pattern grammar"
    )]
    fn fixture_stream_pattern() -> StreamPattern {
        StreamPattern::try_new("tiber:task:fixture".to_owned())
            .expect("fixture stream pattern should be valid")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the store fixture's namespace selection is static and must fail fast if EventCore changes its accepted pattern grammar"
    )]
    fn task_namespace_stream_pattern() -> StreamPattern {
        StreamPattern::try_new("tiber:task:*".to_owned())
            .expect("task namespace pattern should be valid")
    }

    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        reason = "the counted fixture deliberately creates a page-spanning typed history with fail-fast setup"
    )]
    async fn append_counted_fixture_events(store: &FileEventStore) {
        let stream_id = StreamId::try_new("tiber:task:counted-fixture".to_owned())
            .expect("counted fixture stream should be valid");
        let mut writes = StreamWrites::new()
            .register_stream(stream_id.clone(), StreamVersion::new(0))
            .expect("counted fixture stream should register");
        for index in 0..COUNTED_EVENT_COUNT {
            writes = writes
                .append(CountedFixtureEvent {
                    stream_id: stream_id.clone(),
                    text: format!("counted fact {index}"),
                })
                .expect("counted fixture event should append");
        }
        let _slice = store
            .append_events(writes)
            .await
            .expect("counted fixture event transaction should append");
    }

    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        reason = "the malformed-envelope fixture deliberately creates one valid EventCore record that the known application event type cannot decode"
    )]
    async fn append_incompatible_fixture_event(
        store: &FileEventStore,
        stream_id: StreamId,
        expected_version: usize,
    ) {
        let writes = StreamWrites::new()
            .register_stream(stream_id.clone(), StreamVersion::new(expected_version))
            .expect("fixture stream should register")
            .append(IncompatibleFixtureEvent {
                stream_id,
                unexpected: "missing FixtureEvent text".to_owned(),
            })
            .expect("incompatible fixture event should append");
        let _slice = store
            .append_events(writes)
            .await
            .expect("incompatible fixture event transaction should append");
    }

    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        reason = "the identity-mismatch fixture deliberately emits an EventCore-valid envelope whose serialized payload declares another stream"
    )]
    async fn append_stream_mismatched_fixture_event(
        store: &FileEventStore,
        envelope_stream_id: StreamId,
        payload_stream_id: StreamId,
        expected_version: usize,
    ) {
        let writes = StreamWrites::new()
            .register_stream(
                envelope_stream_id.clone(),
                StreamVersion::new(expected_version),
            )
            .expect("fixture stream should register")
            .append(StreamMismatchedFixtureEvent {
                payload_stream_id,
                text: "declared in another payload stream".to_owned(),
                envelope_stream_id,
            })
            .expect("mismatched fixture event should append to its envelope stream");
        let _slice = store
            .append_events(writes)
            .await
            .expect("mismatched fixture event transaction should append");
    }

    #[expect(
        clippy::expect_used,
        reason = "the future-envelope fixture deliberately appends one different EventCore event type to the known stream"
    )]
    async fn append_unrelated_fixture_event(
        store: &FileEventStore,
        stream_id: StreamId,
        expected_version: usize,
    ) {
        let writes = StreamWrites::new()
            .register_stream(stream_id.clone(), StreamVersion::new(expected_version))
            .expect("fixture stream should register")
            .append(UnrelatedFixtureEvent {
                stream_id,
                context: "future workflow context".to_owned(),
            })
            .expect("unrelated fixture event should append");
        let _slice = store
            .append_events(writes)
            .await
            .expect("unrelated fixture event transaction should append");
    }

    #[expect(
        clippy::expect_used,
        reason = "the fixture event writer deliberately fails fast when EventCore cannot construct a read-context history"
    )]
    async fn append_fixture_event_with_read_context(
        store: &FileEventStore,
        read_context: StreamId,
        expected_context_version: usize,
        output_stream: StreamId,
        text: &str,
    ) {
        let writes = StreamWrites::new()
            .register_stream(read_context, StreamVersion::new(expected_context_version))
            .expect("fixture read context should register")
            .register_stream(output_stream.clone(), StreamVersion::new(0))
            .expect("fixture output stream should register")
            .append(FixtureEvent {
                stream_id: output_stream,
                text: text.to_owned(),
            })
            .expect("fixture event should append");
        let _slice = store
            .append_events(writes)
            .await
            .expect("fixture context transaction should append");
    }

    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::shadow_reuse,
        clippy::single_call_fn,
        reason = "the focused malformed-header fixture intentionally uses fail-fast JSON mutation"
    )]
    fn replace_header_field(path: &Path, field: &str, replacement: serde_json::Value) {
        let contents = fs::read_to_string(path).expect("fixture event file should read");
        let (header, rest) = contents
            .split_once('\n')
            .expect("fixture transaction should include header and event");
        let mut header: serde_json::Value =
            serde_json::from_str(header).expect("fixture header should decode");
        header[field] = replacement;
        fs::write(
            path,
            format!(
                "{}\n{rest}",
                serde_json::to_string(&header).expect("fixture header should encode"),
            ),
        )
        .expect("fixture event file should write");
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the fixture enumerator intentionally fails fast and returns a compact collected path list"
    )]
    fn transaction_files(events_directory: &Path) -> Vec<PathBuf> {
        let mut paths = fs::read_dir(events_directory)
            .expect("fixture event directory should read")
            .map(|entry| entry.expect("fixture event entry should read").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    #[expect(
        clippy::arithmetic_side_effects,
        clippy::expect_used,
        clippy::implicit_return,
        clippy::indexing_slicing,
        clippy::single_call_fn,
        reason = "the public ordering regression needs only an intentionally reversed replica-local cursor, while its signed transaction files remain untouched"
    )]
    fn reverse_ingestion_log(repository: &Path) {
        let path = repository.join("eventstore/index/ingestion.log");
        let entries = fs::read_to_string(&path)
            .expect("fixture ingestion log should be readable")
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .expect("fixture ingestion entry should be valid JSON")
            })
            .collect::<Vec<_>>();
        let log = entries
            .into_iter()
            .rev()
            .enumerate()
            .map(|(index, mut entry)| {
                entry["seq"] = serde_json::Value::from(index + 1);
                serde_json::to_string(&entry).expect("fixture ingestion entry should encode")
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{log}\n")).expect("fixture ingestion log should be rewritten");
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the fixture enumerator intentionally fails fast and returns a stable sentinel inventory"
    )]
    fn direct_directory_entries(directory: &Path) -> Vec<PathBuf> {
        let mut paths = fs::read_dir(directory)
            .expect("sentinel directory should read")
            .map(|entry| entry.expect("sentinel directory entry should read").path())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the public-boundary fixture finds its retained disposable snapshot through a unique committed event marker"
    )]
    fn snapshots_containing_event_text(marker: &str) -> Vec<PathBuf> {
        env::temp_dir()
            .read_dir()
            .expect("system temporary directory should read")
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("snapshot"))
            .filter(|snapshot| {
                fs::read_dir(snapshot.join("eventstore/events")).is_ok_and(|entries| {
                    entries.filter_map(Result::ok).any(|entry| {
                        entry.path().extension().is_some_and(|extension| {
                            extension == "jsonl"
                                && fs::read_to_string(entry.path())
                                    .is_ok_and(|contents| contents.contains(marker))
                        })
                    })
                })
            })
            .collect()
    }

    #[expect(
        clippy::expect_used,
        reason = "the fixture copier intentionally fails fast if an independently built replica cannot be prepared"
    )]
    fn copy_transaction_files(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).expect("fixture destination should create");
        for source_file in transaction_files(source) {
            let filename = source_file
                .file_name()
                .expect("fixture transaction should have a file name");
            let target = destination.join(filename);
            fs::copy(source_file, target).expect("fixture transaction should copy");
        }
    }

    #[cfg(unix)]
    #[expect(
        clippy::expect_used,
        reason = "the transport fixture writes one bounded executable that replaces SSH with a local upload-pack while recording use"
    )]
    fn create_git_upload_pack_ssh_helper(helper: &Path) {
        fs::write(
            helper,
            "#!/bin/sh\nprintf 'invoked\\n' >> \"$1\"\nexec git-upload-pack \"$2\"\n",
        )
        .expect("fixture SSH helper should write");
        let mut permissions = fs::metadata(helper)
            .expect("fixture SSH helper metadata should read")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(helper, permissions)
            .expect("fixture SSH helper should become executable");
    }

    #[expect(
        clippy::expect_used,
        reason = "fixture Git commands intentionally expose stderr only through fail-fast assertion diagnostics"
    )]
    fn git<const N: usize>(repository: &Path, arguments: [&str; N]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start");
        assert!(
            output.status.success(),
            "fixture Git command should succeed: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        clippy::implicit_return,
        reason = "the fixture status helper intentionally fails fast if Git cannot start"
    )]
    fn git_status<const N: usize>(repository: &Path, arguments: [&str; N]) -> ExitStatus {
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start")
            .status
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture Git output intentionally fails fast and returns its normalized UTF-8 assertion input"
    )]
    fn git_output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start");
        assert!(
            output.status.success(),
            "fixture Git command should succeed: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout)
            .expect("fixture Git output should be UTF-8")
            .trim()
            .to_owned()
    }

    fn assert_authority_object_is_absent(repository: &Path, revision: &str) {
        let commit = format!("{revision}^{{commit}}");
        assert!(
            !git_status(repository, ["cat-file", "-e", commit.as_str()]).success(),
            "the remote authority object must not enter the caller object database",
        );
    }
}
