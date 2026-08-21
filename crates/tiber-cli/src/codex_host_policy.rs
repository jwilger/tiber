//! Typed authority boundary for the in-process Codex runtime.

use alloc::sync::Arc;
use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use codex_app_server_client::{
    BuiltinPlanDecision, BuiltinPlanDecisionRequest, BuiltinSlashCommand,
    BuiltinSlashCommandRequest, HostPolicy, HostPolicyFuture, HostSlashCommand,
    InProcessClientExit, ServerNotificationDisposition, ServerRequestDisposition,
};
use codex_app_server_protocol::{ClientRequest, JSONRPCErrorError, ServerRequest};
use codex_core::effect_gate::{EffectDenied, EffectGate, EffectGateHandle, EffectRequest};
use eventcore_types::StreamId;
use tiber_session_service::IsolatedTurnKind;

use super::{
    ConversationProjection, NativeProcessCancellation, PendingRepositoryChange,
    ProcessCancellation, apply_process_restart_receipts, cancel_lost_repository_proposal,
    ensure_started_session, native_dynamic_tools, native_effect_failure,
    native_process_result_for_call, native_repository_read_result, native_repository_result,
    native_task_result, publish_accepted_plan_prompt_request, publish_approved_repository_change,
    publish_cancelled_plan, publish_denied_repository_change, publish_inference_interruption,
    publish_inference_observation, publish_isolated_prompt_request, publish_isolated_turn_answer,
    publish_isolated_turn_interruption, publish_plan_prompt_request, publish_prompt_request,
    recover_isolated_turns, resolve_interrupted_native_inference,
};

/// Complete set of model-callable tools owned by Tiber policy.
const TIBER_TOOL_NAMES: [&str; 4] = [
    "tiber_tasks",
    "tiber_repository_read",
    "tiber_repository_proposal",
    "tiber_effect",
];

/// Durable boundary that must admit a prompt before inference begins.
trait PromptAdmission: Send + Sync {
    /// Reconciles durable startup state before any conversational mode begins.
    fn recover(&self) -> Result<(), &'static str> {
        Ok(())
    }

    /// Durably admits one exact owner prompt.
    fn admit(&self, prompt: &str) -> Result<(), &'static str>;
}

/// Durable boundary that records a terminal Codex turn before presentation.
trait TurnCompletionPublication: Send + Sync {
    /// Publishes the exact terminal turn into Tiber history.
    fn publish(&self, turn: &codex_app_server_protocol::Turn) -> Result<(), &'static str>;
}

/// Production prompt admission backed by signed repository history.
struct DurablePromptAdmission {
    /// Once-only, retryable startup recovery coordinator.
    first_turn_recovery: FirstTurnRecovery,
    /// Ephemeral exact proposal awaiting its next owner decision.
    pending_repository: Arc<Mutex<Option<PendingRepositoryChange>>>,
    /// Repository whose signed Tiber history owns the session.
    repository: PathBuf,
}

/// Serializes first-turn recovery and commits completion only after success.
struct FirstTurnRecovery(Mutex<bool>);

impl FirstTurnRecovery {
    /// Creates a recovery coordinator whose work has not yet succeeded.
    fn pending() -> Self {
        Self(Mutex::new(true))
    }

    /// Runs startup recovery at most once successfully and retries failures.
    fn run(&self, recover: impl FnOnce() -> Result<(), &'static str>) -> Result<(), &'static str> {
        let mut pending = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*pending {
            return Ok(());
        }
        recover()?;
        *pending = false;
        Ok(())
    }
}

/// Production completion publisher backed by signed repository history.
struct DurableTurnCompletionPublication {
    /// Repository whose active session receives the observation.
    repository: PathBuf,
}

impl TurnCompletionPublication for DurableTurnCompletionPublication {
    fn publish(&self, turn: &codex_app_server_protocol::Turn) -> Result<(), &'static str> {
        publish_turn_completion(&self.repository, turn)
    }
}

#[cfg(test)]
struct TestTurnCompletionPublication;

#[cfg(test)]
impl TurnCompletionPublication for TestTurnCompletionPublication {
    fn publish(&self, _turn: &codex_app_server_protocol::Turn) -> Result<(), &'static str> {
        Ok(())
    }
}

impl PromptAdmission for DurablePromptAdmission {
    fn recover(&self) -> Result<(), &'static str> {
        let (_initial_binding, session_events) =
            ensure_started_session(&self.repository).map_err(|error| error.code())?;
        self.first_turn_recovery.run(|| {
            let mut projection = ConversationProjection::new();
            apply_process_restart_receipts(&self.repository, &session_events, &mut projection)
                .map_err(|error| error.code())?;
            resolve_interrupted_native_inference(&self.repository).map_err(|error| error.code())?;
            let (_recovered_binding, recovered_events) =
                ensure_started_session(&self.repository).map_err(|error| error.code())?;
            recover_isolated_turns(&self.repository, &recovered_events)
                .map_err(|error| error.code())?;
            cancel_lost_repository_proposal(&self.repository, &recovered_events)
                .map_err(|error| error.code())?;
            Ok(())
        })
    }

    fn admit(&self, prompt: &str) -> Result<(), &'static str> {
        self.recover()?;
        let pending = self
            .pending_repository
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match (prompt, pending) {
            ("approve", Some(pending)) => {
                publish_pending_repository_decision(
                    &self.pending_repository,
                    pending,
                    |pending| {
                        publish_approved_repository_change(&self.repository, pending)
                            .map(|result| match result {
                                super::RepositoryApprovalResult::Applied => None,
                                super::RepositoryApprovalResult::Reproposed {
                                    pending: reproposed,
                                } => Some(reproposed),
                            })
                            .map_err(|error| error.code())
                    },
                )?;
            }
            ("deny", Some(pending)) => {
                publish_pending_repository_decision(
                    &self.pending_repository,
                    pending,
                    |pending| {
                        publish_denied_repository_change(&self.repository, pending)
                            .map(|()| None)
                            .map_err(|error| error.code())
                    },
                )?;
            }
            (_, Some(pending)) => {
                *self
                    .pending_repository
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pending);
                return Err("repository_owner_decision_required");
            }
            (_, None) => {}
        }
        publish_prompt_request(&self.repository, prompt).map_err(|error| error.code())
    }
}

/// Tiber-owned policy attached to every embedded Codex client.
#[expect(
    clippy::pub_with_shorthand,
    reason = "the policy is intentionally visible only to the crate's native TUI entrypoint"
)]
pub(crate) struct TiberHostPolicy {
    /// Durable prompt admission boundary.
    admission: Arc<dyn PromptAdmission>,
    /// Durable terminal-turn publication boundary.
    completion: Arc<dyn TurnCompletionPublication>,
    /// Exact in-memory owner decision awaiting the next turn.
    pending_repository: Arc<Mutex<Option<PendingRepositoryChange>>>,
    /// Stable cancellation handshake for the primary parent conversation.
    process_cancellation: NativeProcessCancellation,
    /// Repository whose signed state authorizes this host.
    repository: PathBuf,
    /// Exact built-in conversational mode awaiting its native turn.
    slash_admission: Arc<Mutex<Option<SlashAdmission>>>,
    /// Independently correlated parent and isolated child turns by Codex thread.
    turns: Arc<Mutex<HashMap<String, AdmittedTurn>>>,
}

