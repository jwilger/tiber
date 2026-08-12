//! Semantic values parsed before entering review commands.

#![expect(
    clippy::missing_trait_methods,
    reason = "serde deserialization uses the stable default in-place implementation"
)]

use alloc::{boxed::Box, collections::BTreeSet, string::String, vec::Vec};
use core::{error::Error, fmt};
use serde::{Deserialize, Serialize, de::Error as _};

/// Maximum number of independently assigned review lenses in one assessment.
pub const MAX_REVIEW_LENSES: usize = 16;

/// Stable expected review failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewError {
    EmptySessionId,
    InvalidSessionId,
    EmptySnapshotId,
    InvalidSnapshotId,
    EmptyLens,
    InvalidLens,
    EmptyAgentId,
    InvalidAgentId,
    EmptyModelRole,
    InvalidModelRole,
    EmptyContextReceipt,
    InvalidContextReceipt,
    EmptyLifecycleReceipt,
    InvalidLifecycleReceipt,
    EmptyEvidenceId,
    InvalidEvidenceId,
    InvalidStream,
    InvalidIteration,
    InvalidAssignmentAttempt,
    NoReviewLenses,
    TooManyReviewLenses,
    DuplicateReviewLens,
}

impl ReviewError {
    /// Returns the stable external failure code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EmptySessionId => "review_empty_session_id",
            Self::InvalidSessionId => "review_invalid_session_id",
            Self::EmptySnapshotId => "review_empty_snapshot_id",
            Self::InvalidSnapshotId => "review_invalid_snapshot_id",
            Self::EmptyLens => "review_empty_lens",
            Self::InvalidLens => "review_invalid_lens",
            Self::EmptyAgentId => "review_empty_agent_id",
            Self::InvalidAgentId => "review_invalid_agent_id",
            Self::EmptyModelRole => "review_empty_model_role",
            Self::InvalidModelRole => "review_invalid_model_role",
            Self::EmptyContextReceipt => "review_empty_context_receipt",
            Self::InvalidContextReceipt => "review_invalid_context_receipt",
            Self::EmptyLifecycleReceipt => "review_empty_lifecycle_receipt",
            Self::InvalidLifecycleReceipt => "review_invalid_lifecycle_receipt",
            Self::EmptyEvidenceId => "review_empty_evidence_id",
            Self::InvalidEvidenceId => "review_invalid_evidence_id",
            Self::InvalidStream => "review_invalid_stream",
            Self::InvalidIteration => "review_invalid_iteration",
            Self::InvalidAssignmentAttempt => "review_invalid_assignment_attempt",
            Self::NoReviewLenses => "review_no_lenses",
            Self::TooManyReviewLenses => "review_too_many_lenses",
            Self::DuplicateReviewLens => "review_duplicate_lens",
        }
    }
}

impl fmt::Display for ReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "semantic parsing failures have no causal source"
)]
impl Error for ReviewError {}

