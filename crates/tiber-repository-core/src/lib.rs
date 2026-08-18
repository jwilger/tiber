//! Pure, assignment-bound authority contracts for repository mutations.

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeSet, string::String, vec::Vec};
use core::{error::Error, fmt, future::Future, pin::Pin};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest as _, Sha256};
use tiber_workflow_core::{
    AgentId, AssignmentEpoch, AssignmentId, AssignmentScope, AttemptNumber, ContextReceiptId,
    DeadlineMilliseconds, EffectId, IdempotencyKey, PolicyDecisionId, SessionId, WorkflowId,
};

/// Defines one repository-local canonical identity.
macro_rules! repository_identity {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        #[expect(
            clippy::implicit_return,
            reason = "canonical identity accessors use idiomatic tail expressions"
        )]
        impl $name {
            /// Returns this identity's canonical text.
            #[must_use]
            #[inline]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Parses one repository-local identity at the external boundary.
            ///
            /// # Errors
            ///
            /// Returns [`RepositoryError`] when the value is empty, oversized,
            /// control-bearing, or contains a delimiter outside the local identity grammar.
            #[inline]
            pub fn parse(value: &str) -> Result<Self, RepositoryError> {
                let canonical = value.trim();
                if canonical.is_empty() {
                    return Err(RepositoryError::EmptySemanticValue);
                }
                if canonical.len() > MAX_REPOSITORY_ID_BYTES
                    || !canonical.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '-' | '_' | '.')
                    })
                {
                    return Err(RepositoryError::InvalidSemanticValue);
                }
                Ok(Self(canonical.to_owned()))
            }
        }

        #[expect(
            clippy::implicit_return,
            clippy::missing_trait_methods,
            reason = "the semantic parser is the sole construction boundary; deserialize_in_place cannot preserve it"
        )]
        impl<'de> Deserialize<'de> for $name {
            #[inline]
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
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

/// Maximum UTF-8 byte length for a repository-local semantic identity.
pub const MAX_REPOSITORY_ID_BYTES: usize = 128;
/// Maximum UTF-8 byte length for one root-relative repository path.
pub const MAX_REPOSITORY_PATH_BYTES: usize = 1_024;
/// Maximum UTF-8 byte length for one path component.
pub const MAX_REPOSITORY_COMPONENT_BYTES: usize = 255;
/// Maximum byte length accepted for a write proposal's raw content.
pub const MAX_REPOSITORY_CONTENT_BYTES: usize = 64 * 1024;

/// Stable failures at the pure repository authority boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "callers must handle each closed repository authority failure explicitly; variants follow authorization flow"
)]
pub enum RepositoryError {
    /// A semantic identity was empty after canonical trimming.
    EmptySemanticValue,
    /// A semantic identity was malformed, oversized, or control-bearing.
    InvalidSemanticValue,
    /// A root-relative path was malformed or escaped its repository form.
    InvalidRepositoryPath,
    /// A proposed write payload exceeded the fixed bounded-content limit.
    ContentTooLarge,
    /// A SHA-256 digest was not a canonical lowercase hexadecimal value.
    InvalidDigest,
    /// A durable safe identity combined incompatible operation, digest, or precondition fields.
    InvalidMutationIdentity,
    /// A durable reconciliation outcome variant conflicted with its embedded receipt state.
    InvalidReconciliationOutcome,
    /// The proposal's complete workflow provenance differed from its assignment context.
    ProposalProvenanceMismatch,
    /// The trusted policy was issued for a different complete assignment context.
    PolicyAssignmentMismatch,
    /// The proposal targeted a different repository from its assignment context.
    RepositoryMismatch,
    /// The policy targeted a different repository from its assignment context.
    PolicyRepositoryMismatch,
    /// The proposal target was outside the component-aware assignment scope.
    AssignmentScopeMismatch,
    /// The policy component scope differed from the assignment scope.
    PolicyScopeMismatch,
    /// The trusted policy did not permit repository mutation.
    CapabilityDenied,
    /// A mutating repository effect lacked explicit owner approval.
    OwnerApprovalRequired,
    /// The supplied owner approval was bound to a different policy or assignment context.
    OwnerApprovalStale,
    /// The supplied owner approval was bound to a different safe proposal identity.
    OwnerApprovalMismatch,
}

#[expect(
    clippy::implicit_return,
    reason = "the stable repository error-code table is clearest as a total tail match"
)]
impl RepositoryError {
    /// Returns the stable machine-readable code for this failure.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EmptySemanticValue => "repository_empty_semantic_value",
            Self::InvalidSemanticValue => "repository_invalid_semantic_value",
            Self::InvalidRepositoryPath => "repository_invalid_path",
            Self::ContentTooLarge => "repository_content_too_large",
            Self::InvalidDigest => "repository_invalid_digest",
            Self::InvalidMutationIdentity => "repository_invalid_mutation_identity",
            Self::InvalidReconciliationOutcome => "repository_invalid_reconciliation_outcome",
            Self::ProposalProvenanceMismatch => "repository_proposal_provenance_mismatch",
            Self::PolicyAssignmentMismatch => "repository_policy_assignment_mismatch",
            Self::RepositoryMismatch => "repository_mismatch",
            Self::PolicyRepositoryMismatch => "repository_policy_repository_mismatch",
            Self::AssignmentScopeMismatch => "repository_assignment_scope_mismatch",
            Self::PolicyScopeMismatch => "repository_policy_scope_mismatch",
            Self::CapabilityDenied => "repository_capability_denied",
            Self::OwnerApprovalRequired => "repository_owner_approval_required",
            Self::OwnerApprovalStale => "repository_owner_approval_stale",
            Self::OwnerApprovalMismatch => "repository_owner_approval_mismatch",
        }
    }
}

impl fmt::Display for RepositoryError {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates directly to the stable error-code table"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "repository contract failures do not wrap a lower-level cause"
)]
impl Error for RepositoryError {}

/// The only safe next action after a closed repository failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "every repository retry classification must remain explicit"
)]
pub enum RepositoryRetryability {
    /// The consumed mutation must not replay; any later mutation needs fresh authorization.
    FreshAuthorizationRequired,
    /// The same ambiguity handle may be retried only through read-only reconciliation.
    ReadOnlyRetryable,
}

impl fmt::Display for RepositoryRetryability {
    #[inline]
    #[expect(
        clippy::pattern_type_mismatch,
        clippy::renamed_function_params,
        reason = "the Display boundary borrows the closed retryability value and names its formatter descriptively"
    )]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::FreshAuthorizationRequired => "fresh authorization required",
            Self::ReadOnlyRetryable => "read-only reconciliation required",
        })
    }
}

/// Stable definitive failure codes for a consumed repository mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "a mutation failure may represent only a conclusively non-applied dispatch state; variants follow dispatch observation order"
)]
pub enum RepositoryMutationFailureCode {
    /// The adapter refused the mutation before attempting repository application.
    PreDispatchRejected,
    /// The adapter proved the typed precondition did not hold before application.
    PreconditionNotMet,
    /// The adapter definitively proved the mutation did not apply.
    DefinitelyNotApplied,
}

#[expect(
    clippy::implicit_return,
    reason = "the definitive mutation-failure code table is clearest as a total tail match"
)]
impl RepositoryMutationFailureCode {
    /// Returns the stable machine-readable code for this definitive mutation failure.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::PreDispatchRejected => "repository_pre_dispatch_rejected",
            Self::PreconditionNotMet => "repository_precondition_not_met",
            Self::DefinitelyNotApplied => "repository_definitely_not_applied",
        }
    }

    /// Returns the only safe next action after this consumed mutation failure.
    #[must_use]
    #[inline]
    pub const fn retryability(self) -> RepositoryRetryability {
        RepositoryRetryability::FreshAuthorizationRequired
    }
}

impl fmt::Display for RepositoryMutationFailureCode {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates directly to the definitive mutation-failure code table"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the closed definitive mutation error has no lower-level source"
)]
impl Error for RepositoryMutationFailureCode {}

/// Stable errors from a read-only repository reconciliation query.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "read-only reconciliation failures remain distinct from mutation application failures"
)]
pub enum RepositoryReconciliationError {
    /// The adapter could not complete the read-only reconciliation query.
    ReadOnlyQueryFailed,
}

#[expect(
    clippy::implicit_return,
    reason = "the read-only reconciliation error-code table is clearest as a total tail match"
)]
impl RepositoryReconciliationError {
    /// Returns the stable machine-readable code for this read-only failure.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ReadOnlyQueryFailed => "repository_read_only_query_failed",
        }
    }

    /// Returns the only safe next action after a failed read-only reconciliation query.
    #[must_use]
    #[inline]
    pub const fn retryability(self) -> RepositoryRetryability {
        RepositoryRetryability::ReadOnlyRetryable
    }
}

impl fmt::Display for RepositoryReconciliationError {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates directly to the stable reconciliation error-code table"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the closed read-only reconciliation error has no lower-level source"
)]
impl Error for RepositoryReconciliationError {}

repository_identity!(
    OwnerApprovalId,
    "A validated durable identity for one explicit owner approval."
);
repository_identity!(
    RepositoryId,
    "A validated identity for the repository selected by an assignment."
);

/// A canonical root-relative file path with no traversal or metadata components.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RepositoryPath(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "path parsing and component accessors follow boundary use rather than alphabetical order"
)]
impl RepositoryPath {
    /// Parses a root-relative path without filesystem interpretation.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::InvalidRepositoryPath`] for empty, absolute,
    /// control-bearing, traversal, duplicate-separator, backslash, or `.git` paths.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, RepositoryError> {
        if value.is_empty()
            || value.len() > MAX_REPOSITORY_PATH_BYTES
            || value.starts_with('/')
            || value.starts_with('\\')
            || value.contains('\\')
            || value.chars().any(char::is_control)
        {
            return Err(RepositoryError::InvalidRepositoryPath);
        }

