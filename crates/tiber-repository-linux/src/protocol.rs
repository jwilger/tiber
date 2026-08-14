use serde::{Deserialize, Serialize};
use tiber_repository_core::{
    RepositoryMutationFailureCode, RepositoryMutationKind, RepositoryMutationPrecondition,
};

#[derive(Deserialize, Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::pub_with_shorthand,
    reason = "mutation variants precede read-only reconciliation in lifecycle order"
)]
/// Closed request vocabulary accepted by the private worker.
pub(crate) enum WorkerRequest {
    /// Applies an absent-or-exact bounded write.
    Write {
        /// Expected SHA-256 of the raw content frame.
        content_digest: String,
        /// Exact bounded raw content length.
        content_length: usize,
        /// Parent network namespace identity that the child must differ from.
        parent_network_namespace: String,
        /// Validated root-relative repository path.
        path: String,
        /// Closed write precondition.
        precondition: WorkerWritePrecondition,
    },
    /// Applies an exact-digest delete.
    Delete {
        /// Parent network namespace identity that the child must differ from.
        parent_network_namespace: String,
        /// Validated root-relative repository path.
        path: String,
        /// Expected SHA-256 of the regular file.
        precondition: String,
    },
    /// Performs a conservative read-only reconciliation query.
    Reconcile {
        /// Safe expected write content digest, when applicable.
        content_digest: Option<String>,
        /// Original closed mutation kind.
        kind: RepositoryMutationKind,
        /// Parent network namespace identity that the child must differ from.
        parent_network_namespace: String,
        /// Validated root-relative repository path.
        path: String,
        /// Original typed mutation precondition.
        precondition: RepositoryMutationPrecondition,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
/// Closed worker-side write precondition.
#[expect(
    clippy::pub_with_shorthand,
    reason = "this shared source is compiled by both the library and private worker binary"
)]
pub(crate) enum WorkerWritePrecondition {
    /// Requires the target to be absent.
    Absent,
    /// Requires the target to match this SHA-256.
    ExactDigest(String),
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
/// Closed response vocabulary emitted by the private worker.
#[expect(
    clippy::pub_with_shorthand,
    reason = "this shared source is compiled by both the library and private worker binary"
)]
pub(crate) enum WorkerResponse {
    /// The worker observed the target mutation as durably applied.
    Applied,
    /// The worker definitively proved no target mutation was applied.
    Rejected {
        /// Closed core failure classification.
        code: RepositoryMutationFailureCode,
    },
    /// The worker cannot prove a terminal historical state.
    StillUnknown,
}