macro_rules! semantic_text {
    ($(#[$meta:meta])* $name:ident, $empty:ident, $invalid:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: &str) -> Result<Self, ReviewError> {
                let value = value.trim();
                if value.is_empty() {
                    return Err(ReviewError::$empty);
                }
                if value.chars().any(char::is_control) {
                    return Err(ReviewError::$invalid);
                }
                Ok(Self(value.to_owned()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(D::Error::custom)
            }
        }
    };
}

semantic_text!(ReviewSessionId, EmptySessionId, InvalidSessionId);
semantic_text!(
    /// Canonical identity of one reviewed source-content scope.
    ///
    /// The repository boundary derives this value from the pinned baseline,
    /// requested scope, in-repository path inventory, bytes, modes, and untracked
    /// content. It deliberately excludes staging partition, `HEAD`, commit,
    /// signature, and push metadata. Identical source content must reuse this
    /// value; exact-commit verification belongs to the delivery boundary.
    ReviewSnapshotId,
    EmptySnapshotId,
    InvalidSnapshotId
);
semantic_text!(ReviewLens, EmptyLens, InvalidLens);
semantic_text!(AgentId, EmptyAgentId, InvalidAgentId);
semantic_text!(ModelRole, EmptyModelRole, InvalidModelRole);
semantic_text!(ContextReceiptId, EmptyContextReceipt, InvalidContextReceipt);
semantic_text!(
    LifecycleReceiptId,
    EmptyLifecycleReceipt,
    InvalidLifecycleReceipt
);
semantic_text!(EvidenceId, EmptyEvidenceId, InvalidEvidenceId);

/// Bounded one-based review iteration.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ReviewIteration(u32);

impl ReviewIteration {
    pub const FIRST: Self = Self(1);

    pub fn parse(value: u32) -> Result<Self, ReviewError> {
        if value == 0 || value > 8 {
            return Err(ReviewError::InvalidIteration);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    pub(crate) fn next(self) -> Result<Self, ReviewError> {
        Self::parse(self.0.saturating_add(1))
    }
}

impl<'de> Deserialize<'de> for ReviewIteration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::parse(u32::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl Default for ReviewIteration {
    fn default() -> Self {
        Self::FIRST
    }
}

/// Bounded one-based assignment attempt within one review iteration.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AssignmentAttempt(u32);

impl AssignmentAttempt {
    pub const FIRST: Self = Self(1);

    pub fn parse(value: u32) -> Result<Self, ReviewError> {
        if value == 0 || value > 3 {
            return Err(ReviewError::InvalidAssignmentAttempt);
        }
        Ok(Self(value))
    }

    pub(crate) fn next(self) -> Result<Self, ReviewError> {
        Self::parse(self.0.saturating_add(1))
    }
}

impl Default for AssignmentAttempt {
    fn default() -> Self {
        Self::FIRST
    }
}

impl<'de> Deserialize<'de> for AssignmentAttempt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::parse(u32::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Closed reviewer work classification.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum AssignmentKind {
    Lens,
    Verifier,
    DeltaRisk,
    RemediationVerifier,
}

/// Complete delta classification for one assessed lens.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LensDeltaClassification {
    lens: ReviewLens,
    affected: bool,
}

impl LensDeltaClassification {
    /// Creates one authoritative affected/unaffected classification.
    #[must_use]
    pub const fn new(lens: ReviewLens, affected: bool) -> Self {
        Self { lens, affected }
    }

    /// Returns the classified lens.
    #[must_use]
    pub const fn lens(&self) -> &ReviewLens {
        &self.lens
    }

    /// Reports whether current evidence for the lens was invalidated.
    #[must_use]
    pub const fn affected(&self) -> bool {
        self.affected
    }
}

/// Collision-free structured assignment identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AssignmentId {
    session: ReviewSessionId,
    lens: ReviewLens,
    iteration: ReviewIteration,
    attempt: AssignmentAttempt,
    kind: AssignmentKind,
    remediation_occurrence: Option<Box<FindingOccurrenceId>>,
}

impl AssignmentId {
    #[must_use]
    pub const fn new(
        session: ReviewSessionId,
        lens: ReviewLens,
        iteration: ReviewIteration,
        attempt: AssignmentAttempt,
        kind: AssignmentKind,
    ) -> Self {
        Self {
            session,
            lens,
            iteration,
            attempt,
            kind,
            remediation_occurrence: None,
        }
    }

    /// Binds remediation work to the full origin assignment and finding evidence.
    #[must_use]
    pub fn with_remediation_occurrence(mut self, occurrence: FindingOccurrenceId) -> Self {
        self.remediation_occurrence = Some(Box::new(occurrence));
        self
    }

    #[must_use]
    pub const fn lens(&self) -> &ReviewLens {
        &self.lens
    }

    #[must_use]
    pub const fn session(&self) -> &ReviewSessionId {
        &self.session
    }

    #[must_use]
    pub const fn iteration(&self) -> ReviewIteration {
        self.iteration
    }

    #[must_use]
    pub const fn attempt(&self) -> AssignmentAttempt {
        self.attempt
    }

    #[must_use]
    pub const fn kind(&self) -> AssignmentKind {
        self.kind
    }

    #[must_use]
    pub fn remediation_occurrence(&self) -> Option<&FindingOccurrenceId> {
        self.remediation_occurrence.as_deref()
    }
}

/// Whether one lens requires a separately assigned verifier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum VerifierRoute {
    NotRequired,
    Required { model_role: ModelRole },
}

/// Risk-selected model routing for one independent review lens.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LensRoute {
    lens: ReviewLens,
    reviewer_model_role: ModelRole,
    verifier: VerifierRoute,
    remediation_model_role: ModelRole,
}

impl LensRoute {
    #[must_use]
    pub const fn new(
        lens: ReviewLens,
        reviewer_model_role: ModelRole,
        verifier: VerifierRoute,
        remediation_model_role: ModelRole,
    ) -> Self {
        Self {
            lens,
            reviewer_model_role,
            verifier,
            remediation_model_role,
        }
    }

    #[must_use]
    pub const fn lens(&self) -> &ReviewLens {
        &self.lens
    }

    #[must_use]
    pub const fn reviewer_model_role(&self) -> &ModelRole {
        &self.reviewer_model_role
    }

    #[must_use]
    pub const fn verifier(&self) -> &VerifierRoute {
        &self.verifier
    }
    #[must_use]
    pub const fn remediation_model_role(&self) -> &ModelRole {
        &self.remediation_model_role
    }
}

/// Complete risk assessment for one exact source-content snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RiskAssessment {
    evidence_id: EvidenceId,
    delta_model_role: ModelRole,
    routes: Vec<LensRoute>,
    agent_id: AgentId,
    model_role: ModelRole,
    context_receipt: ContextReceiptId,
    lifecycle_receipt: LifecycleReceiptId,
}

