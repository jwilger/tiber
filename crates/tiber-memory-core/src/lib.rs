//! Pure, provider-independent contracts for bounded advisory memory.

extern crate alloc;

use alloc::{boxed::Box, format, string::String, sync::Arc, vec::Vec};
use core::{
    fmt,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use serde::{Deserialize, Serialize, de::Error as _};
use sha2::{Digest as _, Sha256};

/// Generates small validated semantic identity newtypes and their serde boundary.
macro_rules! semantic_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        #[expect(
            clippy::implicit_return,
            reason = "validated semantic accessors use idiomatic tail expressions while the workspace restriction lint forbids them"
        )]
        impl $name {
            /// Returns the canonical identity text.
            #[must_use]
            #[inline]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Parses a canonical semantic identity at the external boundary.
            ///
            /// # Errors
            ///
            /// Returns [`MemoryContractError`] for empty, oversized, control-bearing,
            /// or delimiter-bearing input.
            #[inline]
            pub fn parse(value: &str) -> Result<Self, MemoryContractError> {
                let canonical = value.trim();
                if canonical.is_empty() {
                    return Err(MemoryContractError::Empty);
                }
                if canonical.len() > MAX_MEMORY_ID_BYTES
                    || !canonical.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                    })
                {
                    return Err(MemoryContractError::Invalid);
                }
                Ok(Self(canonical.to_owned()))
            }
        }

        #[expect(
            clippy::implicit_return,
            clippy::missing_trait_methods,
            reason = "the semantic parser is the sole construction boundary; serde's optional in-place hook cannot preserve it"
        )]
        impl<'de> Deserialize<'de> for $name {
            #[inline]
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = match String::deserialize(deserializer) {
                    Ok(value) => value,
                    Err(error) => return Err(error),
                };
                match Self::parse(&value) {
                    Ok(parsed) => Ok(parsed),
                    Err(error) => Err(D::Error::custom(error)),
                }
            }
        }
    };
}

/// Maximum UTF-8 byte length accepted for a semantic memory identity.
pub const MAX_MEMORY_ID_BYTES: usize = 128;

/// Maximum UTF-8 byte length accepted for one retained memory payload.
pub const MAX_MEMORY_CONTENT_BYTES: usize = 0x0001_0000;

/// Maximum UTF-8 byte length accepted for a recall query.
pub const MAX_MEMORY_QUERY_BYTES: usize = 4_096;

/// Maximum number of recalled items that one request may admit.
pub const MAX_RECALL_ITEMS: usize = 64;

/// Maximum estimated tokens that one recall request may admit.
pub const MAX_RECALL_TOKENS: usize = 0x4000;

/// Stable failures while constructing a memory-domain value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle every stable memory-contract refusal deliberately"
)]
pub enum MemoryContractError {
    /// A required value was empty after canonical trimming.
    Empty,
    /// A value exceeded its bound or contained a prohibited character.
    Invalid,
    /// A numeric budget or deadline was outside the allowed range.
    InvalidBudget,
    /// A backend identity was valid but belonged to a different trusted scope.
    ScopeMismatch,
}

#[expect(
    clippy::implicit_return,
    reason = "the stable error-code table uses an idiomatic total tail match"
)]
impl MemoryContractError {
    /// Returns this failure's stable machine-readable code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => "memory_contract_empty",
            Self::Invalid => "memory_contract_invalid",
            Self::InvalidBudget => "memory_contract_invalid_budget",
            Self::ScopeMismatch => "memory_contract_scope_mismatch",
        }
    }
}

impl fmt::Display for MemoryContractError {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates directly to the stable code table"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

semantic_id!(AgentId, "A validated identity for the agent using memory.");
semantic_id!(
    MemoryDocumentId,
    "A stable, EventCore-derived retained-document identity."
);
semantic_id!(
    MemoryId,
    "A backend-provided identity for one recalled memory."
);
semantic_id!(
    MemoryKind,
    "A validated kind describing the retained memory's purpose."
);
semantic_id!(
    MemoryOperationId,
    "A backend-provided asynchronous operation identity."
);
semantic_id!(OwnerId, "A validated identity for the repository owner.");
semantic_id!(
    RepositoryId,
    "A validated identity for the current repository."
);
semantic_id!(
    SessionId,
    "A validated identity for the owning Tiber session."
);
semantic_id!(
    TaskId,
    "A validated identity for the associated Tiber task."
);
semantic_id!(TurnId, "A validated identity for the conversation turn.");

/// Chooses the bank derivation strategy for one memory operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "bank isolation is a closed durable policy choice"
)]
pub enum MemoryBankScope {
    /// Put memories in the owner's cross-repository bank.
    OwnerGlobal,
    /// Put memories in a bank isolated to the current repository.
    Repository,
}

/// A backend-neutral bank name derived only from trusted memory provenance.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MemoryBank(String);

#[expect(
    clippy::implicit_return,
    reason = "the derived-bank accessor uses an idiomatic tail expression"
)]
impl MemoryBank {
    /// Returns the backend-neutral derived bank name.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One validated, Tiber-derived memory tag.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct MemoryTag(String);

#[expect(
    clippy::implicit_return,
    reason = "tag parsing and accessors use idiomatic tail expressions while preserving the single parser boundary"
)]
impl MemoryTag {
    /// Returns the canonical tag text.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a trusted tag from a fixed domain prefix and already-validated identity.
    fn derived(prefix: &str, identity: &str) -> Self {
        Self(format!("{prefix}:{identity}"))
    }

    /// Parses a tag returned by an untrusted backend response.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryContractError`] when the tag cannot be represented safely.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, MemoryContractError> {
        let canonical = value.trim();
        if canonical.is_empty() {
            return Err(MemoryContractError::Empty);
        }
        if canonical.len() > MAX_MEMORY_ID_BYTES.saturating_add(MAX_MEMORY_ID_BYTES)
            || !canonical.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
            })
        {
            return Err(MemoryContractError::Invalid);
        }
        Ok(Self(canonical.to_owned()))
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "the tag parser is the sole construction boundary; serde's optional in-place hook cannot preserve it"
)]
impl<'de> Deserialize<'de> for MemoryTag {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match Self::parse(&value) {
            Ok(parsed) => Ok(parsed),
            Err(error) => Err(D::Error::custom(error)),
        }
    }
}

/// A fixed, trusted set of tags derived from a complete memory scope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StrictMemoryTags(Vec<MemoryTag>);

#[expect(
    clippy::implicit_return,
    reason = "scope-tag helpers use idiomatic tail expressions while retaining a fixed trusted derivation"
)]
impl StrictMemoryTags {
    /// Returns whether every trusted tag is present in an untrusted backend result.
    fn all_present_in(&self, tags: &[MemoryTag]) -> bool {
        self.0.iter().all(|expected| tags.contains(expected))
    }

    /// Returns the immutable scope tags in their canonical order.
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[MemoryTag] {
        &self.0
    }