        for component in value.split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.eq_ignore_ascii_case(".git")
                || component.contains(':')
                || component.len() > MAX_REPOSITORY_COMPONENT_BYTES
            {
                return Err(RepositoryError::InvalidRepositoryPath);
            }
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical root-relative path.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "the root-relative parser is the sole deserialization boundary"
)]
impl<'de> Deserialize<'de> for RepositoryPath {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let decoded = String::deserialize(deserializer)?;
        match Self::parse(&decoded) {
            Ok(parsed) => Ok(parsed),
            Err(error) => Err(D::Error::custom(error)),
        }
    }
}

/// The repository-root or component subtree assigned to one agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComponentScope(Option<RepositoryPath>);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "component scope construction follows authority use rather than alphabetical order"
)]
impl ComponentScope {
    /// Creates a scope that contains the entire repository working tree.
    #[must_use]
    #[inline]
    pub const fn repository_root() -> Self {
        Self(None)
    }

    /// Parses one non-root component subtree.
    ///
    /// # Errors
    ///
    /// Returns the same stable path error as [`RepositoryPath::parse`] for an invalid scope path.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, RepositoryError> {
        match RepositoryPath::parse(value) {
            Ok(path) => Ok(Self(Some(path))),
            Err(error) => Err(error),
        }
    }

    /// Returns whether the supplied target is inside this scope on a component boundary.
    #[must_use]
    #[inline]
    pub fn contains(&self, target: &RepositoryPath) -> bool {
        let Some(scope) = self.0.as_ref() else {
            return true;
        };
        target.as_str() == scope.as_str()
            || target
                .as_str()
                .strip_prefix(scope.as_str())
                .is_some_and(|remainder| remainder.starts_with('/'))
    }

    /// Returns the non-root path when this scope is restricted to a component subtree.
    #[must_use]
    #[inline]
    pub fn path(&self) -> Option<&RepositoryPath> {
        self.0.as_ref()
    }
}

/// A canonical SHA-256 digest used as a mutation precondition or safe receipt value.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Sha256Digest([u8; 32]);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "digest construction follows boundary use rather than alphabetical order"
)]
impl Sha256Digest {
    /// Computes the SHA-256 digest of the supplied bytes.
    #[must_use]
    #[inline]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Parses a canonical lowercase hexadecimal SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::InvalidDigest`] when the input is not exactly 64 lowercase hexadecimal bytes.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, RepositoryError> {
        const NIBBLE_BITS: u32 = 4;

        let bytes = value.as_bytes();
        if bytes.len() != 64 {
            return Err(RepositoryError::InvalidDigest);
        }
        let mut decoded: [u8; 32] = [0; 32];
        for (slot, pair) in decoded.iter_mut().zip(bytes.chunks_exact(2)) {
            let &[high_byte, low_byte] = pair else {
                return Err(RepositoryError::InvalidDigest);
            };
            let Some(high) = hex_nibble(high_byte) else {
                return Err(RepositoryError::InvalidDigest);
            };
            let Some(low) = hex_nibble(low_byte) else {
                return Err(RepositoryError::InvalidDigest);
            };
            *slot = (high << NIBBLE_BITS) | low;
        }
        Ok(Self(decoded))
    }

    /// Returns the canonical lowercase hexadecimal digest text.
    #[must_use]
    #[inline]
    pub fn as_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        const NIBBLE_BITS: u32 = 4;

        let mut rendered = String::with_capacity(64);
        for byte in self.0 {
            let high = HEX
                .get(usize::from(byte >> NIBBLE_BITS))
                .copied()
                .unwrap_or(b'?');
            let low = HEX.get(usize::from(byte & 0x0f)).copied().unwrap_or(b'?');
            rendered.push(char::from(high));
            rendered.push(char::from(low));
        }
        rendered
    }
}

#[expect(
    clippy::implicit_return,
    reason = "the canonical serializer delegates directly to serde's string boundary"
)]
impl Serialize for Sha256Digest {
    /// Serializes a digest only as its canonical lowercase hexadecimal semantic value.
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_hex())
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "the canonical digest parser is the sole deserialization boundary"
)]
impl<'de> Deserialize<'de> for Sha256Digest {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let decoded = String::deserialize(deserializer)?;
        match Self::parse(&decoded) {
            Ok(parsed) => Ok(parsed),
            Err(error) => Err(D::Error::custom(error)),
        }
    }
}

/// Bounded raw file content that is deliberately neither serializable nor debug-formattable.
#[derive(Clone, Eq, PartialEq)]
pub struct RepositoryContent(Vec<u8>);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "bounded-content construction follows boundary use rather than alphabetical order"
)]
impl RepositoryContent {
    /// Parses bounded raw file content for one proposed write.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::ContentTooLarge`] when the content exceeds the fixed limit.
    #[inline]
    pub fn from_bytes(value: &[u8]) -> Result<Self, RepositoryError> {
        if value.len() > MAX_REPOSITORY_CONTENT_BYTES {
            return Err(RepositoryError::ContentTooLarge);
        }
        Ok(Self(value.to_vec()))
    }

    /// Returns the raw bytes for the one authorized adapter dispatch only.
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns a safe SHA-256 digest of the bounded content.
    #[must_use]
    #[inline]
    pub fn digest(&self) -> Sha256Digest {
        Sha256Digest::of(&self.0)
    }
}

/// The only repository mutation kinds admitted by this S1 core.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "new repository mutation kinds require an explicit authority and adapter decision; variants follow operation risk"
)]
pub enum RepositoryMutationKind {
    /// Replaces or creates one scoped file subject to a typed precondition.
    Write,
    /// Removes one scoped file subject to an exact current digest.
    Delete,
}

/// The only preconditions admitted for a write mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "write preconditions are closed to avoid generic compare-and-swap semantics"
)]
pub enum WritePrecondition {
    /// Requires that no file exists at the target path.
    Absent,
    /// Requires the target file's current bytes to match exactly.
    ExactDigest(Sha256Digest),
}

/// One untrusted write or delete proposal that cannot itself reach a repository adapter.
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "the two proposal variants remain in user-operation order and require exhaustive authority handling"
)]
pub enum RepositoryMutationProposalOperation {
    /// A bounded write plus its allowed typed precondition.
    Write {
        /// Raw bounded bytes deliberately kept out of diagnostics and receipts.
        content: RepositoryContent,
        /// Required absent-or-exact precondition.
        precondition: WritePrecondition,
    },
    /// A delete restricted to an exact existing-digest precondition.
    Delete {
        /// Required exact current content digest.
        precondition: Sha256Digest,
    },
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "the closed operation projections follow operation flow and borrow the raw-content variant safely"
)]
impl RepositoryMutationProposalOperation {
    /// Returns the closed mutation kind without exposing write content.
    #[must_use]
    #[inline]
    fn kind(&self) -> RepositoryMutationKind {
        match self {
            Self::Write { .. } => RepositoryMutationKind::Write,
            Self::Delete { .. } => RepositoryMutationKind::Delete,
        }
    }

    /// Returns the safe typed precondition without exposing write content.
    #[must_use]
    #[inline]
    fn precondition(&self) -> RepositoryMutationPrecondition {
        match self {
            Self::Write { precondition, .. } => {
                RepositoryMutationPrecondition::Write(*precondition)
            }
            Self::Delete { precondition } => RepositoryMutationPrecondition::Delete(*precondition),
        }
    }

    /// Returns a safe digest of write content when the operation writes bytes.
    #[must_use]
    #[inline]
    fn content_digest(&self) -> Option<Sha256Digest> {
        match self {
            Self::Write { content, .. } => Some(content.digest()),
            Self::Delete { .. } => None,
        }
    }
}

/// A safe typed precondition captured in an authorization, receipt, or reconciliation handle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "receipt consumers must distinguish write and delete compare-and-swap preconditions; variants follow operation flow"
)]
pub enum RepositoryMutationPrecondition {
    /// The write's absent-or-exact precondition.
    Write(WritePrecondition),
    /// The delete's required exact-digest precondition.
    Delete(Sha256Digest),
}

/// Full workflow provenance that must match before a repository mutation is authorized.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryMutationProvenance {
    /// Agent identity bound to the workflow assignment.
    agent_id: AgentId,
    /// Assignment epoch that scopes the attempt.
    assignment_epoch: AssignmentEpoch,
    /// Assignment identity owning this mutation.
    assignment_id: AssignmentId,
    /// Workflow-level assignment scope identity.
    assignment_scope: AssignmentScope,
    /// One-based attempt number under the assignment epoch.
    attempt_number: AttemptNumber,
    /// Trusted context receipt used for authorization.
    context_receipt_id: ContextReceiptId,
    /// Immutable deadline supplied to the adapter boundary.
    deadline_milliseconds: DeadlineMilliseconds,
    /// Durable effect identity for the mutation.
    effect_id: EffectId,
    /// Stable deduplication identity for the mutation.
    idempotency_key: IdempotencyKey,
    /// Trusted policy decision identity.
    policy_decision_id: PolicyDecisionId,
    /// Durable workflow session identity.
    session_id: SessionId,
    /// Workflow continuation identity.
    workflow_id: WorkflowId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::too_many_arguments,
    reason = "the complete immutable repository provenance must be supplied at one authority boundary"
)]
impl RepositoryMutationProvenance {
    /// Creates the complete immutable provenance required for one repository mutation.
    #[must_use]
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

    /// Returns the agent that owns this repository assignment.
    #[must_use]
    #[inline]
    pub const fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    /// Returns the assignment epoch selected by the policy decision.
    #[must_use]
    #[inline]
    pub const fn assignment_epoch(&self) -> AssignmentEpoch {
        self.assignment_epoch
    }

    /// Returns the assignment owning this mutation.
    #[must_use]
    #[inline]
    pub const fn assignment_id(&self) -> &AssignmentId {
        &self.assignment_id
    }

    /// Returns the workflow-assignment scope identity.
    #[must_use]
    #[inline]
    pub const fn assignment_scope(&self) -> &AssignmentScope {
        &self.assignment_scope
    }

    /// Returns the one-based attempt number under this assignment epoch.
    #[must_use]
    #[inline]
    pub const fn attempt_number(&self) -> AttemptNumber {
        self.attempt_number
    }