impl RiskAssessment {
    pub fn parse(
        evidence_id: EvidenceId,
        delta_model_role: ModelRole,
        routes: Vec<LensRoute>,
        agent_id: AgentId,
        model_role: ModelRole,
        context_receipt: ContextReceiptId,
        lifecycle_receipt: LifecycleReceiptId,
    ) -> Result<Self, ReviewError> {
        let assessment = Self {
            evidence_id,
            delta_model_role,
            routes,
            agent_id,
            model_role,
            context_receipt,
            lifecycle_receipt,
        };
        assessment.validate()?;
        Ok(assessment)
    }

    pub(crate) fn validate(&self) -> Result<(), ReviewError> {
        if self.routes.is_empty() {
            return Err(ReviewError::NoReviewLenses);
        }
        if self.routes.len() > MAX_REVIEW_LENSES {
            return Err(ReviewError::TooManyReviewLenses);
        }
        let unique = self
            .routes
            .iter()
            .map(|route| route.lens.clone())
            .collect::<BTreeSet<_>>();
        if unique.len() != self.routes.len() {
            return Err(ReviewError::DuplicateReviewLens);
        }
        Ok(())
    }

    #[must_use]
    pub fn routes(&self) -> &[LensRoute] {
        &self.routes
    }

    #[must_use]
    pub const fn delta_model_role(&self) -> &ModelRole {
        &self.delta_model_role
    }
    #[must_use]
    pub const fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
    #[must_use]
    pub const fn context_receipt(&self) -> &ContextReceiptId {
        &self.context_receipt
    }
    #[must_use]
    pub const fn lifecycle_receipt(&self) -> &LifecycleReceiptId {
        &self.lifecycle_receipt
    }
    #[must_use]
    pub const fn model_role(&self) -> &ModelRole {
        &self.model_role
    }
}

/// Scheduler-issued assignment with authoritative context/lifecycle receipts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewAssignment {
    id: AssignmentId,
    snapshot: ReviewSnapshotId,
    target_snapshot: Option<ReviewSnapshotId>,
    agent_id: AgentId,
    model_role: ModelRole,
    context_receipt: ContextReceiptId,
    lifecycle_receipt: LifecycleReceiptId,
    finding_target: Option<FindingOccurrenceId>,
}

impl ReviewAssignment {
    #[must_use]
    pub const fn new(
        id: AssignmentId,
        snapshot: ReviewSnapshotId,
        agent_id: AgentId,
        model_role: ModelRole,
        context_receipt: ContextReceiptId,
        lifecycle_receipt: LifecycleReceiptId,
    ) -> Self {
        Self {
            id,
            snapshot,
            target_snapshot: None,
            agent_id,
            model_role,
            context_receipt,
            lifecycle_receipt,
            finding_target: None,
        }
    }

    /// Binds delta-risk work to the exact candidate snapshot being classified.
    #[must_use]
    pub fn with_target_snapshot(mut self, target_snapshot: ReviewSnapshotId) -> Self {
        self.target_snapshot = Some(target_snapshot);
        self
    }

    /// Binds a remediation-verifier assignment to one exact finding occurrence.
    #[must_use]
    pub fn with_finding_target(mut self, finding_target: FindingOccurrenceId) -> Self {
        self.finding_target = Some(finding_target);
        self
    }