    /// Derives the complete fixed tag set from trusted memory scope provenance.
    fn from_scope(scope: &MemoryScope, turn_id: Option<&TurnId>) -> Self {
        let mut tags = Vec::with_capacity(7);
        tags.push(MemoryTag::derived("owner", scope.owner_id.as_str()));
        tags.push(MemoryTag::derived(
            "repository",
            scope.repository_id.as_str(),
        ));
        tags.push(MemoryTag::derived("agent", scope.agent_id.as_str()));
        tags.push(MemoryTag::derived("session", scope.session_id.as_str()));
        tags.push(MemoryTag::derived("task", scope.task_id.as_str()));
        tags.push(MemoryTag::derived("kind", scope.memory_kind.as_str()));
        if let Some(current_turn) = turn_id {
            tags.push(MemoryTag::derived("turn", current_turn.as_str()));
        }
        Self(tags)
    }
}

/// Complete trusted provenance from which a memory backend derives bank and tags.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryScope {
    /// Trusted agent identity for the memory namespace.
    agent_id: AgentId,
    /// Explicit isolation strategy chosen by the caller's typed scope.
    bank_scope: MemoryBankScope,
    /// Trusted purpose tag for retained memories.
    memory_kind: MemoryKind,
    /// Trusted owner identity used for owner-global banks.
    owner_id: OwnerId,
    /// Trusted repository identity used for repository-scoped banks.
    repository_id: RepositoryId,
    /// Trusted session that owns the operation.
    session_id: SessionId,
    /// Trusted task that owns the operation.
    task_id: TaskId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the scope construction and derivation methods are grouped by caller lifecycle rather than alphabetically"
)]
impl MemoryScope {
    /// Creates a complete owner-global memory scope.
    #[must_use]
    #[inline]
    pub const fn owner_global(
        owner_id: OwnerId,
        repository_id: RepositoryId,
        agent_id: AgentId,
        session_id: SessionId,
        task_id: TaskId,
        memory_kind: MemoryKind,
    ) -> Self {
        Self {
            agent_id,
            bank_scope: MemoryBankScope::OwnerGlobal,
            memory_kind,
            owner_id,
            repository_id,
            session_id,
            task_id,
        }
    }

    /// Creates a complete repository-isolated memory scope.
    #[must_use]
    #[inline]
    pub const fn repository(
        owner_id: OwnerId,
        repository_id: RepositoryId,
        agent_id: AgentId,
        session_id: SessionId,
        task_id: TaskId,
        memory_kind: MemoryKind,
    ) -> Self {
        Self {
            agent_id,
            bank_scope: MemoryBankScope::Repository,
            memory_kind,
            owner_id,
            repository_id,
            session_id,
            task_id,
        }
    }

    /// Returns the selected bank-isolation strategy.
    #[must_use]
    #[inline]
    pub const fn bank_scope(&self) -> MemoryBankScope {
        self.bank_scope
    }

    /// Derives the only bank this scope may address.
    #[must_use]
    #[inline]
    pub fn bank(&self) -> MemoryBank {
        match self.bank_scope {
            MemoryBankScope::OwnerGlobal => {
                MemoryBank(format!("tiber-owner-{}", self.owner_id.as_str()))
            }
            MemoryBankScope::Repository => {
                MemoryBank(format!("tiber-repository-{}", self.repository_id.as_str()))
            }
        }
    }

    /// Derives the collision-free backend document identity for this complete scope.
    #[must_use]
    #[inline]
    pub fn backend_document_id(&self, document_id: &MemoryDocumentId) -> ScopedMemoryDocumentId {
        let bank_scope = match self.bank_scope {
            MemoryBankScope::OwnerGlobal => "owner-global",
            MemoryBankScope::Repository => "repository",
        };
        let encoded = format!(
            "tiber:v1:{bank_scope}:{}:{}:{}:{}:{}:{}:{}",
            self.owner_id.as_str(),
            self.repository_id.as_str(),
            self.agent_id.as_str(),
            self.session_id.as_str(),
            self.task_id.as_str(),
            self.memory_kind.as_str(),
            document_id.as_str(),
        );
        ScopedMemoryDocumentId {
            document_id: document_id.clone(),
            encoded,
            scope: self.clone(),
        }
    }

    /// Parses and validates a backend document identity against this complete scope.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryContractError::Invalid`] for malformed input and
    /// [`MemoryContractError::ScopeMismatch`] when a well-formed identity was
    /// derived for another scope.
    #[inline]
    pub fn parse_backend_document_id(
        &self,
        value: &str,
    ) -> Result<ScopedMemoryDocumentId, MemoryContractError> {
        let Some((_namespace, raw_document_id)) = value.rsplit_once(':') else {
            return Err(MemoryContractError::Invalid);
        };
        let document_id = MemoryDocumentId::parse(raw_document_id)?;
        let expected = self.backend_document_id(&document_id);
        if expected.as_str() != value {
            return Err(MemoryContractError::ScopeMismatch);
        }
        Ok(expected)
    }

    /// Derives the fixed scope tags, without an individual turn tag.
    #[must_use]
    #[inline]
    pub fn strict_tags(&self) -> StrictMemoryTags {
        StrictMemoryTags::from_scope(self, None)
    }

    /// Derives the fixed scope tags plus the supplied turn tag.
    #[must_use]
    #[inline]
    pub fn strict_tags_for_turn(&self, turn_id: &TurnId) -> StrictMemoryTags {
        StrictMemoryTags::from_scope(self, Some(turn_id))
    }

    /// Derives the one tag used to exclude a recall request's current turn.
    #[must_use]
    #[inline]
    pub fn turn_tag(turn_id: &TurnId) -> MemoryTag {
        MemoryTag::derived("turn", turn_id.as_str())
    }
}

/// A collision-free backend document identity bound to every provenance component.
///
/// Colons delimit the fixed field sequence and are prohibited in every semantic
/// identity, so two distinct scopes or raw document IDs cannot encode alike.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScopedMemoryDocumentId {
    /// Original stable application document identity.
    document_id: MemoryDocumentId,
    /// Backend-facing deterministic representation.
    encoded: String,
    /// Complete trusted scope embedded by the namespace derivation.
    scope: MemoryScope,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "scoped identity derivation and inspection are grouped as one trusted backend boundary"
)]
impl ScopedMemoryDocumentId {
    /// Returns the collision-free backend-facing document identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.encoded
    }

    /// Returns the original application document identity.
    #[must_use]
    pub const fn document_id(&self) -> &MemoryDocumentId {
        &self.document_id
    }

    /// Returns the complete scope embedded in this identity.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        &self.scope
    }
}

/// A backend operation identity bound to the complete scope that created it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryOperationHandle {
    /// Raw backend operation identity.
    operation_id: MemoryOperationId,
    /// Complete trusted scope that submitted the operation.
    scope: MemoryScope,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "the scoped operation handle exposes only immutable identity and provenance accessors"
)]
impl MemoryOperationHandle {
    /// Returns the raw backend operation identity.
    #[must_use]
    pub const fn operation_id(&self) -> &MemoryOperationId {
        &self.operation_id
    }

    /// Returns the complete scope that submitted the operation.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        &self.scope
    }
}

