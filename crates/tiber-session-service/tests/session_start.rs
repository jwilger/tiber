#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use eventcore::model::{CheckStatus, check};
    use tiber_session_service::{
        AssistantText, PromptText, SessionBinding, SessionFact, decide_interrupt_inference,
        decide_observe_inference, decide_request_inference, decide_start_session,
        decide_succeed_session, project_started_session, task_assignment_scope,
    };
    use tiber_tasks_core::TaskId;
    use tiber_workflow_core::{
        AgentId, AssignmentEpoch, AssignmentId, AttemptNumber, ContextReceiptId,
        DeadlineMilliseconds, EffectFailureCode, EffectId, EffectObservation, HarnessState,
        IdempotencyKey, InferEffect, PolicyDecisionId, Retryability, SessionId, WorkflowId,
    };

    #[test]
    fn starting_a_session_emits_its_complete_task_and_workflow_binding() {
        let binding = binding();

        let publication = decide_start_session(&[], binding.clone())
            .expect("valid start should be modeled")
            .expect("new session should publish");
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();

        assert_eq!(consistency_streams, [event.stream_id().clone()]);
        assert_eq!(event.fact(), &SessionFact::SessionStarted { binding });
    }

    #[test]
    fn native_task_identity_has_one_bounded_assignment_scope() {
        let task = TaskId::parse("owner's-[very]-long-task?name").expect("valid task");
        let scope = task_assignment_scope(&task).expect("task scope");

        assert!(scope.as_str().starts_with("task:"));
        assert_eq!(scope, task_assignment_scope(&task).expect("same scope"));
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn session_start_rejects_a_task_with_another_tasks_assignment_scope() {
        let valid = binding();
        let mismatched = SessionBinding::new(
            TaskId::parse("another-task").expect("task"),
            valid.workflow_state().clone(),
        );

        let error = match decide_start_session(&[], mismatched) {
            Ok(_) => panic!("mismatched task scope must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_start_failed");
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn successor_rejects_a_retained_predecessor_with_another_tasks_scope() {
        let malformed = malformed_scope_start();
        let malformed_binding = project_started_session(&malformed).expect("start fact");

        let error = match decide_succeed_session(
            &[malformed],
            malformed_binding,
            binding_for_task("successor-task"),
        ) {
            Ok(_) => panic!("invalid predecessor scope must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_successor_failed");
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn inference_request_rejects_a_retained_invalid_task_scope() {
        let malformed = malformed_scope_start();
        let malformed_effect = project_started_session(&malformed)
            .expect("start")
            .workflow_state()
            .initial_effect()
            .clone();
        let error = match decide_request_inference(
            &[malformed],
            PromptText::parse("prompt").expect("prompt"),
            malformed_effect,
        ) {
            Ok(_) => panic!("invalid retained binding must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_inference_request_failed");
    }

    #[test]
    fn restarting_the_identical_durable_session_does_not_publish_again() {
        let binding = binding();
        let first = decide_start_session(&[], binding.clone())
            .expect("valid start")
            .expect("new session should publish");
        let (started, _) = first.into_event_and_consistency_streams();

        assert!(
            decide_start_session(&[started], binding)
                .expect("identical restart should reconcile")
                .is_none()
        );
    }

    #[test]
    #[expect(
        clippy::cloned_ref_to_slice_refs,
        clippy::min_ident_chars,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn restarting_after_a_durable_successor_reconciles_the_current_owner() {
        let a = binding();
        let b = binding_for_task("successor-task");
        let start = decide_start_session(&[], a.clone())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let successor =
            decide_succeed_session(&[started.clone()], a, b.clone()).expect("successor");
        let (succeeded, _) = successor.into_event_and_consistency_streams();

        assert!(
            decide_start_session(&[started, succeeded], b)
                .expect("current owner reconciles")
                .is_none()
        );
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn starting_a_different_session_over_existing_history_is_rejected() {
        let first = decide_start_session(&[], binding())
            .expect("valid start")
            .expect("new session should publish");
        let (started, _) = first.into_event_and_consistency_streams();
        let different = binding_for_task("different-task");

        let error = match decide_start_session(&[started], different) {
            Ok(_) => panic!("conflicting start must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_already_started");
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn conflicting_retained_start_history_never_reconciles_to_its_last_binding() {
        let first_binding = binding();
        let first = decide_start_session(&[], first_binding)
            .expect("valid first start")
            .expect("first session is new");
        let (first_started, _) = first.into_event_and_consistency_streams();
        let last_binding = binding_for_task("different-task");
        let last = decide_start_session(&[], last_binding.clone())
            .expect("valid isolated start")
            .expect("isolated session is new");
        let (last_started, _) = last.into_event_and_consistency_streams();

        let error = match decide_start_session(&[first_started, last_started], last_binding) {
            Ok(_) => panic!("conflicting retained history must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_already_started");
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::min_ident_chars,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn malformed_retained_successor_chain_cannot_authorize_another_successor() {
        let a = binding();
        let x = binding_for_task("task-x");
        let b = binding_for_task("task-b");
        let c = binding_for_task("task-c");
        let a_start = decide_start_session(&[], a).expect("start").expect("new");
        let (a_started, _) = a_start.into_event_and_consistency_streams();
        let x_start = decide_start_session(&[], x.clone())
            .expect("isolated start")
            .expect("new");
        let (x_started, _) = x_start.into_event_and_consistency_streams();
        let malformed_tail =
            decide_succeed_session(&[x_started], x, b.clone()).expect("isolated successor");
        let (malformed, _) = malformed_tail.into_event_and_consistency_streams();

        let error = match decide_succeed_session(&[a_started, malformed], b, c) {
            Ok(_) => panic!("broken predecessor chain must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_successor_failed");
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::min_ident_chars,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn successor_must_transfer_ownership_to_a_distinct_binding() {
        let a = binding();
        let start = decide_start_session(&[], a.clone())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let error = match decide_succeed_session(&[started], a.clone(), a) {
            Ok(_) => panic!("self-successor must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_successor_failed");
    }

    #[test]
    #[expect(
        clippy::cloned_ref_to_slice_refs,
        clippy::manual_let_else,
        clippy::min_ident_chars,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn successor_cannot_erase_an_unresolved_inference() {
        let a = binding();
        let start = decide_start_session(&[], a.clone())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let request = decide_request_inference(
            &[started.clone()],
            PromptText::parse("pending").expect("prompt"),
            effect(),
        )
        .expect("request");
        let (requested, _) = request.into_event_and_consistency_streams();
        let error = match decide_succeed_session(
            &[started, requested],
            a,
            binding_for_task("successor-task"),
        ) {
            Ok(_) => panic!("pending inference must block successor"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_successor_failed");
    }

    #[test]
    #[expect(
        clippy::min_ident_chars,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn successor_rejects_a_retained_start_from_a_foreign_stream() {
        let a = binding();
        let start = decide_start_session(&[], a.clone())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let encoded = serde_json::to_string(&started).expect("serialize");
        let foreign: tiber_session_service::SessionEvent =
            serde_json::from_str(&encoded.replace("tiber:session:active", "tiber:session:foreign"))
                .expect("foreign start");
        assert!(decide_succeed_session(&[foreign], a, binding_for_task("b")).is_err());
    }

    #[test]
    #[expect(
        clippy::cloned_ref_to_slice_refs,
        clippy::min_ident_chars,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn retained_successor_cannot_launder_an_unresolved_inference() {
        let a = binding();
        let b = binding_for_task("task-b");
        let c = binding_for_task("task-c");
        let (started, requested, observed) = completed_turn();
        let successor =
            decide_succeed_session(&[started.clone()], a, b.clone()).expect("isolated successor");
        let (succeeded, _) = successor.into_event_and_consistency_streams();
        assert!(decide_succeed_session(&[started, requested, succeeded, observed], b, c).is_err());
    }

    #[test]
    #[expect(
        clippy::min_ident_chars,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn successor_cannot_reuse_a_retained_effect_id() {
        let a = binding();
        let (started, requested, observed) = completed_turn();
        let reused_effect = effect_with_ids("effect-1", "session-1:turn-2");
        let b = binding_for_task_with_effect("task-b", reused_effect);

        assert!(decide_succeed_session(&[started, requested, observed], a, b).is_err());
    }

    #[test]
    #[expect(
        clippy::min_ident_chars,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn successor_cannot_reuse_a_retained_idempotency_key() {
        let a = binding();
        let (started, requested, observed) = completed_turn();
        let reused_key = effect_with_ids("effect-2", "session-1:turn-1");
        let b = binding_for_task_with_effect("task-b", reused_key);

        assert!(decide_succeed_session(&[started, requested, observed], a, b).is_err());
    }

    #[test]
    fn every_registered_session_command_has_complete_checked_provenance() {
        let report = check().expect("complete native session model");
        assert_eq!(report.status, CheckStatus::Verified);
        assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    }

    #[test]
    fn active_session_projection_restores_the_complete_started_binding() {
        let binding = binding();
        let publication = decide_start_session(&[], binding.clone())
            .expect("valid start should be modeled")
            .expect("new session");
        let (event, _streams) = publication.into_event_and_consistency_streams();

        assert_eq!(
            project_started_session(&event).expect("start fact"),
            binding
        );
    }

    #[test]
    fn requesting_the_first_turn_emits_the_prompt_before_inference() {
        let start = decide_start_session(&[], binding())
            .expect("valid start should be modeled")
            .expect("new session");
        let (started, _streams) = start.into_event_and_consistency_streams();
        let prompt = PromptText::parse("keep this conversation durable")
            .expect("fixture prompt should be valid");

        let publication = decide_request_inference(&[started], prompt.clone(), effect())
            .expect("first prompt should be modeled");
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();

        assert_eq!(consistency_streams, [event.stream_id().clone()]);
        assert_eq!(
            event.fact(),
            &SessionFact::InferenceRequested {
                effect: effect(),
                predecessor_effect_id: None,
                prompt,
            }
        );
    }

    #[test]
    fn prompt_text_rejects_empty_and_oversized_protocol_inputs() {
        assert_eq!(
            PromptText::parse("")
                .expect_err("empty prompt must fail")
                .code(),
            "session_prompt_empty"
        );
        assert_eq!(
            PromptText::parse(&"x".repeat(16 * 1024 + 1))
                .expect_err("oversized prompt must fail")
                .code(),
            "session_prompt_too_large"
        );
    }

    #[test]
    fn prompt_text_rejects_terminal_control_characters() {
        assert_eq!(
            PromptText::parse("safe\u{1b}[31munsafe")
                .expect_err("terminal control must fail")
                .code(),
            "session_prompt_control_character"
        );
    }

    #[test]
    #[expect(
        clippy::shadow_unrelated,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn started_session_projection_rejects_an_inference_fact_without_panicking() {
        let start = decide_start_session(&[], binding())
            .expect("valid start should be modeled")
            .expect("new session");
        let (started, _streams) = start.into_event_and_consistency_streams();
        let prompt = PromptText::parse("durable").expect("valid prompt");
        let request =
            decide_request_inference(&[started], prompt, effect()).expect("valid request");
        let (requested, _streams) = request.into_event_and_consistency_streams();

        assert_eq!(
            project_started_session(&requested)
                .expect_err("not a start fact")
                .code(),
            "session_fact_not_started"
        );
    }

    #[test]
    #[expect(
        clippy::absolute_paths,
        clippy::shadow_unrelated,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn observing_the_requested_inference_emits_the_assistant_text() {
        let start = decide_start_session(&[], binding())
            .expect("valid start")
            .expect("new session");
        let (started, _streams) = start.into_event_and_consistency_streams();
        let request = decide_request_inference(
            core::slice::from_ref(&started),
            PromptText::parse("durable").expect("valid prompt"),
            effect(),
        )
        .expect("valid request");
        let (requested, _streams) = request.into_event_and_consistency_streams();
        let assistant = AssistantText::parse("hello from Tiber").expect("valid assistant");

        let observation = decide_observe_inference(&[started, requested], assistant.clone())
            .expect("observation should be modeled");
        let (event, streams) = observation.into_event_and_consistency_streams();

        assert_eq!(streams, [event.stream_id().clone()]);
        assert_eq!(
            event.fact(),
            &SessionFact::InferenceObserved {
                effect_id: parsed(EffectId::parse, "effect-1"),
                assistant,
            }
        );
    }

    #[test]
    #[expect(
        clippy::absolute_paths,
        clippy::shadow_unrelated,
        reason = "the public black-box scenario keeps the two successive publication boundaries explicit"
    )]
    fn interrupting_the_requested_inference_emits_a_sanitized_typed_observation() {
        let start = decide_start_session(&[], binding())
            .expect("valid start")
            .expect("new session");
        let (started, _streams) = start.into_event_and_consistency_streams();
        let request = decide_request_inference(
            core::slice::from_ref(&started),
            PromptText::parse("durable").expect("valid prompt"),
            effect(),
        )
        .expect("valid request");
        let (requested, _streams) = request.into_event_and_consistency_streams();
        let observation = EffectObservation::Failed {
            code: EffectFailureCode::parse("process_recovery_interrupted")
                .expect("stable failure code"),
            effect_id: parsed(EffectId::parse, "effect-1"),
            retryability: Retryability::NotRetryable,
        };

        let publication = decide_interrupt_inference(&[started, requested], observation.clone())
            .expect("interruption should be modeled");
        let (event, streams) = publication.into_event_and_consistency_streams();

        assert_eq!(streams, [event.stream_id().clone()]);
        assert_eq!(
            event.fact(),
            &SessionFact::InferenceInterrupted { observation }
        );
    }

    #[test]
    #[expect(
        clippy::cloned_ref_to_slice_refs,
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn observation_rejects_a_request_from_a_foreign_session_stream() {
        let start = decide_start_session(&[], binding())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let request = decide_request_inference(
            &[started.clone()],
            PromptText::parse("prompt").expect("prompt"),
            effect(),
        )
        .expect("request");
        let (requested, _) = request.into_event_and_consistency_streams();
        let encoded = serde_json::to_string(&requested).expect("serialize");
        let foreign: tiber_session_service::SessionEvent =
            serde_json::from_str(&encoded.replace("tiber:session:active", "tiber:session:foreign"))
                .expect("foreign event");

        let error = match decide_observe_inference(
            &[started, foreign],
            AssistantText::parse("answer").expect("assistant"),
        ) {
            Ok(_) => panic!("foreign request must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_inference_observation_failed");
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn request_rejects_retained_foreign_effect_even_after_its_observation() {
        let (started, requested, observed) = completed_turn();
        let encoded = serde_json::to_string(&requested).expect("serialize request");
        let foreign_request: tiber_session_service::SessionEvent =
            serde_json::from_str(&encoded.replace("assignment-1", "assignment-foreign"))
                .expect("foreign request");
        let error = match decide_request_inference(
            &[started, foreign_request, observed],
            PromptText::parse("next").expect("prompt"),
            effect_with_ids("effect-2", "session-1:turn-2"),
        ) {
            Ok(_) => panic!("foreign retained request must fence history"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_inference_request_failed");
    }

    #[test]
    #[expect(
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn observation_rejects_a_retained_mismatched_observation() {
        let (started, requested, observed) = completed_turn();
        let encoded = serde_json::to_string(&observed).expect("serialize observation");
        let mismatched: tiber_session_service::SessionEvent =
            serde_json::from_str(&encoded.replace("effect-1", "wrong-effect"))
                .expect("mismatched observation");
        let error = match decide_observe_inference(
            &[started, requested, mismatched],
            AssistantText::parse("answer").expect("assistant"),
        ) {
            Ok(_) => panic!("mismatched retained observation must fence history"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_inference_observation_failed");
    }

    #[test]
    #[expect(
        clippy::absolute_paths,
        clippy::manual_let_else,
        clippy::panic,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn observing_an_already_observed_effect_is_rejected() {
        let start = decide_start_session(&[], binding())
            .expect("valid start")
            .expect("new session");
        let (started, _) = start.into_event_and_consistency_streams();
        let request = decide_request_inference(
            core::slice::from_ref(&started),
            PromptText::parse("durable").expect("prompt"),
            effect(),
        )
        .expect("request");
        let (requested, _) = request.into_event_and_consistency_streams();
        let observation = decide_observe_inference(
            &[started.clone(), requested.clone()],
            AssistantText::parse("answer").expect("assistant"),
        )
        .expect("observation");
        let (observed, _) = observation.into_event_and_consistency_streams();

        let error = match decide_observe_inference(
            &[started, requested, observed],
            AssistantText::parse("answer").expect("assistant"),
        ) {
            Ok(_) => panic!("duplicate must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "session_modeled_inference_observation_failed");
    }

    #[test]
    #[expect(
        clippy::cloned_ref_to_slice_refs,
        reason = "the bounded integration scenario keeps its failure construction and fixture identities local"
    )]
    fn a_later_turn_cannot_reuse_an_idempotency_key() {
        let start = decide_start_session(&[], binding())
            .expect("valid start")
            .expect("new session");
        let (started, _) = start.into_event_and_consistency_streams();
        let request = decide_request_inference(
            &[started.clone()],
            PromptText::parse("one").expect("prompt"),
            effect(),
        )
        .expect("first request");
        let (requested, _) = request.into_event_and_consistency_streams();
        let observation = decide_observe_inference(
            &[started.clone(), requested.clone()],
            AssistantText::parse("done").expect("assistant"),
        )
        .expect("observation");
        let (observed, _) = observation.into_event_and_consistency_streams();
        let duplicate_key = effect_with_ids("effect-2", "session-1:turn-1");

        assert!(
            decide_request_inference(
                &[started, requested, observed],
                PromptText::parse("two").expect("prompt"),
                duplicate_key
            )
            .is_err()
        );
    }

    #[test]
    fn assistant_text_rejects_output_beyond_the_protocol_limit() {
        assert_eq!(
            AssistantText::parse(&"x".repeat(256 * 1024 + 1))
                .expect_err("oversized assistant must fail")
                .code(),
            "session_assistant_too_large"
        );
    }

    #[test]
    fn assistant_text_rejects_terminal_control_characters() {
        assert_eq!(
            AssistantText::parse("safe\u{1b}[31munsafe")
                .expect_err("terminal control must fail")
                .code(),
            "session_assistant_control_character"
        );
    }

    fn binding() -> SessionBinding {
        let task = TaskId::parse("session-fixture-resume-a-durable-coding-conversation")
            .expect("task identity should be valid");
        let session = parsed(SessionId::parse, "session-1");
        let workflow = parsed(WorkflowId::parse, "workflow-1");
        let assignment = parsed(AssignmentId::parse, "assignment-1");
        let effect = InferEffect::new(
            session,
            parsed(AgentId::parse, "agent-1"),
            workflow,
            assignment,
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

    fn binding_for_task(value: &str) -> SessionBinding {
        binding_for_task_with_effect(value, effect())
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "the fixture intentionally accepts an owned effect before rebuilding its task scope"
    )]
    fn binding_for_task_with_effect(value: &str, effect: InferEffect) -> SessionBinding {
        let task = TaskId::parse(value).expect("task identity");
        let rebound = InferEffect::new(
            effect.session_id().clone(),
            effect.agent_id().clone(),
            effect.workflow_id().clone(),
            effect.assignment_id().clone(),
            task_assignment_scope(&task).expect("task scope"),
            effect.assignment_epoch(),
            effect.attempt_number(),
            effect.context_receipt_id().clone(),
            effect.policy_decision_id().clone(),
            effect.effect_id().clone(),
            effect.idempotency_key().clone(),
            effect.deadline_milliseconds(),
        );
        SessionBinding::new(task, HarnessState::new(rebound))
    }

    #[expect(
        clippy::cloned_ref_to_slice_refs,
        reason = "the fixture retains an owned event while assembling the complete chronological history"
    )]
    fn completed_turn() -> (
        tiber_session_service::SessionEvent,
        tiber_session_service::SessionEvent,
        tiber_session_service::SessionEvent,
    ) {
        let start = decide_start_session(&[], binding())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let request = decide_request_inference(
            &[started.clone()],
            PromptText::parse("prompt").expect("prompt"),
            effect(),
        )
        .expect("request");
        let (requested, _) = request.into_event_and_consistency_streams();
        let observation = decide_observe_inference(
            &[started.clone(), requested.clone()],
            AssistantText::parse("answer").expect("assistant"),
        )
        .expect("observation");
        let (observed, _) = observation.into_event_and_consistency_streams();
        (started, requested, observed)
    }

    fn malformed_scope_start() -> tiber_session_service::SessionEvent {
        let valid = binding();
        let start = decide_start_session(&[], valid.clone())
            .expect("start")
            .expect("new");
        let (started, _) = start.into_event_and_consistency_streams();
        let canonical = task_assignment_scope(valid.task_id()).expect("scope");
        let encoded = serde_json::to_string(&started).expect("event serializes");
        serde_json::from_str(&encoded.replace(canonical.as_str(), "task:wrong"))
            .expect("structurally valid historical event")
    }

    fn effect_with_ids(effect_id: &str, idempotency: &str) -> InferEffect {
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
            parsed(EffectId::parse, effect_id),
            parsed(IdempotencyKey::parse, idempotency),
            base.deadline_milliseconds(),
        )
    }

    #[expect(
        clippy::absolute_paths,
        clippy::panic,
        reason = "the generic fixture keeps its display bound explicit and reports the exact invalid fixture"
    )]
    fn parsed<T, E: core::fmt::Display>(
        parser: impl FnOnce(&str) -> Result<T, E>,
        value: &str,
    ) -> T {
        parser(value).unwrap_or_else(|error| panic!("{value} should parse: {error}"))
    }
}
