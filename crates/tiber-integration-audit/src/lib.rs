//! Provider-neutral, redacted audit facts for memory and external-tool boundaries.

use core::fmt;

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tiber_external_tools_core::{
    AuthorizationContext, BoundReconciliationFailure, BoundReconciliationOutcome,
    BoundToolCallFailure, BoundToolCallOutcome, IdempotencyKey, IntegrationId, OwnerApprovalId,
    ReconciliationOutcome, ToolCallDenial, ToolCallOutcome, ToolClass, ToolName,
};
use tiber_memory_core::{
    CancelOutcome, CancelRequest, ForgetOutcome, ForgetRequest, MemoryBackendError,
    MemoryBackendErrorKind, MemoryOperationHandle, MemoryOperationKind, MemoryOperationState,
    MemoryOperationStatus, MemoryReconciliationHandle, MemoryRetryability, MemorySafeCause,
    MemoryScope, OperationStatusRequest, RecallRequest, RecallResult, ReconcileOutcome,
    ReconcileRequest, RetainEvidence, RetainOutcome, RetainRequest,
};

/// Maximum UTF-8 byte length accepted for an audit receipt identity.
const MAX_AUDIT_ID_BYTES: usize = 128;

/// Failure while parsing a caller-owned semantic audit identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "receipt identity refusal is a deliberately closed public contract"
)]
pub enum AuditIdentityError {
    /// The identity was empty after trimming.
    Empty,
    /// The identity was oversized or contained a non-semantic character.
    Invalid,
}

impl fmt::Display for AuditIdentityError {
    #[expect(
        clippy::implicit_return,
        reason = "display is a direct total projection of the stable code"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Empty => f.write_str("audit_identity_empty"),
            Self::Invalid => f.write_str("audit_identity_invalid"),
        }
    }
}

/// Caller-supplied semantic identity for one durable audit receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AuditReceiptId(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the validated identity parser and accessor remain grouped in boundary-use order"
)]
impl AuditReceiptId {
    /// Parses a bounded semantic receipt identity.
    ///
    /// # Errors
    ///
    /// Returns [`AuditIdentityError`] for an empty, oversized, or malformed identity.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, AuditIdentityError> {
        let canonical = value.trim();
        if canonical.is_empty() {
            return Err(AuditIdentityError::Empty);
        }
        if canonical.len() > MAX_AUDIT_ID_BYTES
            || !canonical.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(AuditIdentityError::Invalid);
        }
        Ok(Self(canonical.to_owned()))
    }

    /// Returns the canonical receipt identity.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Redacted, provider-neutral fact about one memory-boundary interaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "serialized fields follow audit readability rather than alphabetic order"
)]
pub struct MemoryAuditFact {
    /// Stable operation category.
    operation: MemoryOperationKind,
    /// Caller-owned durable receipt identity.
    receipt_id: AuditReceiptId,
    /// Trusted provenance and safe outcome detail.
    #[serde(flatten)]
    detail: MemoryAuditDetail,
}

/// Trusted memory provenance and safe operation outcome.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
#[expect(
    clippy::exhaustive_enums,
    clippy::arbitrary_source_item_ordering,
    reason = "every durable memory audit outcome must receive an explicit redaction decision"
)]
pub enum MemoryAuditDetail {
    /// A retain request was formed; evidence replaces its raw text fields.
    RetainRequested {
        /// Stable application document identity.
        document_id: tiber_memory_core::MemoryDocumentId,
        /// Opaque digest derived by memory core from content and context.
        evidence: RetainEvidence,
        /// Complete trusted memory scope.
        scope: MemoryScope,
        /// Source turn identity.
        turn_id: tiber_memory_core::TurnId,
    },
    /// A retain request was accepted asynchronously.
    RetainAccepted {
        /// Opaque digest derived by memory core from content and context.
        evidence: RetainEvidence,
        /// Backend operation identity with trusted scope.
        operation_handle: MemoryOperationHandle,
    },
    /// A recall completed after mandatory scope and budget filtering.
    RecallAdmitted {
        /// Number of admitted memories.
        admitted_count: usize,
        /// Estimated tokens admitted by the core result.
        admitted_tokens: usize,
        /// Stable document identity excluded from its own recall.
        current_document_id: tiber_memory_core::MemoryDocumentId,
        /// Current source turn excluded from its own recall.
        current_turn_id: tiber_memory_core::TurnId,
        /// Caller-selected item budget.
        item_budget: usize,
        /// Complete trusted memory scope.
        scope: MemoryScope,
        /// Caller-selected token budget.
        token_budget: usize,
    },
    /// One exactly scoped document-forget result.
    Forget {
        /// Stable application document identity.
        document_id: tiber_memory_core::MemoryDocumentId,
        /// Safe closed outcome, when acknowledged.
        result: Option<ForgetOutcome>,
        /// Complete trusted memory scope.
        scope: MemoryScope,
    },
    /// One asynchronous operation status observation.
    Status {
        /// Backend operation identity with trusted scope.
        operation_handle: MemoryOperationHandle,
        /// Safe closed status, when observed.
        state: Option<MemoryOperationState>,
    },
    /// One asynchronous cancellation result.
    Cancel {
        /// Safe closed outcome, when acknowledged.
        result: Option<CancelOutcome>,
        /// Backend operation identity with trusted scope.
        operation_handle: MemoryOperationHandle,
    },
    /// One read-only reconciliation result.
    Reconcile {
        /// The only request-derived target that may be inspected.
        handle: MemoryReconciliationHandle,
        /// Safe closed result, when observed.
        result: Option<ReconcileOutcome>,
    },
    /// A sanitized memory backend failure.
    Failed {
        /// Optional safe causal category.
        cause: Option<MemorySafeCause>,
        /// Stable backend error code.
        code: &'static str,
        /// Stable failure category.
        kind: MemoryBackendErrorKind,
        /// Exact recovery handle, present only for an ambiguous mutation.
        reconciliation: Option<MemoryReconciliationHandle>,
        /// Stable retry classification.
        retryability: MemoryRetryability,
    },
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "audit factories follow operation lifecycle order"
)]
impl MemoryAuditFact {
    /// Constructs a redacted retain-request fact.
    #[must_use]
    pub fn retain(receipt_id: AuditReceiptId, request: &RetainRequest) -> Self {
        Self {
            operation: MemoryOperationKind::Retain,
            receipt_id,
            detail: MemoryAuditDetail::RetainRequested {
                document_id: request.document_id().clone(),
                evidence: request.expected_evidence(),
                scope: request.scope().clone(),
                turn_id: request.turn_id().clone(),
            },
        }
    }