    /// Returns the immutable authoritative-context receipt identity.
    #[must_use]
    #[inline]
    pub const fn context_receipt_id(&self) -> &ContextReceiptId {
        &self.context_receipt_id
    }

    /// Returns the maximum effect deadline for the adapter shell.
    #[must_use]
    #[inline]
    pub const fn deadline_milliseconds(&self) -> DeadlineMilliseconds {
        self.deadline_milliseconds
    }

    /// Returns the durable effect identity.
    #[must_use]
    #[inline]
    pub const fn effect_id(&self) -> &EffectId {
        &self.effect_id
    }

    /// Returns the stable deduplication identity for this mutation.
    #[must_use]
    #[inline]
    pub const fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// Returns the policy decision authorizing this effect.
    #[must_use]
    #[inline]
    pub const fn policy_decision_id(&self) -> &PolicyDecisionId {
        &self.policy_decision_id
    }

    /// Returns the durable session that owns the workflow.
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

/// Trusted assignment context that owns one repository subtree and full workflow provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryAssignmentContext {
    /// Component-aware repository subtree owned by the assignment.
    component_scope: ComponentScope,
    /// Complete trusted workflow provenance.
    provenance: RepositoryMutationProvenance,
    /// Exact repository owned by this assignment.
    repository_id: RepositoryId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "assignment construction and accessors are grouped by authority lifecycle rather than alphabetically"
)]
impl RepositoryAssignmentContext {
    /// Creates the complete trusted assignment boundary for a repository mutation.
    #[must_use]
    #[inline]
    pub fn new(
        provenance: RepositoryMutationProvenance,
        repository_id: RepositoryId,
        component_scope: ComponentScope,
    ) -> Self {
        Self {
            component_scope,
            provenance,
            repository_id,
        }
    }

    /// Returns the component-aware subtree assigned to the mutation.
    #[must_use]
    #[inline]
    pub const fn component_scope(&self) -> &ComponentScope {
        &self.component_scope
    }

    /// Returns the complete immutable workflow provenance.
    #[must_use]
    #[inline]
    pub const fn provenance(&self) -> &RepositoryMutationProvenance {
        &self.provenance
    }

    /// Returns the repository selected by this assignment.
    #[must_use]
    #[inline]
    pub const fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }
}

/// Closed capability vocabulary for this repository-effect core.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "a new repository capability must receive an explicit authority decision"
)]
pub enum RepositoryCapability {
    /// Allows the narrowly typed write/delete repository mutation vocabulary.
    MutateRepository,
}

/// Trusted policy issued for one exact assignment-bound repository context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryMutationPolicy {
    /// Exact trusted assignment context to which the policy is bound.
    assignment: RepositoryAssignmentContext,
    /// Closed capabilities issued by the trusted policy.
    capabilities: BTreeSet<RepositoryCapability>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "policy construction and capability inspection follow the authorization lifecycle"
)]
impl RepositoryMutationPolicy {
    /// Creates a deny-by-default policy bound to one full assignment context.
    #[must_use]
    #[inline]
    pub fn new<Capabilities>(
        assignment: RepositoryAssignmentContext,
        capabilities: Capabilities,
    ) -> Self
    where
        Capabilities: IntoIterator<Item = RepositoryCapability>,
    {
        Self {
            assignment,
            capabilities: capabilities.into_iter().collect(),
        }
    }

    /// Returns the exact assignment context to which this policy is bound.
    #[must_use]
    #[inline]
    pub const fn assignment(&self) -> &RepositoryAssignmentContext {
        &self.assignment
    }

    /// Returns whether the policy allows the named repository capability.
    #[must_use]
    #[inline]
    pub fn permits(&self, capability: RepositoryCapability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// One raw model or caller proposal lacking repository authority.
pub struct RepositoryMutationProposal {
    /// Narrow write-or-delete request that remains untrusted until authorization.
    operation: RepositoryMutationProposalOperation,
    /// Root-relative target selected by the proposer.
    path: RepositoryPath,
    /// Full workflow provenance supplied by the proposer.
    provenance: RepositoryMutationProvenance,
    /// Repository selected by the proposer.
    repository_id: RepositoryId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "write and delete constructors follow the narrowly admitted operation vocabulary"
)]
impl RepositoryMutationProposal {
    /// Creates one raw write proposal that still requires explicit authorization.
    #[must_use]
    #[inline]
    pub fn write(
        provenance: RepositoryMutationProvenance,
        repository_id: RepositoryId,
        path: RepositoryPath,
        content: RepositoryContent,
        precondition: WritePrecondition,
    ) -> Self {
        Self {
            operation: RepositoryMutationProposalOperation::Write {
                content,
                precondition,
            },
            path,
            provenance,
            repository_id,
        }
    }

    /// Creates one raw delete proposal that still requires explicit authorization.
    #[must_use]
    #[inline]
    pub fn delete(
        provenance: RepositoryMutationProvenance,
        repository_id: RepositoryId,
        path: RepositoryPath,
        precondition: Sha256Digest,
    ) -> Self {
        Self {
            operation: RepositoryMutationProposalOperation::Delete { precondition },
            path,
            provenance,
            repository_id,
        }
    }

    /// Returns the complete approval-safe identity of this raw proposal without exposing content.
    #[must_use]
    #[inline]
    pub fn identity(&self) -> RepositoryMutationProposalIdentity {
        RepositoryMutationProposalIdentity {
            content_digest: self.operation.content_digest(),
            kind: self.operation.kind(),
            path: self.path.clone(),
            precondition: self.operation.precondition(),
            provenance: self.provenance.clone(),
            repository_id: self.repository_id.clone(),
        }
    }
}

/// Safe proposal identity that an owner approval binds without retaining raw write content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryMutationProposalIdentity {
    /// Digest of write bytes, omitted for delete proposals.
    content_digest: Option<Sha256Digest>,
    /// Closed operation kind proposed for authorization.
    kind: RepositoryMutationKind,
    /// Exact root-relative target selected by the proposal.
    path: RepositoryPath,
    /// Typed condition the adapter must verify before application.
    precondition: RepositoryMutationPrecondition,
    /// Complete workflow provenance supplied by the proposal.
    provenance: RepositoryMutationProvenance,
    /// Exact repository selected by the proposal.
    repository_id: RepositoryId,
}

impl RepositoryMutationProposalIdentity {
    /// Returns the exact root-relative target selected by the proposal.
    #[must_use]
    #[inline]
    pub const fn path(&self) -> &RepositoryPath {
        &self.path
    }

    /// Returns the exact typed precondition selected by the proposal.
    #[must_use]
    #[inline]
    pub const fn precondition(&self) -> RepositoryMutationPrecondition {
        self.precondition
    }

    /// Returns the complete workflow provenance carried by this safe proposal identity.
    #[must_use]
    #[inline]
    pub const fn provenance(&self) -> &RepositoryMutationProvenance {
        &self.provenance
    }
}

/// Opaque explicit owner approval bound to one safe proposal and policy context.
///
/// This S1 value deliberately omits `Clone`, `Debug`, and serde traits: it is a consumed
/// authority input, not a durable receipt or a raw-content container.
pub struct RepositoryMutationApproval {
    /// Durable identity for the explicit owner decision.
    approval_id: OwnerApprovalId,
    /// Complete trusted policy context, including its assignment context and component scope.
    policy: RepositoryMutationPolicy,
    /// Safe identity of the exact proposal the owner approved.
    proposal: RepositoryMutationProposalIdentity,
}

#[expect(
    clippy::implicit_return,
    reason = "owner issuance binds proposal and policy together before later authorization consumes it"
)]
impl RepositoryMutationApproval {
    /// Issues one owner approval bound to the exact safe proposal identity and trusted policy.
    #[must_use]
    #[inline]
    pub fn issue(
        approval_id: OwnerApprovalId,
        proposal: &RepositoryMutationProposal,
        policy: &RepositoryMutationPolicy,
    ) -> Self {
        Self {
            approval_id,
            policy: policy.clone(),
            proposal: proposal.identity(),
        }
    }
}

/// Safe identity retained after a mutation authorization is consumed by an adapter outcome.
///
/// It retains full trusted provenance, target, operation, approval, and content digest, but never raw write bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RepositoryMutationIdentity {
    /// Digest of write bytes, omitted for deletes.
    content_digest: Option<Sha256Digest>,
    /// Closed mutation kind.
    kind: RepositoryMutationKind,
    /// Explicit owner approval required for a mutation.
    owner_approval: OwnerApprovalId,
    /// Exact root-relative target.
    path: RepositoryPath,
    /// Typed condition that the adapter must verify.
    precondition: RepositoryMutationPrecondition,
    /// Full provenance checked before authorization.
    provenance: RepositoryMutationProvenance,
    /// Exact assigned repository.
    repository_id: RepositoryId,
}

#[expect(
    clippy::implicit_return,
    reason = "safe mutation-identity accessors use idiomatic tail expressions"
)]
impl RepositoryMutationIdentity {
    /// Returns the safe write-content digest, if the operation writes a file.
    #[must_use]
    #[inline]
    pub const fn content_digest(&self) -> Option<Sha256Digest> {
        self.content_digest
    }

    /// Returns the operation kind fixed by authorization.
    #[must_use]
    #[inline]
    pub const fn kind(&self) -> RepositoryMutationKind {
        self.kind
    }

    /// Returns whether this authorized identity retains the exact safe proposal identity.
    #[must_use]
    #[inline]
    pub fn matches_proposal(&self, proposal: &RepositoryMutationProposalIdentity) -> bool {
        self.content_digest == proposal.content_digest
            && self.kind == proposal.kind
            && self.path == proposal.path
            && self.precondition == proposal.precondition
            && self.provenance == proposal.provenance
            && self.repository_id == proposal.repository_id
    }

    /// Returns the explicit owner approval identity bound to the mutation.
    #[must_use]
    #[inline]
    pub const fn owner_approval(&self) -> &OwnerApprovalId {
        &self.owner_approval
    }