impl TiberHostPolicy {
    /// Creates a policy backed by the durable Tiber state in `repository`.
    pub(crate) fn new(repository: PathBuf) -> Self {
        let pending_repository = Arc::new(Mutex::new(None));
        Self {
            admission: Arc::new(DurablePromptAdmission {
                first_turn_recovery: FirstTurnRecovery::pending(),
                pending_repository: Arc::clone(&pending_repository),
                repository: repository.clone(),
            }),
            completion: Arc::new(DurableTurnCompletionPublication {
                repository: repository.clone(),
            }),
            pending_repository,
            process_cancellation: NativeProcessCancellation::default(),
            repository,
            slash_admission: Arc::new(Mutex::new(None)),
            turns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    fn with_admission<Admission>(admission: Arc<Admission>) -> Self
    where
        Admission: PromptAdmission + 'static,
    {
        Self::with_boundaries(admission, Arc::new(TestTurnCompletionPublication))
    }

    #[cfg(test)]
    fn with_boundaries<Admission, Completion>(
        admission: Arc<Admission>,
        completion: Arc<Completion>,
    ) -> Self
    where
        Admission: PromptAdmission + 'static,
        Completion: TurnCompletionPublication + 'static,
    {
        Self {
            admission,
            completion,
            pending_repository: Arc::new(Mutex::new(None)),
            process_cancellation: NativeProcessCancellation::default(),
            repository: PathBuf::from("/workspace"),
            slash_admission: Arc::new(Mutex::new(None)),
            turns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Cancels every admitted native turn in stable thread-id order.
    fn cancel_admitted_turns(&self) {
        let mut cancellations = self
            .turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(thread_id, turn)| (thread_id.clone(), turn.cancellation.clone()))
            .collect::<Vec<_>>();
        cancellations.sort_by(|left, right| left.0.cmp(&right.0));
        for (_thread_id, cancellation) in cancellations {
            cancellation.cancel();
        }
    }
}

impl HostPolicy for TiberHostPolicy {
    fn admit_builtin_slash_command<'policy>(
        &'policy self,
        request: &'policy BuiltinSlashCommandRequest,
    ) -> HostPolicyFuture<'policy, Result<(), String>> {
        Box::pin(async move {
            let parent_thread_id = request
                .thread_id
                .clone()
                .ok_or_else(|| "tiber_builtin_slash_thread_required".to_owned())?;
            let admission = match request.command {
                BuiltinSlashCommand::Plan => SlashAdmission::Plan { parent_thread_id },
                BuiltinSlashCommand::Side => SlashAdmission::Isolated {
                    kind: IsolatedTurnKind::Side,
                    parent_thread_id,
                },
                BuiltinSlashCommand::Btw => SlashAdmission::Isolated {
                    kind: IsolatedTurnKind::Btw,
                    parent_thread_id,
                },
            };
            let mut pending = self
                .slash_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.is_some() {
                return Err("tiber_builtin_slash_already_pending".to_owned());
            }
            *pending = Some(admission);
            Ok(())
        })
    }

    fn observe_builtin_slash_command(&self, _request: &BuiltinSlashCommandRequest) {}

    fn observe_builtin_plan_decision(&self, _request: &BuiltinPlanDecisionRequest) {}

    fn admit_builtin_plan_decision<'policy>(
        &'policy self,
        request: &'policy BuiltinPlanDecisionRequest,
    ) -> HostPolicyFuture<'policy, Result<(), String>> {
        let repository = self.repository.clone();
        let request = request.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || match request.decision {
                BuiltinPlanDecision::Accept | BuiltinPlanDecision::AcceptClearContext => {
                    let prompt = request
                        .implementation_prompt
                        .as_deref()
                        .ok_or_else(|| "tiber_plan_accept_prompt_missing".to_owned())?;
                    publish_accepted_plan_prompt_request(&repository, prompt)
                        .map_err(|error| error.code().to_owned())
                }
                BuiltinPlanDecision::Cancel => {
                    if request.implementation_prompt.is_some() {
                        return Err("tiber_plan_cancel_prompt_unexpected".to_owned());
                    }
                    publish_cancelled_plan(&repository).map_err(|error| error.code().to_owned())
                }
            })
            .await
            .map_err(|_error| "tiber_plan_decision_stopped".to_owned())?
        })
    }

    #[expect(
        clippy::pattern_type_mismatch,
        clippy::too_many_lines,
        clippy::wildcard_enum_match_arm,
        reason = "the auditable typed boundary keeps its complete fail-closed request allowlist and durable turn transaction together"
    )]
    fn admit_client_request(
        &self,
        mut request: ClientRequest,
    ) -> HostPolicyFuture<'_, Result<ClientRequest, JSONRPCErrorError>> {
        Box::pin(async move {
            let method = request.method_name();
            match &mut request {
                ClientRequest::ThreadStart { params, .. } => {
                    params.dynamic_tools = Some(dynamic_tool_specs()?);
                    restrict_thread(
                        &mut params.sandbox,
                        &mut params.config,
                        &mut params.base_instructions,
                    );
                }
                ClientRequest::ThreadResume { params, .. } => restrict_thread(
                    &mut params.sandbox,
                    &mut params.config,
                    &mut params.base_instructions,
                ),
                ClientRequest::ThreadFork { params, .. } => {
                    let pending = self
                        .slash_admission
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(SlashAdmission::Isolated {
                        parent_thread_id, ..
                    }) = pending.as_ref()
                        && parent_thread_id != &params.thread_id
                    {
                        return Err(host_error("tiber_isolated_fork_parent_unauthorized"));
                    }
                    drop(pending);
                    restrict_thread(
                        &mut params.sandbox,
                        &mut params.config,
                        &mut params.base_instructions,
                    );
                }
                ClientRequest::TurnStart { .. } => {}
                ClientRequest::TurnInterrupt { params, .. } => {
                    let mut turns = self
                        .turns
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let Some(active) = turns.get_mut(&params.thread_id) else {
                        return Err(host_error("tiber_turn_interrupt_unauthorized"));
                    };
                    let bound_pending_turn = match active.turn_id.as_deref() {
                        Some(turn_id) if turn_id != params.turn_id => {
                            return Err(host_error("tiber_turn_interrupt_unauthorized"));
                        }
                        None => {
                            active.turn_id = Some(params.turn_id.clone());
                            true
                        }
                        Some(_) => false,
                    };
                    let cancellation = active.cancellation.clone();
                    drop(turns);
                    if bound_pending_turn {
                        cancellation.cancel();
                    }
                    return Ok(request);
                }
                _ if harmless_client_request(method) => return Ok(request),
                _ => return Err(host_error("tiber_client_request_unauthorized")),
            }
            let ClientRequest::TurnStart { params, .. } = &mut request else {
                return Ok(request);
            };
            let prompt = match params.input.as_slice() {
                [codex_app_server_protocol::UserInput::Text { text, .. }] => text.clone(),
                _ => return Err(host_error("tiber_prompt_input_unsupported")),
            };
            let turn_thread_id = params.thread_id.clone();
            let isolated_pending = matches!(
                self.slash_admission
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref(),
                Some(SlashAdmission::Isolated { .. })
            );
            let turn_cancellation = if isolated_pending {
                NativeProcessCancellation::default()
            } else {
                self.process_cancellation.clone()
            };
            {
                let mut turns = self
                    .turns
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if turns.contains_key(&params.thread_id) {
                    return Err(host_error("tiber_turn_already_active"));
                }
                turns.insert(
                    params.thread_id.clone(),
                    AdmittedTurn {
                        authority: TurnAuthority::Ordinary,
                        cancellation: turn_cancellation,
                        turn_id: None,
                    },
                );
            };
            let slash_admission = self
                .slash_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let slash_retry = slash_admission.clone();
            let prompt_admission = Arc::clone(&self.admission);
            let repository = self.repository.clone();
            let prompt_for_admission = prompt.clone();
            let admitted_thread_id = turn_thread_id.clone();
            let admission_result = match tokio::task::spawn_blocking(move || {
                prompt_admission.recover()?;
                match slash_admission {
                    Some(SlashAdmission::Plan { parent_thread_id })
                        if parent_thread_id == admitted_thread_id =>
                    {
                        publish_plan_prompt_request(&repository, &prompt_for_admission)
                            .map(|()| TurnAuthority::Plan)
                            .map_err(|error| error.code())
                    }
                    Some(SlashAdmission::Isolated { kind, .. }) => {
                        publish_isolated_prompt_request(&repository, kind, &prompt_for_admission)
                            .map(TurnAuthority::Isolated)
                            .map_err(|error| error.code())
                    }
                    Some(SlashAdmission::Plan { .. }) => Err("tiber_plan_thread_unauthorized"),
                    None => prompt_admission
                        .admit(&prompt_for_admission)
                        .map(|()| TurnAuthority::Ordinary),
                }
            })
            .await
            {
                Ok(result) => result.map_err(host_error),
                Err(_error) => Err(host_error("tiber_prompt_admission_stopped")),
            };
            let authority = match admission_result {
                Ok(authority) => authority,
                Err(error) => {
                    if let Some(retry) = slash_retry {
                        let mut pending = self
                            .slash_admission
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if pending.is_none() {
                            *pending = Some(retry);
                        }
                    }
                    self.turns
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&turn_thread_id);
                    return Err(error);
                }
            };
            let mut turns = self
                .turns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(admitted) = turns.get_mut(&turn_thread_id) else {
                return Err(host_error("tiber_admitted_turn_lost"));
            };
            admitted.authority = authority;
            drop(turns);
            params.sandbox_policy = Some(codex_app_server_protocol::SandboxPolicy::ReadOnly {
                network_access: false,
            });
            Ok(request)
        })
    }

    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the linked protocol request is borrowed so interception can preserve its exact typed identity"
    )]
    fn intercept_server_request<'policy>(
        &'policy self,
        request: &'policy ServerRequest,
    ) -> HostPolicyFuture<'policy, ServerRequestDisposition> {
        Box::pin(async move {
            let ServerRequest::DynamicToolCall { params, .. } = request else {
                return ServerRequestDisposition::Reject(host_error(
                    "tiber_server_request_unauthorized",
                ));
            };
            if !TIBER_TOOL_NAMES.contains(&params.tool.as_str()) {
                return ServerRequestDisposition::Resolve(native_effect_failure(
                    "tiber_tool_unauthorized",
                    false,
                ));
            }
            let cancellation = {
                let turns = self
                    .turns
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(active) = turns
                    .get(&params.thread_id)
                    .filter(|active| active.turn_id.as_deref() == Some(params.turn_id.as_str()))
                else {
                    return ServerRequestDisposition::Reject(host_error(
                        "tiber_dynamic_tool_turn_unauthorized",
                    ));
                };
                if !matches!(active.authority, TurnAuthority::Ordinary) {
                    return ServerRequestDisposition::Reject(host_error(
                        "tiber_dynamic_tool_mode_unauthorized",
                    ));
                }
                active.cancellation.clone()
            };
            let repository = self.repository.clone();
            let tool = params.tool.clone();
            let arguments = params.arguments.clone();
            let call_id = params.call_id.clone();
            let repository_proposal_pending = self
                .pending_repository
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some();
            let tool_completion = tokio::task::spawn_blocking(move || {
                dispatch_dynamic_tool(
                    &repository,
                    &cancellation,
                    &tool,
                    &arguments,
                    &call_id,
                    repository_proposal_pending,
                )
            })
            .await
            .unwrap_or_else(|_error| DynamicToolCompletion {
                pending_repository: None,
                response: native_effect_failure("tiber_tool_dispatch_stopped", false),
            });
            if let Some(pending) = tool_completion.pending_repository {
                *self
                    .pending_repository
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pending);
            }
            ServerRequestDisposition::Resolve(tool_completion.response)
        })
    }

    fn effect_gate(&self) -> Option<EffectGateHandle> {
        Some(EffectGateHandle::new(TiberEffectGate))
    }

    fn observe_server_notification(
        &self,
        _notification: &codex_app_server_protocol::ServerNotification,
    ) {
    }

    #[expect(
        clippy::pattern_type_mismatch,
        clippy::wildcard_enum_match_arm,
        reason = "only turn lifecycle notifications require admission; all other typed notifications remain observational"
    )]
    fn admit_server_notification<'policy>(
        &'policy self,
        notification: &'policy codex_app_server_protocol::ServerNotification,
    ) -> HostPolicyFuture<'policy, ServerNotificationDisposition> {
        Box::pin(async move {
            match notification {
                codex_app_server_protocol::ServerNotification::TurnStarted(params) => {
                    let mut turns = self
                        .turns
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let Some(admitted) = turns.get_mut(&params.thread_id) else {
                        return ServerNotificationDisposition::Suppress;
                    };
                    match admitted.turn_id.as_deref() {
                        Some(turn_id) if turn_id != params.turn.id => {
                            return ServerNotificationDisposition::Suppress;
                        }
                        None => admitted.turn_id = Some(params.turn.id.clone()),
                        Some(_) => {}
                    }
                    ServerNotificationDisposition::Forward
                }
                codex_app_server_protocol::ServerNotification::TurnCompleted(params) => {
                    let expected = self
                        .turns
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get(&params.thread_id)
                        .cloned();
                    let Some(expected) = expected else {
                        return ServerNotificationDisposition::Suppress;
                    };
                    if expected.turn_id.as_deref() != Some(params.turn.id.as_str()) {
                        return ServerNotificationDisposition::Suppress;
                    }
                    expected.cancellation.clear();
                    let completion_publication = Arc::clone(&self.completion);
                    let authority = expected.authority.clone();
                    let repository = self.repository.clone();
                    let completed_turn = params.turn.clone();
                    let published = tokio::task::spawn_blocking(move || match authority {
                        TurnAuthority::Ordinary => completion_publication.publish(&completed_turn),
                        TurnAuthority::Plan => match &completed_turn.status {
                            codex_app_server_protocol::TurnStatus::Completed => {
                                let assistant = terminal_assistant_text(&completed_turn)?;
                                publish_inference_observation(&repository, assistant)
                                    .map_err(|error| error.code())
                            }
                            _ => publish_inference_interruption(
                                &repository,
                                "native_codex_plan_interrupted",
                            )
                            .map_err(|error| error.code()),
                        },
                        TurnAuthority::Isolated(stream) => match &completed_turn.status {
                            codex_app_server_protocol::TurnStatus::Completed => {
                                let assistant = terminal_assistant_text(&completed_turn)?;
                                publish_isolated_turn_answer(&repository, &stream, assistant)
                                    .map_err(|error| error.code())
                            }
                            _ => publish_isolated_turn_interruption(&repository, &stream)
                                .map_err(|error| error.code()),
                        },
                    })
                    .await
                    .is_ok_and(|result| result.is_ok());
                    if !published {
                        return ServerNotificationDisposition::Suppress;
                    }
                    self.turns
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&params.thread_id);
                    ServerNotificationDisposition::Forward
                }
                _ => ServerNotificationDisposition::Forward,
            }
        })
    }

    fn observe_cancellation_requested(&self, thread_id: &str, turn_id: &str) {
        let cancellation = self
            .turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(thread_id)
            .filter(|turn| turn.turn_id.as_deref() == Some(turn_id))
            .map(|turn| turn.cancellation.clone());
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
        }
    }

    fn observe_client_exit(&self, _exit: InProcessClientExit) {
        self.process_cancellation.cancel();
        self.cancel_admitted_turns();
    }

    fn observe_shutdown(&self) {
        self.process_cancellation.cancel();
        self.cancel_admitted_turns();
    }

    fn native_slash_commands(&self) -> Vec<HostSlashCommand> {
        Vec::new()
    }

    fn execute_native_slash_command<'policy>(
        &'policy self,
        name: &'policy str,
        _args: &'policy str,
    ) -> HostPolicyFuture<'policy, Result<Option<String>, String>> {
        Box::pin(async move { Err(format!("unknown native slash command: {name}")) })
    }
}