    /// Constructs a redacted accepted-retain fact.
    #[must_use]
    pub fn retain_accepted(
        receipt_id: AuditReceiptId,
        request: &RetainRequest,
        outcome: &RetainOutcome,
    ) -> Self {
        Self {
            operation: MemoryOperationKind::Retain,
            receipt_id,
            detail: MemoryAuditDetail::RetainAccepted {
                evidence: request.expected_evidence(),
                operation_handle: outcome.operation().clone(),
            },
        }
    }

    /// Constructs a redacted recall-result fact with counts and budgets only.
    #[must_use]
    pub fn recall(
        receipt_id: AuditReceiptId,
        request: &RecallRequest,
        result: &RecallResult,
    ) -> Self {
        Self {
            operation: MemoryOperationKind::Recall,
            receipt_id,
            detail: MemoryAuditDetail::RecallAdmitted {
                admitted_count: result.memories().len(),
                admitted_tokens: result.admitted_tokens(),
                current_document_id: request.current_document_id().clone(),
                current_turn_id: request.current_turn_id().clone(),
                item_budget: request.item_budget().get(),
                scope: request.scope().clone(),
                token_budget: request.token_budget().get(),
            },
        }
    }

    /// Constructs a scoped forget fact, with an optional acknowledged outcome.
    #[must_use]
    pub fn forget(
        receipt_id: AuditReceiptId,
        request: &ForgetRequest,
        result: Option<ForgetOutcome>,
    ) -> Self {
        Self {
            operation: MemoryOperationKind::Forget,
            receipt_id,
            detail: MemoryAuditDetail::Forget {
                document_id: request.document_id().clone(),
                result,
                scope: request.scope().clone(),
            },
        }
    }

    /// Constructs a scoped operation-status fact.
    #[must_use]
    pub fn status(
        receipt_id: AuditReceiptId,
        request: &OperationStatusRequest,
        status: Option<&MemoryOperationStatus>,
    ) -> Self {
        Self {
            operation: MemoryOperationKind::OperationStatus,
            receipt_id,
            detail: MemoryAuditDetail::Status {
                operation_handle: request.operation().clone(),
                state: status.map(MemoryOperationStatus::state),
            },
        }
    }

    /// Constructs a scoped cancellation fact.
    #[must_use]
    pub fn cancel(
        receipt_id: AuditReceiptId,
        request: &CancelRequest,
        result: Option<CancelOutcome>,
    ) -> Self {
        Self {
            operation: MemoryOperationKind::Cancel,
            receipt_id,
            detail: MemoryAuditDetail::Cancel {
                result,
                operation_handle: request.operation().clone(),
            },
        }
    }

    /// Constructs a fact for read-only reconciliation without granting replay authority.
    #[must_use]
    pub fn reconcile(
        receipt_id: AuditReceiptId,
        request: &ReconcileRequest,
        result: Option<ReconcileOutcome>,
    ) -> Self {
        let operation = match *request.handle().target() {
            tiber_memory_core::ReconcileTarget::CancelOperation(_) => MemoryOperationKind::Cancel,
            tiber_memory_core::ReconcileTarget::ForgetDocument(_) => MemoryOperationKind::Forget,
            tiber_memory_core::ReconcileTarget::RetainDocument(_) => MemoryOperationKind::Retain,
        };
        Self {
            operation,
            receipt_id,
            detail: MemoryAuditDetail::Reconcile {
                handle: request.handle().clone(),
                result,
            },
        }
    }

    /// Constructs a sanitized failure fact; ambiguous failures retain only their recovery handle.
    #[must_use]
    pub fn failed(receipt_id: AuditReceiptId, error: &MemoryBackendError) -> Self {
        Self {
            operation: error.operation(),
            receipt_id,
            detail: MemoryAuditDetail::Failed {
                cause: error.cause(),
                code: error.code(),
                kind: error.kind(),
                reconciliation: error.reconciliation().cloned(),
                retryability: error.retryability(),
            },
        }
    }
}