    /// Returns the exact root-relative target selected by authorization.
    #[must_use]
    #[inline]
    pub const fn path(&self) -> &RepositoryPath {
        &self.path
    }

    /// Returns the typed precondition that an adapter must verify before dispatch.
    #[must_use]
    #[inline]
    pub const fn precondition(&self) -> RepositoryMutationPrecondition {
        self.precondition
    }

    /// Returns the complete workflow provenance that was checked before authorization.
    #[must_use]
    #[inline]
    pub const fn provenance(&self) -> &RepositoryMutationProvenance {
        &self.provenance
    }

    /// Returns the exact assignment-bound repository selected by authorization.
    #[must_use]
    #[inline]
    pub const fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }
}

/// Deserialize-only wire representation validated before becoming a safe identity.
#[derive(Deserialize)]
struct RepositoryMutationIdentityWire {
    /// Digest of raw write content when a write is represented.
    content_digest: Option<Sha256Digest>,
    /// Closed operation kind represented by the durable identity.
    kind: RepositoryMutationKind,
    /// Explicit owner approval bound to the mutation.
    owner_approval: OwnerApprovalId,
    /// Exact root-relative mutation target.
    path: RepositoryPath,
    /// Typed mutation precondition.
    precondition: RepositoryMutationPrecondition,
    /// Complete checked workflow provenance.
    provenance: RepositoryMutationProvenance,
    /// Exact assigned repository identity.
    repository_id: RepositoryId,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "durable identities must reject incompatible operation, content-digest, and precondition fields"
)]
impl<'de> Deserialize<'de> for RepositoryMutationIdentity {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let decoded = RepositoryMutationIdentityWire::deserialize(deserializer)?;
        let RepositoryMutationIdentityWire {
            content_digest,
            kind,
            owner_approval,
            path,
            precondition,
            provenance,
            repository_id,
        } = decoded;
        match (kind, content_digest, precondition) {
            (
                RepositoryMutationKind::Write,
                Some(write_content_digest),
                RepositoryMutationPrecondition::Write(write_precondition),
            ) => Ok(Self {
                content_digest: Some(write_content_digest),
                kind: RepositoryMutationKind::Write,
                owner_approval,
                path,
                precondition: RepositoryMutationPrecondition::Write(write_precondition),
                provenance,
                repository_id,
            }),
            (
                RepositoryMutationKind::Delete,
                None,
                RepositoryMutationPrecondition::Delete(delete_precondition),
            ) => Ok(Self {
                content_digest: None,
                kind: RepositoryMutationKind::Delete,
                owner_approval,
                path,
                precondition: RepositoryMutationPrecondition::Delete(delete_precondition),
                provenance,
                repository_id,
            }),
            _ => Err(D::Error::custom(RepositoryError::InvalidMutationIdentity)),
        }
    }
}

/// The only adapter-visible operation view for an opaque repository mutation token.
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "adapters must handle the complete closed write/delete vocabulary explicitly; variants follow dispatch flow"
)]
pub enum AuthorizedRepositoryOperation<'mutation> {
    /// Writes these bounded bytes only after checking the exact typed precondition.
    Write {
        /// Bounded raw bytes required by the one authorized adapter dispatch.
        content: &'mutation RepositoryContent,
        /// Exact root-relative target path.
        path: &'mutation RepositoryPath,
        /// Required absent-or-exact precondition.
        precondition: WritePrecondition,
    },
    /// Deletes exactly one file only after checking its current digest.
    Delete {
        /// Exact root-relative target path.
        path: &'mutation RepositoryPath,
        /// Required exact current digest.
        precondition: Sha256Digest,
    },
}

/// Opaque mutation authority minted only after every trusted policy fence passes.
///
/// This value intentionally omits `Debug` and `Serialize`, because it retains raw write content for one adapter dispatch.
pub struct AuthorizedRepositoryMutation {
    /// Explicit approval bound to the opaque one-shot authority.
    owner_approval: OwnerApprovalId,
    /// Original raw proposal retained only for the one adapter dispatch.
    proposal: RepositoryMutationProposal,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "opaque mutation accessors and consuming bindings use direct lifecycle expressions"
)]
impl AuthorizedRepositoryMutation {
    /// Returns the full safe identity that remains after this authorization is consumed.
    #[must_use]
    #[inline]
    pub fn identity(&self) -> RepositoryMutationIdentity {
        RepositoryMutationIdentity {
            content_digest: self.proposal.operation.content_digest(),
            kind: self.proposal.operation.kind(),
            owner_approval: self.owner_approval.clone(),
            path: self.proposal.path.clone(),
            precondition: self.proposal.operation.precondition(),
            provenance: self.proposal.provenance.clone(),
            repository_id: self.proposal.repository_id.clone(),
        }
    }

    /// Returns the only raw operation view an adapter may dispatch.
    #[must_use]
    #[inline]
    pub fn operation(&self) -> AuthorizedRepositoryOperation<'_> {
        match &self.proposal.operation {
            RepositoryMutationProposalOperation::Write {
                content,
                precondition,
            } => AuthorizedRepositoryOperation::Write {
                content,
                path: &self.proposal.path,
                precondition: *precondition,
            },
            RepositoryMutationProposalOperation::Delete { precondition } => {
                AuthorizedRepositoryOperation::Delete {
                    path: &self.proposal.path,
                    precondition: *precondition,
                }
            }
        }
    }

    /// Consumes successful dispatch authority into a safe applied receipt.
    #[must_use]
    #[inline]
    pub fn into_applied_receipt(self) -> RepositoryMutationReceipt {
        RepositoryMutationReceipt {
            identity: self.identity(),
        }
    }

    /// Consumes dispatch authority into its exact read-only reconciliation handle.
    ///
    /// The resulting handle cannot recreate or replay this mutation authorization.
    #[must_use]
    #[inline]
    pub fn into_ambiguity(self) -> RepositoryDispatchOutcome {
        RepositoryDispatchOutcome::OutcomeUnknown(RepositoryReconciliation {
            identity: self.identity(),
        })
    }

    /// Consumes dispatch authority into a safe failure transcript without replay authority.
    #[must_use]
    #[inline]
    pub fn into_failure(self, error: RepositoryMutationFailureCode) -> RepositoryMutationFailure {
        RepositoryMutationFailure {
            error,
            identity: self.identity(),
        }
    }
}

/// A safe durable receipt that an adapter observed one authorized mutation as applied.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryMutationReceipt {
    /// Safe identity retained after applied dispatch.
    identity: RepositoryMutationIdentity,
}

#[expect(
    clippy::implicit_return,
    reason = "the applied receipt has one direct safe identity accessor"
)]
impl RepositoryMutationReceipt {
    /// Returns the safe authorization identity for the applied mutation.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &RepositoryMutationIdentity {
        &self.identity
    }
}

/// A safe terminal adapter failure for one consumed mutation authorization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryMutationFailure {
    /// Closed definitive no-application error reported by the adapter.
    error: RepositoryMutationFailureCode,
    /// Safe identity of the consumed request.
    identity: RepositoryMutationIdentity,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "failure accessors are grouped by operational inspection rather than alphabetically"
)]
impl RepositoryMutationFailure {
    /// Returns the stable definitive mutation failure code.
    #[must_use]
    #[inline]
    pub const fn error(&self) -> RepositoryMutationFailureCode {
        self.error
    }

    /// Returns the no-replay directive supplied by the closed mutation failure vocabulary.
    #[must_use]
    #[inline]
    pub const fn retryability(&self) -> RepositoryRetryability {
        self.error.retryability()
    }

    /// Returns safe identity without raw write content or replay authority.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &RepositoryMutationIdentity {
        &self.identity
    }
}

/// A post-dispatch ambiguity that may only be queried through read-only reconciliation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryReconciliation {
    /// Exact safe request identity retained after ambiguity.
    identity: RepositoryMutationIdentity,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the read-only reconciliation handle exposes only safe identity and closed outcome binding"
)]
impl RepositoryReconciliation {
    /// Restores a read-only reconciliation handle from one validated durable identity.
    ///
    /// This never recreates mutation authority or raw write content.
    #[must_use]
    #[inline]
    pub fn from_durable_identity(identity: RepositoryMutationIdentity) -> Self {
        Self { identity }
    }

    /// Returns the exact safe identity derived from the consumed mutation request.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &RepositoryMutationIdentity {
        &self.identity
    }

    /// Binds a read-only reconciliation state to this exact request-derived handle.
    #[must_use]
    #[inline]
    pub fn bind_outcome(
        &self,
        state: RepositoryReconciliationState,
    ) -> RepositoryReconciliationOutcome {
        let receipt = RepositoryReconciliationReceipt {
            identity: self.identity.clone(),
            state,
        };
        match state {
            RepositoryReconciliationState::Applied => {
                RepositoryReconciliationOutcome::Applied(receipt)
            }
            RepositoryReconciliationState::NotApplied => {
                RepositoryReconciliationOutcome::NotApplied(receipt)
            }
            RepositoryReconciliationState::StillUnknown => {
                RepositoryReconciliationOutcome::StillUnknown(receipt)
            }
        }
    }

    /// Binds a safe read-only reconciliation failure without recreating dispatch authority.
    #[must_use]
    #[inline]
    pub fn bind_failure(
        &self,
        error: RepositoryReconciliationError,
    ) -> RepositoryReconciliationFailure {
        RepositoryReconciliationFailure {
            error,
            identity: self.identity.clone(),
        }
    }
}

/// Closed read-only states for an ambiguous repository mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "all ambiguity outcomes must remain explicit and cannot imply automatic replay"
)]
pub enum RepositoryReconciliationState {
    /// The adapter proved the original mutation applied.
    Applied,
    /// The adapter proved the original mutation did not apply.
    NotApplied,
    /// The adapter could not establish either terminal state.
    StillUnknown,
}

/// Safe receipt from a read-only reconciliation query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryReconciliationReceipt {
    /// Safe identity of the original ambiguous request.
    identity: RepositoryMutationIdentity,
    /// Closed state established by a read-only query.
    state: RepositoryReconciliationState,
}