/// Bounded retained content, context, or recall query text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MemoryText(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the text parser precedes its accessor so the single construction boundary is visible first"
)]
impl MemoryText {
    /// Parses nonempty text that may contain normal conversation line breaks.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryContractError`] for empty, oversized, or control-bearing text.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, MemoryContractError> {
        let canonical = value.trim();
        if canonical.is_empty() {
            return Err(MemoryContractError::Empty);
        }
        if canonical.len() > MAX_MEMORY_CONTENT_BYTES
            || !canonical
                .chars()
                .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        {
            return Err(MemoryContractError::Invalid);
        }
        Ok(Self(canonical.to_owned()))
    }

    /// Returns the bounded text.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "the text parser is the sole construction boundary; serde's optional in-place hook cannot preserve it"
)]
impl<'de> Deserialize<'de> for MemoryText {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match Self::parse(&value) {
            Ok(parsed) => Ok(parsed),
            Err(error) => Err(D::Error::custom(error)),
        }
    }
}

/// A bounded number of recall results.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RecallItemBudget(usize);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "budget construction precedes inspection to keep its parser boundary together"
)]
impl RecallItemBudget {
    /// Creates one nonzero bounded item budget.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryContractError::InvalidBudget`] outside the supported range.
    #[inline]
    pub const fn new(value: usize) -> Result<Self, MemoryContractError> {
        if value == 0 || value > MAX_RECALL_ITEMS {
            return Err(MemoryContractError::InvalidBudget);
        }
        Ok(Self(value))
    }

    /// Returns the maximum retained result count.
    #[must_use]
    #[inline]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// A bounded estimated-token budget for recall result text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RecallTokenBudget(usize);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "budget construction precedes inspection to keep its parser boundary together"
)]
impl RecallTokenBudget {
    /// Creates one nonzero bounded token budget.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryContractError::InvalidBudget`] outside the supported range.
    #[inline]
    pub const fn new(value: usize) -> Result<Self, MemoryContractError> {
        if value == 0 || value > MAX_RECALL_TOKENS {
            return Err(MemoryContractError::InvalidBudget);
        }
        Ok(Self(value))
    }

    /// Returns the maximum estimated output tokens.
    #[must_use]
    #[inline]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// A finite deadline applied to one backend operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryDeadline(Duration);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "deadline construction precedes inspection to keep its parser boundary together"
)]
impl MemoryDeadline {
    /// Creates a nonzero deadline no longer than one hour.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryContractError::InvalidBudget`] for zero or excessive deadlines.
    #[inline]
    pub const fn new(value: Duration) -> Result<Self, MemoryContractError> {
        if value.is_zero() || value.as_secs() > 3_600 {
            return Err(MemoryContractError::InvalidBudget);
        }
        Ok(Self(value))
    }

    /// Returns the bounded duration.
    #[must_use]
    #[inline]
    pub const fn get(self) -> Duration {
        self.0
    }
}

/// A cloneable cancellation signal owned by the caller of one memory operation.
#[derive(Clone, Debug, Default)]
pub struct MemoryCancellation(Arc<AtomicBool>);

#[expect(
    clippy::implicit_return,
    reason = "cancellation inspection uses an idiomatic tail expression"
)]
impl MemoryCancellation {
    /// Signals cancellation to every clone of this value.
    #[inline]
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Common bounded options supplied with every backend operation.
#[derive(Clone, Debug)]
pub struct MemoryRequestOptions {
    /// Caller-owned signal used to stop the in-flight operation.
    cancellation: MemoryCancellation,
    /// Absolute duration budget for one operation.
    deadline: MemoryDeadline,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "construction is grouped before the two caller-facing option accessors"
)]
impl MemoryRequestOptions {
    /// Creates options with an explicit deadline and cancellation signal.
    #[must_use]
    #[inline]
    pub const fn new(deadline: MemoryDeadline, cancellation: MemoryCancellation) -> Self {
        Self {
            cancellation,
            deadline,
        }
    }

    /// Returns the caller-owned cancellation signal.
    #[must_use]
    #[inline]
    pub const fn cancellation(&self) -> &MemoryCancellation {
        &self.cancellation
    }

    /// Returns the finite operation deadline.
    #[must_use]
    #[inline]
    pub const fn deadline(&self) -> MemoryDeadline {
        self.deadline
    }
}

/// Opaque request evidence for one exact retained content-and-context pair.
///
/// The versioned SHA-256 value is safe to place in Tiber-owned backend metadata,
/// while its private representation prevents callers from substituting raw
/// retained text as reconciliation evidence.
#[derive(Clone, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RetainEvidence(String);

#[expect(
    clippy::big_endian_bytes,
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the one request-derived digest helper uses explicit network-order framing as its stable cross-platform wire contract"
)]
impl RetainEvidence {
    /// Returns the stable opaque evidence value for adapter-owned metadata.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Derives domain-separated evidence over unambiguous content/context framing.
    fn derive(content: &MemoryText, source_context: &MemoryText) -> Self {
        let retained = content.as_str().as_bytes();
        let source = source_context.as_str().as_bytes();
        let retained_size = u64::try_from(retained.len()).unwrap_or(u64::MAX);
        let source_size = u64::try_from(source.len()).unwrap_or(u64::MAX);
        let mut digest = Sha256::new();
        digest.update(b"tiber-retain-evidence-v1\0");
        digest.update(retained_size.to_be_bytes());
        digest.update(retained);
        digest.update(source_size.to_be_bytes());
        digest.update(source);
        Self(format!("sha256-v1:{:x}", digest.finalize()))
    }
}

impl fmt::Debug for RetainEvidence {
    #[expect(
        clippy::implicit_return,
        reason = "debug output deliberately exposes only the evidence type, never its value or source text"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetainEvidence(<opaque>)")
    }
}

/// One exactly-scoped asynchronous retain request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainRequest {
    /// Bounded content that the backend may retain.
    content: MemoryText,
    /// Bounded trusted source context accompanying the content.
    context: MemoryText,
    /// Stable document identity used as the backend upsert key.
    document_id: MemoryDocumentId,
    /// Complete trusted provenance scope.
    scope: MemoryScope,
    /// Turn whose provenance must be added to the retained tags.
    turn_id: TurnId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "retain construction and accessors are ordered by its externally visible operation lifecycle"
)]
impl RetainRequest {
    /// Creates one stable-document retain request.
    #[must_use]
    pub const fn new(
        scope: MemoryScope,
        document_id: MemoryDocumentId,
        turn_id: TurnId,
        content: MemoryText,
        source_context: MemoryText,
    ) -> Self {
        Self {
            content,
            context: source_context,
            document_id,
            scope,
            turn_id,
        }
    }

    /// Returns the bounded retained content.
    #[must_use]
    pub const fn content(&self) -> &MemoryText {
        &self.content
    }

    /// Returns the human-meaningful source context.
    #[must_use]
    pub const fn context(&self) -> &MemoryText {
        &self.context
    }

    /// Returns the stable upsert document identity.
    #[must_use]
    pub const fn document_id(&self) -> &MemoryDocumentId {
        &self.document_id
    }

    /// Derives the collision-free backend document identity for this request.
    #[must_use]
    pub fn backend_document_id(&self) -> ScopedMemoryDocumentId {
        self.scope.backend_document_id(&self.document_id)
    }

    /// Derives opaque evidence for the exact retained content and source context.
    #[must_use]
    pub fn expected_evidence(&self) -> RetainEvidence {
        RetainEvidence::derive(&self.content, &self.context)
    }