/// Stable, bounded caller-owned code for a typed external adapter failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ExternalFailureCode(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the validated failure-code parser and accessor remain grouped in boundary-use order"
)]
impl ExternalFailureCode {
    /// Parses a bounded semantic safe failure code.
    ///
    /// # Errors
    ///
    /// Returns [`AuditIdentityError`] for an empty, oversized, or malformed code.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, AuditIdentityError> {
        let canonical = value.trim();
        if canonical.is_empty() {
            return Err(AuditIdentityError::Empty);
        }
        if canonical.len() > MAX_AUDIT_ID_BYTES
            || !canonical.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(AuditIdentityError::Invalid);
        }
        Ok(Self(canonical.to_owned()))
    }

    /// Returns the canonical safe failure code.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider-neutral external-tool operation retained in safe failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "audit consumers must handle invocation and reconciliation failures explicitly"
)]
pub enum ExternalToolOperation {
    /// Invoke an authorized configured tool.
    Invoke,
    /// Invoke an authorized read-only reconciliation tool.
    Reconcile,
}

/// Provider-neutral retry classification for an external-tool failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    clippy::exhaustive_enums,
    reason = "audit consumers must handle every safe retry classification explicitly"
)]
pub enum ExternalToolRetryability {
    /// Retrying cannot resolve the failure.
    Permanent,
    /// Reconciliation must establish mutation state before any retry.
    ReconcileRequired,
    /// The caller may retry under its bounded policy.
    Retryable,
}

/// Typed safe external-tool failure projected by an imperative adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExternalToolFailure {
    /// Stable bounded safe failure code.
    code: ExternalFailureCode,
    /// Safe retry classification.
    retryability: ExternalToolRetryability,
}

#[expect(
    clippy::implicit_return,
    reason = "the safe failure constructor directly returns an immutable value object"
)]
impl ExternalToolFailure {
    /// Creates a typed safe failure with no raw adapter or server message.
    #[must_use]
    #[inline]
    pub const fn new(code: ExternalFailureCode, retryability: ExternalToolRetryability) -> Self {
        Self { code, retryability }
    }
}

/// Provider-neutral, redacted fact about one authorized external-tool interaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "serialized fields follow authority and audit readability rather than alphabetic order"
)]
pub struct ExternalToolAuditFact {
    /// Explicit owner approval identity, when already present on authorization.
    approval: Option<OwnerApprovalId>,
    /// Trusted authorization tuple supplied at the policy boundary.
    authorization: AuthorizationContext,
    /// Trusted configured tool effect class.
    class: Option<ToolClass>,
    /// Trusted integration identity without configuration or transport details.
    integration_id: IntegrationId,
    /// Stable mutation idempotency identity, when already present on authorization.
    idempotency_key: Option<IdempotencyKey>,
    /// Exact mutating tool whose outcome a reconciliation operation checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    originating_tool: Option<ToolName>,
    /// Caller-owned durable receipt identity.
    receipt_id: AuditReceiptId,
    /// Exact trusted configured tool name.
    tool: ToolName,
    /// Safe invocation outcome.
    #[serde(flatten)]
    outcome: ExternalToolAuditOutcome,
}

/// Redacted external-tool outcome with no arguments, payload, or server message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
#[expect(
    clippy::exhaustive_enums,
    clippy::arbitrary_source_item_ordering,
    reason = "every external-tool outcome must receive an explicit redaction decision"
)]
pub enum ExternalToolAuditOutcome {
    /// Authorization was denied by a stable core policy error.
    Denied {
        /// Stable policy error code.
        code: &'static str,
    },
    /// An authorized invocation returned bounded untrusted data, represented only by metadata.
    Observed {
        /// Number of UTF-8 payload bytes digested.
        byte_count: usize,
        /// Domain-separated SHA-256 hexadecimal digest.
        payload_sha256: String,
    },
    /// An authorized invocation failed with a caller-projected safe typed code.
    Failed {
        /// Stable safe failure code.
        code: ExternalFailureCode,
        /// Provider-neutral operation that failed.
        operation: ExternalToolOperation,
        /// Safe retry classification.
        retryability: ExternalToolRetryability,
    },
    /// A mutation outcome is unknown and carries only stable recovery identity.
    Unknown {
        /// Original stable idempotency identity.
        idempotency_key: IdempotencyKey,
        /// Configured read-only reconciliation tool.
        reconciliation_tool: ToolName,
    },
    /// A read-only reconciliation reached a safe closed state.
    Reconciled {
        /// Original stable idempotency identity.
        idempotency_key: IdempotencyKey,
        /// Closed reconciliation state.
        state: ReconciliationOutcome,
    },
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "factories follow authorization and recovery lifecycle order and return immutable value objects"
)]
impl ExternalToolAuditFact {
    /// Projects a caller-supplied bound outcome into a redacted audit fact.
    ///
    /// This projection is not independent proof that an adapter dispatched the
    /// call. Production callers should pass the [`BoundToolCallOutcome`]
    /// returned by the concrete adapter that performed the invocation.
    #[must_use]
    pub fn completed(receipt_id: AuditReceiptId, bound_outcome: &BoundToolCallOutcome) -> Self {
        let safe_outcome = match bound_outcome.outcome() {
            ToolCallOutcome::Observed(payload) => {
                let bytes = payload.as_str().as_bytes();
                let mut digest = Sha256::new();
                digest.update(b"tiber-external-tool-payload-v1\0");
                digest.update(bytes);
                ExternalToolAuditOutcome::Observed {
                    byte_count: bytes.len(),
                    payload_sha256: format!("{:x}", digest.finalize()),
                }
            }
            ToolCallOutcome::OutcomeUnknown(reconciliation) => ExternalToolAuditOutcome::Unknown {
                idempotency_key: reconciliation.idempotency_key().clone(),
                reconciliation_tool: reconciliation.status_tool().clone(),
            },
        };
        Self::from_bound_outcome(receipt_id, bound_outcome, safe_outcome)
    }