#[expect(
    clippy::implicit_return,
    reason = "the reconciliation receipt has direct safe inspection accessors"
)]
impl RepositoryReconciliationReceipt {
    /// Returns the safe original mutation identity being reconciled.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &RepositoryMutationIdentity {
        &self.identity
    }

    /// Returns the closed read-only reconciliation state.
    #[must_use]
    #[inline]
    pub const fn state(&self) -> RepositoryReconciliationState {
        self.state
    }
}

/// Closed result of a read-only reconciliation query.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must explicitly handle every terminal or still-ambiguous reconciliation result"
)]
pub enum RepositoryReconciliationOutcome {
    /// The original mutation applied.
    Applied(RepositoryReconciliationReceipt),
    /// The original mutation did not apply; this does not mint replay authority.
    NotApplied(RepositoryReconciliationReceipt),
    /// The original mutation remains unknown; this does not mint replay authority.
    StillUnknown(RepositoryReconciliationReceipt),
}

/// Deserialize-only wire representation validated against its embedded receipt state.
#[derive(Deserialize)]
enum RepositoryReconciliationOutcomeWire {
    /// Wire form of an applied reconciliation outcome.
    Applied(RepositoryReconciliationReceipt),
    /// Wire form of a not-applied reconciliation outcome.
    NotApplied(RepositoryReconciliationReceipt),
    /// Wire form of a still-unknown reconciliation outcome.
    StillUnknown(RepositoryReconciliationReceipt),
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "durable reconciliation outcomes must agree with their receipt's closed state"
)]
impl<'de> Deserialize<'de> for RepositoryReconciliationOutcome {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let decoded = RepositoryReconciliationOutcomeWire::deserialize(deserializer)?;
        match decoded {
            RepositoryReconciliationOutcomeWire::Applied(receipt)
                if receipt.state() == RepositoryReconciliationState::Applied =>
            {
                Ok(Self::Applied(receipt))
            }
            RepositoryReconciliationOutcomeWire::NotApplied(receipt)
                if receipt.state() == RepositoryReconciliationState::NotApplied =>
            {
                Ok(Self::NotApplied(receipt))
            }
            RepositoryReconciliationOutcomeWire::StillUnknown(receipt)
                if receipt.state() == RepositoryReconciliationState::StillUnknown =>
            {
                Ok(Self::StillUnknown(receipt))
            }
            RepositoryReconciliationOutcomeWire::Applied(_)
            | RepositoryReconciliationOutcomeWire::NotApplied(_)
            | RepositoryReconciliationOutcomeWire::StillUnknown(_) => Err(D::Error::custom(
                RepositoryError::InvalidReconciliationOutcome,
            )),
        }
    }
}

/// Safe failure transcript for a read-only reconciliation query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryReconciliationFailure {
    /// Closed read-only reconciliation error.
    error: RepositoryReconciliationError,
    /// Safe identity of the ambiguous request.
    identity: RepositoryMutationIdentity,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "reconciliation failure accessors follow operational inspection rather than alphabetical order"
)]
impl RepositoryReconciliationFailure {
    /// Returns the stable read-only reconciliation failure code.
    #[must_use]
    #[inline]
    pub const fn error(&self) -> RepositoryReconciliationError {
        self.error
    }

    /// Returns the read-only retry directive for this reconciliation failure.
    #[must_use]
    #[inline]
    pub const fn retryability(&self) -> RepositoryRetryability {
        self.error.retryability()
    }

    /// Returns the safe mutation identity being reconciled.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &RepositoryMutationIdentity {
        &self.identity
    }
}

/// Closed result of dispatching exactly one authorized repository mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "adapter callers must distinguish observed application from ambiguity before any later policy decision"
)]
pub enum RepositoryDispatchOutcome {
    /// The adapter observed the mutation as applied.
    Applied(RepositoryMutationReceipt),
    /// The adapter could not determine the result and exposes only reconciliation.
    OutcomeUnknown(RepositoryReconciliation),
}

/// Runtime-neutral future returned by an object-safe repository service port.
pub type RepositoryServiceFuture<'service, Output> =
    Pin<Box<dyn Future<Output = Output> + Send + 'service>>;

/// Imperative repository port that accepts only opaque authorized mutations and read-only reconciliation handles.
pub trait RepositoryService: Send + Sync {
    /// Dispatches exactly one authorized mutation under its immutable provenance deadline.
    fn dispatch(
        &self,
        mutation: AuthorizedRepositoryMutation,
    ) -> RepositoryServiceFuture<'_, Result<RepositoryDispatchOutcome, RepositoryMutationFailure>>;

    /// Performs read-only reconciliation for an exact ambiguity-derived handle without replaying the mutation.
    fn reconcile(
        &self,
        reconciliation: RepositoryReconciliation,
    ) -> RepositoryServiceFuture<
        '_,
        Result<RepositoryReconciliationOutcome, RepositoryReconciliationFailure>,
    >;
}

/// Decodes one canonical lowercase hexadecimal nibble.
#[expect(
    clippy::implicit_return,
    reason = "the total canonical nibble lookup is clearest as a direct match"
)]
#[inline]
fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0' => Some(0),
        b'1' => Some(1),
        b'2' => Some(2),
        b'3' => Some(3),
        b'4' => Some(4),
        b'5' => Some(5),
        b'6' => Some(6),
        b'7' => Some(7),
        b'8' => Some(8),
        b'9' => Some(9),
        b'a' => Some(10),
        b'b' => Some(11),
        b'c' => Some(12),
        b'd' => Some(13),
        b'e' => Some(14),
        b'f' => Some(15),
        _ => None,
    }
}

/// Authorizes a raw proposal into an opaque repository mutation token.
///
/// # Errors
///
/// Returns a stable denial before any adapter can observe, mutate, or reconcile a repository.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the ordered authority checks preserve exact stable denial precedence"
)]
pub fn authorize_mutation(
    proposal: RepositoryMutationProposal,
    assignment: &RepositoryAssignmentContext,
    policy: &RepositoryMutationPolicy,
    owner_approval: Option<RepositoryMutationApproval>,
) -> Result<AuthorizedRepositoryMutation, RepositoryError> {
    if proposal.provenance != assignment.provenance {
        return Err(RepositoryError::ProposalProvenanceMismatch);
    }
    if proposal.repository_id != assignment.repository_id {
        return Err(RepositoryError::RepositoryMismatch);
    }
    if policy.assignment.provenance != assignment.provenance {
        return Err(RepositoryError::PolicyAssignmentMismatch);
    }
    if policy.assignment.repository_id != assignment.repository_id {
        return Err(RepositoryError::PolicyRepositoryMismatch);
    }
    if policy.assignment.component_scope != assignment.component_scope {
        return Err(RepositoryError::PolicyScopeMismatch);
    }
    if !assignment.component_scope.contains(&proposal.path) {
        return Err(RepositoryError::AssignmentScopeMismatch);
    }
    if !policy.permits(RepositoryCapability::MutateRepository) {
        return Err(RepositoryError::CapabilityDenied);
    }
    let Some(approval) = owner_approval else {
        return Err(RepositoryError::OwnerApprovalRequired);
    };
    if approval.policy != *policy {
        return Err(RepositoryError::OwnerApprovalStale);
    }
    if approval.proposal != proposal.identity() {
        return Err(RepositoryError::OwnerApprovalMismatch);
    }
    Ok(AuthorizedRepositoryMutation {
        owner_approval: approval.approval_id,
        proposal,
    })
}

/// Derives the safe identity that may be durably prepared without minting adapter authority.
///
/// # Errors
///
/// Returns the same stable policy denial as [`authorize_mutation`] before any
/// raw proposal can become adapter-visible.
#[inline]
pub fn prepare_mutation_identity(
    proposal: &RepositoryMutationProposal,
    assignment: &RepositoryAssignmentContext,
    policy: &RepositoryMutationPolicy,
    approval_id: OwnerApprovalId,
) -> Result<RepositoryMutationIdentity, RepositoryError> {
    validate_mutation_policy(proposal, assignment, policy)?;
    Ok(RepositoryMutationIdentity {
        content_digest: proposal.operation.content_digest(),
        kind: proposal.operation.kind(),
        owner_approval: approval_id,
        path: proposal.path.clone(),
        precondition: proposal.operation.precondition(),
        provenance: proposal.provenance.clone(),
        repository_id: proposal.repository_id.clone(),
    })
}

/// Consumes a raw proposal into adapter authority only after an exact prepared
/// identity has been reloaded from durable signed history.
///
/// # Errors
///
/// Returns a stable denial when current policy no longer authorizes the proposal
/// or the durable prepared identity differs from the exact proposed operation.
#[inline]
pub fn authorize_prepared_mutation(
    proposal: RepositoryMutationProposal,
    assignment: &RepositoryAssignmentContext,
    policy: &RepositoryMutationPolicy,
    prepared: &RepositoryMutationIdentity,
) -> Result<AuthorizedRepositoryMutation, RepositoryError> {
    let expected = prepare_mutation_identity(
        &proposal,
        assignment,
        policy,
        prepared.owner_approval().clone(),
    )?;
    if expected != *prepared {
        return Err(RepositoryError::OwnerApprovalMismatch);
    }
    Ok(AuthorizedRepositoryMutation {
        owner_approval: prepared.owner_approval().clone(),
        proposal,
    })
}