    /// Derives the only reconciliation target valid for an ambiguous retain.
    #[must_use]
    pub fn reconciliation_handle(&self) -> MemoryReconciliationHandle {
        MemoryReconciliationHandle {
            scope: self.scope.clone(),
            target: ReconcileTarget::RetainDocument(RetainReconciliationTarget {
                document_id: self.backend_document_id(),
                expected_evidence: self.expected_evidence(),
                expected_tags: self.strict_tags(),
            }),
        }
    }

    /// Returns the complete trusted scope.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        &self.scope
    }

    /// Returns the turn being retained.
    #[must_use]
    pub const fn turn_id(&self) -> &TurnId {
        &self.turn_id
    }

    /// Derives the only tags a retain operation may publish.
    #[must_use]
    pub fn strict_tags(&self) -> StrictMemoryTags {
        self.scope.strict_tags_for_turn(&self.turn_id)
    }
}

/// One exactly-scoped advisory recall request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecallRequest {
    /// Stable document identity that must not be recalled into itself.
    current_document_id: MemoryDocumentId,
    /// Current turn whose provenance tag must be excluded.
    current_turn_id: TurnId,
    /// Maximum number of admitted advisory results.
    item_budget: RecallItemBudget,
    /// Bounded retrieval query.
    query: MemoryText,
    /// Complete trusted provenance scope.
    scope: MemoryScope,
    /// Maximum estimated token total for admitted result text.
    token_budget: RecallTokenBudget,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "recall construction and accessors are ordered by its scope-and-budget operation lifecycle"
)]
impl RecallRequest {
    /// Creates one bounded recall request that structurally excludes its current turn.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryContractError::Invalid`] when the query exceeds its dedicated bound.
    pub fn new(
        scope: MemoryScope,
        current_turn_id: TurnId,
        current_document_id: MemoryDocumentId,
        query: MemoryText,
        item_budget: RecallItemBudget,
        token_budget: RecallTokenBudget,
    ) -> Result<Self, MemoryContractError> {
        if query.as_str().len() > MAX_MEMORY_QUERY_BYTES {
            return Err(MemoryContractError::Invalid);
        }
        Ok(Self {
            current_document_id,
            current_turn_id,
            item_budget,
            query,
            scope,
            token_budget,
        })
    }

    /// Returns the stable document identity belonging to the current turn.
    #[must_use]
    pub const fn current_document_id(&self) -> &MemoryDocumentId {
        &self.current_document_id
    }

    /// Returns the turn that must never be included in its own recall.
    #[must_use]
    pub const fn current_turn_id(&self) -> &TurnId {
        &self.current_turn_id
    }

    /// Returns the maximum admitted result count.
    #[must_use]
    pub const fn item_budget(&self) -> RecallItemBudget {
        self.item_budget
    }

    /// Returns the bounded recall query.
    #[must_use]
    pub const fn query(&self) -> &MemoryText {
        &self.query
    }

    /// Returns the complete trusted scope.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        &self.scope
    }

    /// Returns the maximum estimated output tokens.
    #[must_use]
    pub const fn token_budget(&self) -> RecallTokenBudget {
        self.token_budget
    }

    /// Returns the fixed tags that every backend result must contain.
    #[must_use]
    pub fn strict_tags(&self) -> StrictMemoryTags {
        self.scope.strict_tags()
    }

    /// Returns the tag that excludes all facts from the current turn.
    #[must_use]
    pub fn excluded_turn_tag(&self) -> MemoryTag {
        MemoryScope::turn_tag(&self.current_turn_id)
    }
}

/// One narrow document-forget request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgetRequest {
    /// Stable document identity that alone may be deleted.
    document_id: MemoryDocumentId,
    /// Complete trusted provenance scope.
    scope: MemoryScope,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "forget construction and accessors are ordered by the narrow document operation lifecycle"
)]
impl ForgetRequest {
    /// Creates a narrow request to forget exactly one stable document.
    #[must_use]
    pub const fn new(scope: MemoryScope, document_id: MemoryDocumentId) -> Self {
        Self { document_id, scope }
    }

    /// Returns the document to forget.
    #[must_use]
    pub const fn document_id(&self) -> &MemoryDocumentId {
        &self.document_id
    }

    /// Derives the collision-free backend document identity for this request.
    #[must_use]
    pub fn backend_document_id(&self) -> ScopedMemoryDocumentId {
        self.scope.backend_document_id(&self.document_id)
    }

    /// Derives the only reconciliation target valid for an ambiguous forget.
    #[must_use]
    pub fn reconciliation_handle(&self) -> MemoryReconciliationHandle {
        MemoryReconciliationHandle {
            scope: self.scope.clone(),
            target: ReconcileTarget::ForgetDocument(self.backend_document_id()),
        }
    }

    /// Returns the complete trusted scope.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        &self.scope
    }
}

/// A scoped request to inspect one asynchronous operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationStatusRequest {
    /// Scope-bound backend operation to inspect.
    operation: MemoryOperationHandle,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "status construction and accessors are ordered by the operation lifecycle"
)]
impl OperationStatusRequest {
    /// Creates a scoped operation-status request.
    #[must_use]
    pub const fn new(operation: MemoryOperationHandle) -> Self {
        Self { operation }
    }

    /// Returns the scope-bound operation handle.
    #[must_use]
    pub const fn operation(&self) -> &MemoryOperationHandle {
        &self.operation
    }

    /// Returns the operation to inspect.
    #[must_use]
    pub const fn operation_id(&self) -> &MemoryOperationId {
        self.operation.operation_id()
    }

    /// Returns the complete trusted scope.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        self.operation.scope()
    }
}

/// A scoped request to cancel one asynchronous operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelRequest {
    /// Scope-bound backend operation to cancel.
    operation: MemoryOperationHandle,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "cancellation construction and accessors are ordered by the operation lifecycle"
)]
impl CancelRequest {
    /// Creates a scoped operation-cancellation request.
    #[must_use]
    pub const fn new(operation: MemoryOperationHandle) -> Self {
        Self { operation }
    }

    /// Returns the scope-bound operation handle.
    #[must_use]
    pub const fn operation(&self) -> &MemoryOperationHandle {
        &self.operation
    }

    /// Returns the operation to cancel.
    #[must_use]
    pub const fn operation_id(&self) -> &MemoryOperationId {
        self.operation.operation_id()
    }

    /// Derives the only reconciliation target valid for an ambiguous cancellation.
    #[must_use]
    pub fn reconciliation_handle(&self) -> MemoryReconciliationHandle {
        MemoryReconciliationHandle {
            scope: self.operation.scope.clone(),
            target: ReconcileTarget::CancelOperation(self.operation.clone()),
        }
    }

    /// Returns the complete trusted scope.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        self.operation.scope()
    }
}

/// One untrusted backend candidate before core scope and budget filtering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecallCandidate {
    /// Backend-provided document identity validated against a complete scope.
    document_id: ScopedMemoryDocumentId,
    /// Backend-provided memory identity parsed through the semantic boundary.
    id: MemoryId,
    /// Backend-provided tags parsed through the semantic boundary.
    tags: Vec<MemoryTag>,
    /// Backend-provided text parsed through the bounded text boundary.
    text: MemoryText,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "the candidate constructor is a direct bounded value-object construction"
)]
impl RecallCandidate {
    /// Creates one decoded candidate from bounded backend data.
    #[must_use]
    pub const fn new(
        id: MemoryId,
        text: MemoryText,
        document_id: ScopedMemoryDocumentId,
        tags: Vec<MemoryTag>,
    ) -> Self {
        Self {
            document_id,
            id,
            tags,
            text,
        }
    }
}