    /// Constructs a denied fact when no authorized call token exists.
    #[must_use]
    pub fn denied(receipt_id: AuditReceiptId, denial: &ToolCallDenial) -> Self {
        Self {
            approval: None,
            authorization: denial.authorization().clone(),
            class: denial.class(),
            idempotency_key: None,
            integration_id: denial.integration_id().clone(),
            originating_tool: None,
            receipt_id,
            tool: denial.tool().clone(),
            outcome: ExternalToolAuditOutcome::Denied {
                code: denial.error().code(),
            },
        }
    }

    /// Constructs a sanitized failure fact for an already-authorized call.
    #[must_use]
    pub fn failed(
        receipt_id: AuditReceiptId,
        provenance: &BoundToolCallFailure,
        failure: ExternalToolFailure,
    ) -> Self {
        Self::from_failure(
            receipt_id,
            provenance,
            ExternalToolAuditOutcome::Failed {
                code: failure.code,
                operation: ExternalToolOperation::Invoke,
                retryability: failure.retryability,
            },
        )
    }

    /// Constructs a closed reconciliation-result fact.
    #[must_use]
    pub fn reconciled(
        receipt_id: AuditReceiptId,
        reconciliation: &BoundReconciliationOutcome,
    ) -> Self {
        Self {
            approval: reconciliation.approval().cloned(),
            authorization: reconciliation.authorization().clone(),
            class: Some(ToolClass::Observe),
            idempotency_key: Some(reconciliation.idempotency_key().clone()),
            integration_id: reconciliation.integration_id().clone(),
            originating_tool: Some(reconciliation.originating_tool().clone()),
            receipt_id,
            tool: reconciliation.status_tool().clone(),
            outcome: ExternalToolAuditOutcome::Reconciled {
                idempotency_key: reconciliation.idempotency_key().clone(),
                state: reconciliation.outcome(),
            },
        }
    }

    /// Constructs a sanitized failure fact for an authorized reconciliation attempt.
    #[must_use]
    pub fn reconciliation_failed(
        receipt_id: AuditReceiptId,
        reconciliation: &BoundReconciliationFailure,
        failure: ExternalToolFailure,
    ) -> Self {
        Self {
            approval: reconciliation.approval().cloned(),
            authorization: reconciliation.authorization().clone(),
            class: Some(ToolClass::Observe),
            idempotency_key: Some(reconciliation.idempotency_key().clone()),
            integration_id: reconciliation.integration_id().clone(),
            originating_tool: Some(reconciliation.originating_tool().clone()),
            receipt_id,
            tool: reconciliation.status_tool().clone(),
            outcome: ExternalToolAuditOutcome::Failed {
                code: failure.code,
                operation: ExternalToolOperation::Reconcile,
                retryability: failure.retryability,
            },
        }
    }

    /// Copies only trusted identity and authority fields from an opaque bound outcome.
    fn from_bound_outcome(
        receipt_id: AuditReceiptId,
        bound_outcome: &BoundToolCallOutcome,
        outcome: ExternalToolAuditOutcome,
    ) -> Self {
        Self {
            approval: bound_outcome.approval().cloned(),
            authorization: bound_outcome.authorization().clone(),
            class: Some(bound_outcome.class()),
            idempotency_key: bound_outcome.idempotency_key().cloned(),
            integration_id: bound_outcome.integration_id().clone(),
            originating_tool: None,
            receipt_id,
            tool: bound_outcome.tool().clone(),
            outcome,
        }
    }

    /// Copies only trusted identity fields from a consumed, non-replayable failure transcript.
    fn from_failure(
        receipt_id: AuditReceiptId,
        provenance: &BoundToolCallFailure,
        outcome: ExternalToolAuditOutcome,
    ) -> Self {
        Self {
            approval: provenance.approval().cloned(),
            authorization: provenance.authorization().clone(),
            class: Some(provenance.class()),
            idempotency_key: provenance.idempotency_key().cloned(),
            integration_id: provenance.integration_id().clone(),
            originating_tool: None,
            receipt_id,
            tool: provenance.tool().clone(),
            outcome,
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::default_numeric_fallback,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::indexing_slicing,
    clippy::let_and_return,
    clippy::panic,
    reason = "behavior fixtures fail loudly and inspect known JSON object fields for concise redaction assertions"
)]
mod tests {
    use serde_json::json;
    use tiber_external_tools_core::{
        AbsoluteProgram, AgentRole, AssignmentId, AuthorizationContext, ConfiguredTool,
        ExternalToolCapability, IdempotencyKey, IntegrationId, LiteralArgument, McpIntegration,
        McpTransport, OwnerApprovalId, PermissionGrant, PolicyDecisionId, PolicyIntersection,
        ReconciliationOutcome, ScopedPermission, SessionId as ToolSessionId, ToolArguments,
        ToolCallAuthorizationDecision, ToolCallProposal, ToolClass, ToolName, UntrustedPayload,
        WorkflowMode, authorize_tool_call, decide_tool_call,
    };
    use tiber_memory_core::{
        AgentId, CancelOutcome, CancelRequest, ForgetOutcome, ForgetRequest, MemoryBackendError,
        MemoryDocumentId, MemoryKind, MemoryOperationId, MemoryOperationState,
        MemoryOperationStatus, MemoryScope, MemoryText, OperationStatusRequest, OwnerId,
        RecallCandidate, RecallItemBudget, RecallRequest, RecallResult, RecallTokenBudget,
        ReconcileOutcome, ReconcileRequest, RepositoryId, RetainOutcome, RetainRequest, SessionId,
        TaskId, TurnId,
    };

