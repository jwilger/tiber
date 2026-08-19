//! Pure, serializable workflow-harness domain transitions.

extern crate alloc;

use alloc::string::String;
use core::{error::Error, fmt};
use serde::{Deserialize, Serialize, de::Error as _};
use sha2::{Digest as _, Sha256};

/// Defines one validated textual workflow identity newtype.
macro_rules! semantic_text {
    ($name:ident, $empty:ident, $invalid:ident $(, $additional_validation:expr)?) => {
        #[doc = "A validated textual workflow semantic value."]
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        #[expect(
            clippy::implicit_return,
            reason = "validated value accessors use idiomatic tail expressions while the restriction lint forbids them"
        )]
        impl $name {
            /// Returns this value's canonical text.
            #[must_use]
            #[inline]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Parses a workflow semantic value at the external boundary.
            ///
            /// # Errors
            ///
            /// Returns a stable [`HarnessError`] if the text is empty, contains a control character,
            /// exceeds [`MAX_SEMANTIC_TEXT_BYTES`], or violates its type-specific storage constraint.
            #[inline]
            pub fn parse(value: &str) -> Result<Self, HarnessError> {
                let value = value.trim();
                if value.is_empty() {
                    return Err(HarnessError::$empty);
                }
                if value.len() > MAX_SEMANTIC_TEXT_BYTES {
                    return Err(HarnessError::$invalid);
                }
                if value.chars().any(char::is_control) {
                    return Err(HarnessError::$invalid);
                }
                $(
                    if !($additional_validation)(value) {
                        return Err(HarnessError::$invalid);
                    }
                )?
                Ok(Self(value.to_owned()))
            }
        }

        #[expect(
            clippy::implicit_return,
            clippy::missing_trait_methods,
            reason = "the semantic parser is the sole construction boundary; deserialize_in_place cannot preserve that invariant"
        )]
        impl<'de> Deserialize<'de> for $name {
            #[inline]
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let decoded = match String::deserialize(deserializer) {
                    Ok(decoded) => decoded,
                    Err(error) => return Err(error),
                };
                match Self::parse(&decoded) {
                    Ok(parsed) => Ok(parsed),
                    Err(error) => Err(D::Error::custom(error)),
                }
            }
        }
    };
}

/// Defines one validated bounded workflow counter newtype.
macro_rules! bounded_counter {
    ($name:ident, $error:ident) => {
        #[doc = "A positive, bounded workflow counter."]
        #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(u32);

        #[expect(
            clippy::implicit_return,
            reason = "validated counter accessors use idiomatic tail expressions while the restriction lint forbids them"
        )]
        impl $name {
            /// The first valid counter value.
            pub const FIRST: Self = Self(1);

            /// Returns the validated counter value.
            #[must_use]
            #[inline]
            pub const fn get(self) -> u32 {
                self.0
            }

            /// Parses a one-based bounded counter at the external boundary.
            ///
            /// # Errors
            ///
            /// Returns a stable [`HarnessError`] when the counter is zero or exceeds the configured maximum.
            #[inline]
            pub fn parse(value: u32) -> Result<Self, HarnessError> {
                if value == 0 || value > MAX_ASSIGNMENT_COUNTER {
                    return Err(HarnessError::$error);
                }
                Ok(Self(value))
            }
        }

        #[expect(
            clippy::implicit_return,
            reason = "the default is the one valid first counter value"
        )]
        impl Default for $name {
            #[inline]
            fn default() -> Self {
                Self::FIRST
            }
        }

        #[expect(
            clippy::implicit_return,
            clippy::missing_trait_methods,
            reason = "the semantic parser is the sole construction boundary; deserialize_in_place cannot preserve that invariant"
        )]
        impl<'de> Deserialize<'de> for $name {
            #[inline]
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let decoded = match u32::deserialize(deserializer) {
                    Ok(decoded) => decoded,
                    Err(error) => return Err(error),
                };
                match Self::parse(decoded) {
                    Ok(parsed) => Ok(parsed),
                    Err(error) => Err(D::Error::custom(error)),
                }
            }
        }
    };
}