/// A bounded, advisory recalled memory that can never grant authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecalledMemory {
    /// Source document identity after mandatory filtering.
    document_id: MemoryDocumentId,
    /// Source memory identity after mandatory filtering.
    id: MemoryId,
    /// Verified scope tags, never arbitrary server metadata.
    tags: StrictMemoryTags,
    /// Bounded advisory text.
    text: MemoryText,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "advisory memory accessors use direct tail expressions"
)]
impl RecalledMemory {
    /// Returns the source document identity.
    #[must_use]
    pub const fn document_id(&self) -> &MemoryDocumentId {
        &self.document_id
    }

    /// Returns the untrusted source-memory identity.
    #[must_use]
    pub const fn id(&self) -> &MemoryId {
        &self.id
    }

    /// Returns the verified strict provenance tags.
    #[must_use]
    pub const fn tags(&self) -> &StrictMemoryTags {
        &self.tags
    }

    /// Returns advisory untrusted fact text.
    #[must_use]
    pub const fn text(&self) -> &MemoryText {
        &self.text
    }
}

/// Bounded recall output, preserving backend rank order after mandatory filtering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecallResult {
    /// Authoritative estimated-token total computed while admitting memories.
    admitted_tokens: usize,
    /// Rank-preserving memories that passed scope, turn, and budget filtering.
    memories: Vec<RecalledMemory>,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "filtering is intentionally shown before result inspection"
)]
impl RecallResult {
    /// Returns the authoritative estimated-token total for admitted memories.
    #[must_use]
    pub const fn admitted_tokens(&self) -> usize {
        self.admitted_tokens
    }

    /// Filters candidates by trusted scope, current-turn exclusion, and budgets.
    #[must_use]
    pub fn from_candidates(request: &RecallRequest, candidates: Vec<RecallCandidate>) -> Self {
        let expected_tags = request.strict_tags();
        let current_turn_tag = request.excluded_turn_tag();
        let mut memories = Vec::with_capacity(request.item_budget.get());
        let mut consumed_tokens: usize = 0;

        for candidate in candidates {
            if memories.len() == request.item_budget.get()
                || candidate.document_id.scope() != &request.scope
                || candidate
                    .document_id
                    .document_id()
                    .eq(&request.current_document_id)
                || candidate.tags.contains(&current_turn_tag)
                || !expected_tags.all_present_in(&candidate.tags)
            {
                continue;
            }
            let estimated_tokens = candidate
                .text
                .as_str()
                .len()
                .saturating_add(3)
                .saturating_div(4);
            if consumed_tokens.saturating_add(estimated_tokens) > request.token_budget.get() {
                continue;
            }
            consumed_tokens = consumed_tokens.saturating_add(estimated_tokens);
            memories.push(RecalledMemory {
                document_id: candidate.document_id.document_id().clone(),
                id: candidate.id,
                tags: expected_tags.clone(),
                text: candidate.text,
            });
        }
        Self {
            admitted_tokens: consumed_tokens,
            memories,
        }
    }

    /// Returns rank-preserving advisory memories.
    #[must_use]
    pub fn memories(&self) -> &[RecalledMemory] {
        &self.memories
    }
}

/// The outcome of a successfully acknowledged asynchronous retain request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RetainOutcome {
    /// Async backend operation bound to the scope that submitted it.
    operation: MemoryOperationHandle,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "the accepted outcome is a direct immutable value object"
)]
impl RetainOutcome {
    /// Creates an accepted asynchronous retain outcome.
    #[must_use]
    pub fn accepted(request: &RetainRequest, operation_id: MemoryOperationId) -> Self {
        Self {
            operation: MemoryOperationHandle {
                operation_id,
                scope: request.scope.clone(),
            },
        }
    }

    /// Returns the scope-bound operation handle.
    #[must_use]
    pub const fn operation(&self) -> &MemoryOperationHandle {
        &self.operation
    }

    /// Returns the operation that owns completion state.
    #[must_use]
    pub const fn operation_id(&self) -> &MemoryOperationId {
        self.operation.operation_id()
    }
}

/// The outcome of a successfully acknowledged narrow document forget request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "document-forget acknowledgement is a closed adapter contract"
)]
pub enum ForgetOutcome {
    /// The addressed document was already absent.
    AlreadyAbsent,
    /// The addressed document is no longer retained.
    Forgotten,
}

/// Lifecycle state reported for an asynchronous backend operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "Hindsight operation lifecycle states are intentionally closed for this pinned adapter"
)]
pub enum MemoryOperationState {
    /// The operation was cancelled before processing started.
    Cancelled,
    /// The operation completed successfully.
    Completed,
    /// The operation reached a terminal backend failure.
    Failed,
    /// The backend could not find the requested operation.
    NotFound,
    /// The operation is waiting for a backend worker.
    Pending,
    /// A backend worker is actively processing the operation.
    Processing,
}

/// Narrow, sanitized operation-status output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryOperationStatus {
    /// Scope-bound backend operation that was inspected.
    operation: MemoryOperationHandle,
    /// Closed status state projected from the backend's safe response fields.
    state: MemoryOperationState,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "status construction precedes inspection in its public lifecycle API"
)]
impl MemoryOperationStatus {
    /// Creates a sanitized operation status.
    #[must_use]
    pub fn new(request: &OperationStatusRequest, state: MemoryOperationState) -> Self {
        Self {
            operation: request.operation.clone(),
            state,
        }
    }

    /// Returns the scope-bound operation handle.
    #[must_use]
    pub const fn operation(&self) -> &MemoryOperationHandle {
        &self.operation
    }

    /// Returns the operation identity.
    #[must_use]
    pub const fn operation_id(&self) -> &MemoryOperationId {
        self.operation.operation_id()
    }

    /// Returns the closed lifecycle state.
    #[must_use]
    pub const fn state(&self) -> MemoryOperationState {
        self.state
    }
}

/// The outcome of a remote operation-cancellation attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the remote cancellation response is a closed boundary result"
)]
pub enum CancelOutcome {
    /// The backend cancelled the pending operation.
    Cancelled,
    /// The backend has already reached a state it cannot cancel.
    NotCancellable,
}

/// Exact document identity and provenance expected after an ambiguous retain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RetainReconciliationTarget {
    /// Stable document identity bound to the complete memory scope.
    document_id: ScopedMemoryDocumentId,
    /// Opaque evidence bound to the exact retained content and source context.
    expected_evidence: RetainEvidence,
    /// Exact scope and turn tags written by the ambiguous retain request.
    expected_tags: StrictMemoryTags,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "retain reconciliation exposes only its request-derived identity and exact provenance"
)]
impl RetainReconciliationTarget {
    /// Returns the collision-free document identity that may have been retained.
    #[must_use]
    pub const fn document_id(&self) -> &ScopedMemoryDocumentId {
        &self.document_id
    }

    /// Returns the opaque request evidence required to prove this retain applied.
    #[must_use]
    pub const fn expected_evidence(&self) -> &RetainEvidence {
        &self.expected_evidence
    }

    /// Returns the exact strict scope-and-turn tags required to prove this retain applied.
    #[must_use]
    pub const fn expected_tags(&self) -> &StrictMemoryTags {
        &self.expected_tags
    }
}