#[derive(Clone)]
/// One admitted native turn and its eventual Codex runtime identity.
struct AdmittedTurn {
    /// Exact configured-process cancellation handshake for this turn only.
    cancellation: NativeProcessCancellation,
    /// Runtime turn identity, absent until `TurnStarted` arrives.
    turn_id: Option<String>,
    /// Durable conversational authority selected before inference.
    authority: TurnAuthority,
}

#[derive(Clone)]
/// Durable authority attached to one exact admitted native turn.
enum TurnAuthority {
    /// Ordinary coding authority with access to admitted Tiber tools.
    Ordinary,
    /// Planning-only authority with no mutating Tiber tool access.
    Plan,
    /// One independently correlated side or BTW child stream.
    Isolated(StreamId),
}

#[derive(Clone)]
/// Exact built-in slash intent retained until native turn admission succeeds.
enum SlashAdmission {
    /// One planning request bound to its originating thread.
    Plan {
        /// Native thread that issued the Plan request.
        parent_thread_id: String,
    },
    /// One isolated child request bound to its parent and semantic kind.
    Isolated {
        /// Side or BTW identity preserved across retry.
        kind: IsolatedTurnKind,
        /// Native parent thread whose authority the child inherits.
        parent_thread_id: String,
    },
}

/// Extracts the bounded terminal assistant or plan text from a completed turn.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "terminal publication selects the two textual item variants from Codex's non-exhaustive protocol vocabulary"
)]
fn terminal_assistant_text(turn: &codex_app_server_protocol::Turn) -> Result<&str, &'static str> {
    turn.items
        .iter()
        .rev()
        .find_map(|item| match item {
            codex_app_server_protocol::ThreadItem::AgentMessage { text, .. }
            | codex_app_server_protocol::ThreadItem::Plan { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .ok_or("tiber_turn_missing_assistant")
}

/// Publishes one terminal Codex turn through the durable Tiber workflow.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "completion extracts only the linked terminal status and assistant message while rejecting no future effect"
)]
fn publish_turn_completion(
    repository: &std::path::Path,
    turn: &codex_app_server_protocol::Turn,
) -> Result<(), &'static str> {
    match &turn.status {
        codex_app_server_protocol::TurnStatus::Completed => {
            let assistant = turn.items.iter().rev().find_map(|item| match item {
                codex_app_server_protocol::ThreadItem::AgentMessage { text, .. } => Some(text),
                _ => None,
            });
            publish_inference_observation(
                repository,
                assistant.ok_or("tiber_turn_completion_missing_assistant")?,
            )
            .map_err(|error| error.code())
        }
        codex_app_server_protocol::TurnStatus::Failed => {
            publish_inference_interruption(repository, "native_codex_turn_failed")
                .map_err(|error| error.code())
        }
        codex_app_server_protocol::TurnStatus::Interrupted => {
            publish_inference_interruption(repository, "native_codex_turn_interrupted")
                .map_err(|error| error.code())
        }
        codex_app_server_protocol::TurnStatus::InProgress => {
            Err("tiber_turn_completion_in_progress")
        }
    }
}

/// Publishes an owner repository decision and restores its exact value on failure.
fn publish_pending_repository_decision<Pending, Publish>(
    pending_slot: &Mutex<Option<Pending>>,
    pending: Pending,
    publish: Publish,
) -> Result<(), &'static str>
where
    Pending: Clone,
    Publish: FnOnce(Pending) -> Result<Option<Pending>, &'static str>,
{
    let retry = pending.clone();
    match publish(pending) {
        Ok(next) => {
            *pending_slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
            Ok(())
        }
        Err(error) => {
            *pending_slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(retry);
            Err(error)
        }
    }
}

/// Builds a stable JSON-RPC host-policy rejection.
fn host_error(message: &'static str) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32000,
        message: message.to_owned(),
        data: None,
    }
}

/// Rewrites a thread to Tiber's read-only, no-network execution profile.
fn restrict_thread(
    sandbox: &mut Option<codex_app_server_protocol::SandboxMode>,
    config: &mut Option<std::collections::HashMap<String, serde_json::Value>>,
    base_instructions: &mut Option<String>,
) {
    *sandbox = Some(codex_app_server_protocol::SandboxMode::ReadOnly);
    *config = Some(std::collections::HashMap::default());
    *base_instructions = None;
}

