#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    reason = "the signed publication fixture fails fast when its isolated Git or EventCore authority cannot be constructed"
)]
mod tests {
    use core::slice;
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    use eventcore::model::StreamIdentity as _;
    use eventcore_fs::FileEventStore;
    use eventcore_types::{BatchSize, StreamPattern};
    use tempfile::TempDir;
    use tiber_session_service::{
        AssistantText, InferenceMode, IsolatedTurnBinding, IsolatedTurnEvent, IsolatedTurnId,
        IsolatedTurnKind, PromptText, SessionBinding, SessionEvent, SessionFact,
        decide_accept_plan, decide_accept_plan_and_request_inference, decide_cancel_plan,
        decide_close_isolated_turn, decide_observe_inference, decide_observe_isolated_turn,
        decide_open_isolated_turn, decide_propose_plan, decide_request_inference,
        decide_request_isolated_turn, decide_request_plan, decide_start_session,
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
        WorkflowFact, WorkflowStream, decide_advance_workflow,
        decide_initialize_successor_workflow, decide_initialize_workflow,
        decide_record_observation, decide_request_next_effect,
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

    #[expect(
        clippy::single_call_fn,
        reason = "the named third effect keeps the branch-identity fixture distinct and readable"
    )]
    fn effect_three() -> InferEffect {
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
            parsed(EffectId::parse, "effect-3"),
            parsed(IdempotencyKey::parse, "session-1:turn-3"),
            base.deadline_milliseconds(),
        )
    }

    #[tokio::test]
    async fn checked_plan_cancellation_publishes_its_sole_session_fact() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let start = decide_start_session(&[], binding())
            .expect("start is modeled")
            .expect("new session");
        let (started, _) = decide_start_session(&[], binding())
            .expect("start is modeled")
            .expect("new session")
            .into_event_and_consistency_streams();
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens");
        let started_revision = publisher
            .publish_session_start(start)
            .await
            .expect("start publishes");

        let prompt = PromptText::parse("plan the publication boundary").expect("prompt");
        let request = decide_request_plan(slice::from_ref(&started), binding(), prompt, effect())
            .expect("plan request is modeled");
        let (requested, _) = request.into_event_and_consistency_streams();
        let proposal = decide_propose_plan(
            &[started.clone(), requested.clone()],
            AssistantText::parse("Publish only the checked decision.").expect("proposal"),
        )
        .expect("proposal is modeled");
        let (proposed, _) = proposal.into_event_and_consistency_streams();
        let acceptance =
            decide_accept_plan(&[started.clone(), requested.clone(), proposed.clone()])
                .expect("acceptance is modeled")
                .expect("first acceptance emits");
        let mut rejected_publisher =
            TiberEventPublisher::open_at(&fixture.repository, started_revision.revision())
                .expect("acceptance publisher fences started authority");
        assert!(
            rejected_publisher
                .publish_plan_decision(acceptance)
                .await
                .is_err(),
            "acceptance cannot publish without its ordinary workflow continuation"
        );
        assert_eq!(
            TiberEventStore::open(&fixture.repository)
                .expect("rejected authority opens")
                .revision(),
            started_revision.revision()
        );
        let decision = decide_cancel_plan(&[started, requested, proposed])
            .expect("decision is modeled")
            .expect("first decision emits");
        let mut decision_publisher =
            TiberEventPublisher::open_at(&fixture.repository, started_revision.revision())
                .expect("decision publisher fences started authority");
        let decided_revision = decision_publisher
            .publish_plan_decision(decision)
            .await
            .expect("checked plan decision publishes");

        let store = TiberEventStore::open(&fixture.repository).expect("published authority opens");
        assert_eq!(store.revision(), decided_revision.revision());
        let pattern =
            StreamPattern::try_new("tiber:session:active".to_owned()).expect("session pattern");
        let reader = store
            .verified_transaction_reader::<SessionEvent>(&[pattern])
            .expect("session history verifies");
        let events = reader
            .read_page(TransactionEventPage::first(BatchSize::new(8)))
            .expect("session history reads");
        assert!(matches!(
            events.last().map(SessionEvent::fact),
            Some(SessionFact::PlanDecided { .. })
        ));
    }

    #[tokio::test]
    async fn checked_isolated_turn_lifecycle_publishes_only_to_its_child_stream() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let branch = IsolatedTurnBinding::new(binding(), HarnessState::new(effect_two()))
            .expect("branch binding");
        let opened = decide_open_isolated_turn(
            &[],
            IsolatedTurnId::parse("side-store-1").expect("turn id"),
            IsolatedTurnKind::Side,
            branch.clone(),
        )
        .expect("open modeled")
        .expect("new open");
        let opened_event = opened.event().clone();
        let request = decide_request_isolated_turn(
            slice::from_ref(&opened_event),
            PromptText::parse("isolated store prompt").expect("prompt"),
            branch.workflow_state().initial_effect().clone(),
        )
        .expect("request modeled");
        let mut open_publisher =
            TiberEventPublisher::open_at(&fixture.repository, before.revision())
                .expect("publisher opens");
        let opened_revision = open_publisher
            .publish_isolated_turn_open_and_request(opened, request)
            .await
            .expect("isolated open and request publish atomically");
        let (requested_event, _) = decide_request_isolated_turn(
            slice::from_ref(&opened_event),
            PromptText::parse("isolated store prompt").expect("prompt"),
            branch.workflow_state().initial_effect().clone(),
        )
        .expect("request modeled")
        .into_event_and_consistency_streams();
        let observation = decide_observe_isolated_turn(
            &[opened_event.clone(), requested_event.clone()],
            AssistantText::parse("isolated store answer").expect("assistant"),
        )
        .expect("observation modeled");
        let (observed_event, _) = decide_observe_isolated_turn(
            &[opened_event.clone(), requested_event.clone()],
            AssistantText::parse("isolated store answer").expect("assistant"),
        )
        .expect("observation modeled")
        .into_event_and_consistency_streams();
        let mut observation_publisher =
            TiberEventPublisher::open_at(&fixture.repository, opened_revision.revision())
                .expect("observation publisher opens");
        let observed_revision = observation_publisher
            .publish_isolated_turn_observation(observation)
            .await
            .expect("isolated observation publishes");
        let close =
            decide_close_isolated_turn(&[opened_event.clone(), requested_event, observed_event])
                .expect("close modeled")
                .expect("first close emits");
        let mut close_publisher =
            TiberEventPublisher::open_at(&fixture.repository, observed_revision.revision())
                .expect("close publisher opens");
        let closed_revision = close_publisher
            .publish_isolated_turn_close(close)
            .await
            .expect("isolated close publishes");
        let store = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        assert_eq!(store.revision(), closed_revision.revision());
        let pattern = StreamPattern::try_new(opened_event.stream_id().as_ref().to_owned())
            .expect("child pattern");
        let events = store
            .verified_transaction_reader::<IsolatedTurnEvent>(&[pattern])
            .expect("child history verifies")
            .read_page(TransactionEventPage::first(BatchSize::new(4)))
            .expect("child history reads");
        assert_eq!(events.len(), 4);
    }

    #[tokio::test]
    async fn atomic_isolated_open_rejects_a_request_for_another_branch_identity() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let parent = binding();
        let branch_a = IsolatedTurnBinding::new(parent.clone(), HarnessState::new(effect_two()))
            .expect("first branch binding");
        let branch_b = IsolatedTurnBinding::new(parent, HarnessState::new(effect_three()))
            .expect("second branch binding");
        let turn_id = IsolatedTurnId::parse("same-stream-turn").expect("turn id");
        let open_a =
            decide_open_isolated_turn(&[], turn_id.clone(), IsolatedTurnKind::Side, branch_a)
                .expect("first open modeled")
                .expect("first open emitted");
        let open_b =
            decide_open_isolated_turn(&[], turn_id, IsolatedTurnKind::Side, branch_b.clone())
                .expect("second open modeled")
                .expect("second open emitted");
        assert_eq!(open_a.event().stream_id(), open_b.event().stream_id());
        let request_b = decide_request_isolated_turn(
            slice::from_ref(open_b.event()),
            PromptText::parse("foreign branch request").expect("prompt"),
            branch_b.workflow_state().initial_effect().clone(),
        )
        .expect("foreign request modeled");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens");
        assert!(
            publisher
                .publish_isolated_turn_open_and_request(open_a, request_b)
                .await
                .is_err(),
            "same-stream tokens with different branch identities must be rejected"
        );
        let after = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        assert_eq!(after.revision(), before.revision());
    }

    #[tokio::test]
    #[expect(
        clippy::indexing_slicing,
        clippy::panic,
        clippy::pattern_type_mismatch,
        clippy::too_many_lines,
        reason = "the bounded end-to-end fixture asserts exact ordered session and workflow transactions"
    )]
    async fn accepted_plan_and_ordinary_workflow_request_publish_in_one_signed_candidate() {
        let fixture = SignedRepository::new();
        let before = TiberEventStore::open(&fixture.repository).expect("authority opens");
        let start = decide_start_session(&[], binding())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let plan = decide_request_plan(
            slice::from_ref(&started),
            binding(),
            PromptText::parse("plan atomic continuation").expect("prompt"),
            effect(),
        )
        .expect("plan request");
        let (plan_requested, _) = plan.into_event_and_consistency_streams();
        let proposal = decide_propose_plan(
            &[started.clone(), plan_requested.clone()],
            AssistantText::parse("Continue ordinarily.").expect("proposal"),
        )
        .expect("proposal");
        let (proposed, _) = proposal.into_event_and_consistency_streams();
        let planning_effect = effect();
        let planning_stream =
            WorkflowStream::for_effect(&planning_effect).expect("planning stream");
        let planning_initialization = decide_initialize_workflow(
            planning_stream.clone(),
            HarnessState::new(planning_effect.clone()),
        )
        .expect("planning initialization");
        let initialized = planning_initialization.event().clone();
        let planning_request =
            decide_request_next_effect(slice::from_ref(&initialized), planning_stream.clone())
                .expect("planning workflow request");
        let workflow_requested = planning_request.event().clone();
        let planning_observation = decide_record_observation(
            &[initialized.clone(), workflow_requested.clone()],
            planning_stream.clone(),
            EffectObservation::Succeeded {
                effect_id: planning_effect.effect_id().clone(),
                receipt_id: EffectReceiptId::parse("receipt-plan").expect("receipt"),
            },
        )
        .expect("planning observation");
        let workflow_observed = planning_observation.event().clone();
        let planning_advance = decide_advance_workflow(
            &[
                initialized.clone(),
                workflow_requested.clone(),
                workflow_observed.clone(),
            ],
            planning_stream.clone(),
        )
        .expect("planning completion");
        let completed = planning_advance.event().clone();
        let WorkflowFact::WorkflowCompleted { successor, .. } = completed.fact() else {
            panic!("planning workflow must complete");
        };
        let next = successor.initial_effect().clone();
        let session = decide_accept_plan_and_request_inference(
            &[started, plan_requested, proposed],
            PromptText::parse("continue now").expect("prompt"),
            next.clone(),
        )
        .expect("accepted continuation")
        .expect("first acceptance emits");
        let stream = WorkflowStream::for_effect(&next).expect("workflow stream");
        let initialization = decide_initialize_successor_workflow(
            &[
                initialized,
                workflow_requested,
                workflow_observed,
                completed,
            ],
            planning_stream,
            stream.clone(),
        )
        .expect("successor initialization");
        let request =
            decide_request_next_effect(slice::from_ref(initialization.event()), stream.clone())
                .expect("workflow request");
        let mut publisher = TiberEventPublisher::open_at(&fixture.repository, before.revision())
            .expect("publisher opens");
        let final_revision = publisher
            .publish_accepted_plan_inference_with_workflow(session, initialization, request)
            .await
            .expect("atomic accepted continuation publishes");
        let after = TiberEventStore::open(&fixture.repository).expect("authority reopens");
        assert_eq!(after.revision(), final_revision.revision());
        let session_pattern =
            StreamPattern::try_new("tiber:session:active".to_owned()).expect("session pattern");
        let session_events = after
            .verified_transaction_reader::<SessionEvent>(&[session_pattern])
            .expect("session transaction verifies")
            .read_page(TransactionEventPage::first(BatchSize::new(3)))
            .expect("session transaction reads");
        assert_eq!(session_events.len(), 2);
        assert!(matches!(
            session_events[0].fact(),
            SessionFact::PlanDecided {
                decision: tiber_session_service::PlanDecision::Accepted,
                ..
            }
        ));
        assert!(matches!(
            session_events[1].fact(),
            SessionFact::InferenceRequested {
                effect,
                mode: InferenceMode::Ordinary,
                ..
            } if effect == &next
        ));
        let workflow_pattern = StreamPattern::try_new(stream.as_stream_id().as_ref().to_owned())
            .expect("workflow pattern");
        let workflow_events = after
            .verified_transaction_reader::<tiber_workflow_service::WorkflowEvent>(&[
                workflow_pattern,
            ])
            .expect("workflow transaction verifies")
            .read_page(TransactionEventPage::first(BatchSize::new(3)))
            .expect("workflow transaction reads");
        assert_eq!(workflow_events.len(), 2);
        assert!(matches!(
            workflow_events[0].fact(),
            WorkflowFact::WorkflowInitialized { state } if state.initial_effect() == &next
        ));
        assert!(matches!(
            workflow_events[1].fact(),
            WorkflowFact::EffectRequested {
                effect: tiber_workflow_core::TiberEffect::Infer(effect),
                ..
            } if effect == &next
        ));
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
                mode: InferenceMode::Ordinary,
                planning_binding: None,
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