/// Exact stable target used to reconcile one ambiguous memory mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "every ambiguous memory mutation has one closed, non-replay reconciliation target"
)]
pub enum ReconcileTarget {
    /// Determine whether cancellation affected one scope-bound operation.
    CancelOperation(MemoryOperationHandle),
    /// Determine whether an exactly scoped document forget took effect.
    ForgetDocument(ScopedMemoryDocumentId),
    /// Determine whether an exactly scoped document retain took effect.
    RetainDocument(RetainReconciliationTarget),
}

/// Opaque request-derived handle carried by an ambiguous mutation failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryReconciliationHandle {
    /// Complete trusted scope duplicated for uniform recovery inspection.
    scope: MemoryScope,
    /// Mutation-specific stable target; callers cannot substitute arbitrary scope.
    target: ReconcileTarget,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "reconciliation handles are constructed only from already-scoped requests"
)]
impl MemoryReconciliationHandle {
    /// Returns the complete scope bound into the reconciliation target.
    #[must_use]
    pub const fn scope(&self) -> &MemoryScope {
        &self.scope
    }

    /// Returns the exact target that may be inspected without replaying mutation.
    #[must_use]
    pub const fn target(&self) -> &ReconcileTarget {
        &self.target
    }
}

/// Explicit read-only request to resolve an ambiguous mutation outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconcileRequest {
    /// Exact request-derived mutation target to inspect.
    handle: MemoryReconciliationHandle,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "reconciliation request construction preserves the opaque scoped handle"
)]
impl ReconcileRequest {
    /// Returns the exact mutation target to inspect.
    #[must_use]
    pub const fn handle(&self) -> &MemoryReconciliationHandle {
        &self.handle
    }

    /// Creates a read-only reconciliation request without replay authority.
    #[must_use]
    pub const fn new(handle: MemoryReconciliationHandle) -> Self {
        Self { handle }
    }
}

/// Result of read-only mutation reconciliation; none of these states grants replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle every closed reconciliation state without automatic replay"
)]
pub enum ReconcileOutcome {
    /// The original mutation is proven to have taken effect.
    Applied,
    /// The original mutation is proven not to have taken effect.
    NotApplied,
    /// The backend still reports that the original mutation is pending.
    Pending,
    /// Available evidence cannot yet establish the original mutation outcome.
    StillUnknown,
}

/// Operation category retained in typed backend failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "operation context is a closed API surface used for stable diagnostics"
)]
pub enum MemoryOperationKind {
    /// Cancelling an asynchronous operation.
    Cancel,
    /// Forgetting one stable document.
    Forget,
    /// Inspecting an asynchronous operation.
    OperationStatus,
    /// Recalling bounded advisory memories.
    Recall,
    /// Retaining one stable document.
    Retain,
}

/// Stable category for a recoverable memory-backend failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "adapter callers must classify every stable backend failure deliberately"
)]
pub enum MemoryBackendErrorKind {
    /// The caller cancelled before a definitive outcome.
    Cancelled,
    /// The configured deadline elapsed before a definitive outcome.
    DeadlineExceeded,
    /// The backend rejects the requested operation in its current state.
    NotCancellable,
    /// A dispatched mutation has an ambiguous durable outcome and requires reconciliation.
    OutcomeUnknown,
    /// A bounded response or protocol invariant was invalid.
    Protocol,
    /// Transport did not produce a usable response.
    Transport,
    /// The configured backend deliberately does not support this operation.
    Unsupported,
}

/// Whether a caller may retry a failed operation without first reconciling state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "retry handling is a closed policy contract at the memory port"
)]
pub enum MemoryRetryability {
    /// Retrying cannot resolve the failure.
    Permanent,
    /// A mutation may have reached the backend and must be reconciled first.
    ReconcileRequired,
    /// A read-only or pre-dispatch failure may be retried by the caller.
    Retryable,
}

/// A safe, bounded causal classification with no remote body or credential data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "safe causal categories intentionally exclude unbounded remote detail"
)]
pub enum MemorySafeCause {
    /// The transport did not establish a usable connection.
    Connection,
    /// The response was malformed or exceeded an adapter bound.
    Response,
    /// The backend returned an unexpected HTTP status.
    Status,
}

/// Typed, sanitized failure returned by a [`MemoryBackend`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryBackendError {
    /// Optional safe cause classification without remote body or credential data.
    cause: Option<MemorySafeCause>,
    /// Stable failure category.
    kind: MemoryBackendErrorKind,
    /// Operation that encountered the failure.
    operation: MemoryOperationKind,
    /// Exact read-only recovery handle present only for an ambiguous mutation.
    reconciliation: Option<MemoryReconciliationHandle>,
    /// Whether the caller may retry or must reconcile first.
    retryability: MemoryRetryability,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "the failure constructor, ambiguity factory, and diagnostic accessors are grouped by recovery flow"
)]
impl MemoryBackendError {
    /// Creates a typed non-ambiguous backend failure.
    const fn definitive(
        operation: MemoryOperationKind,
        kind: MemoryBackendErrorKind,
        retryability: MemoryRetryability,
        cause: Option<MemorySafeCause>,
    ) -> Self {
        Self {
            cause,
            kind,
            operation,
            reconciliation: None,
            retryability,
        }
    }

    /// Creates a caller-cancelled failure before any ambiguous mutation outcome.
    #[must_use]
    pub const fn cancelled(operation: MemoryOperationKind) -> Self {
        Self::definitive(
            operation,
            MemoryBackendErrorKind::Cancelled,
            MemoryRetryability::Permanent,
            None,
        )
    }

    /// Creates a deadline failure whose retry classification is explicit.
    #[must_use]
    pub const fn deadline_exceeded(
        operation: MemoryOperationKind,
        retryability: MemoryRetryability,
    ) -> Self {
        Self::definitive(
            operation,
            MemoryBackendErrorKind::DeadlineExceeded,
            retryability,
            None,
        )
    }

    /// Creates a non-cancellable terminal failure.
    #[must_use]
    pub const fn not_cancellable() -> Self {
        Self::definitive(
            MemoryOperationKind::Cancel,
            MemoryBackendErrorKind::NotCancellable,
            MemoryRetryability::Permanent,
            None,
        )
    }

    /// Creates the non-retryable ambiguity result required after a dispatched mutation.
    #[must_use]
    pub fn outcome_unknown(reconciliation: MemoryReconciliationHandle) -> Self {
        let operation = match *reconciliation.target() {
            ReconcileTarget::CancelOperation(_) => MemoryOperationKind::Cancel,
            ReconcileTarget::ForgetDocument(_) => MemoryOperationKind::Forget,
            ReconcileTarget::RetainDocument(_) => MemoryOperationKind::Retain,
        };
        Self {
            cause: None,
            kind: MemoryBackendErrorKind::OutcomeUnknown,
            operation,
            reconciliation: Some(reconciliation),
            retryability: MemoryRetryability::ReconcileRequired,
        }
    }

    /// Creates a permanent bounded-protocol failure.
    #[must_use]
    pub const fn protocol(operation: MemoryOperationKind, cause: Option<MemorySafeCause>) -> Self {
        Self::definitive(
            operation,
            MemoryBackendErrorKind::Protocol,
            MemoryRetryability::Permanent,
            cause,
        )
    }