    use super::{
        AuditReceiptId, ExternalFailureCode, ExternalToolAuditFact, ExternalToolFailure,
        ExternalToolRetryability, MemoryAuditFact,
    };

    fn tool_text<T>(
        parser: impl FnOnce(&str) -> Result<T, tiber_external_tools_core::ExternalToolError>,
        value: &str,
    ) -> T {
        parser(value).expect("external-tool fixture identity is valid")
    }

    fn authorization_context() -> AuthorizationContext {
        AuthorizationContext::new(
            tool_text(WorkflowMode::parse, "review"),
            tool_text(AgentRole::parse, "reviewer"),
            tool_text(ToolSessionId::parse, "tool-session-1"),
            tool_text(AssignmentId::parse, "assignment-1"),
            tool_text(PolicyDecisionId::parse, "policy-1"),
        )
    }

    fn memory_scope() -> MemoryScope {
        MemoryScope::repository(
            OwnerId::parse("owner-1").expect("valid owner"),
            RepositoryId::parse("repo-1").expect("valid repository"),
            AgentId::parse("agent-1").expect("valid agent"),
            SessionId::parse("session-1").expect("valid session"),
            TaskId::parse("task-1").expect("valid task"),
            MemoryKind::parse("decision").expect("valid memory kind"),
        )
    }

    fn retain_request() -> RetainRequest {
        RetainRequest::new(
            memory_scope(),
            MemoryDocumentId::parse("document-1").expect("valid document"),
            TurnId::parse("turn-1").expect("valid turn"),
            MemoryText::parse("super-secret-memory").expect("valid content"),
            MemoryText::parse("super-secret-context").expect("valid context"),
        )
    }

    fn authorized_observation() -> tiber_external_tools_core::AuthorizedToolCall {
        let tool = tool_text(ToolName::parse, "read_status");
        let integration = McpIntegration::new(
            tool_text(IntegrationId::parse, "local-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/example").expect("absolute program"),
                arguments: vec![LiteralArgument::parse("--mcp").expect("literal argument")],
            },
            [ConfiguredTool::new(tool.clone(), ToolClass::Observe)],
            None,
        )
        .expect("valid integration");
        let context = authorization_context();
        let grant = PermissionGrant::new([tool.clone()], [ExternalToolCapability::InvokeTools]);
        let policy = PolicyIntersection::new(
            &integration,
            grant.clone(),
            ScopedPermission::new(tool_text(WorkflowMode::parse, "review"), grant.clone()),
            ScopedPermission::new(tool_text(AgentRole::parse, "reviewer"), grant.clone()),
            ScopedPermission::new(
                tool_text(ToolSessionId::parse, "tool-session-1"),
                grant.clone(),
            ),
            ScopedPermission::new(
                tool_text(AssignmentId::parse, "assignment-1"),
                grant.clone(),
            ),
            ScopedPermission::new(tool_text(PolicyDecisionId::parse, "policy-1"), grant),
        );
        let call = authorize_tool_call(
            &integration,
            &policy,
            &context,
            ToolCallProposal::new(
                tool,
                ToolArguments::parse(r#"{"raw_argument":"do-not-audit"}"#)
                    .expect("valid arguments"),
                None,
            ),
            None,
        )
        .expect("observation is authorized");
        call
    }

    fn authorized_mutation() -> tiber_external_tools_core::AuthorizedToolCall {
        authorized_mutation_with("mutation-1", "approval-1")
    }

    fn authorized_mutation_with(
        idempotency_key: &str,
        approval: &str,
    ) -> tiber_external_tools_core::AuthorizedToolCall {
        let mutation_tool = tool_text(ToolName::parse, "apply_change");
        let status_tool = tool_text(ToolName::parse, "mutation_status");
        let integration = McpIntegration::new(
            tool_text(IntegrationId::parse, "local-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/example").expect("absolute program"),
                arguments: vec![LiteralArgument::parse("--mcp").expect("literal argument")],
            },
            [
                ConfiguredTool::new(mutation_tool.clone(), ToolClass::Mutate),
                ConfiguredTool::new(status_tool.clone(), ToolClass::Observe),
            ],
            Some(status_tool.clone()),
        )
        .expect("valid integration");
        let context = authorization_context();
        let grant = PermissionGrant::new(
            [mutation_tool.clone(), status_tool],
            [
                ExternalToolCapability::InvokeTools,
                ExternalToolCapability::ReconcileMutations,
            ],
        );
        let policy = PolicyIntersection::new(
            &integration,
            grant.clone(),
            ScopedPermission::new(tool_text(WorkflowMode::parse, "review"), grant.clone()),
            ScopedPermission::new(tool_text(AgentRole::parse, "reviewer"), grant.clone()),
            ScopedPermission::new(
                tool_text(ToolSessionId::parse, "tool-session-1"),
                grant.clone(),
            ),
            ScopedPermission::new(
                tool_text(AssignmentId::parse, "assignment-1"),
                grant.clone(),
            ),
            ScopedPermission::new(tool_text(PolicyDecisionId::parse, "policy-1"), grant),
        );
        authorize_tool_call(
            &integration,
            &policy,
            &context,
            ToolCallProposal::new(
                mutation_tool,
                ToolArguments::parse(r#"{"raw_argument":"do-not-audit"}"#)
                    .expect("valid arguments"),
                Some(tool_text(IdempotencyKey::parse, idempotency_key)),
            ),
            Some(tool_text(OwnerApprovalId::parse, approval)),
        )
        .expect("mutation is authorized")
    }

