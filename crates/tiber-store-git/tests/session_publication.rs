#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    reason = "the signed publication fixture fails fast when its isolated Git or EventCore authority cannot be constructed"
)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    use eventcore_fs::FileEventStore;
    use eventcore_types::{BatchSize, StreamPattern};
    use tempfile::TempDir;
    use tiber_session_service::{
        AssistantText, PromptText, SessionBinding, SessionEvent, SessionFact,
        decide_observe_inference, decide_request_inference, decide_start_session,
        project_started_session, task_assignment_scope,
    };
    use tiber_store_git::{
        TiberEventStore, TransactionEventPage, publication::TiberEventPublisher,
    };
    use tiber_tasks_core::TaskId;
    use tiber_workflow_core::{
        AgentId, AssignmentEpoch, AssignmentId, AttemptNumber, ContextReceiptId,
        DeadlineMilliseconds, EffectId, EffectObservation, EffectReceiptId, HarnessState,
        IdempotencyKey, InferEffect, PolicyDecisionId, SessionId, WorkflowId,
    };
    use tiber_workflow_service::{
        WorkflowStream, decide_initialize_workflow, decide_record_observation,
        decide_request_next_effect,
    };

    struct SignedRepository {
        _directory: TempDir,
        repository: PathBuf,
    }

    impl SignedRepository {
        fn new() -> Self {
            let directory = TempDir::new().expect("fixture directory should be created");
            let repository = directory.path().join("repository");
            let signing_key = directory.path().join("signing-key");
            git(directory.path(), ["init", utf8(&repository)]);
            git(
                &repository,
                ["config", "user.name", "Tiber Session Fixture"],
            );
            git(
                &repository,
                ["config", "user.email", "session-fixture@example.invalid"],
            );
            let key = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&signing_key)
                .status()
                .expect("fixture signing key command should start");
            assert!(key.success());
            let public_key = fs::read_to_string(signing_key.with_extension("pub"))
                .expect("fixture public key should be readable");
            let allowed_signers = directory.path().join("allowed-signers");
            fs::write(
                &allowed_signers,
                format!("session-fixture@example.invalid {}", public_key.trim()),
            )
            .expect("fixture allowed signers should be written");
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
                .expect("fixture EventCore store should initialize");
            drop(store);
            fs::write(repository.join("eventstore/events/.keep"), "")
                .expect("empty authority marker should be written");
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

    fn binding() -> SessionBinding {
        let task = TaskId::parse("session-fixture-task").expect("task identity should be valid");
        let effect = InferEffect::new(
            parsed(SessionId::parse, "session-1"),
            parsed(AgentId::parse, "agent-1"),
            parsed(WorkflowId::parse, "workflow-1"),
            parsed(AssignmentId::parse, "assignment-1"),
            task_assignment_scope(&task).expect("task scope"),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            parsed(ContextReceiptId::parse, "context-1"),
            parsed(PolicyDecisionId::parse, "policy-1"),
            parsed(EffectId::parse, "effect-1"),
            parsed(IdempotencyKey::parse, "session-1:turn-1"),
            DeadlineMilliseconds::parse(60_000).expect("deadline should be valid"),
        );
        SessionBinding::new(task, HarnessState::new(effect))
    }

    fn effect() -> InferEffect {
        binding().workflow_state().initial_effect().clone()
    }

    fn effect_two() -> InferEffect {
        let base = effect();
        InferEffect::new(
            base.session_id().clone(),
            base.agent_id().clone(),
            base.workflow_id().clone(),
            base.assignment_id().clone(),
            base.assignment_scope().clone(),
            base.assignment_epoch(),
            base.attempt_number(),
            base.context_receipt_id().clone(),
            base.policy_decision_id().clone(),
            parsed(EffectId::parse, "effect-2"),
            parsed(IdempotencyKey::parse, "session-1:turn-2"),
            base.deadline_milliseconds(),
        )
    }

    #[tokio::test]
    #[expect(
        clippy::indexing_slicing,
        clippy::similar_names,
        reason = "the bounded signed-history fixture distinguishes fixed event and stream collections"
    )]
    async fn publishes_one_modeled_session_start_to_signed_authority() {
        let fixture = SignedRepository::new();
        let before =
            TiberEventStore::open(&fixture.repository).expect("signed empty authority should open");
        let binding = binding();
        let publication = decide_start_session(&[], binding.clone())
            .expect("session start should be modeled")
            .expect("session is new");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher should fence the exact signed base");

        let published = publisher
            .publish_session_start(publication)
            .await
            .expect("modeled session start should publish");

        assert_ne!(published.revision(), before.revision());
        let after = TiberEventStore::open(&fixture.repository)
            .expect("published signed authority should reopen");
        let pattern = StreamPattern::try_new("tiber:session:active".to_owned())
            .expect("session stream pattern should be valid");
        let reader = after
            .verified_transaction_reader::<SessionEvent>(&[pattern])
            .expect("session event should verify");
        let events = reader
            .read_page(TransactionEventPage::first(BatchSize::new(1)))
            .expect("session event should read");
        assert_eq!(events.len(), 1);
        assert_eq!(
            project_started_session(&events[0]).expect("start fact"),
            binding
        );
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::shadow_unrelated,
        reason = "the cross-domain fixture keeps explicit fact paths and successive publisher stages visible"
    )]
    #[expect(
        clippy::indexing_slicing,
        reason = "the fixture inspects a fixed two-event workflow publication"
    )]
    async fn publishes_the_modeled_prompt_request_after_the_session_start() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let start = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new");
        let (started, _streams) = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new")
            .into_event_and_consistency_streams();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens");
        let started_revision = publisher
            .publish_session_start(start)
            .await
            .expect("start publishes");
        let prompt = PromptText::parse("keep this conversation durable").expect("prompt valid");
        let request = decide_request_inference(&[started], prompt.clone(), effect())
            .expect("request modeled");
        let workflow_stream = WorkflowStream::for_effect(&effect()).expect("workflow stream");
        let initialization =
            decide_initialize_workflow(workflow_stream.clone(), HarnessState::new(effect()))
                .expect("workflow initialized");
        let workflow_request = decide_request_next_effect(
            core::slice::from_ref(initialization.event()),
            workflow_stream,
        )
        .expect("workflow request");
        let mut publisher =
            TiberEventPublisher::open_at(&fixture.repository, started_revision.revision())
                .expect("publisher reopens");

        publisher
            .publish_inference_request_with_workflow(request, initialization, workflow_request)
            .await
            .expect("request publishes");

        let after = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        let pattern =
            StreamPattern::try_new("tiber:session:active".to_owned()).expect("pattern valid");
        let reader = after
            .verified_transaction_reader::<SessionEvent>(&[pattern])
            .expect("history verifies");
        let events = reader
            .read_page(TransactionEventPage::first(BatchSize::new(2)))
            .expect("history reads");
        assert_eq!(
            events[1].fact(),
            &SessionFact::InferenceRequested {
                effect: effect(),
                predecessor_effect_id: None,
                prompt
            }
        );
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::shadow_unrelated,
        reason = "the mismatch fixture keeps explicit fact paths and successive publisher stages visible"
    )]
    async fn rejects_mismatched_session_and_workflow_effect_tokens() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let start = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new");
        let (started, _) = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new")
            .into_event_and_consistency_streams();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher");
        let revision = publisher
            .publish_session_start(start)
            .await
            .expect("start publishes");
        let session_request = decide_request_inference(
            core::slice::from_ref(&started),
            PromptText::parse("durable").expect("prompt"),
            effect(),
        )
        .expect("session request");
        let foreign = effect_two();
        let stream = WorkflowStream::for_effect(&foreign).expect("stream");
        let initialization = decide_initialize_workflow(stream.clone(), HarnessState::new(foreign))
            .expect("initialization");
        let request =
            decide_request_next_effect(core::slice::from_ref(initialization.event()), stream)
                .expect("request");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, revision.revision())
            .expect("publisher");

        let error = publisher
            .publish_inference_request_with_workflow(session_request, initialization, request)
            .await
            .expect_err("mismatch rejected");

        assert_eq!(
            error.code(),
            "tiber_store_publication_workflow_effect_mismatch"
        );
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::cloned_ref_to_slice_refs,
        clippy::panic,
        clippy::pattern_type_mismatch,
        clippy::shadow_unrelated,
        clippy::too_many_lines,
        reason = "the chronological signed-authority scenario retains each exact modeled fact and exhaustively identifies its required completion variant across two publication attempts"
    )]
    async fn rejects_a_fabricated_same_assignment_effect_after_durable_completion() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let start = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new");
        let (started, _) = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new")
            .into_event_and_consistency_streams();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens");
        let revision = publisher
            .publish_session_start(start)
            .await
            .expect("start publishes");

        let first_prompt = PromptText::parse("durable first turn").expect("prompt");
        let request = decide_request_inference(&[started.clone()], first_prompt.clone(), effect())
            .expect("first request modeled");
        let (requested, _) = decide_request_inference(&[started.clone()], first_prompt, effect())
            .expect("first request modeled")
            .into_event_and_consistency_streams();
        let first_stream = WorkflowStream::for_effect(&effect()).expect("first workflow stream");
        let initialization =
            decide_initialize_workflow(first_stream.clone(), HarnessState::new(effect()))
                .expect("first workflow initialized");
        let initialized = initialization.event().clone();
        let workflow_request =
            decide_request_next_effect(&[initialized.clone()], first_stream.clone())
                .expect("first workflow request");
        let workflow_requested = workflow_request.event().clone();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, revision.revision())
            .expect("publisher reopens");
        let revision = publisher
            .publish_inference_request_with_workflow(request, initialization, workflow_request)
            .await
            .expect("first request publishes");

        let observation = decide_observe_inference(
            &[started.clone(), requested.clone()],
            AssistantText::parse("first answer").expect("assistant"),
        )
        .expect("session observation modeled");
        let (observed, _) = decide_observe_inference(
            &[started.clone(), requested.clone()],
            AssistantText::parse("first answer").expect("assistant"),
        )
        .expect("session observation modeled")
        .into_event_and_consistency_streams();
        let workflow_observation = decide_record_observation(
            &[initialized.clone(), workflow_requested.clone()],
            first_stream.clone(),
            EffectObservation::Succeeded {
                effect_id: effect().effect_id().clone(),
                receipt_id: parsed(EffectReceiptId::parse, "receipt-1"),
            },
        )
        .expect("workflow observation modeled");
        let workflow_observed = workflow_observation.event().clone();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, revision.revision())
            .expect("publisher reopens");
        let revision = publisher
            .publish_inference_observation_with_workflow(observation, workflow_observation)
            .await
            .expect("observation publishes");
        let advance = tiber_workflow_service::decide_advance_workflow(
            &[initialized, workflow_requested, workflow_observed],
            first_stream,
        )
        .expect("workflow completion modeled");
        let completed = advance.event().clone();
        let tiber_workflow_service::WorkflowFact::WorkflowCompleted { successor, .. } =
            completed.fact()
        else {
            panic!("successful first turn must retain its exact successor");
        };
        let fabricated = effect_two();
        assert_ne!(fabricated, *successor.initial_effect());
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, revision.revision())
            .expect("publisher reopens");
        let revision = publisher
            .publish_workflow_advance(advance)
            .await
            .expect("completion publishes");

        let later_request = decide_request_inference(
            &[started, requested, observed],
            PromptText::parse("fabricated second turn").expect("prompt"),
            fabricated.clone(),
        )
        .expect("same-assignment effect passes the session-only model");
        let later_stream = WorkflowStream::for_effect(&fabricated).expect("later workflow stream");
        let later_initialization =
            decide_initialize_workflow(later_stream.clone(), HarnessState::new(fabricated))
                .expect("standalone initialization models");
        let later_workflow_request = decide_request_next_effect(
            core::slice::from_ref(later_initialization.event()),
            later_stream,
        )
        .expect("later workflow request models");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, revision.revision())
            .expect("publisher reopens");

        let error = publisher
            .publish_inference_request_with_workflow(
                later_request,
                later_initialization,
                later_workflow_request,
            )
            .await
            .expect_err("fabricated successor authority must be rejected");

        assert_eq!(
            error.code(),
            "tiber_store_publication_workflow_effect_mismatch"
        );
    }

    #[tokio::test]
    #[expect(
        clippy::cloned_ref_to_slice_refs,
        reason = "the fixture retains its owned request event while constructing signed chronological history"
    )]
    async fn rejects_mismatched_session_and_workflow_observation_tokens() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let (started, _) = decide_start_session(&[], binding())
            .expect("start")
            .expect("session is new")
            .into_event_and_consistency_streams();
        let (requested, _) = decide_request_inference(
            &[started.clone()],
            PromptText::parse("durable").expect("prompt"),
            effect(),
        )
        .expect("request")
        .into_event_and_consistency_streams();
        let session_observation = decide_observe_inference(
            &[started, requested],
            AssistantText::parse("answer").expect("assistant"),
        )
        .expect("session observation");
        let foreign = effect_two();
        let stream = WorkflowStream::for_effect(&foreign).expect("stream");
        let initialization =
            decide_initialize_workflow(stream.clone(), HarnessState::new(foreign.clone()))
                .expect("initialization");
        let initialized = initialization.event().clone();
        let request =
            decide_request_next_effect(&[initialized.clone()], stream.clone()).expect("request");
        let workflow_observation = decide_record_observation(
            &[initialized, request.event().clone()],
            stream,
            EffectObservation::Succeeded {
                effect_id: foreign.effect_id().clone(),
                receipt_id: parsed(EffectReceiptId::parse, "receipt-2"),
            },
        )
        .expect("workflow observation");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher");

        let error = publisher
            .publish_inference_observation_with_workflow(session_observation, workflow_observation)
            .await
            .expect_err("mismatch rejected");

        assert_eq!(
            error.code(),
            "tiber_store_publication_workflow_effect_mismatch"
        );
    }

    #[tokio::test]
    #[expect(
        clippy::cloned_ref_to_slice_refs,
        reason = "the fixture retains an owned request event while assembling signed chronological history"
    )]
    #[expect(
        clippy::absolute_paths,
        clippy::shadow_unrelated,
        reason = "the cross-domain publication fixture keeps explicit fact paths and successive publisher stages visible"
    )]
    async fn publishes_the_modeled_assistant_observation_after_its_request() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let start = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new");
        let (started, _) = decide_start_session(&[], binding())
            .expect("start modeled")
            .expect("session is new")
            .into_event_and_consistency_streams();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens");
        let revision = publisher
            .publish_session_start(start)
            .await
            .expect("start publishes");
        let request = decide_request_inference(
            core::slice::from_ref(&started),
            PromptText::parse("durable").expect("prompt"),
            effect(),
        )
        .expect("request modeled");
        let (requested, _) = decide_request_inference(
            &[started.clone()],
            PromptText::parse("durable").expect("prompt"),
            effect(),
        )
        .expect("request modeled")
        .into_event_and_consistency_streams();
        let workflow_stream = WorkflowStream::for_effect(&effect()).expect("workflow stream");
        let initialization =
            decide_initialize_workflow(workflow_stream.clone(), HarnessState::new(effect()))
                .expect("workflow initialized");
        let initialized_event = initialization.event().clone();
        let workflow_request = decide_request_next_effect(
            core::slice::from_ref(&initialized_event),
            workflow_stream.clone(),
        )
        .expect("workflow request");
        let workflow_requested_event = workflow_request.event().clone();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, revision.revision())
            .expect("publisher opens");
        let revision = publisher
            .publish_inference_request_with_workflow(request, initialization, workflow_request)
            .await
            .expect("request publishes");
        let observation = decide_observe_inference(
            &[started, requested],
            AssistantText::parse("hello from Tiber").expect("assistant"),
        )
        .expect("observation modeled");
        let workflow_observation = decide_record_observation(
            &[initialized_event, workflow_requested_event],
            workflow_stream,
            EffectObservation::Succeeded {
                effect_id: effect().effect_id().clone(),
                receipt_id: parsed(EffectReceiptId::parse, "receipt-1"),
            },
        )
        .expect("workflow observation");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, revision.revision())
            .expect("publisher opens");

        publisher
            .publish_inference_observation_with_workflow(observation, workflow_observation)
            .await
            .expect("observation publishes");
    }

    #[expect(
        clippy::absolute_paths,
        clippy::panic,
        reason = "the generic fixture reports the exact invalid deterministic value"
    )]
    fn parsed<T, E: core::fmt::Display>(
        parser: impl FnOnce(&str) -> Result<T, E>,
        value: &str,
    ) -> T {
        parser(value).unwrap_or_else(|error| panic!("{value} should parse: {error}"))
    }

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
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[expect(
        clippy::single_call_fn,
        reason = "one signed-authority assertion needs the bounded Git output helper"
    )]
    fn git_output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start");
        assert!(output.status.success());
        String::from_utf8(output.stdout).expect("fixture Git output should be UTF-8")
    }

    fn utf8(path: &Path) -> &str {
        path.to_str().expect("fixture paths should be UTF-8")
    }
}
