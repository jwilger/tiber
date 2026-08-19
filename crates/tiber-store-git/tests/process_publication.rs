#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "the signed Git fixture fails fast when isolated authority setup is invalid"
)]
mod tests {
    use core::{iter, time::Duration};
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    use eventcore_fs::FileEventStore;
    use tempfile::TempDir;
    use tiber_process_core::{
        AssignmentWorkflowProvenance, ConfiguredCommand, ConfiguredCommandCatalog,
        ConfiguredCommandId, FixedEnvironment, LiteralArgument, OutputBounds, ProcessInvocationId,
        ProcessRequest, RelativeWorkingDirectory,
    };
    use tiber_process_service::{
        CapturedProcessBytes, ProcessExitStatus, ProcessFact, ProcessReceipt, ProcessStream,
        decide_process_request, decide_record_completed,
    };
    use tiber_store_git::{TiberEventStore, publication::TiberEventPublisher};
    use tiber_workflow_core::{AssignmentId, EffectId, WorkflowId};

    struct SignedRepository {
        _directory: TempDir,
        repository: PathBuf,
    }

    impl SignedRepository {
        fn new() -> Self {
            let directory = TempDir::new().expect("fixture directory");
            let repository = directory.path().join("repository");
            let signing_key = directory.path().join("signing-key");
            git(directory.path(), ["init", utf8(&repository)]);
            git(
                &repository,
                ["config", "user.name", "Tiber Process Fixture"],
            );
            git(
                &repository,
                ["config", "user.email", "process-fixture@example.invalid"],
            );
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&signing_key)
                .status()
                .expect("ssh-keygen starts");
            assert!(status.success());
            let public_key =
                fs::read_to_string(signing_key.with_extension("pub")).expect("public key reads");
            let allowed_signers = directory.path().join("allowed-signers");
            fs::write(
                &allowed_signers,
                format!("process-fixture@example.invalid {}", public_key.trim()),
            )
            .expect("allowed signers writes");
            git(&repository, ["config", "gpg.format", "ssh"]);
            git(&repository, ["config", "commit.gpgsign", "true"]);
            git(
                &repository,
                ["config", "user.signingkey", utf8(&signing_key)],
            );
            git(
                &repository,
                [
                    "config",
                    "gpg.ssh.allowedSignersFile",
                    utf8(&allowed_signers),
                ],
            );
            let store = FileEventStore::open(repository.join("eventstore"))
                .expect("event store initializes");
            drop(store);
            fs::write(repository.join("eventstore/events/.keep"), "")
                .expect("authority marker writes");
            git(&repository, ["add", "eventstore/events/.keep"]);
            git(&repository, ["commit", "-m", "empty authority"]);
            let revision = git_output(&repository, ["rev-parse", "HEAD"]);
            git(
                &repository,
                ["update-ref", "refs/heads/tiber", revision.trim()],
            );
            Self {
                _directory: directory,
                repository,
            }
        }
    }

    #[tokio::test]
    async fn publishes_requested_and_prepared_as_one_ordered_signed_batch() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let (stream, request, catalog) = process_fixture("effect-process-publish");
        let publication = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
            .expect("request is modeled");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens at signed base");

        let signed_revision = publisher
            .publish_process(&stream, publication)
            .await
            .expect("closed process batch publishes");

        assert_ne!(signed_revision.revision(), before.revision());
        let after = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        let history = after
            .read_process_history(&stream)
            .expect("exact signed process history reads");
        assert_eq!(history.revision(), signed_revision.revision());
        assert!(
            matches!(history.events().first().expect("requested event").fact(), ProcessFact::Requested(recorded) if recorded == &request)
        );
        assert!(
            matches!(history.events().get(1).expect("prepared event").fact(), ProcessFact::Prepared(identity) if identity.request() == &request)
        );
    }

    #[tokio::test]
    async fn publishes_one_terminal_fact_after_exact_signed_replay() {
        let fixture = SignedRepository::new();
        let initial = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let (stream, request, catalog) = process_fixture("effect-process-terminal");
        let request_publication = decide_process_request(&[], stream.clone(), request, &catalog)
            .expect("request modeled");
        let mut request_publisher =
            TiberEventPublisher::open_at(&fixture.repository, initial.revision())
                .expect("publisher opens");
        let requested = request_publisher
            .publish_process(&stream, request_publication)
            .await
            .expect("request publishes");
        let requested_store = TiberEventStore::open(&fixture.repository).expect("history opens");
        let requested_history = requested_store
            .read_process_history(&stream)
            .expect("signed history reads");
        let identity = requested_history
            .events()
            .iter()
            .find_map(|event| match event.fact() {
                ProcessFact::Prepared(identity) => Some(identity.clone()),
                _ => None,
            })
            .expect("signed history contains prepared identity");
        let stdout = CapturedProcessBytes::new(Vec::new()).expect("stdout valid");
        let stderr = CapturedProcessBytes::new(Vec::new()).expect("stderr valid");
        let receipt = ProcessReceipt::new(identity, ProcessExitStatus::Exited(0), &stdout, &stderr)
            .expect("receipt valid");
        let completion =
            decide_record_completed(requested_history.events(), stream.clone(), receipt)
                .expect("completion modeled");
        let mut terminal_publisher =
            TiberEventPublisher::open_at(&fixture.repository, requested.revision())
                .expect("publisher reopens at request revision");

        terminal_publisher
            .publish_process(&stream, completion)
            .await
            .expect("terminal fact publishes");

        let after = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        let history = after.read_process_history(&stream).expect("history reads");
        assert_eq!(history.events().len(), 3);
        assert!(matches!(
            history.events().last().expect("terminal event").fact(),
            ProcessFact::Completed(_)
        ));
    }

    #[tokio::test]
    async fn exact_empty_retry_confirms_the_base_without_creating_a_commit() {
        let fixture = SignedRepository::new();
        let initial = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let (stream, request, catalog) = process_fixture("effect-process-retry");
        let first = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
            .expect("request modeled");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, initial.revision())
            .expect("publisher opens");
        let signed_revision = publisher
            .publish_process(&stream, first)
            .await
            .expect("first publication succeeds");
        let store = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        let history = store.read_process_history(&stream).expect("history reads");
        let retry = decide_process_request(history.events(), stream.clone(), request, &catalog)
            .expect("identical request is an idempotent publication");
        let mut retry_publisher =
            TiberEventPublisher::open_at(&fixture.repository, signed_revision.revision())
                .expect("publisher reopens");

        let confirmed = retry_publisher
            .publish_process(&stream, retry)
            .await
            .expect("empty retry confirms its exact base");

        assert_eq!(confirmed.revision(), signed_revision.revision());
        let after = TiberEventStore::open(&fixture.repository).expect("authority still opens");
        assert_eq!(after.revision(), signed_revision.revision());
    }

    #[tokio::test]
    async fn empty_retry_rejects_authority_that_advanced_after_the_stage_opened() {
        let fixture = SignedRepository::new();
        let initial = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let (stream, request, catalog) = process_fixture("effect-process-empty-stale");
        let first = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
            .expect("request modeled");
        let mut first_publisher =
            TiberEventPublisher::open_at(&fixture.repository, initial.revision())
                .expect("publisher opens");
        let signed_revision = first_publisher
            .publish_process(&stream, first)
            .await
            .expect("first publication succeeds");
        let store = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        let history = store.read_process_history(&stream).expect("history reads");
        let retry = decide_process_request(history.events(), stream.clone(), request, &catalog)
            .expect("identical request is an idempotent publication");
        let mut retry_publisher =
            TiberEventPublisher::open_at(&fixture.repository, signed_revision.revision())
                .expect("retry stage opens at current authority");
        git(
            &fixture.repository,
            ["commit", "--allow-empty", "-m", "advance"],
        );
        let advanced = git_output(&fixture.repository, ["rev-parse", "HEAD"]);
        git(
            &fixture.repository,
            ["update-ref", "refs/heads/tiber", advanced.trim()],
        );

        let error = retry_publisher
            .publish_process(&stream, retry)
            .await
            .expect_err("empty retry must reconfirm its pinned authority");

        assert_eq!(error.code(), "tiber_store_publication_authority_changed");
    }

    #[tokio::test]
    async fn rejects_a_publication_at_a_different_process_fence_before_staging() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let (source_stream, request, catalog) = process_fixture("effect-process-source");
        let publication = decide_process_request(&[], source_stream, request, &catalog)
            .expect("source publication modeled");
        let other_stream = ProcessStream::for_invocation(
            &EffectId::parse("effect-process-other").expect("effect id valid"),
            &ProcessInvocationId::parse("invocation-other").expect("invocation id valid"),
        )
        .expect("other stream valid");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens");

        let error = publisher
            .publish_process(&other_stream, publication)
            .await
            .expect_err("mismatched process fence must fail");

        assert_eq!(error.code(), "tiber_store_publication_undeclared_stream");
        let after = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        assert_eq!(after.revision(), before.revision());
    }

    #[test]
    fn stale_signed_base_is_rejected_before_a_process_stage_opens() {
        let fixture = SignedRepository::new();
        let stale = TiberEventStore::open(&fixture.repository).expect("authority opens");
        git(
            &fixture.repository,
            ["commit", "--allow-empty", "-m", "advance"],
        );
        let advanced = git_output(&fixture.repository, ["rev-parse", "HEAD"]);
        git(
            &fixture.repository,
            ["update-ref", "refs/heads/tiber", advanced.trim()],
        );

        let error = TiberEventPublisher::open_at(&fixture.repository, stale.revision())
            .expect_err("stale base must be rejected");

        assert_eq!(error.code(), "tiber_store_publication_authority_changed");
    }

    fn process_fixture(effect: &str) -> (ProcessStream, ProcessRequest, ConfiguredCommandCatalog) {
        let effect_id = EffectId::parse(effect).expect("effect id valid");
        let request = ProcessRequest::for_invocation(
            ConfiguredCommandId::parse("unit-test").expect("command id valid"),
            ProcessInvocationId::parse(&format!("{effect}-invocation"))
                .expect("invocation id valid"),
            AssignmentWorkflowProvenance::new(
                WorkflowId::parse("workflow-3").expect("workflow id valid"),
                AssignmentId::parse("assignment-3").expect("assignment id valid"),
                effect_id.clone(),
            ),
        );
        let command = ConfiguredCommand::new(
            PathBuf::from("/nix/store/example/bin/cargo"),
            vec![LiteralArgument::parse("test").expect("argument valid")],
            RelativeWorkingDirectory::parse(".").expect("working directory valid"),
            FixedEnvironment::new(iter::empty::<(&str, &str)>()).expect("environment valid"),
            Duration::from_secs(30),
            OutputBounds::new(0x4000, 0x2000).expect("output bounds valid"),
        )
        .expect("command valid");
        let catalog = ConfiguredCommandCatalog::new([(
            ConfiguredCommandId::parse("unit-test").expect("command id valid"),
            command,
        )])
        .expect("catalog valid");
        let stream = ProcessStream::for_request(&request).expect("process stream valid");
        (stream, request, catalog)
    }

    fn git<const N: usize>(repository: &Path, arguments: [&str; N]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .status()
            .expect("git starts");
        assert!(status.success());
    }

    fn git_output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .output()
            .expect("git starts");
        assert!(output.status.success());
        String::from_utf8(output.stdout).expect("git output is UTF-8")
    }

    fn utf8(path: &Path) -> &str {
        path.to_str().expect("fixture path is UTF-8")
    }
}
