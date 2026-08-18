use core::fmt;

use tiber_repository_core::{
    ComponentScope, OwnerApprovalId, RepositoryAssignmentContext, RepositoryCapability,
    RepositoryContent, RepositoryId, RepositoryMutationPolicy, RepositoryMutationProposal,
    RepositoryMutationProvenance, RepositoryPath, WritePrecondition,
};
use tiber_workflow_core::{
    AgentId, AssignmentEpoch, AssignmentId, AssignmentScope, AttemptNumber, ContextReceiptId,
    DeadlineMilliseconds, EffectId, IdempotencyKey, PolicyDecisionId, SessionId, WorkflowId,
};

#[must_use]
#[inline]
pub fn write_proposal(content: &[u8]) -> RepositoryMutationProposal {
    write_proposal_for_effect(content, "effect-1")
}

/// # Panics
///
/// Panics when a deterministic fixture identifier cannot be parsed.
#[inline]
pub fn write_request(
    content: &[u8],
) -> (
    RepositoryMutationProposal,
    RepositoryAssignmentContext,
    RepositoryMutationPolicy,
    OwnerApprovalId,
) {
    let proposal = write_proposal(content);
    let identity = proposal.identity();
    let assignment = RepositoryAssignmentContext::new(
        identity.provenance().clone(),
        parsed(RepositoryId::parse, "repo-1"),
        ComponentScope::parse("src").expect("fixture scope should be valid"),
    );
    let policy =
        RepositoryMutationPolicy::new(assignment.clone(), [RepositoryCapability::MutateRepository]);
    (
        proposal,
        assignment,
        policy,
        approval_id("approval-prepared"),
    )
}

/// # Panics
///
/// Panics when a deterministic fixture identifier cannot be parsed.
#[inline]
pub fn write_proposal_for_effect(content: &[u8], effect_id: &str) -> RepositoryMutationProposal {
    RepositoryMutationProposal::write(
        RepositoryMutationProvenance::new(
            parsed(SessionId::parse, "session-1"),
            parsed(AgentId::parse, "agent-1"),
            parsed(WorkflowId::parse, "workflow-1"),
            parsed(AssignmentId::parse, "assignment-1"),
            parsed(AssignmentScope::parse, "repository:src"),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            parsed(ContextReceiptId::parse, "context-1"),
            parsed(PolicyDecisionId::parse, "policy-1"),
            parsed(EffectId::parse, effect_id),
            parsed(IdempotencyKey::parse, "idem-1"),
            DeadlineMilliseconds::parse(1_000).expect("fixture deadline should be valid"),
        ),
        parsed(RepositoryId::parse, "repo-1"),
        parsed(RepositoryPath::parse, "src/lib.rs"),
        RepositoryContent::from_bytes(content).expect("fixture content should be bounded"),
        WritePrecondition::Absent,
    )
}

#[inline]
pub fn approval_id(value: &str) -> OwnerApprovalId {
    parsed(OwnerApprovalId::parse, value)
}

#[expect(
    clippy::panic,
    reason = "invalid deterministic fixture literals are programmer errors and must fail immediately"
)]
fn parsed<T, E: fmt::Display>(parser: impl FnOnce(&str) -> Result<T, E>, value: &str) -> T {
    parser(value).unwrap_or_else(|error| panic!("{value} should parse: {error}"))
}