/// Maximum UTF-8 byte length accepted for every durable workflow identifier and provenance value.
pub const MAX_SEMANTIC_TEXT_BYTES: usize = 256;

/// Maximum `EventCore` `StreamId` length in characters.
const MAX_EVENTCORE_STREAM_ID_CHARACTERS: usize = 255;

/// Durable `EventCore` stream prefix for one workflow session.
const WORKFLOW_STREAM_PREFIX: &str = "tiber:workflow:";

/// Maximum `SessionId` character length compatible with its durable `EventCore` workflow stream.
pub const MAX_SESSION_ID_CHARACTERS: usize =
    MAX_EVENTCORE_STREAM_ID_CHARACTERS - WORKFLOW_STREAM_PREFIX.len();

/// Largest accepted epoch or attempt number.
pub const MAX_ASSIGNMENT_COUNTER: u32 = 64;

/// Largest accepted inference deadline in milliseconds (one hour).
pub const MAX_DEADLINE_MILLISECONDS: u64 = 3_600_000;

/// Stable workflow-harness failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must exhaustively handle every durable workflow failure"
)]
pub enum HarnessError {
    EffectFailed,
    EffectOutcomeUnknown,
    EmptyAgentId,
    EmptyAssignmentId,
    EmptyAssignmentScope,
    EmptyContextReceiptId,
    EmptyEffectFailureCode,
    EmptyEffectId,
    EmptyEffectReceiptId,
    EmptyIdempotencyKey,
    EmptyPolicyDecisionId,
    EmptySessionId,
    EmptyWorkflowId,
    InvalidAgentId,
    InvalidAssignmentEpoch,
    InvalidAssignmentId,
    InvalidAssignmentScope,
    InvalidAttemptNumber,
    InvalidContextReceiptId,
    InvalidDeadlineMilliseconds,
    InvalidEffectFailureCode,
    InvalidEffectId,
    InvalidEffectReceiptId,
    InvalidIdempotencyKey,
    InvalidPolicyDecisionId,
    InvalidSessionId,
    InvalidWorkflowId,
    MismatchedObservation,
    MissingObservation,
    TerminalState,
    UnexpectedObservation,
}

#[expect(
    clippy::implicit_return,
    reason = "the stable error-code table uses an idiomatic total tail match"
)]
impl HarnessError {
    /// Returns the stable external failure code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EffectFailed => "workflow_effect_failed",
            Self::EffectOutcomeUnknown => "workflow_effect_outcome_unknown",
            Self::EmptyAgentId => "workflow_empty_agent_id",
            Self::EmptyAssignmentId => "workflow_empty_assignment_id",
            Self::EmptyAssignmentScope => "workflow_empty_assignment_scope",
            Self::EmptyContextReceiptId => "workflow_empty_context_receipt_id",
            Self::EmptyEffectFailureCode => "workflow_empty_effect_failure_code",
            Self::EmptyEffectId => "workflow_empty_effect_id",
            Self::EmptyEffectReceiptId => "workflow_empty_effect_receipt_id",
            Self::EmptyIdempotencyKey => "workflow_empty_idempotency_key",
            Self::EmptyPolicyDecisionId => "workflow_empty_policy_decision_id",
            Self::EmptySessionId => "workflow_empty_session_id",
            Self::EmptyWorkflowId => "workflow_empty_workflow_id",
            Self::InvalidAgentId => "workflow_invalid_agent_id",
            Self::InvalidAssignmentEpoch => "workflow_invalid_assignment_epoch",
            Self::InvalidAssignmentId => "workflow_invalid_assignment_id",
            Self::InvalidAssignmentScope => "workflow_invalid_assignment_scope",
            Self::InvalidAttemptNumber => "workflow_invalid_attempt_number",
            Self::InvalidContextReceiptId => "workflow_invalid_context_receipt_id",
            Self::InvalidDeadlineMilliseconds => "workflow_invalid_deadline_milliseconds",
            Self::InvalidEffectFailureCode => "workflow_invalid_effect_failure_code",
            Self::InvalidEffectId => "workflow_invalid_effect_id",
            Self::InvalidEffectReceiptId => "workflow_invalid_effect_receipt_id",
            Self::InvalidIdempotencyKey => "workflow_invalid_idempotency_key",
            Self::InvalidPolicyDecisionId => "workflow_invalid_policy_decision_id",
            Self::InvalidSessionId => "workflow_invalid_session_id",
            Self::InvalidWorkflowId => "workflow_invalid_workflow_id",
            Self::MismatchedObservation => "workflow_mismatched_observation",
            Self::MissingObservation => "workflow_missing_observation",
            Self::TerminalState => "workflow_terminal_state",
            Self::UnexpectedObservation => "workflow_unexpected_observation",
        }
    }
}