/// Returns whether a client request is a non-effecting navigation or auth operation.
fn harmless_client_request(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "account/login/cancel"
            | "account/login/start"
            | "account/logout"
            | "account/read"
            | "account/rateLimits/read"
            | "account/usage/read"
            | "app/list"
            | "collaborationMode/list"
            | "config/read"
            | "configRequirements/read"
            | "mcpServer/status/list"
            | "model/list"
            | "skills/list"
            | "thread/list"
            | "thread/read"
            | "thread/loaded/list"
            | "thread/items/list"
            | "thread/turns/list"
    )
}

/// Parses the application-owned dynamic tools into Codex's linked protocol type.
fn dynamic_tool_specs() -> Result<Vec<codex_app_server_protocol::DynamicToolSpec>, JSONRPCErrorError>
{
    native_dynamic_tools()
        .into_iter()
        .map(|declaration| {
            serde_json::from_value(declaration)
                .map_err(|_error| host_error("tiber_dynamic_tool_schema_invalid"))
        })
        .collect()
}

/// Dispatches one correlated dynamic tool request through Tiber authority.
fn dispatch_dynamic_tool(
    repository: &std::path::Path,
    cancellation: &NativeProcessCancellation,
    tool: &str,
    arguments: &serde_json::Value,
    call_id: &str,
    repository_proposal_pending: bool,
) -> DynamicToolCompletion {
    let params = serde_json::json!({ "arguments": arguments, "tool": tool });
    let (response, pending_repository) = match tool {
        "tiber_tasks" => (native_task_result(repository, &params), None),
        "tiber_repository_read" => (native_repository_read_result(repository, &params), None),
        "tiber_repository_proposal" if repository_proposal_pending => (
            native_effect_failure("repository_proposal_pending", false),
            None,
        ),
        "tiber_repository_proposal" => native_repository_result(repository, &params),
        "tiber_effect" => {
            let process_cancellation = ProcessCancellation::default();
            cancellation.install(process_cancellation.clone());
            let response =
                native_process_result_for_call(repository, &params, call_id, &process_cancellation);
            cancellation.clear();
            (response, None)
        }
        _ => (
            native_effect_failure("tiber_tool_unauthorized", false),
            None,
        ),
    };
    DynamicToolCompletion {
        pending_repository,
        response,
    }
}

/// Model-facing response plus any exact repository proposal awaiting its owner.
struct DynamicToolCompletion {
    /// Exact pending proposal created by this tool call, when present.
    pending_repository: Option<PendingRepositoryChange>,
    /// Linked Codex dynamic-tool response payload.
    response: serde_json::Value,
}

/// Lower-level Codex effect gate that leaves all mutation to Tiber tools.
struct TiberEffectGate;

impl EffectGate for TiberEffectGate {
    fn check(&self, effect: &EffectRequest) -> Result<(), EffectDenied> {
        let allowed = matches!(
            effect,
            EffectRequest::Tool { name } if TIBER_TOOL_NAMES.contains(&name.as_str())
        );
        if allowed {
            return Ok(());
        }
        Err(EffectDenied::new(
            "tiber_effect_requires_authority",
            "Codex effects require Tiber authority",
            effect.clone(),
        ))
    }
}