    #[test]
    fn retain_fact_serializes_evidence_and_provenance_without_raw_memory_text() {
        let request = retain_request();

        let fact = MemoryAuditFact::retain(
            AuditReceiptId::parse("audit-1").expect("valid receipt"),
            &request,
        );
        let serialized = serde_json::to_value(&fact).expect("audit fact serializes");

        assert_eq!(serialized["receipt_id"], json!("audit-1"));
        assert_eq!(serialized["operation"], json!("retain"));
        assert_eq!(serialized["document_id"], json!("document-1"));
        assert_eq!(serialized["turn_id"], json!("turn-1"));
        assert!(serialized.get("evidence").is_some());
        let text = serialized.to_string();
        assert!(!text.contains("super-secret-memory"));
        assert!(!text.contains("super-secret-context"));
    }

    #[test]
    fn accepted_retain_fact_preserves_operation_and_evidence_without_raw_text() {
        let request = retain_request();
        let outcome = RetainOutcome::accepted(
            &request,
            MemoryOperationId::parse("operation-1").expect("valid operation"),
        );
        let fact = MemoryAuditFact::retain_accepted(
            AuditReceiptId::parse("audit-retain-accepted").expect("valid receipt"),
            &request,
            &outcome,
        );
        let serialized = serde_json::to_value(&fact).expect("audit fact serializes");

        assert_eq!(serialized["operation"], json!("retain"));
        assert_eq!(
            serialized["operation_handle"]["operation_id"],
            json!("operation-1")
        );
        let text = serialized.to_string();
        assert!(!text.contains("super-secret-memory"));
        assert!(!text.contains("super-secret-context"));
    }

    #[test]
    fn forget_fact_preserves_exact_document_scope_and_closed_result() {
        let request = ForgetRequest::new(
            memory_scope(),
            MemoryDocumentId::parse("document-1").expect("valid document"),
        );
        let fact = MemoryAuditFact::forget(
            AuditReceiptId::parse("audit-forget").expect("valid receipt"),
            &request,
            Some(ForgetOutcome::Forgotten),
        );
        let serialized = serde_json::to_value(fact).expect("audit fact serializes");

        assert_eq!(serialized["document_id"], json!("document-1"));
        assert_eq!(serialized["result"], json!("Forgotten"));
    }

    #[test]
    fn status_and_cancel_facts_preserve_scoped_operation_and_closed_results() {
        let retain = retain_request();
        let accepted = RetainOutcome::accepted(
            &retain,
            MemoryOperationId::parse("operation-1").expect("valid operation"),
        );
        let status_request = OperationStatusRequest::new(accepted.operation().clone());
        let status = MemoryOperationStatus::new(&status_request, MemoryOperationState::Processing);
        let status_fact = MemoryAuditFact::status(
            AuditReceiptId::parse("audit-status").expect("valid receipt"),
            &status_request,
            Some(&status),
        );
        let cancel_request = CancelRequest::new(accepted.operation().clone());
        let cancel_fact = MemoryAuditFact::cancel(
            AuditReceiptId::parse("audit-cancel").expect("valid receipt"),
            &cancel_request,
            Some(CancelOutcome::Cancelled),
        );
        let status_json = serde_json::to_value(status_fact).expect("status fact serializes");
        let cancel_json = serde_json::to_value(cancel_fact).expect("cancel fact serializes");

        assert_eq!(status_json["state"], json!("processing"));
        assert_eq!(cancel_json["result"], json!("Cancelled"));
        assert_eq!(
            cancel_json["operation_handle"]["operation_id"],
            json!("operation-1")
        );
    }

    #[test]
    fn reconciliation_and_unknown_failure_retain_only_existing_recovery_handle() {
        let request = retain_request();
        let handle = request.reconciliation_handle();
        let reconcile_request = ReconcileRequest::new(handle.clone());
        let reconciled = MemoryAuditFact::reconcile(
            AuditReceiptId::parse("audit-memory-reconcile").expect("valid receipt"),
            &reconcile_request,
            Some(ReconcileOutcome::StillUnknown),
        );
        let error = MemoryBackendError::outcome_unknown(handle);
        let failed = MemoryAuditFact::failed(
            AuditReceiptId::parse("audit-memory-failed").expect("valid receipt"),
            &error,
        );
        let reconciled_json = serde_json::to_value(reconciled).expect("reconcile fact serializes");
        let failed_json = serde_json::to_value(failed).expect("failure fact serializes");

        assert_eq!(reconciled_json["result"], json!("StillUnknown"));
        assert_eq!(failed_json["code"], json!("memory_backend_outcome_unknown"));
        assert_eq!(failed_json["retryability"], json!("reconcile_required"));
        let text = format!("{reconciled_json}{failed_json}");
        assert!(!text.contains("super-secret-memory"));
        assert!(!text.contains("super-secret-context"));
    }