    /// Creates a transport failure whose pre-dispatch/read retry classification is explicit.
    #[must_use]
    pub const fn transport(
        operation: MemoryOperationKind,
        retryability: MemoryRetryability,
        cause: Option<MemorySafeCause>,
    ) -> Self {
        Self::definitive(
            operation,
            MemoryBackendErrorKind::Transport,
            retryability,
            cause,
        )
    }

    /// Creates a permanent unsupported-operation failure.
    #[must_use]
    pub const fn unsupported(operation: MemoryOperationKind) -> Self {
        Self::definitive(
            operation,
            MemoryBackendErrorKind::Unsupported,
            MemoryRetryability::Permanent,
            None,
        )
    }

    /// Returns a stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self.kind {
            MemoryBackendErrorKind::Cancelled => "memory_backend_cancelled",
            MemoryBackendErrorKind::DeadlineExceeded => "memory_backend_deadline_exceeded",
            MemoryBackendErrorKind::Protocol => "memory_backend_protocol",
            MemoryBackendErrorKind::Transport => "memory_backend_transport",
            MemoryBackendErrorKind::OutcomeUnknown => "memory_backend_outcome_unknown",
            MemoryBackendErrorKind::NotCancellable => "memory_backend_not_cancellable",
            MemoryBackendErrorKind::Unsupported => "memory_backend_unsupported",
        }
    }

    /// Returns the safe causal classification, if one exists.
    #[must_use]
    pub const fn cause(&self) -> Option<MemorySafeCause> {
        self.cause
    }

    /// Returns the stable failure kind.
    #[must_use]
    pub const fn kind(&self) -> MemoryBackendErrorKind {
        self.kind
    }

    /// Returns the operation that failed.
    #[must_use]
    pub const fn operation(&self) -> MemoryOperationKind {
        self.operation
    }

    /// Returns the exact read-only recovery handle for an ambiguous mutation.
    #[must_use]
    pub const fn reconciliation(&self) -> Option<&MemoryReconciliationHandle> {
        self.reconciliation.as_ref()
    }

    /// Returns whether retry requires reconciliation first.
    #[must_use]
    pub const fn retryability(&self) -> MemoryRetryability {
        self.retryability
    }
}