#[cfg(test)]
#[expect(
    clippy::assertions_on_result_states,
    clippy::default_numeric_fallback,
    clippy::panic,
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "behavior-focused protocol fixtures use compact JSON literals and fail loudly at impossible branches"
)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use codex_app_server_client::HostPolicy as _;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    struct RecordingAdmission(AtomicBool);

    struct RecordingCompletion {
        called: AtomicBool,
        fails: bool,
    }

    struct FailRecoveryOnce {
        inner: DurablePromptAdmission,
        failed: AtomicBool,
    }

    impl PromptAdmission for FailRecoveryOnce {
        fn recover(&self) -> Result<(), &'static str> {
            if !self.failed.swap(true, Ordering::AcqRel) {
                return Err("test_recovery_failed");
            }
            self.inner.recover()
        }

        fn admit(&self, prompt: &str) -> Result<(), &'static str> {
            self.inner.admit(prompt)
        }
    }

    struct DurableFixture {
        _directory: TempDir,
        repository: PathBuf,
    }

    impl DurableFixture {
        fn new() -> Self {
            std::thread::spawn(Self::new_outside_async_runtime)
                .join()
                .expect("fixture construction should join")
        }

        fn new_outside_async_runtime() -> Self {
            let directory = TempDir::new().expect("fixture directory should be created");
            let repository = directory.path().join("repository");
            let signing_key = directory.path().join("signing-key");
            let allowed_signers = directory.path().join("allowed-signers");
            git(directory.path(), ["init", path_text(&repository)]);
            git(
                &repository,
                ["config", "user.name", "Tiber Host Policy Fixture"],
            );
            git(
                &repository,
                ["config", "user.email", "host-policy@example.invalid"],
            );
            assert!(
                Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                    .arg(&signing_key)
                    .status()
                    .expect("fixture signing key generation should start")
                    .success()
            );
            let public_key = fs::read_to_string(signing_key.with_extension("pub"))
                .expect("public key should be readable");
            fs::write(
                &allowed_signers,
                format!("host-policy@example.invalid {}", public_key.trim()),
            )
            .expect("allowed signer should be written");
            git(&repository, ["config", "gpg.format", "ssh"]);
            git(&repository, ["config", "commit.gpgsign", "true"]);
            git(
                &repository,
                ["config", "user.signingkey", path_text(&signing_key)],
            );
            git(
                &repository,
                [
                    "config",
                    "gpg.ssh.allowedSignersFile",
                    path_text(&allowed_signers),
                ],
            );
            let store = eventcore_fs::FileEventStore::open(repository.join("eventstore"))
                .expect("event store should initialize");
            drop(store);
            fs::write(repository.join("eventstore/events/.keep"), "")
                .expect("history marker should be written");
            git(&repository, ["add", "."]);
            git(&repository, ["commit", "-m", "initialize authority"]);
            let revision = git_output(&repository, ["rev-parse", "HEAD"]);
            git(
                &repository,
                ["update-ref", "refs/heads/tiber", revision.trim()],
            );
            let create = crate::tasks::parse(
                [
                    "create",
                    "--id",
                    "20260820-host-policy",
                    "Host policy fixture",
                ]
                .into_iter()
                .map(OsString::from),
            )
            .expect("create command should parse");
            let created =
                crate::tasks::run(&repository, create).expect("fixture task should be created");
            let created_id = created
                .strip_prefix("created ")
                .and_then(|text| text.split_once(" at ").map(|(id, _revision)| id))
                .expect("creation result should contain the durable task id");
            let task_id =
                crate::TaskId::parse(created_id).expect("fixture task id should be valid");
            crate::tasks::start_task_by_id(&repository, &task_id)
                .expect("fixture task should start");
            Self {
                _directory: directory,
                repository,
            }
        }
    }

    fn git<const N: usize>(repository: &Path, arguments: [&str; N]) {
        assert!(
            Command::new("git")
                .args(arguments)
                .current_dir(repository)
                .status()
                .expect("git should start")
                .success()
        );
    }

    fn git_output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .output()
            .expect("git should start");
        assert!(output.status.success());
        String::from_utf8(output.stdout).expect("git output should be UTF-8")
    }

    fn path_text(path: &Path) -> &str {
        path.to_str().expect("fixture paths should be UTF-8")
    }

    impl PromptAdmission for RecordingAdmission {
        fn recover(&self) -> Result<(), &'static str> {
            Ok(())
        }

        fn admit(&self, _prompt: &str) -> Result<(), &'static str> {
            self.0.store(true, Ordering::Release);
            Ok(())
        }
    }

    impl TurnCompletionPublication for RecordingCompletion {
        fn publish(&self, _turn: &codex_app_server_protocol::Turn) -> Result<(), &'static str> {
            self.called.store(true, Ordering::Release);
            if self.fails {
                Err("test_completion_failed")
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn failed_first_turn_recovery_retries_and_success_is_synchronized() {
        let retry_recovery = FirstTurnRecovery::pending();
        let attempts = AtomicUsize::new(0);
        assert_eq!(
            retry_recovery.run(|| {
                attempts.fetch_add(1, Ordering::AcqRel);
                Err("recovery_failed")
            }),
            Err("recovery_failed")
        );
        retry_recovery
            .run(|| {
                attempts.fetch_add(1, Ordering::AcqRel);
                Ok(())
            })
            .expect("failed recovery must remain retryable");
        retry_recovery
            .run(|| panic!("completed recovery must not run twice"))
            .expect("completed recovery should remain committed");
        assert_eq!(attempts.load(Ordering::Acquire), 2);

        let synchronized_recovery = Arc::new(FirstTurnRecovery::pending());
        let concurrent_attempts = Arc::new(AtomicUsize::new(0));
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let first_recovery = Arc::clone(&synchronized_recovery);
        let first_attempts = Arc::clone(&concurrent_attempts);
        let first = std::thread::spawn(move || {
            first_recovery.run(|| {
                first_attempts.fetch_add(1, Ordering::AcqRel);
                started_sender.send(()).expect("start signal should send");
                release_receiver
                    .recv()
                    .expect("release signal should arrive");
                Ok(())
            })
        });
        started_receiver.recv().expect("recovery should start");
        let second_recovery = Arc::clone(&synchronized_recovery);
        let second_attempts = Arc::clone(&concurrent_attempts);
        let second = std::thread::spawn(move || {
            second_recovery.run(|| {
                second_attempts.fetch_add(1, Ordering::AcqRel);
                Ok(())
            })
        });
        assert_eq!(concurrent_attempts.load(Ordering::Acquire), 1);
        release_sender.send(()).expect("recovery should release");
        first
            .join()
            .expect("first recovery should join")
            .expect("first recovery should pass");
        second
            .join()
            .expect("second recovery should join")
            .expect("second recovery should pass");
        assert_eq!(concurrent_attempts.load(Ordering::Acquire), 1);
    }

    #[test]
    fn interrupt_before_process_install_is_applied_during_install() {
        let cancellation = NativeProcessCancellation::default();
        cancellation.cancel();
        assert!(cancellation.cancel_is_latched());

        cancellation.install(ProcessCancellation::default());

        assert!(cancellation.latched_cancel_was_applied());
        cancellation.clear();
        assert!(!cancellation.cancel_is_latched());
    }

    #[test]
    fn backend_exit_and_shutdown_latch_process_cleanup() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        policy.observe_client_exit(InProcessClientExit::RuntimeExited);
        assert!(policy.process_cancellation.cancel_is_latched());
        policy.process_cancellation.clear();
        policy.observe_shutdown();
        assert!(policy.process_cancellation.cancel_is_latched());
    }

    #[test]
    fn repository_decisions_consume_success_and_restore_failure_exactly() {
        let pending = Mutex::new(None);

        *pending.lock().expect("pending decision should lock") = Some("approve-v1".to_owned());
        assert_eq!(
            publish_pending_repository_decision(&pending, "approve-v1".to_owned(), |_proposal| {
                Ok::<Option<String>, &str>(None)
            }),
            Ok(())
        );
        assert!(
            pending
                .lock()
                .expect("approved decision should lock")
                .is_none()
        );

        *pending.lock().expect("pending decision should lock") = Some("deny-v1".to_owned());
        assert_eq!(
            publish_pending_repository_decision(&pending, "deny-v1".to_owned(), |_proposal| {
                Ok::<Option<String>, &str>(None)
            }),
            Ok(())
        );
        assert!(
            pending
                .lock()
                .expect("denied decision should lock")
                .is_none()
        );

        assert_eq!(
            publish_pending_repository_decision(&pending, "proposal-v1".to_owned(), |_proposal| {
                Err::<Option<String>, _>("publication_failed")
            }),
            Err("publication_failed")
        );
        assert_eq!(
            pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref(),
            Some("proposal-v1")
        );
    }

    #[tokio::test]
    async fn native_account_lifecycle_uses_the_linked_codex_api() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        let requests = [
            json!({
                "method":"account/login/start", "id":1,
                "params":{"type":"chatgpt", "appBrand":null}
            }),
            json!({
                "method":"account/login/cancel", "id":2,
                "params":{"loginId":"login-1"}
            }),
            json!({"method":"account/logout", "id":3}),
        ];

        for (request, expected_method) in requests.into_iter().zip([
            "account/login/start",
            "account/login/cancel",
            "account/logout",
        ]) {
            let request: ClientRequest =
                serde_json::from_value(request).expect("account lifecycle fixture should parse");
            let admitted = policy
                .admit_client_request(request)
                .await
                .expect("linked account lifecycle request should remain available");
            assert_eq!(admitted.method_name(), expected_method);
        }
    }

    #[tokio::test]
    async fn native_bootstrap_can_read_config_requirements() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        let request: ClientRequest = serde_json::from_value(json!({
            "method":"configRequirements/read", "id":1
        }))
        .expect("config requirements request should parse");

        let admitted = policy
            .admit_client_request(request)
            .await
            .expect("read-only bootstrap requirements should be admitted");

        assert_eq!(admitted.method_name(), "configRequirements/read");
    }

    #[tokio::test]
    async fn durable_policy_reconciles_an_interrupted_turn_before_admitting_the_retry() {
        let fixture = DurableFixture::new();
        let first = TiberHostPolicy::new(fixture.repository.clone());
        let first_turn: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params":{"threadId":"thread-1","input":[{
                "type":"text", "text":"first", "textElements":[]
            }]}
        }))
        .expect("first turn should parse");
        first
            .admit_client_request(first_turn)
            .await
            .expect("first prompt should be durably admitted");

        let restarted = TiberHostPolicy::new(fixture.repository.clone());
        let retry: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":2,
            "params":{"threadId":"thread-1","input":[{
                "type":"text", "text":"retry", "textElements":[]
            }]}
        }))
        .expect("retry turn should parse");
        restarted
            .admit_client_request(retry)
            .await
            .expect("restart should interrupt the lost inference before admitting the retry");

        let store = crate::TiberEventStore::open(&fixture.repository)
            .expect("durable history should reopen");
        let events = crate::read_session_events(&store).expect("session history should read");
        let interruptions = events
            .iter()
            .filter(|event| {
                matches!(
                    event.fact(),
                    crate::SessionFact::InferenceInterrupted { .. }
                )
            })
            .count();
        let requests = events
            .iter()
            .filter(|event| matches!(event.fact(), crate::SessionFact::InferenceRequested { .. }))
            .count();
        assert_eq!(interruptions, 1);
        assert_eq!(requests, 2);
    }

    #[tokio::test]
    async fn configured_process_dispatch_is_owned_by_the_exact_admitted_turn() {
        let fixture = DurableFixture::new();
        fs::create_dir_all(fixture.repository.join(".tiber"))
            .expect("configuration directory should be created");
        fs::write(
            fixture.repository.join(".tiber/commands.toml"),
            r#"[commands.no-op]
program = "/bin/true"
arguments = []
working-directory = "."
environment = {}
network = false
timeout-milliseconds = 5000
stdout-bytes = 4096
stderr-bytes = 4096
"#,
        )
        .expect("configured command should be written");
        let policy = TiberHostPolicy::new(fixture.repository);
        let turn: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params":{"threadId":"thread-1","input":[{
                "type":"text", "text":"run it", "textElements":[]
            }]}
        }))
        .expect("turn should parse");
        policy
            .admit_client_request(turn)
            .await
            .expect("turn should be durably admitted");
        let started = serde_json::from_value(json!({
            "method":"turn/started", "params":{
                "threadId":"thread-1",
                "turn":{"id":"turn-1","items":[],"status":"inProgress","error":null}
            }
        }))
        .expect("turn-started should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
        let request: ServerRequest = serde_json::from_value(json!({
            "method":"item/tool/call", "id":2,
            "params":{
                "threadId":"thread-1", "turnId":"turn-1", "callId":"call-1",
                "namespace":null, "tool":"tiber_effect",
                "arguments":{"operation":"run_configured_command","command":"no-op"}
            }
        }))
        .expect("configured process call should parse");
        let ServerRequestDisposition::Resolve(result) =
            policy.intercept_server_request(&request).await
        else {
            panic!("configured process must resolve through Tiber")
        };
        let response: codex_app_server_protocol::DynamicToolCallResponse =
            serde_json::from_value(result).expect("response should retain the Codex type");
        let encoded = serde_json::to_string(&response).expect("response should serialize");
        assert!(
            response.success || encoded.contains("process_outcome_unknown"),
            "the allow-listed command must reach Tiber dispatch: {encoded}"
        );
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one durable scenario keeps proposal, restart recovery, verified history, and absence-of-dispatch assertions together"
    )]
    async fn lost_repository_proposal_is_cancelled_on_restart_before_the_next_prompt() {
        let fixture = DurableFixture::new();
        fs::write(fixture.repository.join("owned.txt"), "before\n")
            .expect("repository preimage should be written");
        git(&fixture.repository, ["add", "owned.txt"]);
        git(&fixture.repository, ["commit", "-m", "add owned file"]);
        let policy = TiberHostPolicy::new(fixture.repository.clone());
        let turn: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params":{"threadId":"thread-1","input":[{
                "type":"text", "text":"propose", "textElements":[]
            }]}
        }))
        .expect("turn should parse");
        policy
            .admit_client_request(turn)
            .await
            .expect("turn should admit");
        let started = serde_json::from_value(json!({
            "method":"turn/started", "params":{
                "threadId":"thread-1",
                "turn":{"id":"turn-1","items":[],"status":"inProgress","error":null}
            }
        }))
        .expect("turn-started should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
        let proposal: ServerRequest = serde_json::from_value(json!({
            "method":"item/tool/call", "id":2,
            "params":{
                "threadId":"thread-1", "turnId":"turn-1", "callId":"proposal-1",
                "namespace":null, "tool":"tiber_repository_proposal",
                "arguments":{
                    "action":"write", "path":"owned.txt",
                    "expected":"before\n", "replacement":"after\n"
                }
            }
        }))
        .expect("proposal should parse");
        let ServerRequestDisposition::Resolve(result) =
            policy.intercept_server_request(&proposal).await
        else {
            panic!("proposal should resolve through Tiber")
        };
        let response: codex_app_server_protocol::DynamicToolCallResponse =
            serde_json::from_value(result).expect("proposal response should be typed");
        assert!(response.success);
        drop(policy);

        let restarted = TiberHostPolicy::new(fixture.repository.clone());
        let retry: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":3,
            "params":{"threadId":"thread-1","input":[{
                "type":"text", "text":"continue", "textElements":[]
            }]}
        }))
        .expect("retry should parse");
        restarted
            .admit_client_request(retry)
            .await
            .expect("restart should cancel the lost proposal and interrupt the lost turn");
        let store = crate::TiberEventStore::open(&fixture.repository)
            .expect("durable history should reopen");
        let session_events =
            crate::read_session_events(&store).expect("session history should remain readable");
        let proposed_effect = session_events
            .iter()
            .find_map(|event| match event.fact() {
                crate::SessionFact::InferenceRequested { effect, .. } => Some(effect),
                _ => None,
            })
            .expect("the proposal turn should retain its inference effect");
        let stream = crate::RepositoryMutationStream::for_effect(proposed_effect.effect_id())
            .expect("repository stream should derive from the admitted effect");
        let mutation_events = crate::read_repository_mutation_events(&store, &stream)
            .expect("repository mutation history should read");
        let cancelled = mutation_events
            .iter()
            .filter(|event| {
                matches!(
                    event.fact(),
                    tiber_repository_service::RepositoryMutationFact::Cancelled(_)
                )
            })
            .count();
        let dispatched = mutation_events.iter().any(|event| {
            matches!(
                event.fact(),
                tiber_repository_service::RepositoryMutationFact::Prepared(_)
                    | tiber_repository_service::RepositoryMutationFact::Applied(_)
            )
        });
        assert_eq!(cancelled, 1);
        assert!(
            !dispatched,
            "lost proposals must never reach dispatch or application"
        );
        assert_eq!(
            fs::read_to_string(fixture.repository.join("owned.txt"))
                .expect("repository file should remain readable"),
            "before\n"
        );
    }

    #[tokio::test]
    async fn completed_turn_is_published_before_the_notification_is_forwarded() {
        let admission = Arc::new(RecordingAdmission(AtomicBool::new(false)));
        let completion = Arc::new(RecordingCompletion {
            called: AtomicBool::new(false),
            fails: false,
        });
        let policy = TiberHostPolicy::with_boundaries(admission, Arc::clone(&completion));
        let turn_request: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params": {
                "threadId":"thread-1",
                "input":[{"type":"text", "text":"inspect", "textElements":[]}]
            }
        }))
        .expect("turn fixture should parse");
        policy
            .admit_client_request(turn_request)
            .await
            .expect("turn should be durably admitted");
        let started: codex_app_server_protocol::ServerNotification =
            serde_json::from_value(json!({
                "method":"turn/started",
                "params": {
                    "threadId":"thread-1",
                    "turn":{"id":"turn-1", "items":[], "status":"inProgress", "error":null}
                }
            }))
            .expect("turn-started fixture should parse");
        assert!(matches!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        ));
        policy.process_cancellation.cancel();
        assert!(policy.process_cancellation.cancel_is_latched());
        let completed: codex_app_server_protocol::ServerNotification =
            serde_json::from_value(json!({
                "method":"turn/completed",
                "params": {
                    "threadId":"thread-1",
                    "turn":{
                        "id":"turn-1", "status":"completed", "error":null,
                        "items":[{"type":"agentMessage", "id":"message-1", "text":"done"}]
                    }
                }
            }))
            .expect("turn-completed fixture should parse");
        assert!(matches!(
            policy.admit_server_notification(&completed).await,
            ServerNotificationDisposition::Forward
        ));
        assert!(completion.called.load(Ordering::Acquire));
        assert!(!policy.process_cancellation.cancel_is_latched());
    }

    #[tokio::test]
    async fn interrupt_before_turn_started_binds_and_cancels_the_admitted_turn() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        let turn_request: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params": {
                "threadId":"thread-1",
                "input":[{"type":"text", "text":"inspect", "textElements":[]}]
            }
        }))
        .expect("turn fixture should parse");
        policy
            .admit_client_request(turn_request)
            .await
            .expect("turn should be durably admitted");
        let interrupt: ClientRequest = serde_json::from_value(json!({
            "method":"turn/interrupt", "id":2,
            "params":{"threadId":"thread-1", "turnId":"turn-1"}
        }))
        .expect("interrupt fixture should parse");
        policy.observe_cancellation_requested("thread-1", "turn-1");
        assert!(!policy.process_cancellation.cancel_is_latched());
        policy
            .admit_client_request(interrupt)
            .await
            .expect("the pending admitted turn should bind the interrupt turn id");
        assert!(policy.process_cancellation.cancel_is_latched());

        let started: codex_app_server_protocol::ServerNotification =
            serde_json::from_value(json!({
                "method":"turn/started",
                "params": {
                    "threadId":"thread-1",
                    "turn":{"id":"turn-1", "items":[], "status":"inProgress", "error":null}
                }
            }))
            .expect("turn-started fixture should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
    }

    #[tokio::test]
    async fn failed_completion_publication_suppresses_the_notification() {
        let completion = Arc::new(RecordingCompletion {
            called: AtomicBool::new(false),
            fails: true,
        });
        let policy = TiberHostPolicy::with_boundaries(
            Arc::new(RecordingAdmission(AtomicBool::new(false))),
            Arc::clone(&completion),
        );
        let turn_request: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params": {
                "threadId":"thread-1",
                "input":[{"type":"text", "text":"inspect", "textElements":[]}]
            }
        }))
        .expect("turn fixture should parse");
        policy
            .admit_client_request(turn_request)
            .await
            .expect("turn should be durably admitted");
        let started: codex_app_server_protocol::ServerNotification =
            serde_json::from_value(json!({
                "method":"turn/started",
                "params": {
                    "threadId":"thread-1",
                    "turn":{"id":"turn-1", "items":[], "status":"inProgress", "error":null}
                }
            }))
            .expect("turn-started fixture should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
        policy.process_cancellation.cancel();
        let completed: codex_app_server_protocol::ServerNotification =
            serde_json::from_value(json!({
                "method":"turn/completed",
                "params": {
                    "threadId":"thread-1",
                    "turn":{
                        "id":"turn-1", "status":"completed", "error":null,
                        "items":[{"type":"agentMessage", "id":"message-1", "text":"done"}]
                    }
                }
            }))
            .expect("turn-completed fixture should parse");

        assert_eq!(
            policy.admit_server_notification(&completed).await,
            ServerNotificationDisposition::Suppress
        );
        assert!(completion.called.load(Ordering::Acquire));
        assert!(!policy.process_cancellation.cancel_is_latched());
    }

    #[tokio::test]
    async fn turn_is_admitted_before_it_can_reach_inference() {
        let admission = Arc::new(RecordingAdmission(AtomicBool::new(false)));
        let policy = TiberHostPolicy::with_admission(Arc::clone(&admission));
        let request: ClientRequest = serde_json::from_value(json!({
            "method": "turn/start",
            "id": 1,
            "params": {
                "threadId": "thread-1",
                "input": [{"type":"text", "text":"inspect", "textElements":[]}]
            }
        }))
        .expect("typed turn fixture should parse");

        let admitted = policy
            .admit_client_request(request)
            .await
            .expect("durable admission should release the turn");

        assert!(admission.0.load(Ordering::Acquire));
        let encoded = serde_json::to_value(admitted).expect("admitted turn should serialize");
        assert_eq!(
            encoded.pointer("/params/sandboxPolicy"),
            Some(&json!({"type":"readOnly", "networkAccess":false}))
        );
    }

    #[tokio::test]
    async fn built_in_slash_admission_preserves_plan_side_and_btw_identity() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        for (command, expected) in [
            (BuiltinSlashCommand::Plan, "plan"),
            (BuiltinSlashCommand::Side, "side"),
            (BuiltinSlashCommand::Btw, "btw"),
        ] {
            policy
                .admit_builtin_slash_command(&BuiltinSlashCommandRequest {
                    command,
                    args: "question".to_owned(),
                    thread_id: Some("thread-parent".to_owned()),
                })
                .await
                .expect("exact built-in command should be admitted");
            let pending = policy
                .slash_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("identity should remain pending until its turn");
            let actual = match pending {
                SlashAdmission::Plan { .. } => "plan",
                SlashAdmission::Isolated {
                    kind: IsolatedTurnKind::Side,
                    ..
                } => "side",
                SlashAdmission::Isolated {
                    kind: IsolatedTurnKind::Btw,
                    ..
                } => "btw",
            };
            assert_eq!(actual, expected);
        }
    }

    #[tokio::test]
    async fn failed_durable_admission_restores_the_exact_pending_slash_for_retry() {
        let fixture = DurableFixture::new();
        let mut policy = TiberHostPolicy::new(fixture.repository.clone());
        policy.admission = Arc::new(FailRecoveryOnce {
            inner: DurablePromptAdmission {
                first_turn_recovery: FirstTurnRecovery::pending(),
                pending_repository: Arc::clone(&policy.pending_repository),
                repository: fixture.repository,
            },
            failed: AtomicBool::new(false),
        });
        let slash = BuiltinSlashCommandRequest {
            command: BuiltinSlashCommand::Plan,
            args: "retry the plan".to_owned(),
            thread_id: Some("thread-parent".to_owned()),
        };
        policy
            .admit_builtin_slash_command(&slash)
            .await
            .expect("plan slash should stage");
        let turn = || {
            serde_json::from_value(json!({
                "method":"turn/start", "id":1,
                "params":{"threadId":"thread-parent","input":[{"type":"text","text":"retry the plan","textElements":[]}]}
            }))
            .expect("turn should parse")
        };
        assert!(policy.admit_client_request(turn()).await.is_err());
        assert!(matches!(
            policy
                .slash_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref(),
            Some(SlashAdmission::Plan { parent_thread_id }) if parent_thread_id == "thread-parent"
        ));
        policy
            .admit_client_request(turn())
            .await
            .expect("identical retry should consume the restored plan admission");
    }

    #[tokio::test]
    #[expect(
        clippy::shadow_unrelated,
        clippy::too_many_lines,
        reason = "the durable Plan scenario names each reopened signed-store checkpoint explicitly"
    )]
    async fn native_plan_turn_is_durable_before_start_and_completes_as_proposal() {
        let fixture = DurableFixture::new();
        let policy = TiberHostPolicy::new(fixture.repository.clone());
        policy
            .admit_builtin_slash_command(&BuiltinSlashCommandRequest {
                command: BuiltinSlashCommand::Plan,
                args: "plan safely".to_owned(),
                thread_id: Some("thread-parent".to_owned()),
            })
            .await
            .expect("plan slash should admit");
        let request: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params":{"threadId":"thread-parent","input":[{"type":"text","text":"plan safely","textElements":[]}]}
        }))
        .expect("plan turn should parse");
        policy
            .admit_client_request(request)
            .await
            .expect("planning fact must publish before inference");
        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("authority should open");
        let events = crate::read_session_events(&store).expect("session history should read");
        assert!(matches!(
            events.last().map(tiber_session_service::SessionEvent::fact),
            Some(tiber_session_service::SessionFact::InferenceRequested {
                mode: tiber_session_service::InferenceMode::Planning,
                ..
            })
        ));

        let started = serde_json::from_value(json!({
            "method":"turn/started","params":{"threadId":"thread-parent","turn":{"id":"turn-plan","items":[],"status":"inProgress","error":null}}
        }))
        .expect("started notification should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
        let completed = serde_json::from_value(json!({
            "method":"turn/completed","params":{"threadId":"thread-parent","turn":{"id":"turn-plan","items":[{"type":"plan","id":"plan-1","text":"Use typed effects."}],"status":"completed","error":null}}
        }))
        .expect("completed notification should parse");
        assert_eq!(
            policy.admit_server_notification(&completed).await,
            ServerNotificationDisposition::Forward
        );
        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("authority should reopen");
        let events = crate::read_session_events(&store).expect("session history should read");
        assert!(matches!(
            tiber_session_service::project_plan_restart_state(&events),
            Ok(tiber_session_service::PlanRestartState::AwaitingDecision { .. })
        ));

        let decision = BuiltinPlanDecisionRequest {
            decision: BuiltinPlanDecision::Accept,
            implementation_prompt: Some("implement safely".to_owned()),
        };
        policy
            .admit_builtin_plan_decision(&decision)
            .await
            .expect("native acceptance must publish before its submit effect");
        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("accepted authority should reopen");
        let accepted_event_count = crate::read_session_events(&store)
            .expect("accepted history should read")
            .len();
        policy
            .admit_builtin_plan_decision(&decision)
            .await
            .expect("an ambiguous duplicate acceptance must reconcile as success");
        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("reconciled authority should reopen");
        assert_eq!(
            crate::read_session_events(&store)
                .expect("reconciled history should read")
                .len(),
            accepted_event_count,
            "the duplicate hook must not append a second decision or request"
        );

        let implementation: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":2,
            "params":{"threadId":"thread-parent","input":[{"type":"text","text":"implement safely","textElements":[]}]}
        }))
        .expect("implementation turn should parse");
        policy
            .admit_client_request(implementation)
            .await
            .expect("the already-durable implementation prompt should reconcile exactly");
        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("accepted authority should reopen");
        let events = crate::read_session_events(&store).expect("accepted history should read");
        let [.., decided, requested] = events.as_slice() else {
            panic!("accepted plan should retain two terminal facts");
        };
        assert!(matches!(
            decided.fact(),
            tiber_session_service::SessionFact::PlanDecided {
                decision: tiber_session_service::PlanDecision::Accepted,
                ..
            }
        ));
        assert!(matches!(
            requested.fact(),
            tiber_session_service::SessionFact::InferenceRequested {
                mode: tiber_session_service::InferenceMode::Ordinary,
                prompt,
                ..
            } if prompt.as_str() == "implement safely"
        ));
    }

    #[tokio::test]
    async fn native_plan_cancel_is_durable_before_the_prompt_is_dismissed() {
        let fixture = DurableFixture::new();
        let policy = TiberHostPolicy::new(fixture.repository.clone());
        policy
            .admit_builtin_slash_command(&BuiltinSlashCommandRequest {
                command: BuiltinSlashCommand::Plan,
                args: "plan safely".to_owned(),
                thread_id: Some("thread-parent".to_owned()),
            })
            .await
            .expect("plan slash should admit");
        let request = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params":{"threadId":"thread-parent","input":[{"type":"text","text":"plan safely","textElements":[]}]}
        }))
        .expect("plan turn should parse");
        policy
            .admit_client_request(request)
            .await
            .expect("planning request should publish");
        let started = serde_json::from_value(json!({
            "method":"turn/started","params":{"threadId":"thread-parent","turn":{"id":"turn-plan","items":[],"status":"inProgress","error":null}}
        }))
        .expect("started notification should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
        let completed = serde_json::from_value(json!({
            "method":"turn/completed","params":{"threadId":"thread-parent","turn":{"id":"turn-plan","items":[{"type":"plan","id":"plan-1","text":"Use typed effects."}],"status":"completed","error":null}}
        }))
        .expect("completed notification should parse");
        assert_eq!(
            policy.admit_server_notification(&completed).await,
            ServerNotificationDisposition::Forward
        );

        policy
            .admit_builtin_plan_decision(&BuiltinPlanDecisionRequest {
                decision: BuiltinPlanDecision::Cancel,
                implementation_prompt: None,
            })
            .await
            .expect("native cancellation must publish before dismissal");

        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("cancelled authority should reopen");
        let events = crate::read_session_events(&store).expect("session history should read");
        assert!(matches!(
            tiber_session_service::project_plan_restart_state(&events),
            Ok(tiber_session_service::PlanRestartState::Decided {
                decision: tiber_session_service::PlanDecision::Cancelled,
                ..
            })
        ));
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "the side-turn acceptance scenario keeps parent and child lifecycle assertions in one coherent test"
    )]
    async fn native_side_fork_uses_parent_authority_and_closes_its_child_stream() {
        let fixture = DurableFixture::new();
        let policy = TiberHostPolicy::new(fixture.repository.clone());
        let parent: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":0,
            "params":{"threadId":"thread-parent","input":[{"type":"text","text":"parent work","textElements":[]}]}
        }))
        .expect("parent turn should parse");
        policy
            .admit_client_request(parent)
            .await
            .expect("ordinary parent should remain active");
        let parent_started = serde_json::from_value(json!({
            "method":"turn/started","params":{"threadId":"thread-parent","turn":{"id":"turn-parent","items":[],"status":"inProgress","error":null}}
        }))
        .expect("parent started notification should parse");
        assert_eq!(
            policy.admit_server_notification(&parent_started).await,
            ServerNotificationDisposition::Forward
        );
        policy
            .admit_builtin_slash_command(&BuiltinSlashCommandRequest {
                command: BuiltinSlashCommand::Side,
                args: "answer separately".to_owned(),
                thread_id: Some("thread-parent".to_owned()),
            })
            .await
            .expect("side slash should admit");
        let fork: ClientRequest = serde_json::from_value(json!({
            "method":"thread/fork", "id":1, "params":{"threadId":"thread-parent"}
        }))
        .expect("thread fork should parse");
        policy
            .admit_client_request(fork)
            .await
            .expect("exact parent fork should retain authority");
        let request: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":2,
            "params":{"threadId":"thread-child","input":[{"type":"text","text":"answer separately","textElements":[]}]}
        }))
        .expect("child turn should parse");
        policy
            .admit_client_request(request)
            .await
            .expect("isolated request must publish before inference");
        let started = serde_json::from_value(json!({
            "method":"turn/started","params":{"threadId":"thread-child","turn":{"id":"turn-side","items":[],"status":"inProgress","error":null}}
        }))
        .expect("started notification should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
        let completed = serde_json::from_value(json!({
            "method":"turn/completed","params":{"threadId":"thread-child","turn":{"id":"turn-side","items":[{"type":"agentMessage","id":"message-1","text":"isolated answer"}],"status":"completed","error":null}}
        }))
        .expect("completed notification should parse");
        assert_eq!(
            policy.admit_server_notification(&completed).await,
            ServerNotificationDisposition::Forward
        );

        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("authority should reopen");
        let stream = store
            .stream_ids()
            .iter()
            .find(|stream| stream.as_ref().starts_with("tiber:session:isolated:"))
            .expect("isolated child stream should exist")
            .clone();
        let pattern = eventcore_types::StreamPattern::try_new(stream.as_ref().to_owned())
            .expect("isolated stream pattern should parse");
        let history = store
            .verified_transaction_reader::<tiber_session_service::IsolatedTurnEvent>(&[pattern])
            .expect("isolated history should verify")
            .read_page(tiber_store_git::TransactionEventPage::first(
                eventcore_types::BatchSize::new(5),
            ))
            .expect("isolated history should read");
        assert!(matches!(
            history
                .first()
                .map(tiber_session_service::IsolatedTurnEvent::fact),
            Some(tiber_session_service::IsolatedTurnFact::Opened {
                kind: IsolatedTurnKind::Side,
                ..
            })
        ));
        assert!(matches!(
            history
                .last()
                .map(tiber_session_service::IsolatedTurnEvent::fact),
            Some(tiber_session_service::IsolatedTurnFact::Closed)
        ));
        let turns = policy
            .turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(turns.contains_key("thread-parent"));
        assert!(!turns.contains_key("thread-child"));
        assert_eq!(
            turns
                .get("thread-parent")
                .and_then(|turn| turn.turn_id.as_deref()),
            Some("turn-parent")
        );
    }

    #[tokio::test]
    async fn restart_interrupts_and_closes_an_unfinished_isolated_turn_without_replay() {
        let fixture = DurableFixture::new();
        let repository = fixture.repository.clone();
        let stream = tokio::task::spawn_blocking(move || {
            let (_binding, _events) =
                crate::ensure_started_session(&repository).expect("active session should start");
            crate::publish_isolated_prompt_request(
                &repository,
                IsolatedTurnKind::Btw,
                "unfinished aside",
            )
            .expect("isolated request should publish")
        })
        .await
        .expect("isolated setup should join");
        let restarted = TiberHostPolicy::new(fixture.repository.clone());
        let request: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":1,
            "params":{"threadId":"thread-parent","input":[{"type":"text","text":"continue after restart","textElements":[]}]}
        }))
        .expect("restart turn should parse");
        restarted
            .admit_client_request(request)
            .await
            .expect("restart recovery should close without replay before ordinary admission");
        let store = tiber_store_git::TiberEventStore::open(&fixture.repository)
            .expect("recovered authority should open");
        let pattern = eventcore_types::StreamPattern::try_new(stream.as_ref().to_owned())
            .expect("isolated stream pattern should parse");
        let history = store
            .verified_transaction_reader::<tiber_session_service::IsolatedTurnEvent>(&[pattern])
            .expect("isolated history should verify")
            .read_page(tiber_store_git::TransactionEventPage::first(
                eventcore_types::BatchSize::new(5),
            ))
            .expect("isolated history should read");
        assert!(history.iter().any(|event| matches!(
            event.fact(),
            tiber_session_service::IsolatedTurnFact::InferenceInterrupted { .. }
        )));
        assert!(matches!(
            history
                .last()
                .map(tiber_session_service::IsolatedTurnEvent::fact),
            Some(tiber_session_service::IsolatedTurnFact::Closed)
        ));
    }

    #[test]
    fn effect_gate_denies_native_effects_and_non_tiber_tools() {
        let gate = TiberEffectGate;
        for effect in [
            EffectRequest::FileWrite {
                paths: vec!["src/lib.rs".to_owned()],
            },
            EffectRequest::Process {
                command: vec!["true".to_owned()],
                cwd: "/workspace".to_owned(),
            },
            EffectRequest::Network {
                destination: Some("example.com".to_owned()),
            },
            EffectRequest::Tool {
                name: "shell".to_owned(),
            },
        ] {
            assert_eq!(
                gate.check(&effect)
                    .expect_err("effect must be denied")
                    .code(),
                "tiber_effect_requires_authority"
            );
        }
        assert!(
            gate.check(&EffectRequest::Tool {
                name: "tiber_tasks".to_owned()
            })
            .is_ok()
        );
    }

    #[test]
    fn thread_start_receives_only_tiber_dynamic_tools() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime should build");
        let request: ClientRequest = serde_json::from_value(json!({
            "method":"thread/start", "id":1, "params":{}
        }))
        .expect("thread fixture should parse");
        let admitted = runtime
            .block_on(policy.admit_client_request(request))
            .expect("thread admission should succeed");
        let encoded = serde_json::to_value(admitted).expect("thread should serialize");
        assert_eq!(
            encoded
                .pointer("/params/dynamicTools")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(TIBER_TOOL_NAMES.len())
        );
    }

    #[tokio::test]
    async fn non_dynamic_server_requests_fail_closed() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        let request: ServerRequest = serde_json::from_value(json!({
            "method":"item/commandExecution/requestApproval",
            "id":1,
            "params": {
                "threadId":"thread-1", "turnId":"turn-1", "itemId":"item-1",
                "startedAtMs":0, "command":"true", "cwd":"/workspace", "reason":null
            }
        }))
        .expect("approval fixture should parse");
        assert!(matches!(
            policy.intercept_server_request(&request).await,
            ServerRequestDisposition::Reject(_)
        ));
    }

    #[tokio::test]
    async fn unknown_dynamic_tools_resolve_to_a_typed_inert_failure() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        let request: ServerRequest = serde_json::from_value(json!({
            "method":"item/tool/call", "id":1,
            "params": {
                "threadId":"thread-1", "turnId":"turn-1", "callId":"call-1",
                "namespace":null, "tool":"ambient_shell", "arguments":{}
            }
        }))
        .expect("dynamic-tool fixture should parse");
        let ServerRequestDisposition::Resolve(response) =
            policy.intercept_server_request(&request).await
        else {
            panic!("unknown dynamic tool must receive an application-owned failure");
        };
        let typed_response: codex_app_server_protocol::DynamicToolCallResponse =
            serde_json::from_value(response).expect("host result must match the linked Codex type");
        assert!(!typed_response.success);
        assert!(
            serde_json::to_string(&typed_response)
                .expect("typed response should serialize")
                .contains("tiber_tool_unauthorized")
        );
    }

    #[tokio::test]
    async fn known_dynamic_tools_require_the_exact_active_turn() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        let tool_request = |thread_id: &str, turn_id: &str| {
            serde_json::from_value(json!({
                "method":"item/tool/call", "id":1,
                "params": {
                    "threadId":thread_id, "turnId":turn_id, "callId":"call-1",
                    "namespace":null, "tool":"tiber_repository_read",
                    "arguments":{"operation":"read_file", "path":"missing.txt"}
                }
            }))
            .expect("known dynamic-tool fixture should parse")
        };
        assert!(matches!(
            policy
                .intercept_server_request(&tool_request("thread-1", "turn-1"))
                .await,
            ServerRequestDisposition::Reject(_)
        ));

        let turn_request: ClientRequest = serde_json::from_value(json!({
            "method":"turn/start", "id":2,
            "params": {
                "threadId":"thread-1",
                "input":[{"type":"text", "text":"inspect", "textElements":[]}]
            }
        }))
        .expect("turn fixture should parse");
        policy
            .admit_client_request(turn_request)
            .await
            .expect("turn should be durably admitted");
        let started = serde_json::from_value(json!({
            "method":"turn/started",
            "params": {
                "threadId":"thread-1",
                "turn":{"id":"turn-1", "items":[], "status":"inProgress", "error":null}
            }
        }))
        .expect("turn-started fixture should parse");
        assert_eq!(
            policy.admit_server_notification(&started).await,
            ServerNotificationDisposition::Forward
        );
        assert!(matches!(
            policy
                .intercept_server_request(&tool_request("thread-1", "turn-other"))
                .await,
            ServerRequestDisposition::Reject(_)
        ));
        assert!(matches!(
            policy
                .intercept_server_request(&tool_request("thread-1", "turn-1"))
                .await,
            ServerRequestDisposition::Resolve(_)
        ));
    }

    #[tokio::test]
    async fn plan_and_isolated_turns_deny_every_tiber_dynamic_tool() {
        let policy =
            TiberHostPolicy::with_admission(Arc::new(RecordingAdmission(AtomicBool::new(false))));
        {
            let mut turns = policy
                .turns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            turns.insert(
                "plan-thread".to_owned(),
                AdmittedTurn {
                    authority: TurnAuthority::Plan,
                    cancellation: NativeProcessCancellation::default(),
                    turn_id: Some("plan-turn".to_owned()),
                },
            );
            turns.insert(
                "side-thread".to_owned(),
                AdmittedTurn {
                    authority: TurnAuthority::Isolated(
                        eventcore_types::StreamId::try_new(
                            "tiber:session:isolated:test".to_owned(),
                        )
                        .expect("test stream should parse"),
                    ),
                    cancellation: NativeProcessCancellation::default(),
                    turn_id: Some("side-turn".to_owned()),
                },
            );
        };
        for (thread_id, turn_id) in [("plan-thread", "plan-turn"), ("side-thread", "side-turn")] {
            for tool in TIBER_TOOL_NAMES {
                let request: ServerRequest = serde_json::from_value(json!({
                    "method":"item/tool/call", "id":1,
                    "params":{"threadId":thread_id,"turnId":turn_id,"callId":"call-1","namespace":null,"tool":tool,"arguments":{}}
                }))
                .expect("dynamic tool should parse");
                let ServerRequestDisposition::Reject(error) =
                    policy.intercept_server_request(&request).await
                else {
                    panic!("non-ordinary turn must reject every Tiber tool");
                };
                assert_eq!(error.message, "tiber_dynamic_tool_mode_unauthorized");
            }
        }
    }
}