    #[test]
    fn recall_fact_serializes_budgets_and_admission_totals_without_query_or_text() {
        let scope = MemoryScope::repository(
            OwnerId::parse("owner-1").expect("valid owner"),
            RepositoryId::parse("repo-1").expect("valid repository"),
            AgentId::parse("agent-1").expect("valid agent"),
            SessionId::parse("session-1").expect("valid session"),
            TaskId::parse("task-1").expect("valid task"),
            MemoryKind::parse("decision").expect("valid memory kind"),
        );
        let request = RecallRequest::new(
            scope.clone(),
            TurnId::parse("turn-current").expect("valid turn"),
            MemoryDocumentId::parse("document-current").expect("valid document"),
            MemoryText::parse("secret-query").expect("valid query"),
            RecallItemBudget::new(4).expect("valid item budget"),
            RecallTokenBudget::new(100).expect("valid token budget"),
        )
        .expect("valid recall request");
        let candidate = RecallCandidate::new(
            tiber_memory_core::MemoryId::parse("memory-1").expect("valid memory"),
            MemoryText::parse("secret-recalled-text").expect("valid recalled text"),
            scope.backend_document_id(
                &MemoryDocumentId::parse("document-source").expect("valid source document"),
            ),
            scope.strict_tags().as_slice().to_vec(),
        );
        let result = RecallResult::from_candidates(&request, vec![candidate]);

        let fact = MemoryAuditFact::recall(
            AuditReceiptId::parse("audit-recall").expect("valid receipt"),
            &request,
            &result,
        );
        let serialized = serde_json::to_value(&fact).expect("audit fact serializes");

        assert_eq!(serialized["item_budget"], json!(4));
        assert_eq!(serialized["token_budget"], json!(100));
        assert_eq!(serialized["admitted_count"], json!(1));
        assert_eq!(
            serialized["admitted_tokens"],
            json!(result.admitted_tokens())
        );
        assert_eq!(serialized["current_document_id"], json!("document-current"));
        assert_eq!(serialized["current_turn_id"], json!("turn-current"));
        let text = serialized.to_string();
        assert!(!text.contains("secret-query"));
        assert!(!text.contains("secret-recalled-text"));
    }

    #[test]
    fn observed_tool_fact_digests_payload_and_omits_arguments_and_payload() {
        let call = authorized_observation();
        let bound_outcome = call.bind_observation(
            UntrustedPayload::bounded("secret-server-payload").expect("bounded payload"),
        );
        let fact = ExternalToolAuditFact::completed(
            AuditReceiptId::parse("audit-tool").expect("valid receipt"),
            &bound_outcome,
        );
        let serialized = serde_json::to_value(&fact).expect("audit fact serializes");

        assert_eq!(serialized["integration_id"], json!("local-tools"));
        assert_eq!(serialized["tool"], json!("read_status"));
        assert_eq!(serialized["class"], json!("Observe"));
        assert_eq!(serialized["byte_count"], json!(21));
        assert_eq!(
            serialized["payload_sha256"],
            json!("ae0d42a4694d712b67eefbf16339b926ce23dc62d0d04096c0093b79a2f95712")
        );
        let text = serialized.to_string();
        assert!(!text.contains("do-not-audit"));
        assert!(!text.contains("secret-server-payload"));
        assert!(!format!("{fact:?}").contains("secret-server-payload"));
    }

    #[test]
    fn denied_tool_fact_contains_only_stable_error_and_trusted_context() {
        let configured_tool = tool_text(ToolName::parse, "read_status");
        let integration = McpIntegration::new(
            tool_text(IntegrationId::parse, "local-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/example").expect("absolute program"),
                arguments: vec![LiteralArgument::parse("--mcp").expect("literal argument")],
            },
            [ConfiguredTool::new(configured_tool, ToolClass::Observe)],
            None,
        )
        .expect("valid integration");
        let context = authorization_context();
        let empty_grant = PermissionGrant::new([], []);
        let policy = PolicyIntersection::new(
            &integration,
            empty_grant.clone(),
            ScopedPermission::new(
                tool_text(WorkflowMode::parse, "review"),
                empty_grant.clone(),
            ),
            ScopedPermission::new(tool_text(AgentRole::parse, "reviewer"), empty_grant.clone()),
            ScopedPermission::new(
                tool_text(ToolSessionId::parse, "tool-session-1"),
                empty_grant.clone(),
            ),
            ScopedPermission::new(
                tool_text(AssignmentId::parse, "assignment-1"),
                empty_grant.clone(),
            ),
            ScopedPermission::new(tool_text(PolicyDecisionId::parse, "policy-1"), empty_grant),
        );
        let decision = decide_tool_call(
            &integration,
            &policy,
            &context,
            ToolCallProposal::new(
                tool_text(ToolName::parse, "apply_change"),
                ToolArguments::parse(r#"{"raw_argument":"do-not-audit"}"#)
                    .expect("valid arguments"),
                None,
            ),
            None,
        );
        let ToolCallAuthorizationDecision::Denied(denial) = decision else {
            panic!("fixture must be denied");
        };
        let fact = ExternalToolAuditFact::denied(
            AuditReceiptId::parse("audit-denied").expect("valid receipt"),
            &denial,
        );
        let serialized = serde_json::to_value(fact).expect("audit fact serializes");

        assert_eq!(serialized["code"], json!("external_tools_unknown_tool"));
        assert!(!serialized.to_string().contains("do-not-audit"));
        assert!(serialized.get("arguments").is_none());
        assert!(serialized.get("transport").is_none());
        assert!(serialized.get("roots").is_none());
    }