impl fmt::Display for HarnessError {
    #[expect(
        clippy::implicit_return,
        reason = "the display implementation delegates directly to the stable code table"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

semantic_text!(AgentId, EmptyAgentId, InvalidAgentId);
semantic_text!(AssignmentId, EmptyAssignmentId, InvalidAssignmentId);
semantic_text!(
    AssignmentScope,
    EmptyAssignmentScope,
    InvalidAssignmentScope
);
semantic_text!(
    ContextReceiptId,
    EmptyContextReceiptId,
    InvalidContextReceiptId
);
semantic_text!(
    EffectFailureCode,
    EmptyEffectFailureCode,
    InvalidEffectFailureCode
);
semantic_text!(EffectId, EmptyEffectId, InvalidEffectId);
semantic_text!(
    EffectReceiptId,
    EmptyEffectReceiptId,
    InvalidEffectReceiptId
);
semantic_text!(IdempotencyKey, EmptyIdempotencyKey, InvalidIdempotencyKey);
semantic_text!(
    PolicyDecisionId,
    EmptyPolicyDecisionId,
    InvalidPolicyDecisionId
);
semantic_text!(
    SessionId,
    EmptySessionId,
    InvalidSessionId,
    |value: &str| {
        value.chars().count() <= MAX_SESSION_ID_CHARACTERS
            && !value
                .chars()
                .any(|character| matches!(character, '*' | '?' | '[' | ']'))
    }
);
semantic_text!(WorkflowId, EmptyWorkflowId, InvalidWorkflowId);

#[expect(
    clippy::missing_trait_methods,
    reason = "semantic harness failures have no causal source"
)]
impl Error for HarnessError {}

bounded_counter!(AssignmentEpoch, InvalidAssignmentEpoch);
bounded_counter!(AttemptNumber, InvalidAttemptNumber);

/// Positive, bounded execution deadline supplied to the effect shell.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DeadlineMilliseconds(u64);