/// Validates proposal provenance, scope, and capability before authorization.
#[expect(
    clippy::single_call_fn,
    reason = "the named policy-validation boundary keeps authorization checks closed and auditable"
)]
#[inline]
fn validate_mutation_policy(
    proposal: &RepositoryMutationProposal,
    assignment: &RepositoryAssignmentContext,
    policy: &RepositoryMutationPolicy,
) -> Result<(), RepositoryError> {
    if proposal.provenance != assignment.provenance {
        return Err(RepositoryError::ProposalProvenanceMismatch);
    }
    if proposal.repository_id != assignment.repository_id {
        return Err(RepositoryError::RepositoryMismatch);
    }
    if policy.assignment.provenance != assignment.provenance {
        return Err(RepositoryError::PolicyAssignmentMismatch);
    }
    if policy.assignment.repository_id != assignment.repository_id {
        return Err(RepositoryError::PolicyRepositoryMismatch);
    }
    if policy.assignment.component_scope != assignment.component_scope {
        return Err(RepositoryError::PolicyScopeMismatch);
    }
    if !assignment.component_scope.contains(&proposal.path) {
        return Err(RepositoryError::AssignmentScopeMismatch);
    }
    if !policy.permits(RepositoryCapability::MutateRepository) {
        return Err(RepositoryError::CapabilityDenied);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::{
        future::ready,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use serde::de::DeserializeOwned;
    use tiber_workflow_core::HarnessError;

    #[derive(Default)]
    struct CountingRepositoryService {
        /// Number of dispatch calls accepted by the fake adapter.
        dispatches: Arc<AtomicUsize>,
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fake adapter's counter has one direct observation accessor"
    )]
    impl CountingRepositoryService {
        #[must_use]
        fn dispatch_count(&self) -> usize {
            self.dispatches.load(Ordering::Acquire)
        }
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fake adapter returns ready futures without introducing runtime behavior"
    )]
    impl RepositoryService for CountingRepositoryService {
        fn dispatch(
            &self,
            mutation: AuthorizedRepositoryMutation,
        ) -> RepositoryServiceFuture<'_, Result<RepositoryDispatchOutcome, RepositoryMutationFailure>>
        {
            self.dispatches.fetch_add(1, Ordering::AcqRel);
            Box::pin(ready(Ok(RepositoryDispatchOutcome::Applied(
                mutation.into_applied_receipt(),
            ))))
        }

        fn reconcile(
            &self,
            reconciliation: RepositoryReconciliation,
        ) -> RepositoryServiceFuture<
            '_,
            Result<RepositoryReconciliationOutcome, RepositoryReconciliationFailure>,
        > {
            Box::pin(ready(Ok(
                reconciliation.bind_outcome(RepositoryReconciliationState::StillUnknown)
            )))
        }
    }

    #[test]
    fn owner_approved_scoped_write_mints_an_opaque_mutation_token() {
        let (proposal, assignment, policy, approval) = valid_write_request();

        let authorized = authorize_with_approval(proposal, &assignment, &policy, approval);

        assert!(
            authorized.is_ok(),
            "a complete owner-approved write inside its assigned component must authorize"
        );
    }

    #[test]
    fn owner_approval_is_bound_to_the_exact_proposal_and_trusted_context() {
        let (proposal_a, assignment, policy, approval_id) = valid_write_request();
        let approval_a =
            RepositoryMutationApproval::issue(approval_id.clone(), &proposal_a, &policy);
        let proposal_b = scoped_write_proposal(
            assignment.provenance().clone(),
            assignment.repository_id().clone(),
            "src/other.rs",
        );

        assert_authorization_error(
            authorize_mutation(proposal_b, &assignment, &policy, Some(approval_a)),
            RepositoryError::OwnerApprovalMismatch,
        );
        assert_authorization_error(
            authorize_mutation(proposal_a, &assignment, &policy, None),
            RepositoryError::OwnerApprovalRequired,
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the stale-context fixture must fail loudly if its known provenance variation disappears"
    )]
    fn owner_approval_bound_to_a_prior_assignment_context_is_stale() {
        let (proposal_a, assignment_a, policy_a, approval_id) = valid_write_request();
        let approval_a = RepositoryMutationApproval::issue(approval_id, &proposal_a, &policy_a);
        let Some(stale_provenance) = provenance_mismatches(assignment_a.provenance())
            .into_iter()
            .next()
        else {
            panic!("the complete provenance mismatch fixture must not be empty");
        };
        let assignment_b = RepositoryAssignmentContext::new(
            stale_provenance,
            assignment_a.repository_id().clone(),
            assignment_a.component_scope().clone(),
        );
        let policy_b = RepositoryMutationPolicy::new(
            assignment_b.clone(),
            [RepositoryCapability::MutateRepository],
        );
        let proposal_b = scoped_write_proposal(
            assignment_b.provenance().clone(),
            assignment_b.repository_id().clone(),
            "src/lib.rs",
        );

        assert_authorization_error(
            authorize_mutation(proposal_b, &assignment_b, &policy_b, Some(approval_a)),
            RepositoryError::OwnerApprovalStale,
        );
    }

    #[test]
    fn repository_service_port_accepts_no_operational_options() {
        let (proposal, assignment, policy, approval) = valid_write_request();
        let mutation = must_authorize(proposal, &assignment, &policy, approval);
        let service = CountingRepositoryService::default();

        let _future = service.dispatch(mutation);
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "phase-safe wire fixtures must fail loudly if their safe transcript cannot serialize"
    )]
    fn mutation_and_reconciliation_failures_have_phase_specific_retry_directives() {
        let (proposal, assignment, policy, approval) = valid_write_request();
        let terminal = must_authorize(proposal, &assignment, &policy, approval.clone())
            .into_failure(RepositoryMutationFailureCode::PreconditionNotMet);
        assert_eq!(
            terminal.retryability(),
            RepositoryRetryability::FreshAuthorizationRequired
        );
        let serialized_terminal = match serde_json::to_string(&terminal) {
            Ok(serialized) => serialized,
            Err(error) => panic!("definitive mutation failure must serialize: {error}"),
        };
        assert_deserialization_rejected::<RepositoryMutationFailure>(
            &serialized_terminal.replacen("\"PreconditionNotMet\"", "\"ReadOnlyQueryFailed\"", 1),
        );

        let ambiguity = must_authorize(
            scoped_write_proposal(
                assignment.provenance().clone(),
                assignment.repository_id().clone(),
                "src/other.rs",
            ),
            &assignment,
            &policy,
            approval,
        )
        .into_ambiguity();
        let reconciliation = match ambiguity {
            RepositoryDispatchOutcome::OutcomeUnknown(reconciliation) => reconciliation,
            RepositoryDispatchOutcome::Applied(_) => {
                panic!("a post-dispatch uncertainty must remain reconcilable")
            }
        };
        let read_only_failure =
            reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed);
        assert_eq!(
            read_only_failure.retryability(),
            RepositoryRetryability::ReadOnlyRetryable
        );
        let serialized_read_only_failure = match serde_json::to_string(&read_only_failure) {
            Ok(serialized) => serialized,
            Err(error) => panic!("read-only reconciliation failure must serialize: {error}"),
        };
        assert_deserialization_rejected::<RepositoryReconciliationFailure>(
            &serialized_read_only_failure.replacen(
                "\"ReadOnlyQueryFailed\"",
                "\"PreconditionNotMet\"",
                1,
            ),
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "path and scope fixture parsing must fail loudly"
    )]
    fn root_relative_paths_reject_component_confusion_and_scopes_match_components() {
        for invalid in [
            "",
            "/absolute/file",
            "\\\\network\\share",
            "C:/workspace/file",
            "src//file",
            "src/",
            "./src/file",
            "src/../file",
            ".git/config",
            "src/.GIT/config",
            "src/line\nbreak",
        ] {
            assert_eq!(
                RepositoryPath::parse(invalid),
                Err(RepositoryError::InvalidRepositoryPath),
                "{invalid:?} must not become a root-relative repository path"
            );
        }

        let scope = match ComponentScope::parse("src") {
            Ok(scope) => scope,
            Err(error) => panic!("test scope must parse: {error}"),
        };
        let inside = match RepositoryPath::parse("src/lib.rs") {
            Ok(path) => path,
            Err(error) => panic!("test path must parse: {error}"),
        };
        let sibling_prefix = match RepositoryPath::parse("src2/lib.rs") {
            Ok(path) => path,
            Err(error) => panic!("test path must parse: {error}"),
        };
        assert!(scope.contains(&inside));
        assert!(!scope.contains(&sibling_prefix));
        assert!(ComponentScope::repository_root().contains(&sibling_prefix));
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "serialization fixture construction must fail loudly"
    )]
    fn sha_256_digests_are_canonical_safe_text_values() {
        let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let digest = Sha256Digest::of(b"");
        assert_eq!(digest.as_hex(), expected);
        assert_eq!(Sha256Digest::parse(expected), Ok(digest));
        assert_eq!(
            Sha256Digest::parse(&expected.to_uppercase()),
            Err(RepositoryError::InvalidDigest)
        );
        let serialized = match serde_json::to_string(&digest) {
            Ok(serialized) => serialized,
            Err(error) => panic!("digest must serialize: {error}"),
        };
        assert_eq!(serialized, format!("\"{expected}\""));
    }

    #[test]
    fn every_provenance_key_must_match_assignment_and_policy_before_token_creation() {
        let (_proposal, assignment, policy, approval) = valid_write_request();
        let baseline = assignment.provenance().clone();
        let repository = assignment.repository_id().clone();

        for mismatch in provenance_mismatches(&baseline) {
            let result = authorize_with_approval(
                scoped_write_proposal(mismatch, repository.clone(), "src/lib.rs"),
                &assignment,
                &policy,
                approval.clone(),
            );
            assert_authorization_error(result, RepositoryError::ProposalProvenanceMismatch);
        }

        for mismatch in provenance_mismatches(&baseline) {
            let mismatched_policy = RepositoryMutationPolicy::new(
                RepositoryAssignmentContext::new(
                    mismatch,
                    repository.clone(),
                    assignment.component_scope().clone(),
                ),
                [RepositoryCapability::MutateRepository],
            );
            let result = authorize_with_approval(
                scoped_write_proposal(baseline.clone(), repository.clone(), "src/lib.rs"),
                &assignment,
                &mismatched_policy,
                approval.clone(),
            );
            assert_authorization_error(result, RepositoryError::PolicyAssignmentMismatch);
        }
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "operation fixture parsing and exhaustive branch assertions must fail loudly"
    )]
    fn authorized_write_and_delete_expose_only_the_typed_operation_and_precondition() {
        let (proposal, assignment, policy, approval) = valid_write_request();
        let write = must_authorize(proposal, &assignment, &policy, approval.clone());
        let write_identity = write.identity();
        assert_eq!(write_identity.kind(), RepositoryMutationKind::Write);
        assert_eq!(
            write_identity.precondition(),
            RepositoryMutationPrecondition::Write(WritePrecondition::Absent)
        );
        assert_eq!(write_identity.owner_approval(), &approval);
        assert_eq!(write_identity.repository_id(), assignment.repository_id());
        assert_eq!(
            write_identity.provenance().deadline_milliseconds().get(),
            1_000
        );
        match write.operation() {
            AuthorizedRepositoryOperation::Write {
                content,
                path,
                precondition,
            } => {
                assert_eq!(path.as_str(), "src/lib.rs");
                assert_eq!(precondition, WritePrecondition::Absent);
                assert_eq!(content.as_bytes(), b"pub fn one() {}\n");
                assert_eq!(write_identity.content_digest(), Some(content.digest()));
            }
            AuthorizedRepositoryOperation::Delete { .. } => {
                panic!("a write proposal must mint a write-only adapter operation")
            }
        }

        let expected = Sha256Digest::of(b"previous file bytes");
        let delete = must_authorize(
            RepositoryMutationProposal::delete(
                assignment.provenance().clone(),
                assignment.repository_id().clone(),
                match RepositoryPath::parse("src/old.rs") {
                    Ok(path) => path,
                    Err(error) => panic!("test path must parse: {error}"),
                },
                expected,
            ),
            &assignment,
            &policy,
            approval,
        );
        assert_eq!(delete.identity().kind(), RepositoryMutationKind::Delete);
        assert_eq!(
            delete.identity().precondition(),
            RepositoryMutationPrecondition::Delete(expected)
        );
        assert_eq!(delete.identity().content_digest(), None);
        match delete.operation() {
            AuthorizedRepositoryOperation::Delete { path, precondition } => {
                assert_eq!(path.as_str(), "src/old.rs");
                assert_eq!(precondition, expected);
            }
            AuthorizedRepositoryOperation::Write { .. } => {
                panic!("a delete proposal must mint a delete-only adapter operation")
            }
        }
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "policy-scope fixture parsing must fail loudly"
    )]
    fn repository_capability_repository_scope_and_approval_fences_refuse_before_token_creation() {
        let (_proposal, assignment, policy, approval) = valid_write_request();
        let provenance = assignment.provenance().clone();
        let repository = assignment.repository_id().clone();

        assert_authorization_error(
            authorize_with_approval(
                scoped_write_proposal(provenance.clone(), repository.clone(), "tests/escape.rs"),
                &assignment,
                &policy,
                approval.clone(),
            ),
            RepositoryError::AssignmentScopeMismatch,
        );
        assert_authorization_error(
            authorize_with_approval(
                scoped_write_proposal(
                    provenance.clone(),
                    repository_value(RepositoryId::parse, "repo-2"),
                    "src/lib.rs",
                ),
                &assignment,
                &policy,
                approval.clone(),
            ),
            RepositoryError::RepositoryMismatch,
        );
        assert_authorization_error(
            authorize_with_approval(
                scoped_write_proposal(provenance.clone(), repository.clone(), "src/lib.rs"),
                &assignment,
                &RepositoryMutationPolicy::new(assignment.clone(), []),
                approval.clone(),
            ),
            RepositoryError::CapabilityDenied,
        );
        assert_authorization_error(
            authorize_mutation(
                scoped_write_proposal(provenance.clone(), repository.clone(), "src/lib.rs"),
                &assignment,
                &policy,
                None,
            ),
            RepositoryError::OwnerApprovalRequired,
        );
        assert_authorization_error(
            authorize_with_approval(
                scoped_write_proposal(provenance.clone(), repository.clone(), "src/lib.rs"),
                &assignment,
                &RepositoryMutationPolicy::new(
                    RepositoryAssignmentContext::new(
                        provenance.clone(),
                        repository_value(RepositoryId::parse, "repo-2"),
                        assignment.component_scope().clone(),
                    ),
                    [RepositoryCapability::MutateRepository],
                ),
                approval.clone(),
            ),
            RepositoryError::PolicyRepositoryMismatch,
        );
        assert_authorization_error(
            authorize_with_approval(
                scoped_write_proposal(provenance, repository, "src/lib.rs"),
                &assignment,
                &RepositoryMutationPolicy::new(
                    RepositoryAssignmentContext::new(
                        assignment.provenance().clone(),
                        assignment.repository_id().clone(),
                        match ComponentScope::parse("tests") {
                            Ok(scope) => scope,
                            Err(error) => panic!("test scope must parse: {error}"),
                        },
                    ),
                    [RepositoryCapability::MutateRepository],
                ),
                approval,
            ),
            RepositoryError::PolicyScopeMismatch,
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "safe receipt serialization and deserialization fixtures must fail loudly"
    )]
    fn safe_receipts_round_trip_and_reject_malformed_semantic_values() {
        let (proposal, assignment, policy, approval) = valid_write_request();
        let receipt =
            must_authorize(proposal, &assignment, &policy, approval).into_applied_receipt();
        assert_safe_transcript(&receipt, "pub fn one() {}\n");
        let serialized = match serde_json::to_string(&receipt) {
            Ok(serialized) => serialized,
            Err(error) => panic!("safe receipt must serialize: {error}"),
        };
        let restored: RepositoryMutationReceipt = match serde_json::from_str(&serialized) {
            Ok(restored) => restored,
            Err(error) => panic!("safe receipt must deserialize: {error}"),
        };
        assert_eq!(restored, receipt);

        let (ambiguity_proposal, ambiguity_assignment, ambiguity_policy, ambiguity_approval) =
            valid_write_request();
        let ambiguity = match must_authorize(
            ambiguity_proposal,
            &ambiguity_assignment,
            &ambiguity_policy,
            ambiguity_approval,
        )
        .into_ambiguity()
        {
            RepositoryDispatchOutcome::OutcomeUnknown(ambiguity) => ambiguity,
            RepositoryDispatchOutcome::Applied(_) => {
                panic!("direct ambiguity conversion cannot produce an applied receipt")
            }
        };
        let serialized_ambiguity = match serde_json::to_string(&ambiguity) {
            Ok(ambiguity_json) => ambiguity_json,
            Err(error) => panic!("safe ambiguity must serialize: {error}"),
        };
        let restored_ambiguity: RepositoryReconciliation =
            match serde_json::from_str(&serialized_ambiguity) {
                Ok(restored_reconciliation) => restored_reconciliation,
                Err(error) => panic!("safe ambiguity must deserialize: {error}"),
            };
        assert_eq!(restored_ambiguity, ambiguity);

        assert_deserialization_code::<RepositoryPath>(
            "\"src/../outside.rs\"",
            RepositoryError::InvalidRepositoryPath.code(),
        );
        assert_deserialization_code::<RepositoryId>(
            "\"repo\\nother\"",
            RepositoryError::InvalidSemanticValue.code(),
        );
        assert_deserialization_code::<Sha256Digest>(
            "\"E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855\"",
            RepositoryError::InvalidDigest.code(),
        );
        let impossible_identity =
            serialized.replacen("\"kind\":\"Write\"", "\"kind\":\"Delete\"", 1);
        assert_deserialization_code::<RepositoryMutationReceipt>(
            &impossible_identity,
            RepositoryError::InvalidMutationIdentity.code(),
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the denial branch must fail loudly if it ever produces dispatch authority"
    )]
    fn denied_proposal_never_reaches_the_repository_service_port() {
        let (_proposal, assignment, policy, approval) = valid_write_request();
        let service = CountingRepositoryService::default();
        let denied = authorize_with_approval(
            scoped_write_proposal(
                assignment.provenance().clone(),
                assignment.repository_id().clone(),
                "tests/not-assigned.rs",
            ),
            &assignment,
            &policy,
            approval,
        );
        match denied {
            Err(RepositoryError::AssignmentScopeMismatch) => {}
            Err(error) => panic!("test proposal must fail at the assignment scope: {error}"),
            Ok(mutation) => {
                let _future = service.dispatch(mutation);
                panic!("a denied raw proposal must not receive a dispatchable mutation token");
            }
        }
        let _: &dyn RepositoryService = &service;
        assert_eq!(service.dispatch_count(), 0);
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the ambiguity branch must fail loudly if it is reported as applied"
    )]
    fn terminal_failure_and_post_dispatch_ambiguity_have_distinct_non_replayable_results() {
        let (proposal, assignment, policy, approval) = valid_write_request();
        let terminal_failure = must_authorize(proposal, &assignment, &policy, approval.clone())
            .into_failure(RepositoryMutationFailureCode::PreconditionNotMet);
        assert_eq!(
            terminal_failure.error(),
            RepositoryMutationFailureCode::PreconditionNotMet
        );
        assert_eq!(
            terminal_failure.retryability(),
            RepositoryRetryability::FreshAuthorizationRequired
        );
        assert_safe_transcript(&terminal_failure, "pub fn one() {}\n");

        let ambiguity = must_authorize(
            scoped_write_proposal(
                assignment.provenance().clone(),
                assignment.repository_id().clone(),
                "src/lib.rs",
            ),
            &assignment,
            &policy,
            approval,
        )
        .into_ambiguity();
        let reconciliation = match ambiguity {
            RepositoryDispatchOutcome::OutcomeUnknown(reconciliation) => reconciliation,
            RepositoryDispatchOutcome::Applied(_) => {
                panic!("post-dispatch ambiguity must not be recorded as an applied receipt")
            }
        };
        assert_eq!(reconciliation.identity().path().as_str(), "src/lib.rs");
        assert_safe_transcript(&reconciliation, "public behavior fixture");

        for state in [
            RepositoryReconciliationState::Applied,
            RepositoryReconciliationState::NotApplied,
            RepositoryReconciliationState::StillUnknown,
        ] {
            let outcome = reconciliation.bind_outcome(state);
            assert_reconciliation_state(outcome, state, reconciliation.identity());
        }
        let applied_outcome = reconciliation.bind_outcome(RepositoryReconciliationState::Applied);
        let serialized_applied_outcome = match serde_json::to_string(&applied_outcome) {
            Ok(serialized) => serialized,
            Err(error) => panic!("applied reconciliation outcome must serialize: {error}"),
        };
        let impossible_outcome = serialized_applied_outcome.replacen(
            "\"state\":\"Applied\"",
            "\"state\":\"NotApplied\"",
            1,
        );
        assert_deserialization_code::<RepositoryReconciliationOutcome>(
            &impossible_outcome,
            RepositoryError::InvalidReconciliationOutcome.code(),
        );
        let reconciliation_failure =
            reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed);
        assert_eq!(
            reconciliation_failure.error(),
            RepositoryReconciliationError::ReadOnlyQueryFailed
        );
        assert_eq!(
            reconciliation_failure.retryability(),
            RepositoryRetryability::ReadOnlyRetryable
        );
        assert_safe_transcript(&reconciliation_failure, "public behavior fixture");
    }

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        reason = "test fixture parsing must fail loudly"
    )]
    fn workflow_value<T>(parse: impl FnOnce(&str) -> Result<T, HarnessError>, value: &str) -> T {
        match parse(value) {
            Ok(parsed) => parsed,
            Err(error) => panic!("workflow fixture value must parse: {error}"),
        }
    }

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        reason = "test fixture parsing must fail loudly"
    )]
    fn repository_value<T>(
        parse: impl FnOnce(&str) -> Result<T, RepositoryError>,
        value: &str,
    ) -> T {
        match parse(value) {
            Ok(parsed) => parsed,
            Err(error) => panic!("repository fixture value must parse: {error}"),
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        clippy::panic,
        reason = "the assertion consumes any opaque token so a denied test case cannot retain authority"
    )]
    fn assert_authorization_error(
        result: Result<AuthorizedRepositoryMutation, RepositoryError>,
        expected: RepositoryError,
    ) {
        match result {
            Err(actual) => assert_eq!(actual, expected),
            Ok(_) => panic!("mismatched authority must not mint an opaque mutation token"),
        }
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fixture binds a fresh opaque owner approval before exercising public authorization"
    )]
    fn authorize_with_approval(
        proposal: RepositoryMutationProposal,
        assignment: &RepositoryAssignmentContext,
        policy: &RepositoryMutationPolicy,
        approval_id: OwnerApprovalId,
    ) -> Result<AuthorizedRepositoryMutation, RepositoryError> {
        let approval = RepositoryMutationApproval::issue(approval_id, &proposal, policy);
        authorize_mutation(proposal, assignment, policy, Some(approval))
    }

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        reason = "successful authorization fixture construction must fail loudly in tests"
    )]
    fn must_authorize(
        proposal: RepositoryMutationProposal,
        assignment: &RepositoryAssignmentContext,
        policy: &RepositoryMutationPolicy,
        approval: OwnerApprovalId,
    ) -> AuthorizedRepositoryMutation {
        match authorize_with_approval(proposal, assignment, policy, approval) {
            Ok(authorized) => authorized,
            Err(error) => panic!("test proposal must authorize: {error}"),
        }
    }

    #[expect(
        clippy::panic,
        reason = "durable semantic deserialization failures must expose the stable parser code"
    )]
    fn assert_deserialization_code<T>(json: &str, expected_code: &str)
    where
        T: DeserializeOwned,
    {
        match serde_json::from_str::<T>(json) {
            Err(error) => assert!(error.to_string().contains(expected_code)),
            Ok(_) => panic!("malformed durable semantic value must not deserialize"),
        }
    }

    fn assert_deserialization_rejected<T>(json: &str)
    where
        T: DeserializeOwned,
    {
        assert!(serde_json::from_str::<T>(json).is_err());
    }

    #[expect(
        clippy::panic,
        reason = "safe transcript assertions report unexpected serialization failures loudly"
    )]
    fn assert_safe_transcript<T>(value: &T, raw_content: &str)
    where
        T: Serialize + fmt::Debug,
    {
        assert!(!format!("{value:?}").contains(raw_content));
        let serialized = match serde_json::to_string(value) {
            Ok(serialized) => serialized,
            Err(error) => panic!("safe transcript must serialize: {error}"),
        };
        assert!(!serialized.contains(raw_content));
    }

    #[expect(
        clippy::panic,
        clippy::single_call_fn,
        reason = "the one focused ambiguity test keeps all closed-state assertions in a named public helper"
    )]
    fn assert_reconciliation_state(
        outcome: RepositoryReconciliationOutcome,
        expected: RepositoryReconciliationState,
        identity: &RepositoryMutationIdentity,
    ) {
        let ((
            RepositoryReconciliationOutcome::Applied(receipt),
            RepositoryReconciliationState::Applied,
        )
        | (
            RepositoryReconciliationOutcome::NotApplied(receipt),
            RepositoryReconciliationState::NotApplied,
        )
        | (
            RepositoryReconciliationOutcome::StillUnknown(receipt),
            RepositoryReconciliationState::StillUnknown,
        )) = (outcome, expected)
        else {
            panic!("reconciliation outcome variant must match its closed state");
        };
        assert_eq!(receipt.state(), expected);
        assert_eq!(receipt.identity(), identity);
    }

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        reason = "the test fixture creates a public write proposal through checked constructors"
    )]
    fn scoped_write_proposal(
        provenance: RepositoryMutationProvenance,
        repository: RepositoryId,
        path: &str,
    ) -> RepositoryMutationProposal {
        RepositoryMutationProposal::write(
            provenance,
            repository,
            match RepositoryPath::parse(path) {
                Ok(parsed_path) => parsed_path,
                Err(error) => panic!("test path must parse: {error}"),
            },
            match RepositoryContent::from_bytes(b"public behavior fixture") {
                Ok(content) => content,
                Err(error) => panic!("test content must parse: {error}"),
            },
            WritePrecondition::Absent,
        )
    }

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        clippy::too_many_lines,
        reason = "the test explicitly varies each public workflow provenance identity"
    )]
    fn provenance_mismatches(
        baseline: &RepositoryMutationProvenance,
    ) -> Vec<RepositoryMutationProvenance> {
        vec![
            RepositoryMutationProvenance::new(
                workflow_value(SessionId::parse, "session-2"),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                workflow_value(AgentId::parse, "agent-2"),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                workflow_value(WorkflowId::parse, "workflow-2"),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                workflow_value(AssignmentId::parse, "assignment-2"),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                workflow_value(AssignmentScope::parse, "repository:other"),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                match AssignmentEpoch::parse(2) {
                    Ok(value) => value,
                    Err(error) => panic!("test epoch must parse: {error}"),
                },
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                match AttemptNumber::parse(2) {
                    Ok(value) => value,
                    Err(error) => panic!("test attempt must parse: {error}"),
                },
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                workflow_value(ContextReceiptId::parse, "context-2"),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                workflow_value(PolicyDecisionId::parse, "policy-2"),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                workflow_value(EffectId::parse, "effect-2"),
                baseline.idempotency_key().clone(),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                workflow_value(IdempotencyKey::parse, "idem-2"),
                baseline.deadline_milliseconds(),
            ),
            RepositoryMutationProvenance::new(
                baseline.session_id().clone(),
                baseline.agent_id().clone(),
                baseline.workflow_id().clone(),
                baseline.assignment_id().clone(),
                baseline.assignment_scope().clone(),
                baseline.assignment_epoch(),
                baseline.attempt_number(),
                baseline.context_receipt_id().clone(),
                baseline.policy_decision_id().clone(),
                baseline.effect_id().clone(),
                baseline.idempotency_key().clone(),
                match DeadlineMilliseconds::parse(2_000) {
                    Ok(value) => value,
                    Err(error) => panic!("test deadline must parse: {error}"),
                },
            ),
        ]
    }

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        reason = "test fixture construction must fail loudly"
    )]
    fn valid_write_request() -> (
        RepositoryMutationProposal,
        RepositoryAssignmentContext,
        RepositoryMutationPolicy,
        OwnerApprovalId,
    ) {
        let provenance = RepositoryMutationProvenance::new(
            workflow_value(SessionId::parse, "session-1"),
            workflow_value(AgentId::parse, "agent-1"),
            workflow_value(WorkflowId::parse, "workflow-1"),
            workflow_value(AssignmentId::parse, "assignment-1"),
            workflow_value(AssignmentScope::parse, "repository:src"),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            workflow_value(ContextReceiptId::parse, "context-1"),
            workflow_value(PolicyDecisionId::parse, "policy-1"),
            workflow_value(EffectId::parse, "effect-1"),
            workflow_value(IdempotencyKey::parse, "idem-1"),
            match DeadlineMilliseconds::parse(1_000) {
                Ok(deadline) => deadline,
                Err(error) => panic!("test deadline must parse: {error}"),
            },
        );
        let repository = repository_value(RepositoryId::parse, "repo-1");
        let assignment = RepositoryAssignmentContext::new(
            provenance.clone(),
            repository.clone(),
            match ComponentScope::parse("src") {
                Ok(scope) => scope,
                Err(error) => panic!("test component scope must parse: {error}"),
            },
        );
        let policy = RepositoryMutationPolicy::new(
            assignment.clone(),
            [RepositoryCapability::MutateRepository],
        );
        let proposal = RepositoryMutationProposal::write(
            provenance,
            repository,
            match RepositoryPath::parse("src/lib.rs") {
                Ok(path) => path,
                Err(error) => panic!("test path must parse: {error}"),
            },
            match RepositoryContent::from_bytes(b"pub fn one() {}\n") {
                Ok(content) => content,
                Err(error) => panic!("test content must parse: {error}"),
            },
            WritePrecondition::Absent,
        );
        (
            proposal,
            assignment,
            policy,
            repository_value(OwnerApprovalId::parse, "approval-1"),
        )
    }
}