    #[test]
    fn failed_tool_fact_preserves_typed_safe_code_operation_and_retryability() {
        let failure_provenance = authorized_observation().bind_failure();
        let failure = ExternalToolFailure::new(
            ExternalFailureCode::parse("rmcp-transport").expect("valid failure code"),
            ExternalToolRetryability::Retryable,
        );
        let fact = ExternalToolAuditFact::failed(
            AuditReceiptId::parse("audit-failed").expect("valid receipt"),
            &failure_provenance,
            failure,
        );
        let serialized = serde_json::to_value(&fact).expect("audit fact serializes");

        assert_eq!(serialized["code"], json!("rmcp-transport"));
        assert_eq!(serialized["operation"], json!("invoke"));
        assert_eq!(serialized["retryability"], json!("retryable"));
        assert!(!serialized.to_string().contains("do-not-audit"));
        assert!(!format!("{fact:?}").contains("do-not-audit"));
    }

    #[test]
    fn unknown_tool_fact_uses_only_bound_reconciliation_identity() {
        let call = authorized_mutation();
        let bound_outcome = call
            .bind_ambiguity()
            .unwrap_or_else(|_failure| panic!("mutation must bind its own ambiguity"));
        let fact = ExternalToolAuditFact::completed(
            AuditReceiptId::parse("audit-unknown").expect("valid receipt"),
            &bound_outcome,
        );
        let serialized = serde_json::to_value(&fact).expect("audit fact serializes");

        assert_eq!(serialized["outcome"], json!("unknown"));
        assert_eq!(serialized["idempotency_key"], json!("mutation-1"));
        assert_eq!(serialized["reconciliation_tool"], json!("mutation_status"));
        assert!(!serialized.to_string().contains("do-not-audit"));
        assert!(!format!("{fact:?}").contains("do-not-audit"));
    }

    #[test]
    fn reconciliation_facts_preserve_token_context_and_safe_states_only() {
        let call = authorized_mutation();
        let reconciliation = call.reconciliation().expect("mutation has reconciliation");
        let bound_outcome = reconciliation.bind_outcome(ReconciliationOutcome::Committed);
        let bound_failure = reconciliation.bind_failure();
        let reconciled = ExternalToolAuditFact::reconciled(
            AuditReceiptId::parse("audit-reconciled").expect("valid receipt"),
            &bound_outcome,
        );
        let failed = ExternalToolAuditFact::reconciliation_failed(
            AuditReceiptId::parse("audit-reconcile-failed").expect("valid receipt"),
            &bound_failure,
            ExternalToolFailure::new(
                ExternalFailureCode::parse("rmcp-timeout").expect("valid failure code"),
                ExternalToolRetryability::Retryable,
            ),
        );
        let reconciled_json =
            serde_json::to_value(&reconciled).expect("reconciled fact serializes");
        let failed_json = serde_json::to_value(&failed).expect("failed fact serializes");

        assert_eq!(reconciled_json["state"], json!("Committed"));
        assert_eq!(reconciled_json["tool"], json!("mutation_status"));
        assert_eq!(reconciled_json["originating_tool"], json!("apply_change"));
        assert_eq!(reconciled_json["approval"], json!("approval-1"));
        assert_eq!(failed_json["operation"], json!("reconcile"));
        assert_eq!(failed_json["retryability"], json!("retryable"));
        assert_eq!(failed_json["originating_tool"], json!("apply_change"));
        assert_eq!(failed_json["approval"], json!("approval-1"));
        assert!(!format!("{reconciled:?}{failed:?}").contains("do-not-audit"));
    }

    #[test]
    fn reconciliation_fact_cannot_attribute_one_mutation_outcome_to_another_token() {
        let first_reconciliation = authorized_mutation_with("mutation-1", "approval-1")
            .reconciliation()
            .expect("first mutation has reconciliation");
        let second_reconciliation = authorized_mutation_with("mutation-2", "approval-2")
            .reconciliation()
            .expect("second mutation has reconciliation");
        let outcome_from_first_mutation =
            first_reconciliation.bind_outcome(ReconciliationOutcome::Committed);

        let fact = ExternalToolAuditFact::reconciled(
            AuditReceiptId::parse("audit-cross-token").expect("valid receipt"),
            &outcome_from_first_mutation,
        );
        let serialized = serde_json::to_value(fact).expect("audit fact serializes");

        assert_eq!(
            serialized["idempotency_key"],
            json!(outcome_from_first_mutation.idempotency_key().as_str())
        );
        assert_eq!(serialized["approval"], json!("approval-1"));
        assert_ne!(
            serialized["idempotency_key"],
            json!(second_reconciliation.idempotency_key().as_str())
        );
    }
}
