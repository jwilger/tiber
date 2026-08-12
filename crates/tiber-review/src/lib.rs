//! Native `EventCore` authority for Tiber multi-agent final review.
//!
//! This crate has no review aggregate. Every write is a business-domain
//! [`eventcore::ModelCommand`] with command-specific folded state containing
//! only the facts needed for that command's decision. Read views are separate
//! projections and never become write authority.

#![forbid(unsafe_code)]
#![expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    clippy::implicit_return,
    clippy::missing_errors_doc,
    clippy::missing_inline_in_public_items,
    clippy::module_name_repetitions,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    reason = "the closed domain vocabulary follows command flow; command names intentionally repeat their bounded domain modules; typed railway propagation and borrowed event matching keep command-specific folds direct"
)]
#![expect(
    clippy::exhaustive_structs,
    clippy::impl_trait_in_params,
    reason = "EventCore checked-model derives generate exhaustive internal graph nodes with implementation-detail parameter shapes"
)]

extern crate alloc;

use alloc::collections::BTreeSet;

#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod assignment;
#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod clean;
#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod delta;
#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod resolution;
#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod result;
#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod risk;
#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod supersession;
#[cfg_attr(
    not(test),
    expect(
        clippy::missing_docs_in_private_items,
        reason = "EventCore checked-model derives generate internal builders and graph wiring; public domain types and command entry points are documented explicitly"
    )
)]
pub mod types;

use eventcore::{Event, ModelEvent, ModelOutput, StreamId, mapping};
use serde::{Deserialize, Serialize};

use types::{
    AssignmentAttempt, AssignmentId, AssignmentKind, AssignmentResult, ContextReceiptId,
    EvidenceId, FindingOccurrenceId, ReviewAssignment, ReviewError, ReviewLens, ReviewSessionId,
    ReviewSnapshotId, RiskAssessment, VerifierRoute,
};

/// Semantic stream identity for one native review session.
#[derive(Clone, Debug, Eq, PartialEq, eventcore::StreamIdentity)]
pub struct ReviewStream(StreamId);

impl ReviewStream {
    /// Creates the session stream at an external-input boundary.
    pub fn for_session(session: &ReviewSessionId) -> Result<Self, ReviewError> {
        StreamId::try_new(format!("tiber:review:{}", session.as_str()))
            .map(Self)
            .map_err(|_source| ReviewError::InvalidStream)
    }

    /// Recovers the semantic session identity guaranteed by construction.
    pub fn session(&self) -> Result<ReviewSessionId, ReviewError> {
        let raw = self.0.as_ref();
        let session = raw
            .strip_prefix("tiber:review:")
            .ok_or(ReviewError::InvalidStream)?;
        ReviewSessionId::parse(session)
    }
}

/// Immutable review-domain facts emitted by modeled commands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReviewFact {
    /// Risk assessment selected the complete review routing for this snapshot.
    RiskAssessed {
        /// Exact source-content snapshot assessed.
        snapshot: ReviewSnapshotId,
        /// Complete selected routing and evidence.
        assessment: RiskAssessment,
    },
    /// The scheduler issued a bounded fresh-context assignment.
    AssignmentIssued { assignment: ReviewAssignment },
    /// A result matching scheduler-owned assignment provenance was accepted.
    AssignmentResultAccepted { result: AssignmentResult },
    /// A failed, cancelled, or stale assignment was explicitly superseded.
    AssignmentSuperseded {
        assignment_id: AssignmentId,
        replacement_attempt: AssignmentAttempt,
        reason: EvidenceId,
    },
    /// A material source delta was classified against the prior review scope.
    DeltaReassessed {
        /// Snapshot whose evidence was reassessed.
        from_snapshot: ReviewSnapshotId,
        /// New exact source-content snapshot.
        to_snapshot: ReviewSnapshotId,
        /// Lenses whose prior results were invalidated.
        affected_lenses: BTreeSet<ReviewLens>,
        /// Evidence binding the delta classification.
        evidence_id: EvidenceId,
    },
    /// A separately verified remediation resolved one exact finding occurrence.
    FindingResolutionVerified {
        /// Exact assignment-bound finding occurrence.
        finding_id: FindingOccurrenceId,
        /// Evidence produced by the remediation verifier.
        evidence_id: EvidenceId,
    },
    /// All required current evidence passed the final-review gate.
    CleanReviewAccepted {
        /// Exact reviewed source-content snapshot.
        snapshot: ReviewSnapshotId,
        /// Evidence binding the clean decision.
        evidence_id: EvidenceId,
    },
}

/// Durable `EventCore` event for a native review-session stream.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
pub struct ReviewEvent {
    /// Durable session stream that owns this review fact.
    stream: StreamId,
    /// Immutable business-domain fact emitted by a modeled command.
    fact: ReviewFact,
}

impl Event for ReviewEvent {
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }

    fn event_type_name() -> &'static str {
        "TiberReviewEvent"
    }
}

/// Read projection proving every durable event field is consumed independently
/// from command decision state.
#[derive(ModelOutput)]
#[non_exhaustive]
pub struct ReviewEventView {
    /// Projected stream identity.
    stream: StreamId,
    /// Projected domain fact.
    fact: ReviewFact,
}

mapping! { ReviewEventStreamToView: ReviewEvent.stream => ReviewEventView.stream using clone; }
mapping! { ReviewEventFactToView: ReviewEvent.fact => ReviewEventView.fact using clone; }

impl ReviewEventView {
    /// Projects one durable fact for query-side consumers.
    #[must_use]
    pub fn from_event(event: &ReviewEvent) -> Self {
        Self::model_builder()
            .stream(ReviewEventStreamToView::apply(event))
            .fact(ReviewEventFactToView::apply(event))
            .build()
            .into_inner()
    }

    /// Returns the projected stream identity.
    #[must_use]
    pub const fn stream(&self) -> &StreamId {
        &self.stream
    }

    /// Returns the projected domain fact.
    #[must_use]
    pub const fn fact(&self) -> &ReviewFact {
        &self.fact
    }
}