    #[must_use]
    pub const fn id(&self) -> &AssignmentId {
        &self.id
    }
    #[must_use]
    pub const fn snapshot(&self) -> &ReviewSnapshotId {
        &self.snapshot
    }
    #[must_use]
    pub const fn target_snapshot(&self) -> Option<&ReviewSnapshotId> {
        self.target_snapshot.as_ref()
    }
    #[must_use]
    pub const fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
    #[must_use]
    pub const fn model_role(&self) -> &ModelRole {
        &self.model_role
    }
    #[must_use]
    pub const fn context_receipt(&self) -> &ContextReceiptId {
        &self.context_receipt
    }
    #[must_use]
    pub const fn lifecycle_receipt(&self) -> &LifecycleReceiptId {
        &self.lifecycle_receipt
    }
    #[must_use]
    pub const fn finding_target(&self) -> Option<&FindingOccurrenceId> {
        self.finding_target.as_ref()
    }
}

/// Finding severity relevant to the clean-review gate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FindingSeverity {
    Observation,
    Blocking,
}

/// Finding identity bound to the exact assignment that observed it.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FindingOccurrenceId {
    assignment_id: AssignmentId,
    evidence_id: EvidenceId,
}

impl FindingOccurrenceId {
    #[must_use]
    pub const fn new(assignment_id: AssignmentId, evidence_id: EvidenceId) -> Self {
        Self {
            assignment_id,
            evidence_id,
        }
    }

    #[must_use]
    pub const fn assignment_id(&self) -> &AssignmentId {
        &self.assignment_id
    }
    #[must_use]
    pub const fn evidence_id(&self) -> &EvidenceId {
        &self.evidence_id
    }
}

/// One typed finding occurrence in a reviewer result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FindingOccurrence {
    id: FindingOccurrenceId,
    severity: FindingSeverity,
}

impl FindingOccurrence {
    #[must_use]
    pub const fn new(id: FindingOccurrenceId, severity: FindingSeverity) -> Self {
        Self { id, severity }
    }

    #[must_use]
    pub const fn id(&self) -> &FindingOccurrenceId {
        &self.id
    }
    #[must_use]
    pub const fn severity(&self) -> FindingSeverity {
        self.severity
    }
}

/// Untrusted reviewer content bound to scheduler-owned assignment receipts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssignmentResult {
    assignment_id: AssignmentId,
    snapshot: ReviewSnapshotId,
    agent_id: AgentId,
    model_role: ModelRole,
    context_receipt: ContextReceiptId,
    lifecycle_receipt: LifecycleReceiptId,
    evidence_id: EvidenceId,
    findings: Vec<FindingOccurrence>,
    delta_classifications: Vec<LensDeltaClassification>,
}

impl AssignmentResult {
    #[expect(
        clippy::too_many_arguments,
        reason = "the boundary constructor deliberately requires every scheduler-owned provenance field to prevent ambient or inferred attestation"
    )]
    #[must_use]
    pub const fn new(
        assignment_id: AssignmentId,
        snapshot: ReviewSnapshotId,
        agent_id: AgentId,
        model_role: ModelRole,
        context_receipt: ContextReceiptId,
        lifecycle_receipt: LifecycleReceiptId,
        evidence_id: EvidenceId,
        findings: Vec<FindingOccurrence>,
    ) -> Self {
        Self {
            assignment_id,
            snapshot,
            agent_id,
            model_role,
            context_receipt,
            lifecycle_receipt,
            evidence_id,
            findings,
            delta_classifications: Vec::new(),
        }
    }

    /// Supplies the complete per-lens classification produced by a delta-risk assignment.
    #[must_use]
    pub fn with_delta_classifications(
        mut self,
        classifications: Vec<LensDeltaClassification>,
    ) -> Self {
        self.delta_classifications = classifications;
        self
    }

    #[must_use]
    pub const fn assignment_id(&self) -> &AssignmentId {
        &self.assignment_id
    }
    #[must_use]
    pub const fn snapshot(&self) -> &ReviewSnapshotId {
        &self.snapshot
    }
    #[must_use]
    pub const fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
    #[must_use]
    pub const fn model_role(&self) -> &ModelRole {
        &self.model_role
    }
    #[must_use]
    pub const fn context_receipt(&self) -> &ContextReceiptId {
        &self.context_receipt
    }
    #[must_use]
    pub const fn lifecycle_receipt(&self) -> &LifecycleReceiptId {
        &self.lifecycle_receipt
    }
    #[must_use]
    pub fn findings(&self) -> &[FindingOccurrence] {
        &self.findings
    }
    #[must_use]
    pub fn delta_classifications(&self) -> &[LensDeltaClassification] {
        &self.delta_classifications
    }
    #[must_use]
    pub const fn evidence_id(&self) -> &EvidenceId {
        &self.evidence_id
    }
}