impl fmt::Display for MemoryBackendError {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates directly to the stable error code"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Boxed future used to keep [`MemoryBackend`] object-safe and runtime-neutral.
pub type MemoryFuture<'operation, Output> =
    Pin<Box<dyn Future<Output = Result<Output, MemoryBackendError>> + Send + 'operation>>;

/// Swappable imperative boundary for bounded advisory memory operations.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the operations follow the externally visible retain, recall, forget, status, cancel lifecycle"
)]
pub trait MemoryBackend: Send + Sync {
    /// Retains exactly one stable document asynchronously.
    fn retain<'operation>(
        &'operation self,
        request: &'operation RetainRequest,
        options: &'operation MemoryRequestOptions,
    ) -> MemoryFuture<'operation, RetainOutcome>;

    /// Recalls bounded advisory memories for a prior turn only.
    fn recall<'operation>(
        &'operation self,
        request: &'operation RecallRequest,
        options: &'operation MemoryRequestOptions,
    ) -> MemoryFuture<'operation, RecallResult>;

    /// Forgets exactly one stable document.
    fn forget<'operation>(
        &'operation self,
        request: &'operation ForgetRequest,
        options: &'operation MemoryRequestOptions,
    ) -> MemoryFuture<'operation, ForgetOutcome>;

    /// Returns the sanitized lifecycle state of one asynchronous operation.
    fn operation_status<'operation>(
        &'operation self,
        request: &'operation OperationStatusRequest,
        options: &'operation MemoryRequestOptions,
    ) -> MemoryFuture<'operation, MemoryOperationStatus>;

    /// Reconciles an ambiguous mutation through read-only evidence without replaying it.
    fn reconcile<'operation>(
        &'operation self,
        request: &'operation ReconcileRequest,
        options: &'operation MemoryRequestOptions,
    ) -> MemoryFuture<'operation, ReconcileOutcome>;

    /// Attempts to cancel one asynchronous operation without replaying it.
    fn cancel<'operation>(
        &'operation self,
        request: &'operation CancelRequest,
        options: &'operation MemoryRequestOptions,
    ) -> MemoryFuture<'operation, CancelOutcome>;
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::{
        AgentId, ForgetRequest, MemoryBackendError, MemoryBankScope, MemoryContractError,
        MemoryDocumentId, MemoryId, MemoryKind, MemoryOperationId, MemoryRetryability, MemoryScope,
        MemoryTag, MemoryText, OwnerId, RecallCandidate, RecallItemBudget, RecallRequest,
        RecallResult, RecallTokenBudget, ReconcileTarget, RepositoryId, RetainOutcome,
        RetainRequest, SessionId, TaskId, TurnId,
    };

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixed semantic fixture literals must parse before a contract-focused test can exercise the result"
    )]
    fn parsed<T>(value: &str, parser: fn(&str) -> Result<T, MemoryContractError>) -> T {
        parser(value).expect("fixture identity is valid")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the complete fixed scope is a direct test fixture expression"
    )]
    fn scope() -> MemoryScope {
        MemoryScope::repository(
            parsed("owner", OwnerId::parse),
            parsed("repo", RepositoryId::parse),
            parsed("agent", AgentId::parse),
            parsed("session", SessionId::parse),
            parsed("task", TaskId::parse),
            parsed("turn-summary", MemoryKind::parse),
        )
    }

    #[test]
    fn repository_scope_derives_a_repository_bank_and_complete_tags() {
        let scope = scope();

        assert_eq!(scope.bank_scope(), MemoryBankScope::Repository);
        assert_eq!(scope.bank().as_str(), "tiber-repository-repo");
        assert_eq!(
            scope
                .strict_tags_for_turn(&parsed("turn-1", TurnId::parse))
                .as_slice()
                .iter()
                .map(MemoryTag::as_str)
                .collect::<Vec<_>>(),
            vec![
                "owner:owner",
                "repository:repo",
                "agent:agent",
                "session:session",
                "task:task",
                "kind:turn-summary",
                "turn:turn-1",
            ]
        );
    }

    #[test]
    fn semantic_values_reject_controls_and_oversized_input() {
        assert_eq!(
            AgentId::parse(" agent\u{0007}"),
            Err(MemoryContractError::Invalid)
        );
        assert_eq!(
            AgentId::parse(&"a".repeat(super::MAX_MEMORY_ID_BYTES.saturating_add(1))),
            Err(MemoryContractError::Invalid)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixed valid fixture budgets and request construction keep this test focused on result filtering"
    )]
    fn recall_drops_same_turn_cross_scope_and_over_budget_candidates() {
        let scope = scope();
        let current_turn = parsed("turn-current", TurnId::parse);
        let current_document = parsed("doc-current", MemoryDocumentId::parse);
        let request = RecallRequest::new(
            scope.clone(),
            current_turn.clone(),
            current_document.clone(),
            parsed("useful history", MemoryText::parse),
            RecallItemBudget::new(2).expect("fixture budget is valid"),
            RecallTokenBudget::new(8).expect("fixture budget is valid"),
        )
        .expect("fixture query is valid");
        let scope_tags = scope.strict_tags();
        let current_turn_tag = MemoryScope::turn_tag(&current_turn);
        let matching_tags = scope_tags.as_slice().to_vec();
        let mut same_turn_tags = matching_tags.clone();
        same_turn_tags.push(current_turn_tag);
        let mut cross_scope_tags = matching_tags.clone();
        cross_scope_tags.retain(|tag| tag.as_str() != "task:task");
        let candidates = vec![
            RecallCandidate::new(
                parsed("same-doc", MemoryId::parse),
                parsed("same document", MemoryText::parse),
                scope.backend_document_id(&current_document),
                matching_tags.clone(),
            ),
            RecallCandidate::new(
                parsed("same-turn", MemoryId::parse),
                parsed("same turn", MemoryText::parse),
                scope.backend_document_id(&parsed("doc-same-turn", MemoryDocumentId::parse)),
                same_turn_tags,
            ),
            RecallCandidate::new(
                parsed("cross", MemoryId::parse),
                parsed("cross scope", MemoryText::parse),
                scope.backend_document_id(&parsed("doc-cross", MemoryDocumentId::parse)),
                cross_scope_tags,
            ),
            RecallCandidate::new(
                parsed("valid", MemoryId::parse),
                parsed("small", MemoryText::parse),
                scope.backend_document_id(&parsed("doc-valid", MemoryDocumentId::parse)),
                matching_tags,
            ),
        ];

        let result = RecallResult::from_candidates(&request, candidates);

        assert_eq!(result.memories().len(), 1);
        assert_eq!(
            result.memories().first().map(|memory| memory.id().as_str()),
            Some("valid")
        );
        let serialized = serde_json::to_value(&result).expect("recall result serializes");
        assert_eq!(
            serialized.get("admitted_tokens"),
            Some(&serde_json::Value::from(result.admitted_tokens()))
        );
    }

    #[test]
    fn outcome_unknown_requires_reconciliation_instead_of_retry() {
        let request = RetainRequest::new(
            scope(),
            parsed("same-raw-document", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("content", MemoryText::parse),
            parsed("source", MemoryText::parse),
        );
        let error = MemoryBackendError::outcome_unknown(request.reconciliation_handle());

        assert_eq!(error.code(), "memory_backend_outcome_unknown");
        assert_eq!(error.retryability(), MemoryRetryability::ReconcileRequired);
        assert!(matches!(
            error
                .reconciliation()
                .map(super::MemoryReconciliationHandle::target),
            Some(ReconcileTarget::RetainDocument(_))
        ));
    }

    #[test]
    fn owner_global_document_namespace_distinguishes_equal_raw_ids_across_repositories() {
        let first_scope = MemoryScope::owner_global(
            parsed("owner", OwnerId::parse),
            parsed("repo-one", RepositoryId::parse),
            parsed("agent", AgentId::parse),
            parsed("session", SessionId::parse),
            parsed("task", TaskId::parse),
            parsed("turn-summary", MemoryKind::parse),
        );
        let second_scope = MemoryScope::owner_global(
            parsed("owner", OwnerId::parse),
            parsed("repo-two", RepositoryId::parse),
            parsed("agent", AgentId::parse),
            parsed("session", SessionId::parse),
            parsed("task", TaskId::parse),
            parsed("turn-summary", MemoryKind::parse),
        );
        let raw = parsed("same-raw-document", MemoryDocumentId::parse);

        let first = ForgetRequest::new(first_scope, raw.clone()).backend_document_id();
        let second = ForgetRequest::new(second_scope, raw).backend_document_id();

        assert_ne!(first.as_str(), second.as_str());
    }

    #[test]
    fn retain_reconciliation_distinguishes_equal_documents_from_different_turns() {
        let first = RetainRequest::new(
            scope(),
            parsed("stable-document", MemoryDocumentId::parse),
            parsed("turn-one", TurnId::parse),
            parsed("content", MemoryText::parse),
            parsed("source", MemoryText::parse),
        );
        let second = RetainRequest::new(
            scope(),
            parsed("stable-document", MemoryDocumentId::parse),
            parsed("turn-two", TurnId::parse),
            parsed("content", MemoryText::parse),
            parsed("source", MemoryText::parse),
        );

        assert_ne!(
            first.reconciliation_handle(),
            second.reconciliation_handle()
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "serialization of a fixed bounded fixture must succeed before this test can inspect its safe projection"
    )]
    fn retain_reconciliation_binds_exact_content_and_context_without_exposing_them() {
        let first = RetainRequest::new(
            scope(),
            parsed("stable-document", MemoryDocumentId::parse),
            parsed("same-turn", TurnId::parse),
            parsed("first retained content", MemoryText::parse),
            parsed("same source", MemoryText::parse),
        );
        let second = RetainRequest::new(
            scope(),
            parsed("stable-document", MemoryDocumentId::parse),
            parsed("same-turn", TurnId::parse),
            parsed("second retained content", MemoryText::parse),
            parsed("same source", MemoryText::parse),
        );
        let third = RetainRequest::new(
            scope(),
            parsed("stable-document", MemoryDocumentId::parse),
            parsed("same-turn", TurnId::parse),
            parsed("first retained content", MemoryText::parse),
            parsed("different source", MemoryText::parse),
        );
        let first_handle = first.reconciliation_handle();

        assert_ne!(first_handle, second.reconciliation_handle());
        assert_ne!(first_handle, third.reconciliation_handle());
        assert_ne!(first.expected_evidence(), second.expected_evidence());
        assert_ne!(first.expected_evidence(), third.expected_evidence());
        let debug = format!("{first_handle:?}");
        let serialized = serde_json::to_string(&first_handle)
            .expect("opaque reconciliation evidence remains serializable");
        assert!(!debug.contains(first.content().as_str()));
        assert!(!debug.contains(first.context().as_str()));
        assert!(!serialized.contains(first.content().as_str()));
        assert!(!serialized.contains(first.context().as_str()));
    }

    #[test]
    fn backend_document_namespace_rejects_a_different_scope() {
        let first_scope = scope();
        let other_scope = MemoryScope::repository(
            parsed("owner", OwnerId::parse),
            parsed("another-repo", RepositoryId::parse),
            parsed("agent", AgentId::parse),
            parsed("session", SessionId::parse),
            parsed("task", TaskId::parse),
            parsed("turn-summary", MemoryKind::parse),
        );
        let encoded = first_scope.backend_document_id(&parsed("document", MemoryDocumentId::parse));

        assert_eq!(
            other_scope.parse_backend_document_id(encoded.as_str()),
            Err(MemoryContractError::ScopeMismatch)
        );
    }

    #[test]
    fn operation_handles_carry_the_scope_that_accepted_them() {
        let request = RetainRequest::new(
            scope(),
            parsed("document", MemoryDocumentId::parse),
            parsed("turn", TurnId::parse),
            parsed("content", MemoryText::parse),
            parsed("source", MemoryText::parse),
        );
        let outcome = RetainOutcome::accepted(
            &request,
            parsed("backend-operation", MemoryOperationId::parse),
        );

        assert_eq!(outcome.operation().scope(), request.scope());
        assert_eq!(
            outcome.operation_id(),
            &parsed("backend-operation", MemoryOperationId::parse)
        );
    }
}