#[expect(
    clippy::implicit_return,
    reason = "validated deadline accessors use idiomatic tail expressions while the restriction lint forbids them"
)]
impl DeadlineMilliseconds {
    /// Returns the validated deadline in milliseconds.
    #[must_use]
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Parses one positive deadline at the external boundary.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::InvalidDeadlineMilliseconds`] if the value is
    /// zero or exceeds the configured upper bound.
    #[inline]
    pub fn parse(value: u64) -> Result<Self, HarnessError> {
        if value == 0 || value > MAX_DEADLINE_MILLISECONDS {
            return Err(HarnessError::InvalidDeadlineMilliseconds);
        }
        Ok(Self(value))
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "the semantic parser is the sole construction boundary; deserialize_in_place cannot preserve that invariant"
)]
impl<'de> Deserialize<'de> for DeadlineMilliseconds {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let decoded = u64::deserialize(deserializer)?;
        match Self::parse(decoded) {
            Ok(parsed) => Ok(parsed),
            Err(error) => Err(D::Error::custom(error)),
        }
    }
}

/// Complete immutable provenance required to infer one assignment result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InferEffect {
    /// Agent that owns this inference assignment.
    agent_id: AgentId,
    /// Assignment generation selected by the policy decision.
    assignment_epoch: AssignmentEpoch,
    /// Assignment the inference must complete.
    assignment_id: AssignmentId,
    /// Scope of work supplied to the agent.
    assignment_scope: AssignmentScope,
    /// One-based attempt under the durable assignment.
    attempt_number: AttemptNumber,
    /// Receipt identifying the context made available to the agent.
    context_receipt_id: ContextReceiptId,
    /// Maximum wall-clock budget for the shell effect.
    deadline_milliseconds: DeadlineMilliseconds,
    /// Idempotent effect identity for the shell execution.
    effect_id: EffectId,
    /// Deduplication key required for an external effect retry.
    idempotency_key: IdempotencyKey,
    /// Policy decision that authorized this effect.
    policy_decision_id: PolicyDecisionId,
    /// Durable session that owns this workflow.
    session_id: SessionId,
    /// Workflow continuation that requested this effect.
    workflow_id: WorkflowId,
}

#[expect(
    clippy::implicit_return,
    reason = "immutable effect-envelope accessors use idiomatic tail expressions while the restriction lint forbids them"
)]
impl InferEffect {
    /// Returns the agent that owns this inference assignment.
    #[must_use]
    #[inline]
    pub const fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    /// Returns the assignment generation selected by the policy decision.
    #[must_use]
    #[inline]
    pub const fn assignment_epoch(&self) -> AssignmentEpoch {
        self.assignment_epoch
    }

    /// Returns the assignment this inference must complete.
    #[must_use]
    #[inline]
    pub const fn assignment_id(&self) -> &AssignmentId {
        &self.assignment_id
    }

    /// Returns the scope of work supplied to the agent.
    #[must_use]
    #[inline]
    pub const fn assignment_scope(&self) -> &AssignmentScope {
        &self.assignment_scope
    }

    /// Returns the one-based attempt under the durable assignment.
    #[must_use]
    #[inline]
    pub const fn attempt_number(&self) -> AttemptNumber {
        self.attempt_number
    }

    /// Returns the receipt identifying context made available to the agent.
    #[must_use]
    #[inline]
    pub const fn context_receipt_id(&self) -> &ContextReceiptId {
        &self.context_receipt_id
    }

    /// Returns the maximum wall-clock budget for the shell effect.
    #[must_use]
    #[inline]
    pub const fn deadline_milliseconds(&self) -> DeadlineMilliseconds {
        self.deadline_milliseconds
    }

    /// Returns the idempotent effect identity for the shell execution.
    #[must_use]
    #[inline]
    pub const fn effect_id(&self) -> &EffectId {
        &self.effect_id
    }

    /// Returns the deduplication key required for an external effect retry.
    #[must_use]
    #[inline]
    pub const fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// Creates the complete immutable provenance envelope for one inference.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "each independent provenance value must be supplied at the immutable effect boundary"
    )]
    #[inline]
    pub fn new(
        session_id: SessionId,
        agent_id: AgentId,
        workflow_id: WorkflowId,
        assignment_id: AssignmentId,
        assignment_scope: AssignmentScope,
        assignment_epoch: AssignmentEpoch,
        attempt_number: AttemptNumber,
        context_receipt_id: ContextReceiptId,
        policy_decision_id: PolicyDecisionId,
        effect_id: EffectId,
        idempotency_key: IdempotencyKey,
        deadline_milliseconds: DeadlineMilliseconds,
    ) -> Self {
        Self {
            agent_id,
            assignment_epoch,
            assignment_id,
            assignment_scope,
            attempt_number,
            context_receipt_id,
            deadline_milliseconds,
            effect_id,
            idempotency_key,
            policy_decision_id,
            session_id,
            workflow_id,
        }
    }

    /// Returns the policy decision that authorized this effect.
    #[must_use]
    #[inline]
    pub const fn policy_decision_id(&self) -> &PolicyDecisionId {
        &self.policy_decision_id
    }

    /// Returns the durable session that owns this workflow.
    #[must_use]
    #[inline]
    pub const fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Returns the workflow continuation that requested this effect.
    #[must_use]
    #[inline]
    pub const fn workflow_id(&self) -> &WorkflowId {
        &self.workflow_id
    }
}

/// Closed domain effects requested by the pure harness.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the initial closed effect vocabulary intentionally has one variant"
)]
pub enum TiberEffect {
    /// Runs one fully-provenanced inference assignment through the effect shell.
    Infer(InferEffect),
}

/// Whether a reported failed effect would be retryable in a later policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "retry policy must handle every durable retryability result"
)]
pub enum Retryability {
    /// The failure must not be retried without a new policy decision.
    NotRetryable,
    /// A future policy may retry the effect using its idempotency key.
    Retryable,
}

/// A shell's observation of one requested effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the trampoline must handle every durable effect outcome"
)]
pub enum EffectObservation {
    /// The shell observed a terminal failed effect execution.
    Failed {
        /// Stable failure code supplied by the shell.
        code: EffectFailureCode,
        /// Effect whose result the shell observed.
        effect_id: EffectId,
        /// Whether a later policy could retry this failure.
        retryability: Retryability,
    },
    /// The shell could not determine whether the requested effect completed.
    OutcomeUnknown {
        /// Effect whose result the shell could not reconcile.
        effect_id: EffectId,
    },
    /// The shell observed a successful effect execution.
    Succeeded {
        /// Effect whose result the shell observed.
        effect_id: EffectId,
        /// Durable receipt returned by the successful shell execution.
        receipt_id: EffectReceiptId,
    },
}

impl EffectObservation {
    /// Returns the effect this observation belongs to.
    #[must_use]
    #[expect(
        clippy::implicit_return,
        clippy::ref_patterns,
        reason = "the closed observation match returns a borrowed shared field without cloning its semantic identity"
    )]
    #[inline]
    pub const fn effect_id(&self) -> &EffectId {
        match *self {
            Self::Failed { ref effect_id, .. }
            | Self::OutcomeUnknown { ref effect_id }
            | Self::Succeeded { ref effect_id, .. } => effect_id,
        }
    }
}

/// Lifecycle phase of one immutable infer assignment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the trampoline must make an explicit decision for every phase"
)]
pub enum HarnessPhase {
    /// The workflow has durably accepted its initial continuation state.
    Completed,
    /// The workflow has not emitted its initial effect yet.
    Ready,
    /// The workflow has stopped with a typed terminal error.
    Stopped,
    /// The workflow has emitted an effect and awaits its durable observation.
    WaitingForInference,
}

/// Serializable state for the deterministic trampoline.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessState {
    /// Immutable effect provenance carried through every successor state.
    initial_effect: InferEffect,
    /// Current deterministic continuation phase.
    phase: HarnessPhase,
}

#[expect(
    clippy::implicit_return,
    reason = "continuation constructors and accessors use idiomatic tail expressions while the restriction lint forbids them"
)]
impl HarnessState {
    /// Returns the immutable effect envelope associated with this continuation.
    #[must_use]
    #[inline]
    pub const fn initial_effect(&self) -> &InferEffect {
        &self.initial_effect
    }

    /// Creates a ready state associated with exactly one immutable inference effect.
    #[must_use]
    #[inline]
    pub fn new(initial_effect: InferEffect) -> Self {
        Self {
            initial_effect,
            phase: HarnessPhase::Ready,
        }
    }

    /// Returns the continuation's current phase.
    #[must_use]
    #[inline]
    pub const fn phase(&self) -> HarnessPhase {
        self.phase
    }
}

/// One result of advancing the pure workflow trampoline.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    clippy::large_enum_variant,
    reason = "each closed trampoline branch carries its complete serializable continuation state"
)]
pub enum TrampolineStep {
    /// The workflow completed with a durable effect receipt.
    Complete {
        /// Receipt supplied by the successful observed effect.
        receipt: EffectReceiptId,
        /// Terminal state reached by the pure trampoline.
        state: HarnessState,
    },
    /// The workflow should persist and execute exactly one closed effect.
    Continue {
        /// Closed effect description for the imperative shell.
        effect: TiberEffect,
        /// Checkpoint to resume only after an observation is durable.
        state: HarnessState,
    },
    /// The workflow reached a typed stopped terminal state.
    Stop {
        /// Stable reason the pure trampoline declined to continue.
        error: HarnessError,
        /// Terminal state reached by the pure trampoline.
        state: HarnessState,
    },
}

/// Advances the harness once without executing effects or re-emitting pending work.
#[must_use]
#[expect(
    clippy::implicit_return,
    reason = "the deterministic transition table is clearest as a total tail match"
)]
#[inline]
pub fn step(state: &HarnessState, observation: Option<&EffectObservation>) -> TrampolineStep {
    match state.phase {
        HarnessPhase::Completed | HarnessPhase::Stopped => TrampolineStep::Stop {
            error: HarnessError::TerminalState,
            state: state.clone(),
        },
        HarnessPhase::Ready => match observation {
            None => TrampolineStep::Continue {
                effect: TiberEffect::Infer(state.initial_effect.clone()),
                state: HarnessState {
                    initial_effect: state.initial_effect.clone(),
                    phase: HarnessPhase::WaitingForInference,
                },
            },
            Some(_) => stopped(state, HarnessError::UnexpectedObservation),
        },
        HarnessPhase::WaitingForInference => {
            let Some(observed) = observation else {
                return stopped(state, HarnessError::MissingObservation);
            };
            if observed.effect_id() != state.initial_effect.effect_id() {
                return stopped(state, HarnessError::MismatchedObservation);
            }

            match observed.clone() {
                EffectObservation::Failed { .. } => stopped(state, HarnessError::EffectFailed),
                EffectObservation::OutcomeUnknown { .. } => {
                    stopped(state, HarnessError::EffectOutcomeUnknown)
                }
                EffectObservation::Succeeded { receipt_id, .. } => TrampolineStep::Complete {
                    receipt: receipt_id,
                    state: HarnessState {
                        initial_effect: state.initial_effect.clone(),
                        phase: HarnessPhase::Completed,
                    },
                },
            }
        }
    }
}

/// Creates the next ready inference continuation after one successful turn.
///
/// The workflow core, rather than an adapter, owns the successor effect identity.
///
/// # Errors
///
/// Returns [`HarnessError::TerminalState`] unless the supplied workflow completed.
#[inline]
#[expect(
    clippy::needless_return,
    reason = "the explicit return conforms to the workspace return-style policy"
)]
pub fn continue_after_completion(state: &HarnessState) -> Result<HarnessState, HarnessError> {
    if state.phase != HarnessPhase::Completed {
        return Err(HarnessError::TerminalState);
    }
    return successor_state(state);
}

/// Creates a new ready inference continuation after a terminal stopped turn.
///
/// This does not retry or reclassify the failed effect. It derives a distinct
/// successor identity from the closed predecessor so a later owner prompt can
/// begin under fresh effect authority.
///
/// # Errors
///
/// Returns [`HarnessError::TerminalState`] unless the supplied workflow stopped.
#[inline]
#[expect(
    clippy::needless_return,
    reason = "the explicit return conforms to the workspace return-style policy"
)]
pub fn continue_after_interruption(state: &HarnessState) -> Result<HarnessState, HarnessError> {
    if state.phase != HarnessPhase::Stopped {
        return Err(HarnessError::TerminalState);
    }
    return successor_state(state);
}

#[expect(
    clippy::format_collect,
    clippy::needless_return,
    clippy::question_mark_used,
    reason = "the bounded SHA-256 successor identifiers preserve typed parsing failures at the workflow authority boundary and conform to the workspace's explicit-return rule"
)]
/// Derives the next effect identity shared by successful and interrupted turns.
fn successor_state(state: &HarnessState) -> Result<HarnessState, HarnessError> {
    let prior = state.initial_effect();
    let digest = Sha256::digest(prior.effect_id().as_str().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let effect = InferEffect::new(
        prior.session_id().clone(),
        prior.agent_id().clone(),
        prior.workflow_id().clone(),
        prior.assignment_id().clone(),
        prior.assignment_scope().clone(),
        prior.assignment_epoch(),
        prior.attempt_number(),
        ContextReceiptId::parse(&format!("context-{digest}"))?,
        PolicyDecisionId::parse(&format!("policy-{digest}"))?,
        EffectId::parse(&format!("effect-{digest}"))?,
        IdempotencyKey::parse(&format!("{}:{digest}", prior.session_id().as_str()))?,
        prior.deadline_milliseconds(),
    );
    return Ok(HarnessState::new(effect));
}

/// Creates a stopped successor that preserves immutable effect provenance.
#[expect(
    clippy::implicit_return,
    reason = "the helper returns its one stopped successor as an idiomatic tail expression"
)]
#[inline]
fn stopped(state: &HarnessState, error: HarnessError) -> TrampolineStep {
    TrampolineStep::Stop {
        error,
        state: HarnessState {
            initial_effect: state.initial_effect.clone(),
            phase: HarnessPhase::Stopped,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        clippy::shadow_reuse,
        reason = "test fixture parsing must fail loudly"
    )]
    fn id<T>(parse: impl FnOnce(&str) -> Result<T, HarnessError>, value: &str) -> T {
        match parse(value) {
            Ok(value) => value,
            Err(error) => panic!("test identifier must parse: {error}"),
        }
    }

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        reason = "test fixture construction must fail loudly"
    )]
    fn effect() -> InferEffect {
        InferEffect::new(
            id(SessionId::parse, "session-1"),
            id(AgentId::parse, "agent-1"),
            id(WorkflowId::parse, "workflow-1"),
            id(AssignmentId::parse, "assignment-1"),
            id(AssignmentScope::parse, "task:compile"),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            id(ContextReceiptId::parse, "context-1"),
            id(PolicyDecisionId::parse, "policy-1"),
            id(EffectId::parse, "effect-1"),
            id(IdempotencyKey::parse, "idem-1"),
            match DeadlineMilliseconds::parse(1_000) {
                Ok(value) => value,
                Err(error) => panic!("test deadline must parse: {error}"),
            },
        )
    }

    #[expect(
        clippy::implicit_return,
        reason = "test fixture construction is direct"
    )]
    fn observation(effect_id: &str) -> EffectObservation {
        EffectObservation::Succeeded {
            effect_id: id(EffectId::parse, effect_id),
            receipt_id: id(EffectReceiptId::parse, "receipt-1"),
        }
    }

    #[test]
    fn semantic_values_trim_and_reject_empty_or_control_text() {
        let session = id(SessionId::parse, "  session-1  ");
        assert_eq!(session.as_str(), "session-1");
        assert_eq!(SessionId::parse(" \t "), Err(HarnessError::EmptySessionId));
        assert_eq!(
            AgentId::parse("agent\n1"),
            Err(HarnessError::InvalidAgentId)
        );
        assert_eq!(
            AssignmentEpoch::parse(0),
            Err(HarnessError::InvalidAssignmentEpoch)
        );
        assert_eq!(
            AttemptNumber::parse(65),
            Err(HarnessError::InvalidAttemptNumber)
        );
        assert_eq!(
            DeadlineMilliseconds::parse(0),
            Err(HarnessError::InvalidDeadlineMilliseconds)
        );
    }

    #[test]
    fn session_id_is_compatible_with_its_workflow_stream() {
        let maximum = "s".repeat(MAX_SESSION_ID_CHARACTERS);
        assert_eq!(id(SessionId::parse, &maximum).as_str(), maximum);
        assert_eq!(
            format!("{WORKFLOW_STREAM_PREFIX}{maximum}").chars().count(),
            MAX_EVENTCORE_STREAM_ID_CHARACTERS
        );

        let over_limit = format!("{maximum}s");
        assert_eq!(
            SessionId::parse(&over_limit),
            Err(HarnessError::InvalidSessionId)
        );
        for glob_character in ["*", "?", "[", "]"] {
            assert_eq!(
                SessionId::parse(&format!("session{glob_character}id")),
                Err(HarnessError::InvalidSessionId)
            );
        }
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "a successful deserialization of invalid durable text is impossible test setup"
    )]
    fn semantic_text_accepts_its_byte_limit_and_rejects_larger_values() {
        let maximum = "x".repeat(MAX_SEMANTIC_TEXT_BYTES);
        assert_eq!(id(AgentId::parse, &maximum).as_str(), maximum);

        let over_limit = format!("{maximum}x");
        assert_eq!(
            AgentId::parse(&over_limit),
            Err(HarnessError::InvalidAgentId)
        );
        let serialized = format!("\"{over_limit}\"");
        let Err(deserialization_error) = serde_json::from_str::<AgentId>(&serialized) else {
            panic!("over-limit durable text must not deserialize");
        };
        assert!(
            deserialization_error
                .to_string()
                .contains(HarnessError::InvalidAgentId.code())
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "serialization assertions report impossible test setup failures"
    )]
    fn state_round_trips_through_json() {
        let state = HarnessState::new(effect());
        let json = match serde_json::to_string(&state) {
            Ok(json) => json,
            Err(error) => panic!("state must serialize: {error}"),
        };
        let restored: HarnessState = match serde_json::from_str(&json) {
            Ok(value) => value,
            Err(error) => panic!("state must deserialize: {error}"),
        };
        assert_eq!(restored, state);
    }

    #[test]
    fn ready_state_emits_the_complete_identity_envelope_once() {
        let initial = effect();
        let state = HarnessState::new(initial.clone());
        assert_eq!(
            step(&state, None),
            TrampolineStep::Continue {
                state: HarnessState {
                    initial_effect: initial.clone(),
                    phase: HarnessPhase::WaitingForInference,
                },
                effect: TiberEffect::Infer(initial),
            }
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        clippy::wildcard_enum_match_arm,
        reason = "test pattern failures must name unexpected trampoline results"
    )]
    fn matching_success_completes_and_terminal_state_does_not_mutate() {
        let TrampolineStep::Continue { state: waiting, .. } =
            step(&HarnessState::new(effect()), None)
        else {
            panic!("ready state must continue");
        };
        let complete = step(&waiting, Some(&observation("effect-1")));
        let completed_state = match complete {
            TrampolineStep::Complete { state, receipt } => {
                assert_eq!(receipt.as_str(), "receipt-1");
                state
            }
            _ => panic!("matching success must complete"),
        };
        assert_eq!(completed_state.phase(), HarnessPhase::Completed);
        assert_eq!(
            step(&completed_state, None),
            TrampolineStep::Stop {
                state: completed_state,
                error: HarnessError::TerminalState,
            }
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "the successor test isolates the two expected trampoline states before asserting the durable identity derivation"
    )]
    fn completed_workflow_mints_its_own_distinct_successor_effect() {
        let first = effect();
        let TrampolineStep::Continue { state: waiting, .. } =
            step(&HarnessState::new(first.clone()), None)
        else {
            panic!("ready state must continue");
        };
        let TrampolineStep::Complete {
            state: completed, ..
        } = step(&waiting, Some(&observation("effect-1")))
        else {
            panic!("observed state must complete");
        };

        let successor = continue_after_completion(&completed)
            .expect("completed workflow owns its continuation");

        assert_eq!(successor.phase(), HarnessPhase::Ready);
        assert_ne!(successor.initial_effect().effect_id(), first.effect_id());
        assert_ne!(
            successor.initial_effect().idempotency_key(),
            first.idempotency_key()
        );
        assert_eq!(
            successor.initial_effect().assignment_scope(),
            first.assignment_scope()
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        clippy::wildcard_enum_match_arm,
        reason = "test pattern failures must name unexpected trampoline results"
    )]
    fn failed_unknown_missing_and_mismatched_observations_stop_without_reemitting() {
        let TrampolineStep::Continue { state: waiting, .. } =
            step(&HarnessState::new(effect()), None)
        else {
            panic!("ready state must continue");
        };
        let cases = [
            (None, HarnessError::MissingObservation),
            (
                Some(observation("other-effect")),
                HarnessError::MismatchedObservation,
            ),
            (
                Some(EffectObservation::Failed {
                    effect_id: id(EffectId::parse, "effect-1"),
                    code: id(EffectFailureCode::parse, "remote_unavailable"),
                    retryability: Retryability::Retryable,
                }),
                HarnessError::EffectFailed,
            ),
            (
                Some(EffectObservation::OutcomeUnknown {
                    effect_id: id(EffectId::parse, "effect-1"),
                }),
                HarnessError::EffectOutcomeUnknown,
            ),
        ];
        for (observation, error) in cases {
            match step(&waiting, observation.as_ref()) {
                TrampolineStep::Stop {
                    state,
                    error: actual,
                } => {
                    assert_eq!(state.phase(), HarnessPhase::Stopped);
                    assert_eq!(actual, error);
                }
                _ => panic!("non-success waiting observation must stop"),
            }
        }
    }
}
