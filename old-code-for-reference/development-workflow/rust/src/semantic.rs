//! Semantic capability policy shared by every Development System service.
//!
//! This module deliberately has no notion of a harness tool name, a shell
//! command string, or a Git/forge verb.  A caller receives a capability only
//! through a validated assignment and every mutating service checks the same
//! assignment, configuration digest, scope, and expiry again immediately
//! before performing its operation.
//!
//! Host plugins expose advisory reader, setup, lifecycle, and review surfaces. Keep the other
//! service cores compiled and tested for native use by standalone Tiber, which
//! owns identity, authorization, isolation, and effect execution.
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use eventcore::{
    execute, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
    CommandError, ModelCommand, ModelInput, ModelState, RetryPolicy, StreamId,
};
use eventcore_types::EventStore;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tiber_git::git_event_store::{GitEventStore, GitEventStoreAuthority};

use crate::workflow::__eventcore_model_workflowauthorityevent;
use crate::workflow::{WorkflowAuthorityEvent, WorkflowAuthorityFact, WorkflowAuthorityStream};

pub const CONFIG_FILE: &str = ".development-system.toml";
pub const SCHEMA_VERSION: u32 = 3;
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_COMMAND_RECEIPTS: usize = 256;
const MAX_CHECKPOINTS: usize = 128;
static CHECKPOINT_OPERATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CHECKPOINT_ABORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static RUNNER_SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const PROTECTED_PATHS: &[&str] = &[
    ".development-system.toml",
    ".development-system/",
    ".codex/agents/development-system-",
    ".claude/agents/development-system-",
    ".git/",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityCategory {
    Tests,
    Source,
    Documentation,
    DeveloperEnvironment,
    BuildOutput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCapability {
    Tests,
    Implementation,
    Documentation,
    DeveloperEnvironment,
    Verification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    Denied,
    Allowed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    String,
    Integer,
    Boolean,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub category: CapabilityCategory,
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedService {
    pub readiness_command: String,
    pub shutdown_command: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCommand {
    pub argv: Vec<String>,
    pub capability: CommandCapability,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterType>,
    #[serde(default)]
    pub output_scopes: Vec<String>,
    #[serde(default)]
    pub network: Option<NetworkPolicy>,
    #[serde(default)]
    pub environment: Vec<String>,
    #[serde(default)]
    pub service: Option<ManagedService>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SigningRequirements {
    #[serde(default)]
    pub commit: bool,
    #[serde(default)]
    pub tag: bool,
}

/// Existing repository policy retained during the schema-v2 to schema-v3
/// migration. These settings remain outside the mutation-capability decision,
/// but are not disposable setup metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryPolicy {
    pub mode: String,
    #[serde(default)]
    pub trunk_branch: Option<String>,
    #[serde(default)]
    pub merge_method: MergeMethod,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeProvider {
    GitHub,
    GitLab,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgePolicy {
    pub provider: ForgeProvider,
    /// Provider-native repository identity, for example `owner/name`.
    /// Remote URLs are never accepted by a semantic forge operation.
    pub repository: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FeaturePolicy {
    #[serde(default)]
    pub tiber: bool,
    #[serde(default)]
    pub agentic_systems: bool,
    #[serde(default)]
    pub eval_case_reporting: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreePolicy {
    pub root: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TiberPolicy {
    pub max_queued: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub schema_version: u32,
    pub scopes: BTreeMap<String, Scope>,
    #[serde(default)]
    pub commands: BTreeMap<String, ProjectCommand>,
    #[serde(default)]
    pub signing: Option<SigningRequirements>,
    #[serde(default)]
    pub diagnostic_mode: Option<bool>,
    #[serde(default)]
    pub model_routing: BTreeMap<String, String>,
    #[serde(default)]
    pub delivery: Option<DeliveryPolicy>,
    #[serde(default)]
    pub forge: Option<Box<ForgePolicy>>,
    #[serde(default)]
    pub features: Option<FeaturePolicy>,
    #[serde(default)]
    pub worktrees: Option<WorktreePolicy>,
    #[serde(default)]
    pub tiber: Option<TiberPolicy>,
    /// Final-review routing is consumed by the review service, which retains
    /// its own strict parser. Preserve this established policy verbatim while
    /// setup adds the capability schema around it.
    #[serde(default)]
    pub final_review: Option<toml::Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigState {
    Absent,
    Valid(ProjectConfig),
    Invalid(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Coordinator,
    Explorer,
    SystemDiagnostician,
    TestAuthor,
    Implementer,
    DocumentationAuthor,
    EnvironmentMaintainer,
    Verifier,
    Reviewer,
    Delivery,
    CiRecovery,
    Setup,
}

pub fn role_allows_scope(role: &Role, category: &CapabilityCategory) -> bool {
    matches!(
        (role, category),
        (Role::TestAuthor, CapabilityCategory::Tests)
            | (Role::Implementer, CapabilityCategory::Source)
            | (Role::DocumentationAuthor, CapabilityCategory::Documentation)
            | (
                Role::EnvironmentMaintainer,
                CapabilityCategory::DeveloperEnvironment
            )
            | (Role::Verifier, CapabilityCategory::BuildOutput)
    )
}

pub fn role_allows_command(role: &Role, capability: &CommandCapability) -> bool {
    matches!(
        (role, capability),
        (Role::TestAuthor, CommandCapability::Tests)
            | (Role::Implementer, CommandCapability::Implementation)
            | (Role::DocumentationAuthor, CommandCapability::Documentation)
            | (
                Role::EnvironmentMaintainer,
                CommandCapability::DeveloperEnvironment
            )
            | (Role::Verifier, CommandCapability::Verification)
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub id: String,
    pub role: Role,
    pub state_epoch: u64,
    pub scope_ids: BTreeSet<String>,
    #[serde(default)]
    pub command_ids: BTreeSet<String>,
    pub expires_at: u64,
    pub configuration_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandReceipt {
    pub id: String,
    pub assignment_id: String,
    pub command_id: String,
    pub state_epoch: u64,
    pub configuration_digest: String,
    pub succeeded: bool,
    #[serde(default)]
    pub output_digest: String,
    #[serde(default)]
    pub observed_output_digests: BTreeMap<String, Option<String>>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Checkpoint {
    pub id: String,
    pub state_epoch: u64,
    pub index_tree: String,
    pub owned_paths: BTreeSet<String>,
    #[serde(default)]
    pub authorized_scope_ids: BTreeSet<String>,
    #[serde(default)]
    pub command_policy_digest: String,
    #[serde(default)]
    pub evidence_ids: BTreeSet<String>,
    #[serde(default)]
    pub predecessor: Option<String>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointAbortOperation {
    pub operation_id: String,
    pub checkpoint_id: String,
    pub checkpoint_tree: String,
    pub expected_index_tree: String,
    pub affected_paths: BTreeSet<String>,
    pub path_digests: BTreeMap<String, Option<String>>,
    pub authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointAbortReceipt {
    pub operation_id: String,
    pub archive_relative_path: String,
    pub restored_index_tree: String,
    pub completed_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedCommitOperation {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub checkpoint_id: String,
    pub parent_commit: String,
    pub message: String,
    pub message_digest: String,
    pub authorized_at: u64,
}

/// A request to authorize a signed commit.  The current epoch is an optimistic
/// concurrency observation, not a fact supplied by the caller: the command
/// folds the authoritative lifecycle and records the folded epoch in the fact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SignedCommitIntent {
    operation_id: String,
    assignment_id: String,
    expected_state_epoch: u64,
    checkpoint_id: String,
    parent_commit: String,
    message: String,
    message_digest: String,
    configuration_digest: String,
    authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedCommitReceipt {
    pub operation_id: String,
    pub assignment_id: String,
    pub checkpoint_id: String,
    pub parent_commit: String,
    pub tree: String,
    pub commit: String,
    pub message_digest: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedTagOperation {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub commit_operation_id: String,
    pub target_commit: String,
    pub tag_name: String,
    pub message: String,
    pub message_digest: String,
    pub authorized_at: u64,
}

/// A request to authorize a signed tag.  As with commits, the emitted epoch is
/// derived while folding the authority stream rather than trusted from input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SignedTagIntent {
    operation_id: String,
    assignment_id: String,
    expected_state_epoch: u64,
    commit_operation_id: String,
    target_commit: String,
    tag_name: String,
    message: String,
    message_digest: String,
    configuration_digest: String,
    authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedTagReceipt {
    pub operation_id: String,
    pub assignment_id: String,
    pub commit_operation_id: String,
    pub target_commit: String,
    pub tag_name: String,
    pub tag_object: String,
    pub message_digest: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FetchRefOperation {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub remote: String,
    pub remote_ref: String,
    pub authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FetchRefIntent {
    operation_id: String,
    assignment_id: String,
    expected_state_epoch: u64,
    remote: String,
    remote_ref: String,
    configuration_digest: String,
    authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FetchRefReceipt {
    pub operation_id: String,
    pub assignment_id: String,
    pub remote: String,
    pub remote_ref: String,
    pub object_id: String,
    pub fetched_at: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushSourceKind {
    Commit,
    Tag,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PushRefOperation {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub remote: String,
    pub remote_ref: String,
    pub source_kind: PushSourceKind,
    pub source_operation_id: String,
    pub source_object: String,
    pub expected_remote_object: Option<String>,
    pub authorized_at: u64,
}

/// A push authorization request contains only caller observations.  The signed
/// source object is intentionally absent: it is derived from the authoritative
/// signed commit/tag receipt while deciding the command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PushRefIntent {
    operation_id: String,
    assignment_id: String,
    expected_state_epoch: u64,
    remote: String,
    remote_ref: String,
    source_kind: PushSourceKind,
    source_operation_id: String,
    expected_remote_object: Option<String>,
    configuration_digest: String,
    authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PushRefReceipt {
    pub operation_id: String,
    pub assignment_id: String,
    pub remote: String,
    pub remote_ref: String,
    pub source_object: String,
    pub previous_remote_object: Option<String>,
    pub pushed_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenPullRequestOperation {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub provider: ForgeProvider,
    pub repository: String,
    pub push_operation_id: String,
    pub head_ref: String,
    pub base_branch: String,
    pub title: String,
    pub body: String,
    pub authorized_at: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct OpenPullRequestIntent {
    operation_id: String,
    assignment_id: String,
    expected_state_epoch: u64,
    provider: ForgeProvider,
    repository: String,
    push_operation_id: String,
    head_ref: String,
    base_branch: String,
    title: String,
    body: String,
    configuration_digest: String,
    authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenPullRequestReceipt {
    pub operation_id: String,
    pub assignment_id: String,
    pub provider: ForgeProvider,
    pub repository: String,
    pub push_operation_id: String,
    pub pull_request_url: String,
    pub opened_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdatePullRequestOperation {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub open_operation_id: String,
    pub provider: ForgeProvider,
    pub repository: String,
    pub pull_request_url: String,
    pub title: String,
    pub body: String,
    pub authorized_at: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct UpdatePullRequestIntent {
    operation_id: String,
    assignment_id: String,
    expected_state_epoch: u64,
    open_operation_id: String,
    title: String,
    body: String,
    configuration_digest: String,
    authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdatePullRequestReceipt {
    pub operation_id: String,
    pub assignment_id: String,
    pub open_operation_id: String,
    pub pull_request_url: String,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergePullRequestOperation {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub open_operation_id: String,
    pub provider: ForgeProvider,
    pub repository: String,
    pub pull_request_url: String,
    pub method: MergeMethod,
    pub authorized_at: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct MergePullRequestIntent {
    operation_id: String,
    assignment_id: String,
    expected_state_epoch: u64,
    open_operation_id: String,
    method: MergeMethod,
    configuration_digest: String,
    authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergePullRequestReceipt {
    pub operation_id: String,
    pub assignment_id: String,
    pub open_operation_id: String,
    pub pull_request_url: String,
    pub method: MergeMethod,
    pub merged_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceFileWrite {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub scope_id: String,
    pub path: String,
    pub before_digest: String,
    pub after_digest: String,
    pub authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceFileDeletion {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub scope_id: String,
    pub path: String,
    pub before_digest: String,
    pub authorized_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceFileMove {
    pub operation_id: String,
    pub assignment_id: String,
    pub state_epoch: u64,
    pub scope_id: String,
    pub from: String,
    pub to: String,
    pub source_digest: String,
    pub destination_digest: String,
    pub authorized_at: u64,
}

/// Immutable workflow facts. Assignments and command receipts deliberately
/// live in the same EventCore stream as workflow authority, rather than in a
/// sidecar cache that could disagree with lifecycle state after a restart.
type WorkflowFact = WorkflowAuthorityFact;

/// Retired semantic authority vocabulary. It exists only to deserialize the
/// former Git stream at the compatibility boundary; no current command emits
/// it or accepts it as an ordinary intent.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum LegacySemanticFact {
    AssignmentIssued { assignment: Assignment },
    EpochAdvanced { state_epoch: u64 },
    CommandReceiptRecorded { receipt: CommandReceipt },
    CheckpointCaptured { checkpoint: Checkpoint },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LegacySemanticEvent {
    stream: StreamId,
    fact: LegacySemanticFact,
}

impl eventcore::Event for LegacySemanticEvent {
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }

    fn event_type_name() -> &'static str {
        "DevelopmentDisciplineWorkflowEvent"
    }
}

#[derive(Clone, Debug)]
struct LegacySemanticImport {
    source_id: String,
    facts: Vec<LegacySemanticFact>,
}

#[derive(ModelInput)]
struct ImportLegacySemanticRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    import: LegacySemanticImport,
}

#[derive(ModelCommand)]
struct ImportLegacySemantic {
    #[stream]
    stream: WorkflowAuthorityStream,
    import: LegacySemanticImport,
}

mapping! { ImportLegacySemanticRequestToStream: ImportLegacySemanticRequest.stream => ImportLegacySemantic.stream using clone; }
mapping! { ImportLegacySemanticRequestToImport: ImportLegacySemanticRequest.import => ImportLegacySemantic.import using clone; }
mapping! { ImportLegacySemanticStreamToEvent: ImportLegacySemantic.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }

#[derive(ModelState)]
struct ImportLegacySemanticState {
    #[model(default)]
    imported_sources: BTreeSet<String>,
}

fn fold_imported_semantic_source(
    fact: &WorkflowFact,
    previous: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut sources = previous.clone();
    if let WorkflowFact::LegacySemanticHistoryImported { source_id, .. } = fact {
        sources.insert(source_id.clone());
    }
    sources
}
mapping! { ImportLegacySemanticEventToSources:
    (WorkflowAuthorityEvent.fact, previous(ImportLegacySemanticState.imported_sources)) => ImportLegacySemanticState.imported_sources
    using fold_imported_semantic_source;
}

fn legacy_semantic_history_imported_fact(import: &LegacySemanticImport) -> WorkflowFact {
    WorkflowFact::LegacySemanticHistoryImported {
        source_id: import.source_id.clone(),
        facts: import.facts.clone(),
    }
}
mapping! { ImportLegacySemanticToFact:
    ImportLegacySemantic.import => WorkflowAuthorityEvent.fact
    using legacy_semantic_history_imported_fact;
}

impl ModelCommandLogic for ImportLegacySemantic {
    type Event = WorkflowAuthorityEvent;
    type State = ImportLegacySemanticState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        ImportLegacySemanticState::model_builder()
            .imported_sources(ImportLegacySemanticEventToSources::apply((
                event,
                state.as_ref(),
            )))
            .build()
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if self.import.source_id.is_empty() || self.import.facts.is_empty() {
            return Err(CommandError::ValidationError(
                "development_system.legacy_import_invalid".to_string(),
            ));
        }
        if self.import.facts.len() > MAX_CHECKPOINTS {
            return Err(CommandError::ValidationError(
                "development_system.legacy_import_too_large".to_string(),
            ));
        }
        if state
            .as_ref()
            .imported_sources
            .contains(&self.import.source_id)
        {
            return Ok(ModeledEvents::none(
                "legacy semantic source already imported",
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(ImportLegacySemanticStreamToEvent::apply(self))
                .fact(ImportLegacySemanticToFact::apply(self))
                .build(),
        ))
    }
}

fn workflow_event_stream(stream: &WorkflowAuthorityStream) -> StreamId {
    crate::workflow::workflow_authority_event_stream(stream)
}

#[derive(Clone, Debug, Default)]
struct WorkflowProjection {
    state_epoch: u64,
    assignments: BTreeMap<String, Assignment>,
    command_receipts: BTreeMap<String, CommandReceipt>,
    checkpoints: BTreeMap<String, Checkpoint>,
    checkpoint_order: Vec<String>,
    last_checkpoint_id: Option<String>,
    accepted_evidence_ids: BTreeSet<String>,
    workflow_owned_paths: BTreeSet<String>,
    workflow_path_owners: BTreeMap<String, String>,
    paths_changed_since_checkpoint: BTreeSet<String>,
    file_write_authorizations: BTreeMap<String, WorkspaceFileWrite>,
    completed_file_write_ids: BTreeSet<String>,
    file_delete_authorizations: BTreeMap<String, WorkspaceFileDeletion>,
    completed_file_delete_ids: BTreeSet<String>,
    file_move_authorizations: BTreeMap<String, WorkspaceFileMove>,
    completed_file_move_ids: BTreeSet<String>,
    checkpoint_abort_authorizations: BTreeMap<String, CheckpointAbortOperation>,
    checkpoint_abort_receipts: BTreeMap<String, CheckpointAbortReceipt>,
    signed_commit_receipts: BTreeMap<String, SignedCommitReceipt>,
    signed_commit_authorizations: BTreeMap<String, SignedCommitOperation>,
    signed_tag_authorizations: BTreeMap<String, SignedTagOperation>,
    signed_tag_receipts: BTreeMap<String, SignedTagReceipt>,
    fetch_ref_authorizations: BTreeMap<String, FetchRefOperation>,
    fetch_ref_receipts: BTreeMap<String, FetchRefReceipt>,
    push_ref_authorizations: BTreeMap<String, PushRefOperation>,
    push_ref_receipts: BTreeMap<String, PushRefReceipt>,
    pull_request_open_authorizations: BTreeMap<String, OpenPullRequestOperation>,
    pull_request_open_receipts: BTreeMap<String, OpenPullRequestReceipt>,
    pull_request_update_authorizations: BTreeMap<String, UpdatePullRequestOperation>,
    pull_request_update_receipts: BTreeMap<String, UpdatePullRequestReceipt>,
    pull_request_merge_authorizations: BTreeMap<String, MergePullRequestOperation>,
    pull_request_merge_receipts: BTreeMap<String, MergePullRequestReceipt>,
}

impl WorkflowProjection {
    fn apply_fact(mut self, fact: &WorkflowFact) -> Self {
        self.state_epoch = folded_epoch(fact, &self.state_epoch);
        match fact {
            WorkflowFact::LegacySemanticHistoryImported { facts, .. } => {
                for fact in facts {
                    let current = match fact {
                        LegacySemanticFact::AssignmentIssued { assignment } => {
                            WorkflowFact::AssignmentIssued {
                                assignment: assignment.clone(),
                            }
                        }
                        LegacySemanticFact::CommandReceiptRecorded { receipt } => {
                            WorkflowFact::CommandReceiptRecorded {
                                receipt: receipt.clone(),
                            }
                        }
                        LegacySemanticFact::CheckpointCaptured { checkpoint } => {
                            WorkflowFact::CheckpointCaptured {
                                checkpoint: checkpoint.clone(),
                            }
                        }
                        // The former capability stream maintained a separate
                        // epoch. Lifecycle remains the only epoch authority.
                        LegacySemanticFact::EpochAdvanced { .. } => continue,
                    };
                    self = self.apply_fact(&current);
                }
            }
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                if facts.iter().any(|fact| {
                    matches!(fact, crate::workflow::LifecycleFact::CheckpointAbortApplied)
                }) {
                    self.accepted_evidence_ids.clear();
                }
            }
            WorkflowFact::AssignmentIssued { assignment } => {
                self.assignments
                    .insert(assignment.id.clone(), assignment.clone());
            }
            WorkflowFact::Lifecycle(crate::workflow::LifecycleFact::CheckpointAbortApplied) => {
                self.accepted_evidence_ids.clear();
            }
            WorkflowFact::Lifecycle(_) => {}
            WorkflowFact::RedEvidenceAccepted { receipt_id } => {
                self.accepted_evidence_ids = [receipt_id.clone()].into_iter().collect();
            }
            WorkflowFact::GreenEvidenceAccepted { receipt_id } => {
                self.accepted_evidence_ids.insert(receipt_id.clone());
            }
            WorkflowFact::VerificationEvidenceAccepted { receipt_id } => {
                self.accepted_evidence_ids.insert(receipt_id.clone());
            }
            WorkflowFact::CommandReceiptRecorded { receipt } => {
                self.command_receipts
                    .insert(receipt.id.clone(), receipt.clone());
                while self.command_receipts.len() > MAX_COMMAND_RECEIPTS {
                    if let Some(oldest) = self.command_receipts.keys().next().cloned() {
                        self.command_receipts.remove(&oldest);
                    }
                }
            }
            WorkflowFact::CheckpointCaptured { checkpoint } => {
                self.checkpoints
                    .insert(checkpoint.id.clone(), checkpoint.clone());
                self.checkpoint_order.push(checkpoint.id.clone());
                self.last_checkpoint_id = Some(checkpoint.id.clone());
                self.paths_changed_since_checkpoint.clear();
                while self.checkpoints.len() > MAX_CHECKPOINTS {
                    if let Some(oldest) = self.checkpoint_order.first().cloned() {
                        self.checkpoints.remove(&oldest);
                        self.checkpoint_order.remove(0);
                    }
                }
            }
            WorkflowFact::FileWriteAuthorized { operation } => {
                self.file_write_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::FileWritten { operation } => {
                self.workflow_owned_paths.insert(operation.path.clone());
                self.workflow_path_owners
                    .insert(operation.path.clone(), operation.assignment_id.clone());
                self.paths_changed_since_checkpoint
                    .insert(operation.path.clone());
                self.completed_file_write_ids
                    .insert(operation.operation_id.clone());
            }
            WorkflowFact::FileDeleteAuthorized { operation } => {
                self.file_delete_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::FileDeleted { operation } => {
                self.workflow_owned_paths.insert(operation.path.clone());
                self.workflow_path_owners
                    .insert(operation.path.clone(), operation.assignment_id.clone());
                self.paths_changed_since_checkpoint
                    .insert(operation.path.clone());
                self.completed_file_delete_ids
                    .insert(operation.operation_id.clone());
            }
            WorkflowFact::FileMoveAuthorized { operation } => {
                self.file_move_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::FileMoved { operation } => {
                self.workflow_owned_paths.insert(operation.from.clone());
                self.workflow_owned_paths.insert(operation.to.clone());
                self.workflow_path_owners
                    .insert(operation.from.clone(), operation.assignment_id.clone());
                self.workflow_path_owners
                    .insert(operation.to.clone(), operation.assignment_id.clone());
                self.paths_changed_since_checkpoint
                    .insert(operation.from.clone());
                self.paths_changed_since_checkpoint
                    .insert(operation.to.clone());
                self.completed_file_move_ids
                    .insert(operation.operation_id.clone());
            }
            WorkflowFact::CheckpointAbortAuthorized { operation } => {
                self.checkpoint_abort_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::CheckpointAbortCompleted { receipt } => {
                self.checkpoint_abort_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
                self.paths_changed_since_checkpoint.clear();
            }
            WorkflowFact::SignedCommitCreated { receipt } => {
                self.signed_commit_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
            }
            WorkflowFact::SignedCommitAuthorized { operation } => {
                self.signed_commit_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::SignedTagAuthorized { operation } => {
                self.signed_tag_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::SignedTagCreated { receipt } => {
                self.signed_tag_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
            }
            WorkflowFact::RemoteRefFetchAuthorized { operation } => {
                self.fetch_ref_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::RemoteRefFetched { receipt } => {
                self.fetch_ref_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
                while self.fetch_ref_receipts.len() > MAX_COMMAND_RECEIPTS {
                    if let Some(oldest) = self.fetch_ref_receipts.keys().next().cloned() {
                        self.fetch_ref_receipts.remove(&oldest);
                    }
                }
            }
            WorkflowFact::RemoteRefPushAuthorized { operation } => {
                self.push_ref_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::RemoteRefPushed { receipt } => {
                self.push_ref_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
                while self.push_ref_receipts.len() > MAX_COMMAND_RECEIPTS {
                    if let Some(oldest) = self.push_ref_receipts.keys().next().cloned() {
                        self.push_ref_receipts.remove(&oldest);
                    }
                }
            }
            WorkflowFact::PullRequestOpenAuthorized { operation } => {
                self.pull_request_open_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::PullRequestOpened { receipt } => {
                self.pull_request_open_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
                while self.pull_request_open_receipts.len() > MAX_COMMAND_RECEIPTS {
                    if let Some(oldest) = self.pull_request_open_receipts.keys().next().cloned() {
                        self.pull_request_open_receipts.remove(&oldest);
                    }
                }
            }
            WorkflowFact::PullRequestUpdateAuthorized { operation } => {
                self.pull_request_update_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::PullRequestUpdated { receipt } => {
                self.pull_request_update_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
                while self.pull_request_update_receipts.len() > MAX_COMMAND_RECEIPTS {
                    if let Some(oldest) = self.pull_request_update_receipts.keys().next().cloned() {
                        self.pull_request_update_receipts.remove(&oldest);
                    }
                }
            }
            WorkflowFact::PullRequestMergeAuthorized { operation } => {
                self.pull_request_merge_authorizations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            WorkflowFact::PullRequestMerged { receipt } => {
                self.pull_request_merge_receipts
                    .insert(receipt.operation_id.clone(), receipt.clone());
                while self.pull_request_merge_receipts.len() > MAX_COMMAND_RECEIPTS {
                    if let Some(oldest) = self.pull_request_merge_receipts.keys().next().cloned() {
                        self.pull_request_merge_receipts.remove(&oldest);
                    }
                }
            }
        }
        self
    }
}

fn folded_epoch(fact: &WorkflowFact, previous: &u64) -> u64 {
    match fact {
        WorkflowFact::LegacySemanticHistoryImported { .. } => *previous,
        WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
            facts.iter().fold(*previous, |epoch, fact| match fact {
                crate::workflow::LifecycleFact::WorkflowStarted { .. } => 1,
                _ => epoch.saturating_add(1),
            })
        }
        WorkflowFact::Lifecycle(crate::workflow::LifecycleFact::WorkflowStarted { .. }) => 1,
        WorkflowFact::Lifecycle(_) => previous.saturating_add(1),
        WorkflowFact::AssignmentIssued { .. }
        | WorkflowFact::CommandReceiptRecorded { .. }
        | WorkflowFact::CheckpointCaptured { .. }
        | WorkflowFact::FileWriteAuthorized { .. }
        | WorkflowFact::FileWritten { .. }
        | WorkflowFact::FileDeleteAuthorized { .. }
        | WorkflowFact::FileDeleted { .. }
        | WorkflowFact::FileMoveAuthorized { .. }
        | WorkflowFact::FileMoved { .. }
        | WorkflowFact::CheckpointAbortAuthorized { .. }
        | WorkflowFact::CheckpointAbortCompleted { .. }
        | WorkflowFact::SignedCommitCreated { .. }
        | WorkflowFact::SignedCommitAuthorized { .. }
        | WorkflowFact::SignedTagAuthorized { .. }
        | WorkflowFact::SignedTagCreated { .. }
        | WorkflowFact::RemoteRefFetchAuthorized { .. }
        | WorkflowFact::RemoteRefFetched { .. }
        | WorkflowFact::RemoteRefPushAuthorized { .. }
        | WorkflowFact::RemoteRefPushed { .. }
        | WorkflowFact::PullRequestOpenAuthorized { .. }
        | WorkflowFact::PullRequestOpened { .. }
        | WorkflowFact::PullRequestUpdateAuthorized { .. }
        | WorkflowFact::PullRequestUpdated { .. }
        | WorkflowFact::PullRequestMergeAuthorized { .. }
        | WorkflowFact::PullRequestMerged { .. }
        | WorkflowFact::RedEvidenceAccepted { .. }
        | WorkflowFact::GreenEvidenceAccepted { .. }
        | WorkflowFact::VerificationEvidenceAccepted { .. } => *previous,
    }
}

fn folded_last_checkpoint_id(fact: &WorkflowFact, previous: &Option<String>) -> Option<String> {
    match fact {
        WorkflowFact::CheckpointCaptured { checkpoint } => Some(checkpoint.id.clone()),
        WorkflowFact::LegacySemanticHistoryImported { facts, .. } => {
            facts
                .iter()
                .fold(previous.clone(), |last, fact| match fact {
                    LegacySemanticFact::CheckpointCaptured { checkpoint } => {
                        Some(checkpoint.id.clone())
                    }
                    _ => last,
                })
        }
        _ => previous.clone(),
    }
}

/// Assignment issuance folds only issued identities and the lifecycle epoch
/// needed to reject a stale issuance intent in this same authority stream.
#[derive(ModelState)]
struct IssueAssignmentState {
    #[model(default)]
    assignment_id_seen: bool,
    #[model(default)]
    state_epoch: u64,
}
fn folded_target_assignment_seen(fact: &WorkflowFact, target_id: &str, previous: &bool) -> bool {
    *previous
        || matches!(fact, WorkflowFact::AssignmentIssued { assignment } if assignment.id == target_id)
        || matches!(fact, WorkflowFact::LegacySemanticHistoryImported { facts, .. } if facts.iter().any(|fact| matches!(fact, LegacySemanticFact::AssignmentIssued { assignment } if assignment.id == target_id)))
}
mapping! { IssueAssignmentEventToSeen:
(WorkflowAuthorityEvent.fact, IssueAssignment.assignment_id, previous(IssueAssignmentState.assignment_id_seen)) => IssueAssignmentState.assignment_id_seen using folded_target_assignment_seen; }
mapping! { IssueAssignmentEventToEpoch:
(WorkflowAuthorityEvent.fact, previous(IssueAssignmentState.state_epoch)) => IssueAssignmentState.state_epoch using folded_epoch; }
impl IssueAssignmentState {
    fn from_event(
        previous: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &IssueAssignment,
    ) -> Modeled<Self> {
        Self::model_builder()
            .assignment_id_seen(IssueAssignmentEventToSeen::apply((
                event,
                command,
                previous.as_ref(),
            )))
            .state_epoch(IssueAssignmentEventToEpoch::apply((
                event,
                previous.as_ref(),
            )))
            .build()
    }
}

/// Receipt recording needs the current epoch, known assignments, and receipt identities.
#[derive(ModelState)]
struct RecordCommandReceiptState {
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    assignment_exists: bool,
    #[model(default)]
    receipt_seen: bool,
}
mapping! { RecordReceiptEventToEpoch:
(WorkflowAuthorityEvent.fact, previous(RecordCommandReceiptState.state_epoch)) => RecordCommandReceiptState.state_epoch using folded_epoch; }
fn folded_receipt_assignment_exists(
    fact: &WorkflowFact,
    intent: &ReceiptIntent,
    previous: &bool,
) -> bool {
    *previous
        || matches!(fact, WorkflowFact::AssignmentIssued { assignment } if assignment.id == intent.assignment_id)
        || matches!(fact, WorkflowFact::LegacySemanticHistoryImported { facts, .. } if facts.iter().any(|fact| matches!(fact, LegacySemanticFact::AssignmentIssued { assignment } if assignment.id == intent.assignment_id)))
}
fn folded_target_receipt_seen(
    fact: &WorkflowFact,
    intent: &ReceiptIntent,
    previous: &bool,
) -> bool {
    *previous
        || matches!(fact, WorkflowFact::CommandReceiptRecorded { receipt } if receipt.id == intent.id)
        || matches!(fact, WorkflowFact::LegacySemanticHistoryImported { facts, .. } if facts.iter().any(|fact| matches!(fact, LegacySemanticFact::CommandReceiptRecorded { receipt } if receipt.id == intent.id)))
}
mapping! { RecordReceiptEventToAssignmentExists:
(WorkflowAuthorityEvent.fact, RecordCommandReceipt.receipt, previous(RecordCommandReceiptState.assignment_exists)) => RecordCommandReceiptState.assignment_exists using folded_receipt_assignment_exists; }
mapping! { RecordReceiptEventToSeen:
(WorkflowAuthorityEvent.fact, RecordCommandReceipt.receipt, previous(RecordCommandReceiptState.receipt_seen)) => RecordCommandReceiptState.receipt_seen using folded_target_receipt_seen; }
impl RecordCommandReceiptState {
    fn from_event(
        previous: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &RecordCommandReceipt,
    ) -> Modeled<Self> {
        Self::model_builder()
            .state_epoch(RecordReceiptEventToEpoch::apply((event, previous.as_ref())))
            .assignment_exists(RecordReceiptEventToAssignmentExists::apply((
                event,
                command,
                previous.as_ref(),
            )))
            .receipt_seen(RecordReceiptEventToSeen::apply((
                event,
                command,
                previous.as_ref(),
            )))
            .build()
    }
}

/// Checkpoint capture needs the previous checkpoint and current lifecycle
/// epoch. Both are folded from the same authority stream that this command
/// appends to.
#[derive(ModelState)]
struct CaptureCheckpointState {
    #[model(default)]
    last_checkpoint_id: Option<String>,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    accepted_evidence_ids: BTreeSet<String>,
}

fn folded_accepted_evidence_ids(
    fact: &WorkflowFact,
    previous: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut evidence_ids = previous.clone();
    match fact {
        WorkflowFact::RedEvidenceAccepted { receipt_id } => {
            evidence_ids = [receipt_id.clone()].into_iter().collect();
        }
        WorkflowFact::GreenEvidenceAccepted { receipt_id } => {
            evidence_ids.insert(receipt_id.clone());
        }
        WorkflowFact::VerificationEvidenceAccepted { receipt_id } => {
            evidence_ids.insert(receipt_id.clone());
        }
        WorkflowFact::Lifecycle(crate::workflow::LifecycleFact::CleanReviewEvidenceAccepted {
            evidence_id,
        }) => {
            evidence_ids.insert(evidence_id.clone());
        }
        WorkflowFact::Lifecycle(crate::workflow::LifecycleFact::CheckpointAbortApplied) => {
            evidence_ids.clear();
        }
        _ => {}
    }
    evidence_ids
}
mapping! { CaptureCheckpointEventToLastId:
(WorkflowAuthorityEvent.fact, previous(CaptureCheckpointState.last_checkpoint_id)) => CaptureCheckpointState.last_checkpoint_id using folded_last_checkpoint_id; }
mapping! { CaptureCheckpointEventToEpoch:
(WorkflowAuthorityEvent.fact, previous(CaptureCheckpointState.state_epoch)) => CaptureCheckpointState.state_epoch using folded_epoch; }
mapping! { CaptureCheckpointEventToEvidenceIds:
(WorkflowAuthorityEvent.fact, previous(CaptureCheckpointState.accepted_evidence_ids)) => CaptureCheckpointState.accepted_evidence_ids using folded_accepted_evidence_ids; }
impl CaptureCheckpointState {
    fn from_event(previous: Modeled<Self>, event: &WorkflowAuthorityEvent) -> Modeled<Self> {
        Self::model_builder()
            .last_checkpoint_id(CaptureCheckpointEventToLastId::apply((
                event,
                previous.as_ref(),
            )))
            .state_epoch(CaptureCheckpointEventToEpoch::apply((
                event,
                previous.as_ref(),
            )))
            .accepted_evidence_ids(CaptureCheckpointEventToEvidenceIds::apply((
                event,
                previous.as_ref(),
            )))
            .build()
    }
}

#[derive(ModelState)]
struct SignedCommitState {
    #[model(default)]
    authorization: Option<SignedCommitOperation>,
    #[model(default)]
    receipt: Option<SignedCommitReceipt>,
}

/// The authorization command deliberately folds only the delivery facts it
/// needs: its assignment, epoch, checkpoint, clean review, and own operation.
#[derive(ModelState)]
struct AuthorizeSignedCommitState {
    #[model(default)]
    authorization: Option<SignedCommitOperation>,
    #[model(default)]
    receipt: Option<SignedCommitReceipt>,
    #[model(default)]
    assignment: Option<Assignment>,
    #[model(default)]
    checkpoint: Option<Checkpoint>,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    delivering: bool,
    #[model(default)]
    clean_review_observed: bool,
}

fn folded_signed_commit_authorization(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<SignedCommitOperation>,
) -> Option<SignedCommitOperation> {
    match fact {
        WorkflowFact::SignedCommitAuthorized { operation }
            if operation.operation_id == operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}

fn folded_signed_commit_receipt(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<SignedCommitReceipt>,
) -> Option<SignedCommitReceipt> {
    match fact {
        WorkflowFact::SignedCommitCreated { receipt } if receipt.operation_id == operation_id => {
            Some(receipt.clone())
        }
        _ => previous.clone(),
    }
}

fn folded_signed_commit_authorization_for_receipt(
    fact: &WorkflowFact,
    receipt: &SignedCommitReceipt,
    previous: &Option<SignedCommitOperation>,
) -> Option<SignedCommitOperation> {
    folded_signed_commit_authorization(fact, &receipt.operation_id, previous)
}

fn folded_signed_commit_receipt_for_receipt(
    fact: &WorkflowFact,
    receipt: &SignedCommitReceipt,
    previous: &Option<SignedCommitReceipt>,
) -> Option<SignedCommitReceipt> {
    folded_signed_commit_receipt(fact, &receipt.operation_id, previous)
}

mapping! { RecordSignedCommitEventToAuthorization:
(WorkflowAuthorityEvent.fact, RecordSignedCommit.receipt, previous(SignedCommitState.authorization)) => SignedCommitState.authorization using folded_signed_commit_authorization_for_receipt; }
mapping! { RecordSignedCommitEventToReceipt:
(WorkflowAuthorityEvent.fact, RecordSignedCommit.receipt, previous(SignedCommitState.receipt)) => SignedCommitState.receipt using folded_signed_commit_receipt_for_receipt; }

fn fold_delivery_lifecycle(
    fact: &crate::workflow::LifecycleFact,
    delivering: &mut bool,
    clean_review_observed: &mut bool,
) {
    match fact {
        crate::workflow::LifecycleFact::CleanReviewAccepted
        | crate::workflow::LifecycleFact::CleanReviewEvidenceAccepted { .. } => {
            *clean_review_observed = true;
        }
        crate::workflow::LifecycleFact::WorkflowStarted { .. }
        | crate::workflow::LifecycleFact::RedEvidenceAccepted
        | crate::workflow::LifecycleFact::ReturnedToRed
        | crate::workflow::LifecycleFact::CheckpointAbortApplied => {
            *clean_review_observed = false;
            *delivering = false;
        }
        crate::workflow::LifecycleFact::DeliveryAuthorized => *delivering = true,
        crate::workflow::LifecycleFact::DeliveryCompleted
        | crate::workflow::LifecycleFact::WorkflowAbandoned => *delivering = false,
        _ => {}
    }
}

impl AuthorizeSignedCommitState {
    fn from_event(
        previous: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &AuthorizeSignedCommit,
    ) -> Modeled<Self> {
        let mut state = previous.into_inner();
        state.state_epoch = folded_epoch(&event.fact, &state.state_epoch);
        state.authorization = folded_signed_commit_authorization(
            &event.fact,
            &command.intent.operation_id,
            &state.authorization,
        );
        state.receipt =
            folded_signed_commit_receipt(&event.fact, &command.intent.operation_id, &state.receipt);
        match &event.fact {
            WorkflowFact::AssignmentIssued { assignment }
                if assignment.id == command.intent.assignment_id =>
            {
                state.assignment = Some(assignment.clone());
            }
            WorkflowFact::CheckpointCaptured { checkpoint }
                if checkpoint.id == command.intent.checkpoint_id =>
            {
                state.checkpoint = Some(checkpoint.clone());
            }
            WorkflowFact::Lifecycle(fact) => {
                fold_delivery_lifecycle(
                    fact,
                    &mut state.delivering,
                    &mut state.clean_review_observed,
                );
            }
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                for fact in facts {
                    fold_delivery_lifecycle(
                        fact,
                        &mut state.delivering,
                        &mut state.clean_review_observed,
                    );
                }
            }
            WorkflowFact::LegacySemanticHistoryImported { facts, .. } => {
                for fact in facts {
                    match fact {
                        LegacySemanticFact::AssignmentIssued { assignment }
                            if assignment.id == command.intent.assignment_id =>
                        {
                            state.assignment = Some(assignment.clone());
                        }
                        LegacySemanticFact::CheckpointCaptured { checkpoint }
                            if checkpoint.id == command.intent.checkpoint_id =>
                        {
                            state.checkpoint = Some(checkpoint.clone());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
}

#[derive(ModelState)]
struct SignedTagState {
    #[model(default)]
    authorization: Option<SignedTagOperation>,
    #[model(default)]
    receipt: Option<SignedTagReceipt>,
}

/// The tag authorization additionally needs the specific completed commit it
/// names; unrelated delivery history is intentionally not retained here.
#[derive(ModelState)]
struct AuthorizeSignedTagState {
    #[model(default)]
    authorization: Option<SignedTagOperation>,
    #[model(default)]
    receipt: Option<SignedTagReceipt>,
    #[model(default)]
    assignment: Option<Assignment>,
    #[model(default)]
    commit_receipt: Option<SignedCommitReceipt>,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    delivering: bool,
    #[model(default)]
    clean_review_observed: bool,
}

fn folded_signed_tag_authorization(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<SignedTagOperation>,
) -> Option<SignedTagOperation> {
    match fact {
        WorkflowFact::SignedTagAuthorized { operation }
            if operation.operation_id == operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_signed_tag_receipt(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<SignedTagReceipt>,
) -> Option<SignedTagReceipt> {
    match fact {
        WorkflowFact::SignedTagCreated { receipt } if receipt.operation_id == operation_id => {
            Some(receipt.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_signed_tag_authorization_for_receipt(
    fact: &WorkflowFact,
    receipt: &SignedTagReceipt,
    previous: &Option<SignedTagOperation>,
) -> Option<SignedTagOperation> {
    folded_signed_tag_authorization(fact, &receipt.operation_id, previous)
}
fn folded_signed_tag_receipt_for_receipt(
    fact: &WorkflowFact,
    receipt: &SignedTagReceipt,
    previous: &Option<SignedTagReceipt>,
) -> Option<SignedTagReceipt> {
    folded_signed_tag_receipt(fact, &receipt.operation_id, previous)
}
mapping! { RecordSignedTagEventToAuthorization:
(WorkflowAuthorityEvent.fact, RecordSignedTag.receipt, previous(SignedTagState.authorization)) => SignedTagState.authorization using folded_signed_tag_authorization_for_receipt; }
mapping! { RecordSignedTagEventToReceipt:
(WorkflowAuthorityEvent.fact, RecordSignedTag.receipt, previous(SignedTagState.receipt)) => SignedTagState.receipt using folded_signed_tag_receipt_for_receipt; }

impl AuthorizeSignedTagState {
    fn from_event(
        previous: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &AuthorizeSignedTag,
    ) -> Modeled<Self> {
        let mut state = previous.into_inner();
        state.state_epoch = folded_epoch(&event.fact, &state.state_epoch);
        state.authorization = folded_signed_tag_authorization(
            &event.fact,
            &command.intent.operation_id,
            &state.authorization,
        );
        state.receipt =
            folded_signed_tag_receipt(&event.fact, &command.intent.operation_id, &state.receipt);
        match &event.fact {
            WorkflowFact::AssignmentIssued { assignment }
                if assignment.id == command.intent.assignment_id =>
            {
                state.assignment = Some(assignment.clone());
            }
            WorkflowFact::SignedCommitCreated { receipt }
                if receipt.operation_id == command.intent.commit_operation_id =>
            {
                state.commit_receipt = Some(receipt.clone());
            }
            WorkflowFact::Lifecycle(fact) => {
                fold_delivery_lifecycle(
                    fact,
                    &mut state.delivering,
                    &mut state.clean_review_observed,
                );
            }
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                for fact in facts {
                    fold_delivery_lifecycle(
                        fact,
                        &mut state.delivering,
                        &mut state.clean_review_observed,
                    );
                }
            }
            WorkflowFact::LegacySemanticHistoryImported { facts, .. } => {
                for fact in facts {
                    if let LegacySemanticFact::AssignmentIssued { assignment } = fact {
                        if assignment.id == command.intent.assignment_id {
                            state.assignment = Some(assignment.clone());
                        }
                    }
                }
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
}

#[derive(ModelState)]
struct FetchRefState {
    #[model(default)]
    authorization: Option<FetchRefOperation>,
    #[model(default)]
    receipt: Option<FetchRefReceipt>,
}
#[derive(ModelState)]
struct AuthorizeFetchRefState {
    #[model(default)]
    authorization: Option<FetchRefOperation>,
    #[model(default)]
    receipt: Option<FetchRefReceipt>,
    #[model(default)]
    delivery_assignment: Option<RemoteDeliveryAssignment>,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    delivering: bool,
    #[model(default)]
    clean_review_observed: bool,
}

/// The remote commands need only the delivery assignment attributes which
/// authorize a network mutation; retaining scopes and command IDs would make
/// their checked state wider without affecting a decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RemoteDeliveryAssignment {
    state_epoch: u64,
    configuration_digest: String,
    expires_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
struct RemoteDeliveryGate {
    assignment: Option<RemoteDeliveryAssignment>,
    delivering: bool,
    clean_review_observed: bool,
}

fn folded_remote_delivery_assignment(
    fact: &WorkflowFact,
    assignment_id: &str,
    previous: &Option<RemoteDeliveryAssignment>,
) -> Option<RemoteDeliveryAssignment> {
    match fact {
        WorkflowFact::AssignmentIssued { assignment }
            if assignment.id == assignment_id && assignment.role == Role::Delivery =>
        {
            Some(RemoteDeliveryAssignment {
                state_epoch: assignment.state_epoch,
                configuration_digest: assignment.configuration_digest.clone(),
                expires_at: assignment.expires_at,
            })
        }
        WorkflowFact::LegacySemanticHistoryImported { facts, .. } => {
            facts
                .iter()
                .fold(previous.clone(), |assignment, fact| match fact {
                    LegacySemanticFact::AssignmentIssued { assignment: issued }
                        if issued.id == assignment_id && issued.role == Role::Delivery =>
                    {
                        Some(RemoteDeliveryAssignment {
                            state_epoch: issued.state_epoch,
                            configuration_digest: issued.configuration_digest.clone(),
                            expires_at: issued.expires_at,
                        })
                    }
                    _ => assignment,
                })
        }
        _ => previous.clone(),
    }
}
fn folded_fetch_ref_authorization(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<FetchRefOperation>,
) -> Option<FetchRefOperation> {
    match fact {
        WorkflowFact::RemoteRefFetchAuthorized { operation }
            if operation.operation_id == operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_fetch_ref_receipt(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<FetchRefReceipt>,
) -> Option<FetchRefReceipt> {
    match fact {
        WorkflowFact::RemoteRefFetched { receipt } if receipt.operation_id == operation_id => {
            Some(receipt.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_fetch_ref_authorization_for_receipt(
    fact: &WorkflowFact,
    receipt: &FetchRefReceipt,
    previous: &Option<FetchRefOperation>,
) -> Option<FetchRefOperation> {
    folded_fetch_ref_authorization(fact, &receipt.operation_id, previous)
}
fn folded_fetch_ref_receipt_for_receipt(
    fact: &WorkflowFact,
    receipt: &FetchRefReceipt,
    previous: &Option<FetchRefReceipt>,
) -> Option<FetchRefReceipt> {
    folded_fetch_ref_receipt(fact, &receipt.operation_id, previous)
}
mapping! { RecordRemoteRefFetchedEventToAuthorization:
(WorkflowAuthorityEvent.fact, RecordRemoteRefFetched.receipt, previous(FetchRefState.authorization)) => FetchRefState.authorization using folded_fetch_ref_authorization_for_receipt; }
mapping! { RecordRemoteRefFetchedEventToReceipt:
(WorkflowAuthorityEvent.fact, RecordRemoteRefFetched.receipt, previous(FetchRefState.receipt)) => FetchRefState.receipt using folded_fetch_ref_receipt_for_receipt; }

impl AuthorizeFetchRefState {
    fn from_event(
        state: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &AuthorizeRemoteRefFetch,
    ) -> Modeled<Self> {
        let mut state = state.into_inner();
        state.state_epoch = folded_epoch(&event.fact, &state.state_epoch);
        state.authorization = folded_fetch_ref_authorization(
            &event.fact,
            &command.intent.operation_id,
            &state.authorization,
        );
        state.receipt =
            folded_fetch_ref_receipt(&event.fact, &command.intent.operation_id, &state.receipt);
        state.delivery_assignment = folded_remote_delivery_assignment(
            &event.fact,
            &command.intent.assignment_id,
            &state.delivery_assignment,
        );
        match &event.fact {
            WorkflowFact::Lifecycle(fact) => fold_delivery_lifecycle(
                fact,
                &mut state.delivering,
                &mut state.clean_review_observed,
            ),
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                for fact in facts {
                    fold_delivery_lifecycle(
                        fact,
                        &mut state.delivering,
                        &mut state.clean_review_observed,
                    );
                }
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
}

#[derive(ModelState)]
struct PushRefState {
    #[model(default)]
    authorization: Option<PushRefOperation>,
    #[model(default)]
    receipt: Option<PushRefReceipt>,
}
#[derive(ModelState)]
struct AuthorizePushRefState {
    #[model(default)]
    authorization: Option<PushRefOperation>,
    #[model(default)]
    receipt: Option<PushRefReceipt>,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    delivery_gate: RemoteDeliveryGate,
    #[model(default)]
    signed_source_object: Option<String>,
    #[model(default)]
    signed_source_assignment_id: Option<String>,
}
fn folded_push_ref_authorization(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<PushRefOperation>,
) -> Option<PushRefOperation> {
    match fact {
        WorkflowFact::RemoteRefPushAuthorized { operation }
            if operation.operation_id == operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_push_ref_receipt(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<PushRefReceipt>,
) -> Option<PushRefReceipt> {
    match fact {
        WorkflowFact::RemoteRefPushed { receipt } if receipt.operation_id == operation_id => {
            Some(receipt.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_push_ref_authorization_for_receipt(
    fact: &WorkflowFact,
    receipt: &PushRefReceipt,
    previous: &Option<PushRefOperation>,
) -> Option<PushRefOperation> {
    folded_push_ref_authorization(fact, &receipt.operation_id, previous)
}
fn folded_push_ref_receipt_for_receipt(
    fact: &WorkflowFact,
    receipt: &PushRefReceipt,
    previous: &Option<PushRefReceipt>,
) -> Option<PushRefReceipt> {
    folded_push_ref_receipt(fact, &receipt.operation_id, previous)
}
mapping! { RecordRemoteRefPushedEventToAuthorization:
(WorkflowAuthorityEvent.fact, RecordRemoteRefPushed.receipt, previous(PushRefState.authorization)) => PushRefState.authorization using folded_push_ref_authorization_for_receipt; }
mapping! { RecordRemoteRefPushedEventToReceipt:
(WorkflowAuthorityEvent.fact, RecordRemoteRefPushed.receipt, previous(PushRefState.receipt)) => PushRefState.receipt using folded_push_ref_receipt_for_receipt; }

fn folded_push_source(
    fact: &WorkflowFact,
    intent: &PushRefIntent,
    object: &Option<String>,
    assignment_id: &Option<String>,
) -> (Option<String>, Option<String>) {
    match (intent.source_kind, fact) {
        (PushSourceKind::Commit, WorkflowFact::SignedCommitCreated { receipt })
            if receipt.operation_id == intent.source_operation_id =>
        {
            (
                Some(receipt.commit.clone()),
                Some(receipt.assignment_id.clone()),
            )
        }
        (PushSourceKind::Tag, WorkflowFact::SignedTagCreated { receipt })
            if receipt.operation_id == intent.source_operation_id =>
        {
            (
                Some(receipt.tag_object.clone()),
                Some(receipt.assignment_id.clone()),
            )
        }
        _ => (object.clone(), assignment_id.clone()),
    }
}

impl AuthorizePushRefState {
    fn from_event(
        previous: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &AuthorizeRemoteRefPush,
    ) -> Modeled<Self> {
        let mut state = previous.into_inner();
        state.state_epoch = folded_epoch(&event.fact, &state.state_epoch);
        state.authorization = folded_push_ref_authorization(
            &event.fact,
            &command.intent.operation_id,
            &state.authorization,
        );
        state.receipt =
            folded_push_ref_receipt(&event.fact, &command.intent.operation_id, &state.receipt);
        state.delivery_gate.assignment = folded_remote_delivery_assignment(
            &event.fact,
            &command.intent.assignment_id,
            &state.delivery_gate.assignment,
        );
        (
            state.signed_source_object,
            state.signed_source_assignment_id,
        ) = folded_push_source(
            &event.fact,
            &command.intent,
            &state.signed_source_object,
            &state.signed_source_assignment_id,
        );
        match &event.fact {
            WorkflowFact::Lifecycle(fact) => fold_delivery_lifecycle(
                fact,
                &mut state.delivery_gate.delivering,
                &mut state.delivery_gate.clean_review_observed,
            ),
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                for fact in facts {
                    fold_delivery_lifecycle(
                        fact,
                        &mut state.delivery_gate.delivering,
                        &mut state.delivery_gate.clean_review_observed,
                    );
                }
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
}

#[derive(ModelState)]
struct OpenPullRequestState {
    #[model(default)]
    authorization: Option<OpenPullRequestOperation>,
    #[model(default)]
    receipt: Option<OpenPullRequestReceipt>,
}
#[derive(ModelState)]
struct AuthorizeOpenPullRequestState {
    #[model(default)]
    authorization: Option<OpenPullRequestOperation>,
    #[model(default)]
    receipt: Option<OpenPullRequestReceipt>,
    #[model(default)]
    delivery_gate: RemoteDeliveryGate,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    push_assignment_id: Option<String>,
    #[model(default)]
    push_head_ref: Option<String>,
}
impl AuthorizeOpenPullRequestState {
    fn from_event(
        state: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &OpenPullRequest,
    ) -> Modeled<Self> {
        let mut state = state.into_inner();
        state.state_epoch = folded_epoch(&event.fact, &state.state_epoch);
        state.authorization = folded_open_pr_authorization(
            &event.fact,
            &command.intent.operation_id,
            &state.authorization,
        );
        state.receipt =
            folded_open_pr_receipt(&event.fact, &command.intent.operation_id, &state.receipt);
        state.delivery_gate.assignment = folded_remote_delivery_assignment(
            &event.fact,
            &command.intent.assignment_id,
            &state.delivery_gate.assignment,
        );
        if let WorkflowFact::RemoteRefPushed { receipt } = &event.fact {
            if receipt.operation_id == command.intent.push_operation_id {
                state.push_assignment_id = Some(receipt.assignment_id.clone());
                state.push_head_ref = receipt
                    .remote_ref
                    .strip_prefix("refs/heads/")
                    .map(str::to_string);
            }
        }
        match &event.fact {
            WorkflowFact::Lifecycle(fact) => fold_delivery_lifecycle(
                fact,
                &mut state.delivery_gate.delivering,
                &mut state.delivery_gate.clean_review_observed,
            ),
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                for fact in facts {
                    fold_delivery_lifecycle(
                        fact,
                        &mut state.delivery_gate.delivering,
                        &mut state.delivery_gate.clean_review_observed,
                    );
                }
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
}

fn folded_open_pr_authorization(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<OpenPullRequestOperation>,
) -> Option<OpenPullRequestOperation> {
    match fact {
        WorkflowFact::PullRequestOpenAuthorized { operation }
            if operation.operation_id == operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}

fn folded_open_pr_receipt(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<OpenPullRequestReceipt>,
) -> Option<OpenPullRequestReceipt> {
    match fact {
        WorkflowFact::PullRequestOpened { receipt } if receipt.operation_id == operation_id => {
            Some(receipt.clone())
        }
        _ => previous.clone(),
    }
}

fn folded_open_pr_authorization_for_receipt(
    fact: &WorkflowFact,
    receipt: &OpenPullRequestReceipt,
    previous: &Option<OpenPullRequestOperation>,
) -> Option<OpenPullRequestOperation> {
    folded_open_pr_authorization(fact, &receipt.operation_id, previous)
}
fn folded_open_pr_receipt_for_receipt(
    fact: &WorkflowFact,
    receipt: &OpenPullRequestReceipt,
    previous: &Option<OpenPullRequestReceipt>,
) -> Option<OpenPullRequestReceipt> {
    folded_open_pr_receipt(fact, &receipt.operation_id, previous)
}

mapping! { RecordPullRequestOpenedEventToAuthorization:
(WorkflowAuthorityEvent.fact, RecordPullRequestOpened.receipt, previous(OpenPullRequestState.authorization)) => OpenPullRequestState.authorization using folded_open_pr_authorization_for_receipt; }
mapping! { RecordPullRequestOpenedEventToReceipt:
(WorkflowAuthorityEvent.fact, RecordPullRequestOpened.receipt, previous(OpenPullRequestState.receipt)) => OpenPullRequestState.receipt using folded_open_pr_receipt_for_receipt; }

#[derive(ModelState)]
struct UpdatePullRequestState {
    #[model(default)]
    authorization: Option<UpdatePullRequestOperation>,
    #[model(default)]
    receipt: Option<UpdatePullRequestReceipt>,
}
#[derive(ModelState)]
struct AuthorizeUpdatePullRequestState {
    #[model(default)]
    authorization: Option<UpdatePullRequestOperation>,
    #[model(default)]
    receipt: Option<UpdatePullRequestReceipt>,
    #[model(default)]
    delivery_gate: RemoteDeliveryGate,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    opened: Option<OpenPullRequestReceipt>,
}
impl AuthorizeUpdatePullRequestState {
    fn from_event(
        state: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &UpdatePullRequest,
    ) -> Modeled<Self> {
        let mut state = state.into_inner();
        state.state_epoch = folded_epoch(&event.fact, &state.state_epoch);
        state.authorization = folded_update_pr_authorization(
            &event.fact,
            &command.intent.operation_id,
            &state.authorization,
        );
        state.receipt =
            folded_update_pr_receipt(&event.fact, &command.intent.operation_id, &state.receipt);
        state.delivery_gate.assignment = folded_remote_delivery_assignment(
            &event.fact,
            &command.intent.assignment_id,
            &state.delivery_gate.assignment,
        );
        if let WorkflowFact::PullRequestOpened { receipt } = &event.fact {
            if receipt.operation_id == command.intent.open_operation_id {
                state.opened = Some(receipt.clone());
            }
        }
        match &event.fact {
            WorkflowFact::Lifecycle(fact) => fold_delivery_lifecycle(
                fact,
                &mut state.delivery_gate.delivering,
                &mut state.delivery_gate.clean_review_observed,
            ),
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                for fact in facts {
                    fold_delivery_lifecycle(
                        fact,
                        &mut state.delivery_gate.delivering,
                        &mut state.delivery_gate.clean_review_observed,
                    );
                }
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
}
fn folded_update_pr_authorization(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<UpdatePullRequestOperation>,
) -> Option<UpdatePullRequestOperation> {
    match fact {
        WorkflowFact::PullRequestUpdateAuthorized { operation }
            if operation.operation_id == operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_update_pr_receipt(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<UpdatePullRequestReceipt>,
) -> Option<UpdatePullRequestReceipt> {
    match fact {
        WorkflowFact::PullRequestUpdated { receipt } if receipt.operation_id == operation_id => {
            Some(receipt.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_update_pr_authorization_for_receipt(
    fact: &WorkflowFact,
    receipt: &UpdatePullRequestReceipt,
    previous: &Option<UpdatePullRequestOperation>,
) -> Option<UpdatePullRequestOperation> {
    folded_update_pr_authorization(fact, &receipt.operation_id, previous)
}
fn folded_update_pr_receipt_for_receipt(
    fact: &WorkflowFact,
    receipt: &UpdatePullRequestReceipt,
    previous: &Option<UpdatePullRequestReceipt>,
) -> Option<UpdatePullRequestReceipt> {
    folded_update_pr_receipt(fact, &receipt.operation_id, previous)
}
mapping! { RecordPullRequestUpdatedEventToAuthorization:
(WorkflowAuthorityEvent.fact, RecordPullRequestUpdated.receipt, previous(UpdatePullRequestState.authorization)) => UpdatePullRequestState.authorization using folded_update_pr_authorization_for_receipt; }
mapping! { RecordPullRequestUpdatedEventToReceipt:
(WorkflowAuthorityEvent.fact, RecordPullRequestUpdated.receipt, previous(UpdatePullRequestState.receipt)) => UpdatePullRequestState.receipt using folded_update_pr_receipt_for_receipt; }

#[derive(ModelState)]
struct MergePullRequestState {
    #[model(default)]
    authorization: Option<MergePullRequestOperation>,
    #[model(default)]
    receipt: Option<MergePullRequestReceipt>,
}
#[derive(ModelState)]
struct AuthorizeMergePullRequestState {
    #[model(default)]
    authorization: Option<MergePullRequestOperation>,
    #[model(default)]
    receipt: Option<MergePullRequestReceipt>,
    #[model(default)]
    delivery_gate: RemoteDeliveryGate,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    opened: Option<OpenPullRequestReceipt>,
    #[model(default)]
    updated: Option<UpdatePullRequestReceipt>,
}
impl AuthorizeMergePullRequestState {
    fn from_event(
        state: Modeled<Self>,
        event: &WorkflowAuthorityEvent,
        command: &MergePullRequest,
    ) -> Modeled<Self> {
        let mut state = state.into_inner();
        state.state_epoch = folded_epoch(&event.fact, &state.state_epoch);
        state.authorization = folded_merge_pr_authorization(
            &event.fact,
            &command.intent.operation_id,
            &state.authorization,
        );
        state.receipt =
            folded_merge_pr_receipt(&event.fact, &command.intent.operation_id, &state.receipt);
        state.delivery_gate.assignment = folded_remote_delivery_assignment(
            &event.fact,
            &command.intent.assignment_id,
            &state.delivery_gate.assignment,
        );
        match &event.fact {
            WorkflowFact::PullRequestOpened { receipt }
                if receipt.operation_id == command.intent.open_operation_id =>
            {
                state.opened = Some(receipt.clone());
            }
            WorkflowFact::PullRequestUpdated { receipt }
                if receipt.open_operation_id == command.intent.open_operation_id =>
            {
                state.updated = Some(receipt.clone());
            }
            WorkflowFact::Lifecycle(fact) => fold_delivery_lifecycle(
                fact,
                &mut state.delivery_gate.delivering,
                &mut state.delivery_gate.clean_review_observed,
            ),
            WorkflowFact::LegacyLifecycleHistoryImported { facts, .. } => {
                for fact in facts {
                    fold_delivery_lifecycle(
                        fact,
                        &mut state.delivery_gate.delivering,
                        &mut state.delivery_gate.clean_review_observed,
                    );
                }
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
}
fn folded_merge_pr_authorization(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<MergePullRequestOperation>,
) -> Option<MergePullRequestOperation> {
    match fact {
        WorkflowFact::PullRequestMergeAuthorized { operation }
            if operation.operation_id == operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_merge_pr_receipt(
    fact: &WorkflowFact,
    operation_id: &str,
    previous: &Option<MergePullRequestReceipt>,
) -> Option<MergePullRequestReceipt> {
    match fact {
        WorkflowFact::PullRequestMerged { receipt } if receipt.operation_id == operation_id => {
            Some(receipt.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_merge_pr_authorization_for_receipt(
    fact: &WorkflowFact,
    receipt: &MergePullRequestReceipt,
    previous: &Option<MergePullRequestOperation>,
) -> Option<MergePullRequestOperation> {
    folded_merge_pr_authorization(fact, &receipt.operation_id, previous)
}
fn folded_merge_pr_receipt_for_receipt(
    fact: &WorkflowFact,
    receipt: &MergePullRequestReceipt,
    previous: &Option<MergePullRequestReceipt>,
) -> Option<MergePullRequestReceipt> {
    folded_merge_pr_receipt(fact, &receipt.operation_id, previous)
}
mapping! { RecordPullRequestMergedEventToAuthorization:
(WorkflowAuthorityEvent.fact, RecordPullRequestMerged.receipt, previous(MergePullRequestState.authorization)) => MergePullRequestState.authorization using folded_merge_pr_authorization_for_receipt; }
mapping! { RecordPullRequestMergedEventToReceipt:
(WorkflowAuthorityEvent.fact, RecordPullRequestMerged.receipt, previous(MergePullRequestState.receipt)) => MergePullRequestState.receipt using folded_merge_pr_receipt_for_receipt; }

fn folded_initial_dirty_paths(
    fact: &WorkflowFact,
    previous: &Option<BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    match fact {
        WorkflowFact::Lifecycle(crate::workflow::LifecycleFact::WorkflowStarted {
            initial_repository,
            ..
        }) => Some(initial_repository.dirty_paths.clone()),
        _ => previous.clone(),
    }
}

fn folded_write_assignment(
    fact: &WorkflowFact,
    operation: &WorkspaceFileWrite,
    previous: &Option<Assignment>,
) -> Option<Assignment> {
    match fact {
        WorkflowFact::AssignmentIssued { assignment }
            if assignment.id == operation.assignment_id =>
        {
            Some(assignment.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_write_authorization(
    fact: &WorkflowFact,
    target: &WorkspaceFileWrite,
    previous: &Option<WorkspaceFileWrite>,
) -> Option<WorkspaceFileWrite> {
    match fact {
        WorkflowFact::FileWriteAuthorized { operation }
            if operation.operation_id == target.operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_write_completed(
    fact: &WorkflowFact,
    target: &WorkspaceFileWrite,
    previous: &bool,
) -> bool {
    *previous
        || matches!(fact, WorkflowFact::FileWritten { operation } if operation.operation_id == target.operation_id)
}

#[derive(ModelState)]
struct AuthorizeFileWriteState {
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    assignment: Option<Assignment>,
    #[model(default)]
    authorization: Option<WorkspaceFileWrite>,
    #[model(default)]
    completed: bool,
    #[model(default)]
    initial_dirty_paths: Option<BTreeSet<String>>,
}
mapping! { AuthorizeFileWriteEventToEpoch:
(WorkflowAuthorityEvent.fact, previous(AuthorizeFileWriteState.state_epoch)) => AuthorizeFileWriteState.state_epoch using folded_epoch; }
mapping! { AuthorizeFileWriteEventToAssignment:
(WorkflowAuthorityEvent.fact, AuthorizeFileWrite.operation, previous(AuthorizeFileWriteState.assignment)) => AuthorizeFileWriteState.assignment using folded_write_assignment; }
mapping! { AuthorizeFileWriteEventToAuthorization:
(WorkflowAuthorityEvent.fact, AuthorizeFileWrite.operation, previous(AuthorizeFileWriteState.authorization)) => AuthorizeFileWriteState.authorization using folded_write_authorization; }
mapping! { AuthorizeFileWriteEventToCompleted:
(WorkflowAuthorityEvent.fact, AuthorizeFileWrite.operation, previous(AuthorizeFileWriteState.completed)) => AuthorizeFileWriteState.completed using folded_write_completed; }
mapping! { AuthorizeFileWriteEventToInitialDirtyPaths:
(WorkflowAuthorityEvent.fact, previous(AuthorizeFileWriteState.initial_dirty_paths)) => AuthorizeFileWriteState.initial_dirty_paths using folded_initial_dirty_paths; }

#[derive(ModelState)]
struct ConfirmFileWrittenState {
    #[model(default)]
    authorization: Option<WorkspaceFileWrite>,
    #[model(default)]
    completed: bool,
}
mapping! { ConfirmFileWrittenEventToAuthorization:
(WorkflowAuthorityEvent.fact, ConfirmFileWritten.operation, previous(ConfirmFileWrittenState.authorization)) => ConfirmFileWrittenState.authorization using folded_write_authorization; }
mapping! { ConfirmFileWrittenEventToCompleted:
(WorkflowAuthorityEvent.fact, ConfirmFileWritten.operation, previous(ConfirmFileWrittenState.completed)) => ConfirmFileWrittenState.completed using folded_write_completed; }

fn folded_delete_assignment(
    fact: &WorkflowFact,
    operation: &WorkspaceFileDeletion,
    previous: &Option<Assignment>,
) -> Option<Assignment> {
    match fact {
        WorkflowFact::AssignmentIssued { assignment }
            if assignment.id == operation.assignment_id =>
        {
            Some(assignment.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_delete_authorization(
    fact: &WorkflowFact,
    target: &WorkspaceFileDeletion,
    previous: &Option<WorkspaceFileDeletion>,
) -> Option<WorkspaceFileDeletion> {
    match fact {
        WorkflowFact::FileDeleteAuthorized { operation }
            if operation.operation_id == target.operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_delete_completed(
    fact: &WorkflowFact,
    target: &WorkspaceFileDeletion,
    previous: &bool,
) -> bool {
    *previous
        || matches!(fact, WorkflowFact::FileDeleted { operation } if operation.operation_id == target.operation_id)
}

#[derive(ModelState)]
struct AuthorizeFileDeleteState {
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    assignment: Option<Assignment>,
    #[model(default)]
    authorization: Option<WorkspaceFileDeletion>,
    #[model(default)]
    initial_dirty_paths: Option<BTreeSet<String>>,
}
mapping! { AuthorizeFileDeleteEventToEpoch:
(WorkflowAuthorityEvent.fact, previous(AuthorizeFileDeleteState.state_epoch)) => AuthorizeFileDeleteState.state_epoch using folded_epoch; }
mapping! { AuthorizeFileDeleteEventToAssignment:
(WorkflowAuthorityEvent.fact, AuthorizeFileDelete.operation, previous(AuthorizeFileDeleteState.assignment)) => AuthorizeFileDeleteState.assignment using folded_delete_assignment; }
mapping! { AuthorizeFileDeleteEventToAuthorization:
(WorkflowAuthorityEvent.fact, AuthorizeFileDelete.operation, previous(AuthorizeFileDeleteState.authorization)) => AuthorizeFileDeleteState.authorization using folded_delete_authorization; }
mapping! { AuthorizeFileDeleteEventToInitialDirtyPaths:
(WorkflowAuthorityEvent.fact, previous(AuthorizeFileDeleteState.initial_dirty_paths)) => AuthorizeFileDeleteState.initial_dirty_paths using folded_initial_dirty_paths; }

#[derive(ModelState)]
struct ConfirmFileDeletedState {
    #[model(default)]
    authorization: Option<WorkspaceFileDeletion>,
    #[model(default)]
    completed: bool,
}
mapping! { ConfirmFileDeletedEventToAuthorization:
(WorkflowAuthorityEvent.fact, ConfirmFileDeleted.operation, previous(ConfirmFileDeletedState.authorization)) => ConfirmFileDeletedState.authorization using folded_delete_authorization; }
mapping! { ConfirmFileDeletedEventToCompleted:
(WorkflowAuthorityEvent.fact, ConfirmFileDeleted.operation, previous(ConfirmFileDeletedState.completed)) => ConfirmFileDeletedState.completed using folded_delete_completed; }

fn folded_move_assignment(
    fact: &WorkflowFact,
    operation: &WorkspaceFileMove,
    previous: &Option<Assignment>,
) -> Option<Assignment> {
    match fact {
        WorkflowFact::AssignmentIssued { assignment }
            if assignment.id == operation.assignment_id =>
        {
            Some(assignment.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_move_authorization(
    fact: &WorkflowFact,
    target: &WorkspaceFileMove,
    previous: &Option<WorkspaceFileMove>,
) -> Option<WorkspaceFileMove> {
    match fact {
        WorkflowFact::FileMoveAuthorized { operation }
            if operation.operation_id == target.operation_id =>
        {
            Some(operation.clone())
        }
        _ => previous.clone(),
    }
}
fn folded_move_completed(fact: &WorkflowFact, target: &WorkspaceFileMove, previous: &bool) -> bool {
    *previous
        || matches!(fact, WorkflowFact::FileMoved { operation } if operation.operation_id == target.operation_id)
}
#[derive(ModelState)]
struct AuthorizeFileMoveState {
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    assignment: Option<Assignment>,
    #[model(default)]
    authorization: Option<WorkspaceFileMove>,
    #[model(default)]
    initial_dirty_paths: Option<BTreeSet<String>>,
}
mapping! { AuthorizeFileMoveEventToEpoch:
(WorkflowAuthorityEvent.fact, previous(AuthorizeFileMoveState.state_epoch)) => AuthorizeFileMoveState.state_epoch using folded_epoch; }
mapping! { AuthorizeFileMoveEventToAssignment:
(WorkflowAuthorityEvent.fact, AuthorizeFileMove.operation, previous(AuthorizeFileMoveState.assignment)) => AuthorizeFileMoveState.assignment using folded_move_assignment; }
mapping! { AuthorizeFileMoveEventToAuthorization:
(WorkflowAuthorityEvent.fact, AuthorizeFileMove.operation, previous(AuthorizeFileMoveState.authorization)) => AuthorizeFileMoveState.authorization using folded_move_authorization; }
mapping! { AuthorizeFileMoveEventToInitialDirtyPaths:
(WorkflowAuthorityEvent.fact, previous(AuthorizeFileMoveState.initial_dirty_paths)) => AuthorizeFileMoveState.initial_dirty_paths using folded_initial_dirty_paths; }
#[derive(ModelState)]
struct ConfirmFileMovedState {
    #[model(default)]
    authorization: Option<WorkspaceFileMove>,
    #[model(default)]
    completed: bool,
}
mapping! { ConfirmFileMovedEventToAuthorization:
(WorkflowAuthorityEvent.fact, ConfirmFileMoved.operation, previous(ConfirmFileMovedState.authorization)) => ConfirmFileMovedState.authorization using folded_move_authorization; }
mapping! { ConfirmFileMovedEventToCompleted:
(WorkflowAuthorityEvent.fact, ConfirmFileMoved.operation, previous(ConfirmFileMovedState.completed)) => ConfirmFileMovedState.completed using folded_move_completed; }

#[derive(ModelInput)]
struct AuthorizeCheckpointAbortRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    operation: CheckpointAbortOperation,
}

#[derive(ModelCommand)]
struct AuthorizeCheckpointAbort {
    #[stream]
    stream: WorkflowAuthorityStream,
    operation: CheckpointAbortOperation,
}

mapping! { AuthorizeCheckpointAbortRequestToStream: AuthorizeCheckpointAbortRequest.stream => AuthorizeCheckpointAbort.stream using clone; }
mapping! { AuthorizeCheckpointAbortRequestToOperation: AuthorizeCheckpointAbortRequest.operation => AuthorizeCheckpointAbort.operation using clone; }
mapping! { AuthorizeCheckpointAbortStreamToEvent: AuthorizeCheckpointAbort.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn checkpoint_abort_authorized_fact(
    operation: &CheckpointAbortOperation,
    _: &bool,
    _: &Option<String>,
    _: &Option<String>,
    _: &BTreeSet<String>,
    _: &bool,
) -> WorkflowFact {
    WorkflowFact::CheckpointAbortAuthorized {
        operation: operation.clone(),
    }
}
mapping! { AuthorizeCheckpointAbortToFact:
    (AuthorizeCheckpointAbort.operation, AuthorizeCheckpointAbortState.abort_permitted, AuthorizeCheckpointAbortState.last_checkpoint_id, AuthorizeCheckpointAbortState.last_checkpoint_tree, AuthorizeCheckpointAbortState.changed_paths, AuthorizeCheckpointAbortState.operation_id_seen) => WorkflowAuthorityEvent.fact
    using checkpoint_abort_authorized_fact;
}

#[derive(ModelState)]
struct AuthorizeCheckpointAbortState {
    #[model(default)]
    abort_permitted: bool,
    #[model(default)]
    last_checkpoint_id: Option<String>,
    #[model(default)]
    last_checkpoint_tree: Option<String>,
    #[model(default)]
    changed_paths: BTreeSet<String>,
    #[model(default)]
    operation_id_seen: bool,
}

impl ModelCommandLogic for AuthorizeCheckpointAbort {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeCheckpointAbortState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        if let WorkflowFact::Lifecycle(fact) = &event.fact {
            state.abort_permitted = !matches!(
                crate::workflow::phase_after_lifecycle_fact(fact),
                crate::workflow::Phase::Delivered | crate::workflow::Phase::Abandoned
            );
        }
        match &event.fact {
            WorkflowFact::CheckpointCaptured { checkpoint } => {
                state.last_checkpoint_id = Some(checkpoint.id.clone());
                state.last_checkpoint_tree = Some(checkpoint.index_tree.clone());
                state.changed_paths.clear();
            }
            WorkflowFact::FileWritten { operation } => {
                state.changed_paths.insert(operation.path.clone());
            }
            WorkflowFact::FileDeleted { operation } => {
                state.changed_paths.insert(operation.path.clone());
            }
            WorkflowFact::FileMoved { operation } => {
                state.changed_paths.insert(operation.from.clone());
                state.changed_paths.insert(operation.to.clone());
            }
            WorkflowFact::CheckpointAbortAuthorized { operation }
                if operation.operation_id == self.operation.operation_id =>
            {
                state.operation_id_seen = true;
            }
            WorkflowFact::CheckpointAbortCompleted { .. } => state.changed_paths.clear(),
            _ => {}
        }
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if !state.as_ref().abort_permitted {
            return Err(CommandError::ValidationError(
                "development_system.checkpoint_abort_phase_invalid".to_string(),
            ));
        }
        let checkpoint_id = state.as_ref().last_checkpoint_id.as_ref().ok_or_else(|| {
            CommandError::ValidationError(
                "development_system.checkpoint_abort_checkpoint_required".to_string(),
            )
        })?;
        let checkpoint_tree = state
            .as_ref()
            .last_checkpoint_tree
            .as_ref()
            .ok_or_else(|| {
                CommandError::ValidationError(
                    "development_system.checkpoint_abort_checkpoint_required".to_string(),
                )
            })?;
        if self.operation.operation_id.is_empty()
            || state.as_ref().operation_id_seen
            || &self.operation.checkpoint_id != checkpoint_id
            || &self.operation.checkpoint_tree != checkpoint_tree
            || self.operation.affected_paths != state.as_ref().changed_paths
            || self.operation.affected_paths.is_empty()
        {
            return Err(CommandError::ValidationError(
                "development_system.checkpoint_abort_stale_preview".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeCheckpointAbortStreamToEvent::apply(self))
                .fact(AuthorizeCheckpointAbortToFact::apply((
                    self,
                    state.as_ref(),
                    state.as_ref(),
                    state.as_ref(),
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct CompleteCheckpointAbortRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: CheckpointAbortReceipt,
}

#[derive(ModelCommand)]
struct CompleteCheckpointAbort {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: CheckpointAbortReceipt,
}
mapping! { CompleteCheckpointAbortRequestToStream: CompleteCheckpointAbortRequest.stream => CompleteCheckpointAbort.stream using clone; }
mapping! { CompleteCheckpointAbortRequestToReceipt: CompleteCheckpointAbortRequest.receipt => CompleteCheckpointAbort.receipt using clone; }
mapping! { CompleteCheckpointAbortStreamToEvent: CompleteCheckpointAbort.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn checkpoint_abort_completed_fact(
    receipt: &CheckpointAbortReceipt,
    _: &bool,
    _: &bool,
) -> WorkflowFact {
    WorkflowFact::CheckpointAbortCompleted {
        receipt: receipt.clone(),
    }
}
mapping! { CompleteCheckpointAbortToFact:
    (CompleteCheckpointAbort.receipt, CompleteCheckpointAbortState.authorized, CompleteCheckpointAbortState.completed) => WorkflowAuthorityEvent.fact
    using checkpoint_abort_completed_fact;
}
fn checkpoint_abort_lifecycle_fact(_: &CheckpointAbortReceipt) -> WorkflowFact {
    WorkflowFact::Lifecycle(crate::workflow::LifecycleFact::CheckpointAbortApplied)
}
mapping! { CompleteCheckpointAbortToLifecycleFact: CompleteCheckpointAbort.receipt => WorkflowAuthorityEvent.fact using checkpoint_abort_lifecycle_fact; }

#[derive(ModelState)]
struct CompleteCheckpointAbortState {
    #[model(default)]
    authorized: bool,
    #[model(default)]
    completed: bool,
}

impl ModelCommandLogic for CompleteCheckpointAbort {
    type Event = WorkflowAuthorityEvent;
    type State = CompleteCheckpointAbortState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        match &event.fact {
            WorkflowFact::CheckpointAbortAuthorized { operation }
                if operation.operation_id == self.receipt.operation_id =>
            {
                state.authorized = true;
            }
            WorkflowFact::CheckpointAbortCompleted { receipt }
                if receipt.operation_id == self.receipt.operation_id =>
            {
                state.completed = true;
            }
            _ => {}
        }
        Modeled::from_built(state)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if !state.as_ref().authorized
            || state.as_ref().completed
            || self.receipt.archive_relative_path.is_empty()
        {
            return Err(CommandError::ValidationError(
                "development_system.checkpoint_abort_completion_invalid".to_string(),
            ));
        }
        let mut events = ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(CompleteCheckpointAbortStreamToEvent::apply(self))
                .fact(CompleteCheckpointAbortToFact::apply((
                    self,
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        );
        events.push(
            WorkflowAuthorityEvent::model_builder()
                .stream(CompleteCheckpointAbortStreamToEvent::apply(self))
                .fact(CompleteCheckpointAbortToLifecycleFact::apply(self))
                .build(),
        );
        Ok(events)
    }
}
fn validate_assignment_issue_intent(
    state: &IssueAssignmentState,
    assignment_id: &str,
    state_epoch: u64,
    expires_at: u64,
) -> Result<(), CommandError> {
    valid_identifier(assignment_id, "assignment").map_err(CommandError::ValidationError)?;
    if expires_at == 0 {
        return Err(CommandError::ValidationError(
            "development_system.assignment_expiry_invalid".to_string(),
        ));
    }
    if state.state_epoch == 0 || state_epoch != state.state_epoch {
        return Err(CommandError::ValidationError(
            "development_system.assignment_stale_epoch".to_string(),
        ));
    }
    if state.assignment_id_seen {
        return Err(CommandError::ValidationError(
            "development_system.assignment_id_reused".to_string(),
        ));
    }
    Ok(())
}

fn validate_checkpoint_capture(
    state: &CaptureCheckpointState,
    intent: &CheckpointIntent,
) -> Result<(), CommandError> {
    if intent.id.is_empty()
        || (intent.index_tree.len() != 40 && intent.index_tree.len() != 64)
        || intent.command_policy_digest.is_empty()
        || intent.expected_state_epoch == 0
        || intent.expected_state_epoch != state.state_epoch
        || intent.expected_predecessor != state.last_checkpoint_id
    {
        return Err(CommandError::ValidationError(
            "development_system.checkpoint_invalid".to_string(),
        ));
    }
    if intent.evidence_ids != state.accepted_evidence_ids {
        return Err(CommandError::ValidationError(
            "development_system.checkpoint_evidence_out_of_sync".to_string(),
        ));
    }
    Ok(())
}

/// Boundary observations for a checkpoint capture. This is intentionally not
/// the durable fact: the command validates the predecessor against its folded
/// history before emitting `CheckpointCaptured` through a checked mapping.
#[derive(Clone, Debug)]
struct CheckpointIntent {
    id: String,
    /// CAS observation from the boundary. The emitted checkpoint records the
    /// epoch folded by this command over the authority stream.
    expected_state_epoch: u64,
    index_tree: String,
    owned_paths: BTreeSet<String>,
    authorized_scope_ids: BTreeSet<String>,
    command_policy_digest: String,
    evidence_ids: BTreeSet<String>,
    expected_predecessor: Option<String>,
    created_at: u64,
}

fn checkpoint_captured_fact(intent: &CheckpointIntent, state_epoch: &u64) -> WorkflowFact {
    WorkflowFact::CheckpointCaptured {
        checkpoint: Checkpoint {
            id: intent.id.clone(),
            state_epoch: *state_epoch,
            index_tree: intent.index_tree.clone(),
            owned_paths: intent.owned_paths.clone(),
            authorized_scope_ids: intent.authorized_scope_ids.clone(),
            command_policy_digest: intent.command_policy_digest.clone(),
            evidence_ids: intent.evidence_ids.clone(),
            predecessor: intent.expected_predecessor.clone(),
            created_at: intent.created_at,
        },
    }
}

/// A coordinator's intent to issue a capability assignment. The command holds
/// the requested authorization dimensions, not a caller-built assignment
/// fact; `decide` creates the durable fact only after checking its folded
/// state.
#[derive(ModelInput)]
struct IssueAssignmentRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    assignment_id: String,
    #[model(origin)]
    role: Role,
    #[model(origin)]
    /// A compare-and-swap observation from the coordinator. The durable
    /// assignment epoch is derived from the command's folded authority state,
    /// never copied from this boundary observation.
    expected_state_epoch: u64,
    #[model(origin)]
    scope_ids: BTreeSet<String>,
    #[model(origin)]
    command_ids: BTreeSet<String>,
    #[model(origin)]
    expires_at: u64,
    #[model(origin)]
    configuration_digest: String,
}

#[derive(ModelCommand)]
struct IssueAssignment {
    #[stream]
    stream: WorkflowAuthorityStream,
    assignment_id: String,
    role: Role,
    expected_state_epoch: u64,
    scope_ids: BTreeSet<String>,
    command_ids: BTreeSet<String>,
    expires_at: u64,
    configuration_digest: String,
}

mapping! { IssueAssignmentRequestToStream: IssueAssignmentRequest.stream => IssueAssignment.stream using clone; }
mapping! { IssueAssignmentRequestToId: IssueAssignmentRequest.assignment_id => IssueAssignment.assignment_id using clone; }
mapping! { IssueAssignmentRequestToRole: IssueAssignmentRequest.role => IssueAssignment.role using clone; }
mapping! { IssueAssignmentRequestToExpectedEpoch: IssueAssignmentRequest.expected_state_epoch => IssueAssignment.expected_state_epoch using clone; }
mapping! { IssueAssignmentRequestToScopes: IssueAssignmentRequest.scope_ids => IssueAssignment.scope_ids using clone; }
mapping! { IssueAssignmentRequestToCommands: IssueAssignmentRequest.command_ids => IssueAssignment.command_ids using clone; }
mapping! { IssueAssignmentRequestToExpiry: IssueAssignmentRequest.expires_at => IssueAssignment.expires_at using clone; }
mapping! { IssueAssignmentRequestToDigest: IssueAssignmentRequest.configuration_digest => IssueAssignment.configuration_digest using clone; }
mapping! { IssueAssignmentStreamToEvent: IssueAssignment.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }

fn assignment_issued_from_intent(
    assignment_id: &str,
    role: &Role,
    state_epoch: &u64,
    scope_ids: &BTreeSet<String>,
    command_ids: &BTreeSet<String>,
    expires_at: &u64,
    configuration_digest: &str,
) -> WorkflowFact {
    WorkflowFact::AssignmentIssued {
        assignment: Assignment {
            id: assignment_id.to_string(),
            role: role.clone(),
            state_epoch: *state_epoch,
            scope_ids: scope_ids.clone(),
            command_ids: command_ids.clone(),
            expires_at: *expires_at,
            configuration_digest: configuration_digest.to_string(),
        },
    }
}

mapping! { IssueAssignmentToFact:
(IssueAssignment.assignment_id, IssueAssignment.role, IssueAssignmentState.state_epoch,
 IssueAssignment.scope_ids, IssueAssignment.command_ids, IssueAssignment.expires_at,
 IssueAssignment.configuration_digest) => WorkflowAuthorityEvent.fact using assignment_issued_from_intent; }

impl ModelCommandLogic for IssueAssignment {
    type Event = WorkflowAuthorityEvent;
    type State = IssueAssignmentState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        IssueAssignmentState::from_event(state, event, self)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        validate_assignment_issue_intent(
            state.as_ref(),
            &self.assignment_id,
            self.expected_state_epoch,
            self.expires_at,
        )?;
        if self.expected_state_epoch != state.as_ref().state_epoch {
            return Err(CommandError::ValidationError(
                "development_system.assignment_stale_epoch".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(IssueAssignmentStreamToEvent::apply(self))
                .fact(IssueAssignmentToFact::apply((
                    self,
                    self,
                    state.as_ref(),
                    self,
                    self,
                    self,
                    self,
                )))
                .build(),
        ))
    }
}
#[derive(ModelInput)]
struct RecordCommandReceiptRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: ReceiptIntent,
}

/// Typed runner observation. This remains command intent data until
/// `RecordCommandReceipt::decide` emits the immutable receipt fact.
#[derive(Clone, Debug)]
struct ReceiptIntent {
    id: String,
    assignment_id: String,
    command_id: String,
    /// Compare-and-swap observation. The receipt fact receives the epoch
    /// folded by `RecordCommandReceipt`, not this boundary value.
    expected_state_epoch: u64,
    configuration_digest: String,
    succeeded: bool,
    output_digest: String,
    observed_output_digests: BTreeMap<String, Option<String>>,
    created_at: u64,
}

#[derive(ModelCommand)]
struct RecordCommandReceipt {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: ReceiptIntent,
}

mapping! { RecordReceiptRequestToStream: RecordCommandReceiptRequest.stream => RecordCommandReceipt.stream using clone; }
mapping! { RecordReceiptRequestToIntent: RecordCommandReceiptRequest.receipt => RecordCommandReceipt.receipt using clone; }
mapping! { RecordReceiptStreamToEvent: RecordCommandReceipt.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }

fn receipt_recorded_from_intent(intent: &ReceiptIntent, state_epoch: &u64) -> WorkflowFact {
    WorkflowFact::CommandReceiptRecorded {
        receipt: CommandReceipt {
            id: intent.id.clone(),
            assignment_id: intent.assignment_id.clone(),
            command_id: intent.command_id.clone(),
            state_epoch: *state_epoch,
            configuration_digest: intent.configuration_digest.clone(),
            succeeded: intent.succeeded,
            output_digest: intent.output_digest.clone(),
            observed_output_digests: intent.observed_output_digests.clone(),
            created_at: intent.created_at,
        },
    }
}
mapping! { RecordReceiptToFact:
    (RecordCommandReceipt.receipt, RecordCommandReceiptState.state_epoch) => WorkflowAuthorityEvent.fact
    using receipt_recorded_from_intent;
}

impl ModelCommandLogic for RecordCommandReceipt {
    type Event = WorkflowAuthorityEvent;
    type State = RecordCommandReceiptState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        RecordCommandReceiptState::from_event(state, event, self)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if !state.as_ref().assignment_exists {
            return Err(CommandError::ValidationError(
                "development_system.command_receipt_assignment_missing".to_string(),
            ));
        }
        if state.as_ref().receipt_seen {
            return Err(CommandError::ValidationError(
                "development_system.command_receipt_duplicate".to_string(),
            ));
        }
        if self.receipt.expected_state_epoch != state.as_ref().state_epoch {
            return Err(CommandError::ValidationError(format!(
                "development_system.command_receipt_stale_epoch expected={} received={}",
                state.as_ref().state_epoch,
                self.receipt.expected_state_epoch
            )));
        }
        if self.receipt.output_digest.is_empty() {
            return Err(CommandError::ValidationError(
                "development_system.command_receipt_output_digest_missing".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordReceiptStreamToEvent::apply(self))
                .fact(RecordReceiptToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}
#[derive(ModelInput)]
struct CaptureCheckpointRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: CheckpointIntent,
}

#[derive(ModelCommand)]
struct CaptureCheckpoint {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: CheckpointIntent,
}

mapping! { CaptureCheckpointRequestToStream: CaptureCheckpointRequest.stream => CaptureCheckpoint.stream using clone; }
mapping! { CaptureCheckpointRequestToIntent: CaptureCheckpointRequest.intent => CaptureCheckpoint.intent using clone; }
mapping! { CaptureCheckpointStreamToEvent: CaptureCheckpoint.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
mapping! { CaptureCheckpointToFact:
    (CaptureCheckpoint.intent, CaptureCheckpointState.state_epoch) => WorkflowAuthorityEvent.fact
    using checkpoint_captured_fact;
}

impl ModelCommandLogic for CaptureCheckpoint {
    type Event = WorkflowAuthorityEvent;
    type State = CaptureCheckpointState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        CaptureCheckpointState::from_event(state, event)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        validate_checkpoint_capture(state.as_ref(), &self.intent)?;
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(CaptureCheckpointStreamToEvent::apply(self))
                .fact(CaptureCheckpointToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeSignedCommitRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: SignedCommitIntent,
}

#[derive(ModelCommand)]
struct AuthorizeSignedCommit {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: SignedCommitIntent,
}

mapping! { AuthorizeSignedCommitRequestToStream: AuthorizeSignedCommitRequest.stream => AuthorizeSignedCommit.stream using clone; }
mapping! { AuthorizeSignedCommitRequestToIntent: AuthorizeSignedCommitRequest.intent => AuthorizeSignedCommit.intent using clone; }
mapping! { AuthorizeSignedCommitStreamToEvent: AuthorizeSignedCommit.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
#[allow(clippy::too_many_arguments)] // Every folded authorization input is an EventCore mapping origin.
fn signed_commit_authorized_fact(
    intent: &SignedCommitIntent,
    state_epoch: &u64,
    _authorization: &Option<SignedCommitOperation>,
    _receipt: &Option<SignedCommitReceipt>,
    _assignment: &Option<Assignment>,
    _checkpoint: &Option<Checkpoint>,
    _delivering: &bool,
    _clean_review_observed: &bool,
) -> WorkflowFact {
    WorkflowFact::SignedCommitAuthorized {
        operation: SignedCommitOperation {
            operation_id: intent.operation_id.clone(),
            assignment_id: intent.assignment_id.clone(),
            state_epoch: *state_epoch,
            checkpoint_id: intent.checkpoint_id.clone(),
            parent_commit: intent.parent_commit.clone(),
            message: intent.message.clone(),
            message_digest: intent.message_digest.clone(),
            authorized_at: intent.authorized_at,
        },
    }
}
mapping! { AuthorizeSignedCommitToFact:
    (AuthorizeSignedCommit.intent, AuthorizeSignedCommitState.state_epoch, AuthorizeSignedCommitState.authorization, AuthorizeSignedCommitState.receipt, AuthorizeSignedCommitState.assignment, AuthorizeSignedCommitState.checkpoint, AuthorizeSignedCommitState.delivering, AuthorizeSignedCommitState.clean_review_observed) => WorkflowAuthorityEvent.fact
    using signed_commit_authorized_fact;
}

impl ModelCommandLogic for AuthorizeSignedCommit {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeSignedCommitState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeSignedCommitState::from_event(state, event, self)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if let Some(existing) = &state.as_ref().authorization {
            if existing.operation_id == self.intent.operation_id {
                return Ok(ModeledEvents::none("signed commit already authorized"));
            }
            return Err(CommandError::ValidationError(
                "development_system.signed_commit_operation_reused".to_string(),
            ));
        }
        if state.as_ref().receipt.is_some() {
            return Err(CommandError::ValidationError(
                "development_system.signed_commit_already_completed".to_string(),
            ));
        }
        let state = state.as_ref();
        if !state.delivering || !state.clean_review_observed {
            return Err(CommandError::ValidationError(
                "development_system.signed_commit_phase_denied".to_string(),
            ));
        }
        if self.intent.expected_state_epoch != state.state_epoch {
            return Err(CommandError::ValidationError(
                "development_system.signed_commit_stale_epoch".to_string(),
            ));
        }
        let assignment = state.assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        if assignment.role != Role::Delivery
            || assignment.state_epoch != state.state_epoch
            || assignment.configuration_digest != self.intent.configuration_digest
            || assignment.expires_at < self.intent.authorized_at
        {
            return Err(CommandError::ValidationError(
                "development_system.signed_commit_assignment_denied".to_string(),
            ));
        }
        let checkpoint = state.checkpoint.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.checkpoint_required".to_string())
        })?;
        if checkpoint.state_epoch != state.state_epoch {
            return Err(CommandError::ValidationError(
                "development_system.checkpoint_stale".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeSignedCommitStreamToEvent::apply(self))
                .fact(AuthorizeSignedCommitToFact::apply((
                    self, state, state, state, state, state, state, state,
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct RecordSignedCommitRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: SignedCommitReceipt,
}

#[derive(ModelCommand)]
struct RecordSignedCommit {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: SignedCommitReceipt,
}

mapping! { RecordSignedCommitRequestToStream: RecordSignedCommitRequest.stream => RecordSignedCommit.stream using clone; }
mapping! { RecordSignedCommitRequestToReceipt: RecordSignedCommitRequest.receipt => RecordSignedCommit.receipt using clone; }
mapping! { RecordSignedCommitStreamToEvent: RecordSignedCommit.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn signed_commit_created_fact(receipt: &SignedCommitReceipt) -> WorkflowFact {
    WorkflowFact::SignedCommitCreated {
        receipt: receipt.clone(),
    }
}
mapping! { RecordSignedCommitToFact: RecordSignedCommit.receipt => WorkflowAuthorityEvent.fact using signed_commit_created_fact; }

impl ModelCommandLogic for RecordSignedCommit {
    type Event = WorkflowAuthorityEvent;
    type State = SignedCommitState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        SignedCommitState::model_builder()
            .authorization(RecordSignedCommitEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .receipt(RecordSignedCommitEventToReceipt::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let authorization = state.as_ref().authorization.as_ref().ok_or_else(|| {
            CommandError::ValidationError(
                "development_system.signed_commit_not_authorized".to_string(),
            )
        })?;
        if authorization.assignment_id != self.receipt.assignment_id
            || authorization.checkpoint_id != self.receipt.checkpoint_id
            || authorization.parent_commit != self.receipt.parent_commit
            || authorization.message_digest != self.receipt.message_digest
        {
            return Err(CommandError::ValidationError(
                "development_system.signed_commit_receipt_mismatch".to_string(),
            ));
        }
        if let Some(existing) = &state.as_ref().receipt {
            if existing == &self.receipt {
                return Ok(ModeledEvents::none(
                    "identical signed commit receipt already recorded",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.signed_commit_receipt_reused".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordSignedCommitStreamToEvent::apply(self))
                .fact(RecordSignedCommitToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeSignedTagRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: SignedTagIntent,
}
#[derive(ModelCommand)]
struct AuthorizeSignedTag {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: SignedTagIntent,
}
mapping! { AuthorizeSignedTagRequestToStream: AuthorizeSignedTagRequest.stream => AuthorizeSignedTag.stream using clone; }
mapping! { AuthorizeSignedTagRequestToIntent: AuthorizeSignedTagRequest.intent => AuthorizeSignedTag.intent using clone; }
mapping! { AuthorizeSignedTagStreamToEvent: AuthorizeSignedTag.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
#[allow(clippy::too_many_arguments)] // Every folded authorization input is an EventCore mapping origin.
fn signed_tag_authorized_fact(
    intent: &SignedTagIntent,
    state_epoch: &u64,
    _authorization: &Option<SignedTagOperation>,
    _receipt: &Option<SignedTagReceipt>,
    _assignment: &Option<Assignment>,
    _commit_receipt: &Option<SignedCommitReceipt>,
    _delivering: &bool,
    _clean_review_observed: &bool,
) -> WorkflowFact {
    WorkflowFact::SignedTagAuthorized {
        operation: SignedTagOperation {
            operation_id: intent.operation_id.clone(),
            assignment_id: intent.assignment_id.clone(),
            state_epoch: *state_epoch,
            commit_operation_id: intent.commit_operation_id.clone(),
            target_commit: intent.target_commit.clone(),
            tag_name: intent.tag_name.clone(),
            message: intent.message.clone(),
            message_digest: intent.message_digest.clone(),
            authorized_at: intent.authorized_at,
        },
    }
}
mapping! { AuthorizeSignedTagToFact:
    (AuthorizeSignedTag.intent, AuthorizeSignedTagState.state_epoch, AuthorizeSignedTagState.authorization, AuthorizeSignedTagState.receipt, AuthorizeSignedTagState.assignment, AuthorizeSignedTagState.commit_receipt, AuthorizeSignedTagState.delivering, AuthorizeSignedTagState.clean_review_observed) => WorkflowAuthorityEvent.fact
    using signed_tag_authorized_fact;
}
impl ModelCommandLogic for AuthorizeSignedTag {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeSignedTagState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeSignedTagState::from_event(state, event, self)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if let Some(existing) = &state.as_ref().authorization {
            if existing.operation_id == self.intent.operation_id {
                return Ok(ModeledEvents::none("signed tag already authorized"));
            }
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_operation_reused".to_string(),
            ));
        }
        if state.as_ref().receipt.is_some() {
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_already_completed".to_string(),
            ));
        }
        let state = state.as_ref();
        if !state.delivering || !state.clean_review_observed {
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_phase_denied".to_string(),
            ));
        }
        if self.intent.expected_state_epoch != state.state_epoch {
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_stale_epoch".to_string(),
            ));
        }
        let assignment = state.assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        if assignment.role != Role::Delivery
            || assignment.state_epoch != state.state_epoch
            || assignment.configuration_digest != self.intent.configuration_digest
            || assignment.expires_at < self.intent.authorized_at
        {
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_assignment_denied".to_string(),
            ));
        }
        let commit_receipt = state.commit_receipt.as_ref().ok_or_else(|| {
            CommandError::ValidationError(
                "development_system.signed_commit_receipt_unknown".to_string(),
            )
        })?;
        if commit_receipt.assignment_id != self.intent.assignment_id
            || commit_receipt.commit != self.intent.target_commit
        {
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_commit_assignment_mismatch".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeSignedTagStreamToEvent::apply(self))
                .fact(AuthorizeSignedTagToFact::apply((
                    self, state, state, state, state, state, state, state,
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct RecordSignedTagRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: SignedTagReceipt,
}
#[derive(ModelCommand)]
struct RecordSignedTag {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: SignedTagReceipt,
}
mapping! { RecordSignedTagRequestToStream: RecordSignedTagRequest.stream => RecordSignedTag.stream using clone; }
mapping! { RecordSignedTagRequestToReceipt: RecordSignedTagRequest.receipt => RecordSignedTag.receipt using clone; }
mapping! { RecordSignedTagStreamToEvent: RecordSignedTag.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn signed_tag_created_fact(receipt: &SignedTagReceipt) -> WorkflowFact {
    WorkflowFact::SignedTagCreated {
        receipt: receipt.clone(),
    }
}
mapping! { RecordSignedTagToFact: RecordSignedTag.receipt => WorkflowAuthorityEvent.fact using signed_tag_created_fact; }
impl ModelCommandLogic for RecordSignedTag {
    type Event = WorkflowAuthorityEvent;
    type State = SignedTagState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        SignedTagState::model_builder()
            .authorization(RecordSignedTagEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .receipt(RecordSignedTagEventToReceipt::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let authorization = state.as_ref().authorization.as_ref().ok_or_else(|| {
            CommandError::ValidationError(
                "development_system.signed_tag_not_authorized".to_string(),
            )
        })?;
        if authorization.assignment_id != self.receipt.assignment_id
            || authorization.commit_operation_id != self.receipt.commit_operation_id
            || authorization.target_commit != self.receipt.target_commit
            || authorization.tag_name != self.receipt.tag_name
            || authorization.message_digest != self.receipt.message_digest
        {
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_receipt_mismatch".to_string(),
            ));
        }
        if let Some(existing) = &state.as_ref().receipt {
            if existing == &self.receipt {
                return Ok(ModeledEvents::none(
                    "identical signed tag receipt already recorded",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.signed_tag_receipt_reused".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordSignedTagStreamToEvent::apply(self))
                .fact(RecordSignedTagToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeRemoteRefFetchRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: FetchRefIntent,
}
#[derive(ModelCommand)]
struct AuthorizeRemoteRefFetch {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: FetchRefIntent,
}
mapping! { AuthorizeRemoteRefFetchRequestToStream: AuthorizeRemoteRefFetchRequest.stream => AuthorizeRemoteRefFetch.stream using clone; }
mapping! { AuthorizeRemoteRefFetchRequestToIntent: AuthorizeRemoteRefFetchRequest.intent => AuthorizeRemoteRefFetch.intent using clone; }
mapping! { AuthorizeRemoteRefFetchStreamToEvent: AuthorizeRemoteRefFetch.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn remote_ref_fetch_authorized_fact(
    intent: &FetchRefIntent,
    state_epoch: &u64,
    _authorization: &Option<FetchRefOperation>,
    _receipt: &Option<FetchRefReceipt>,
    _delivery_assignment: &Option<RemoteDeliveryAssignment>,
    _delivering: &bool,
    _clean_review_observed: &bool,
) -> WorkflowFact {
    WorkflowFact::RemoteRefFetchAuthorized {
        operation: FetchRefOperation {
            operation_id: intent.operation_id.clone(),
            assignment_id: intent.assignment_id.clone(),
            state_epoch: *state_epoch,
            remote: intent.remote.clone(),
            remote_ref: intent.remote_ref.clone(),
            authorized_at: intent.authorized_at,
        },
    }
}
mapping! { AuthorizeRemoteRefFetchToFact:
(AuthorizeRemoteRefFetch.intent, AuthorizeFetchRefState.state_epoch, AuthorizeFetchRefState.authorization, AuthorizeFetchRefState.receipt, AuthorizeFetchRefState.delivery_assignment, AuthorizeFetchRefState.delivering, AuthorizeFetchRefState.clean_review_observed) => WorkflowAuthorityEvent.fact using remote_ref_fetch_authorized_fact; }
impl ModelCommandLogic for AuthorizeRemoteRefFetch {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeFetchRefState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeFetchRefState::from_event(state, event, self)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if let Some(existing) = &state.as_ref().authorization {
            if existing.operation_id == self.intent.operation_id {
                return Ok(ModeledEvents::none("remote ref fetch already authorized"));
            }
            return Err(CommandError::ValidationError(
                "development_system.fetch_ref_operation_reused".to_string(),
            ));
        }
        if state.as_ref().receipt.is_some() {
            return Err(CommandError::ValidationError(
                "development_system.fetch_ref_already_completed".to_string(),
            ));
        }
        let state = state.as_ref();
        let assignment = state.delivery_assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        if !state.delivering
            || !state.clean_review_observed
            || self.intent.expected_state_epoch != state.state_epoch
            || assignment.state_epoch != state.state_epoch
            || assignment.configuration_digest != self.intent.configuration_digest
            || assignment.expires_at < self.intent.authorized_at
        {
            return Err(CommandError::ValidationError(
                "development_system.fetch_ref_assignment_denied".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeRemoteRefFetchStreamToEvent::apply(self))
                .fact(AuthorizeRemoteRefFetchToFact::apply((
                    self, state, state, state, state, state, state,
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct RecordRemoteRefFetchedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: FetchRefReceipt,
}
#[derive(ModelCommand)]
struct RecordRemoteRefFetched {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: FetchRefReceipt,
}
mapping! { RecordRemoteRefFetchedRequestToStream: RecordRemoteRefFetchedRequest.stream => RecordRemoteRefFetched.stream using clone; }
mapping! { RecordRemoteRefFetchedRequestToReceipt: RecordRemoteRefFetchedRequest.receipt => RecordRemoteRefFetched.receipt using clone; }
mapping! { RecordRemoteRefFetchedStreamToEvent: RecordRemoteRefFetched.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn remote_ref_fetched_fact(receipt: &FetchRefReceipt) -> WorkflowFact {
    WorkflowFact::RemoteRefFetched {
        receipt: receipt.clone(),
    }
}
mapping! { RecordRemoteRefFetchedToFact: RecordRemoteRefFetched.receipt => WorkflowAuthorityEvent.fact using remote_ref_fetched_fact; }
impl ModelCommandLogic for RecordRemoteRefFetched {
    type Event = WorkflowAuthorityEvent;
    type State = FetchRefState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        FetchRefState::model_builder()
            .authorization(RecordRemoteRefFetchedEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .receipt(RecordRemoteRefFetchedEventToReceipt::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let authorization = state.as_ref().authorization.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.fetch_ref_not_authorized".to_string())
        })?;
        if authorization.assignment_id != self.receipt.assignment_id
            || authorization.remote != self.receipt.remote
            || authorization.remote_ref != self.receipt.remote_ref
        {
            return Err(CommandError::ValidationError(
                "development_system.fetch_ref_receipt_mismatch".to_string(),
            ));
        }
        if let Some(existing) = &state.as_ref().receipt {
            if existing == &self.receipt {
                return Ok(ModeledEvents::none(
                    "identical fetch receipt already recorded",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.fetch_ref_receipt_reused".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordRemoteRefFetchedStreamToEvent::apply(self))
                .fact(RecordRemoteRefFetchedToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeRemoteRefPushRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: PushRefIntent,
}
#[derive(ModelCommand)]
struct AuthorizeRemoteRefPush {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: PushRefIntent,
}
mapping! { AuthorizeRemoteRefPushRequestToStream: AuthorizeRemoteRefPushRequest.stream => AuthorizeRemoteRefPush.stream using clone; }
mapping! { AuthorizeRemoteRefPushRequestToIntent: AuthorizeRemoteRefPushRequest.intent => AuthorizeRemoteRefPush.intent using clone; }
mapping! { AuthorizeRemoteRefPushStreamToEvent: AuthorizeRemoteRefPush.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn remote_ref_push_authorized_fact(
    intent: &PushRefIntent,
    state_epoch: &u64,
    signed_source_object: &Option<String>,
    _authorization: &Option<PushRefOperation>,
    _receipt: &Option<PushRefReceipt>,
    _delivery_gate: &RemoteDeliveryGate,
    _signed_source_assignment_id: &Option<String>,
) -> WorkflowFact {
    WorkflowFact::RemoteRefPushAuthorized {
        operation: PushRefOperation {
            operation_id: intent.operation_id.clone(),
            assignment_id: intent.assignment_id.clone(),
            state_epoch: *state_epoch,
            remote: intent.remote.clone(),
            remote_ref: intent.remote_ref.clone(),
            source_kind: intent.source_kind,
            source_operation_id: intent.source_operation_id.clone(),
            source_object: signed_source_object
                .clone()
                .expect("push source is validated before fact construction"),
            expected_remote_object: intent.expected_remote_object.clone(),
            authorized_at: intent.authorized_at,
        },
    }
}
mapping! { AuthorizeRemoteRefPushToFact:
(AuthorizeRemoteRefPush.intent, AuthorizePushRefState.state_epoch, AuthorizePushRefState.signed_source_object, AuthorizePushRefState.authorization, AuthorizePushRefState.receipt, AuthorizePushRefState.delivery_gate, AuthorizePushRefState.signed_source_assignment_id) => WorkflowAuthorityEvent.fact using remote_ref_push_authorized_fact; }
impl ModelCommandLogic for AuthorizeRemoteRefPush {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizePushRefState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizePushRefState::from_event(state, event, self)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if let Some(existing) = &state.as_ref().authorization {
            if existing.operation_id == self.intent.operation_id {
                return Ok(ModeledEvents::none("remote ref push already authorized"));
            }
            return Err(CommandError::ValidationError(
                "development_system.push_ref_operation_reused".to_string(),
            ));
        }
        if state.as_ref().receipt.is_some() {
            return Err(CommandError::ValidationError(
                "development_system.push_ref_already_completed".to_string(),
            ));
        }
        let state = state.as_ref();
        let assignment = state.delivery_gate.assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        if !state.delivery_gate.delivering
            || !state.delivery_gate.clean_review_observed
            || self.intent.expected_state_epoch != state.state_epoch
            || assignment.state_epoch != state.state_epoch
            || assignment.configuration_digest != self.intent.configuration_digest
            || assignment.expires_at < self.intent.authorized_at
        {
            return Err(CommandError::ValidationError(
                "development_system.push_ref_assignment_denied".to_string(),
            ));
        }
        if state.signed_source_object.is_none()
            || state.signed_source_assignment_id.as_deref() != Some(&self.intent.assignment_id)
        {
            return Err(CommandError::ValidationError(
                "development_system.push_source_unknown".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeRemoteRefPushStreamToEvent::apply(self))
                .fact(AuthorizeRemoteRefPushToFact::apply((
                    self, state, state, state, state, state, state,
                )))
                .build(),
        ))
    }
}
#[derive(ModelInput)]
struct RecordRemoteRefPushedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: PushRefReceipt,
}
#[derive(ModelCommand)]
struct RecordRemoteRefPushed {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: PushRefReceipt,
}
mapping! { RecordRemoteRefPushedRequestToStream: RecordRemoteRefPushedRequest.stream => RecordRemoteRefPushed.stream using clone; }
mapping! { RecordRemoteRefPushedRequestToReceipt: RecordRemoteRefPushedRequest.receipt => RecordRemoteRefPushed.receipt using clone; }
mapping! { RecordRemoteRefPushedStreamToEvent: RecordRemoteRefPushed.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn remote_ref_pushed_fact(receipt: &PushRefReceipt) -> WorkflowFact {
    WorkflowFact::RemoteRefPushed {
        receipt: receipt.clone(),
    }
}
mapping! { RecordRemoteRefPushedToFact: RecordRemoteRefPushed.receipt => WorkflowAuthorityEvent.fact using remote_ref_pushed_fact; }
impl ModelCommandLogic for RecordRemoteRefPushed {
    type Event = WorkflowAuthorityEvent;
    type State = PushRefState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        PushRefState::model_builder()
            .authorization(RecordRemoteRefPushedEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .receipt(RecordRemoteRefPushedEventToReceipt::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let authorization = state.as_ref().authorization.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.push_ref_not_authorized".to_string())
        })?;
        if authorization.assignment_id != self.receipt.assignment_id
            || authorization.remote != self.receipt.remote
            || authorization.remote_ref != self.receipt.remote_ref
            || authorization.source_object != self.receipt.source_object
            || authorization.expected_remote_object != self.receipt.previous_remote_object
        {
            return Err(CommandError::ValidationError(
                "development_system.push_ref_receipt_mismatch".to_string(),
            ));
        }
        if let Some(existing) = &state.as_ref().receipt {
            if existing == &self.receipt {
                return Ok(ModeledEvents::none(
                    "identical push receipt already recorded",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.push_ref_receipt_reused".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordRemoteRefPushedStreamToEvent::apply(self))
                .fact(RecordRemoteRefPushedToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct OpenPullRequestRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: OpenPullRequestIntent,
}
#[derive(ModelCommand)]
struct OpenPullRequest {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: OpenPullRequestIntent,
}
mapping! { OpenPullRequestRequestToStream: OpenPullRequestRequest.stream => OpenPullRequest.stream using clone; }
mapping! { OpenPullRequestRequestToIntent: OpenPullRequestRequest.intent => OpenPullRequest.intent using clone; }
mapping! { OpenPullRequestStreamToEvent: OpenPullRequest.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn pull_request_open_authorized_fact(
    intent: &OpenPullRequestIntent,
    epoch: &u64,
    head_ref: &Option<String>,
    _authorization: &Option<OpenPullRequestOperation>,
    _receipt: &Option<OpenPullRequestReceipt>,
    _delivery_gate: &RemoteDeliveryGate,
    _push_assignment_id: &Option<String>,
) -> WorkflowFact {
    WorkflowFact::PullRequestOpenAuthorized {
        operation: OpenPullRequestOperation {
            operation_id: intent.operation_id.clone(),
            assignment_id: intent.assignment_id.clone(),
            state_epoch: *epoch,
            provider: intent.provider.clone(),
            repository: intent.repository.clone(),
            push_operation_id: intent.push_operation_id.clone(),
            head_ref: head_ref.clone().expect("validated push head"),
            base_branch: intent.base_branch.clone(),
            title: intent.title.clone(),
            body: intent.body.clone(),
            authorized_at: intent.authorized_at,
        },
    }
}
mapping! { OpenPullRequestToFact: (OpenPullRequest.intent, AuthorizeOpenPullRequestState.state_epoch, AuthorizeOpenPullRequestState.push_head_ref, AuthorizeOpenPullRequestState.authorization, AuthorizeOpenPullRequestState.receipt, AuthorizeOpenPullRequestState.delivery_gate, AuthorizeOpenPullRequestState.push_assignment_id) => WorkflowAuthorityEvent.fact using pull_request_open_authorized_fact; }
impl ModelCommandLogic for OpenPullRequest {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeOpenPullRequestState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeOpenPullRequestState::from_event(state, event, self)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if let Some(existing) = &state.as_ref().authorization {
            if existing.operation_id == self.intent.operation_id {
                return Ok(ModeledEvents::none(
                    "pull request opening already authorized",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.open_pr_operation_reused".to_string(),
            ));
        }
        if state.as_ref().receipt.is_some() {
            return Err(CommandError::ValidationError(
                "development_system.pull_request_already_opened".to_string(),
            ));
        }
        let state = state.as_ref();
        let assignment = state.delivery_gate.assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        if !state.delivery_gate.delivering
            || !state.delivery_gate.clean_review_observed
            || self.intent.expected_state_epoch != state.state_epoch
            || assignment.state_epoch != state.state_epoch
            || assignment.configuration_digest != self.intent.configuration_digest
            || assignment.expires_at < self.intent.authorized_at
            || state.push_assignment_id.as_deref() != Some(&self.intent.assignment_id)
            || state.push_head_ref.is_none()
        {
            return Err(CommandError::ValidationError(
                "development_system.open_pr_assignment_denied".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(OpenPullRequestStreamToEvent::apply(self))
                .fact(OpenPullRequestToFact::apply((
                    self, state, state, state, state, state, state,
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct RecordPullRequestOpenedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: OpenPullRequestReceipt,
}
#[derive(ModelCommand)]
struct RecordPullRequestOpened {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: OpenPullRequestReceipt,
}
mapping! { RecordPullRequestOpenedRequestToStream: RecordPullRequestOpenedRequest.stream => RecordPullRequestOpened.stream using clone; }
mapping! { RecordPullRequestOpenedRequestToReceipt: RecordPullRequestOpenedRequest.receipt => RecordPullRequestOpened.receipt using clone; }
mapping! { RecordPullRequestOpenedStreamToEvent: RecordPullRequestOpened.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn pull_request_opened_fact(receipt: &OpenPullRequestReceipt) -> WorkflowFact {
    WorkflowFact::PullRequestOpened {
        receipt: receipt.clone(),
    }
}
mapping! { RecordPullRequestOpenedToFact: RecordPullRequestOpened.receipt => WorkflowAuthorityEvent.fact using pull_request_opened_fact; }
impl ModelCommandLogic for RecordPullRequestOpened {
    type Event = WorkflowAuthorityEvent;
    type State = OpenPullRequestState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        OpenPullRequestState::model_builder()
            .authorization(RecordPullRequestOpenedEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .receipt(RecordPullRequestOpenedEventToReceipt::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let operation = state.as_ref().authorization.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.open_pr_not_authorized".to_string())
        })?;
        if operation.assignment_id != self.receipt.assignment_id
            || operation.provider != self.receipt.provider
            || operation.repository != self.receipt.repository
            || operation.push_operation_id != self.receipt.push_operation_id
        {
            return Err(CommandError::ValidationError(
                "development_system.open_pr_receipt_mismatch".to_string(),
            ));
        }
        if let Some(existing) = &state.as_ref().receipt {
            if existing == &self.receipt {
                return Ok(ModeledEvents::none(
                    "identical pull request receipt already recorded",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.open_pr_receipt_reused".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordPullRequestOpenedStreamToEvent::apply(self))
                .fact(RecordPullRequestOpenedToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct UpdatePullRequestRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: UpdatePullRequestIntent,
}
#[derive(ModelCommand)]
struct UpdatePullRequest {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: UpdatePullRequestIntent,
}
mapping! { UpdatePullRequestRequestToStream: UpdatePullRequestRequest.stream => UpdatePullRequest.stream using clone; }
mapping! { UpdatePullRequestRequestToIntent: UpdatePullRequestRequest.intent => UpdatePullRequest.intent using clone; }
mapping! { UpdatePullRequestStreamToEvent: UpdatePullRequest.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn pull_request_update_authorized_fact(
    intent: &UpdatePullRequestIntent,
    epoch: &u64,
    opened: &Option<OpenPullRequestReceipt>,
    _authorization: &Option<UpdatePullRequestOperation>,
    _receipt: &Option<UpdatePullRequestReceipt>,
    _delivery_gate: &RemoteDeliveryGate,
) -> WorkflowFact {
    WorkflowFact::PullRequestUpdateAuthorized {
        operation: UpdatePullRequestOperation {
            operation_id: intent.operation_id.clone(),
            assignment_id: intent.assignment_id.clone(),
            state_epoch: *epoch,
            open_operation_id: intent.open_operation_id.clone(),
            provider: opened
                .as_ref()
                .expect("validated open receipt")
                .provider
                .clone(),
            repository: opened
                .as_ref()
                .expect("validated open receipt")
                .repository
                .clone(),
            pull_request_url: opened
                .as_ref()
                .expect("validated open receipt")
                .pull_request_url
                .clone(),
            title: intent.title.clone(),
            body: intent.body.clone(),
            authorized_at: intent.authorized_at,
        },
    }
}
mapping! { UpdatePullRequestToFact: (UpdatePullRequest.intent, AuthorizeUpdatePullRequestState.state_epoch, AuthorizeUpdatePullRequestState.opened, AuthorizeUpdatePullRequestState.authorization, AuthorizeUpdatePullRequestState.receipt, AuthorizeUpdatePullRequestState.delivery_gate) => WorkflowAuthorityEvent.fact using pull_request_update_authorized_fact; }
impl ModelCommandLogic for UpdatePullRequest {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeUpdatePullRequestState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeUpdatePullRequestState::from_event(state, event, self)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if let Some(existing) = &state.as_ref().authorization {
            if existing.operation_id == self.intent.operation_id {
                return Ok(ModeledEvents::none(
                    "pull request update already authorized",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.update_pr_operation_reused".to_string(),
            ));
        }
        if state.as_ref().receipt.is_some() {
            return Err(CommandError::ValidationError(
                "development_system.pull_request_already_updated".to_string(),
            ));
        }
        let state = state.as_ref();
        let assignment = state.delivery_gate.assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        if !state.delivery_gate.delivering
            || !state.delivery_gate.clean_review_observed
            || self.intent.expected_state_epoch != state.state_epoch
            || assignment.state_epoch != state.state_epoch
            || assignment.configuration_digest != self.intent.configuration_digest
            || assignment.expires_at < self.intent.authorized_at
            || state
                .opened
                .as_ref()
                .is_none_or(|opened| opened.assignment_id != self.intent.assignment_id)
        {
            return Err(CommandError::ValidationError(
                "development_system.update_pr_assignment_denied".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(UpdatePullRequestStreamToEvent::apply(self))
                .fact(UpdatePullRequestToFact::apply((
                    self, state, state, state, state, state,
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct RecordPullRequestUpdatedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: UpdatePullRequestReceipt,
}
#[derive(ModelCommand)]
struct RecordPullRequestUpdated {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: UpdatePullRequestReceipt,
}
mapping! { RecordPullRequestUpdatedRequestToStream: RecordPullRequestUpdatedRequest.stream => RecordPullRequestUpdated.stream using clone; }
mapping! { RecordPullRequestUpdatedRequestToReceipt: RecordPullRequestUpdatedRequest.receipt => RecordPullRequestUpdated.receipt using clone; }
mapping! { RecordPullRequestUpdatedStreamToEvent: RecordPullRequestUpdated.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn pull_request_updated_fact(receipt: &UpdatePullRequestReceipt) -> WorkflowFact {
    WorkflowFact::PullRequestUpdated {
        receipt: receipt.clone(),
    }
}
mapping! { RecordPullRequestUpdatedToFact: RecordPullRequestUpdated.receipt => WorkflowAuthorityEvent.fact using pull_request_updated_fact; }
impl ModelCommandLogic for RecordPullRequestUpdated {
    type Event = WorkflowAuthorityEvent;
    type State = UpdatePullRequestState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        UpdatePullRequestState::model_builder()
            .authorization(RecordPullRequestUpdatedEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .receipt(RecordPullRequestUpdatedEventToReceipt::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let operation = state.as_ref().authorization.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.update_pr_not_authorized".to_string())
        })?;
        if operation.assignment_id != self.receipt.assignment_id
            || operation.open_operation_id != self.receipt.open_operation_id
            || operation.pull_request_url != self.receipt.pull_request_url
        {
            return Err(CommandError::ValidationError(
                "development_system.update_pr_receipt_mismatch".to_string(),
            ));
        }
        if let Some(existing) = &state.as_ref().receipt {
            if existing == &self.receipt {
                return Ok(ModeledEvents::none(
                    "identical pull request update receipt already recorded",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.update_pr_receipt_reused".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordPullRequestUpdatedStreamToEvent::apply(self))
                .fact(RecordPullRequestUpdatedToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct MergePullRequestRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    intent: MergePullRequestIntent,
}
#[derive(ModelCommand)]
struct MergePullRequest {
    #[stream]
    stream: WorkflowAuthorityStream,
    intent: MergePullRequestIntent,
}
mapping! { MergePullRequestRequestToStream: MergePullRequestRequest.stream => MergePullRequest.stream using clone; }
mapping! { MergePullRequestRequestToIntent: MergePullRequestRequest.intent => MergePullRequest.intent using clone; }
mapping! { MergePullRequestStreamToEvent: MergePullRequest.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn pull_request_merge_authorized_fact(
    intent: &MergePullRequestIntent,
    epoch: &u64,
    opened: &Option<OpenPullRequestReceipt>,
    updated: &Option<UpdatePullRequestReceipt>,
    _authorization: &Option<MergePullRequestOperation>,
    _receipt: &Option<MergePullRequestReceipt>,
    _delivery_gate: &RemoteDeliveryGate,
) -> WorkflowFact {
    let opened = opened.as_ref().expect("validated open receipt");
    WorkflowFact::PullRequestMergeAuthorized {
        operation: MergePullRequestOperation {
            operation_id: intent.operation_id.clone(),
            assignment_id: intent.assignment_id.clone(),
            state_epoch: *epoch,
            open_operation_id: intent.open_operation_id.clone(),
            provider: opened.provider.clone(),
            repository: opened.repository.clone(),
            pull_request_url: updated.as_ref().map_or_else(
                || opened.pull_request_url.clone(),
                |updated| updated.pull_request_url.clone(),
            ),
            method: intent.method,
            authorized_at: intent.authorized_at,
        },
    }
}
mapping! { MergePullRequestToFact: (MergePullRequest.intent, AuthorizeMergePullRequestState.state_epoch, AuthorizeMergePullRequestState.opened, AuthorizeMergePullRequestState.updated, AuthorizeMergePullRequestState.authorization, AuthorizeMergePullRequestState.receipt, AuthorizeMergePullRequestState.delivery_gate) => WorkflowAuthorityEvent.fact using pull_request_merge_authorized_fact; }
impl ModelCommandLogic for MergePullRequest {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeMergePullRequestState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeMergePullRequestState::from_event(state, event, self)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if let Some(existing) = &state.as_ref().authorization {
            if existing.operation_id == self.intent.operation_id {
                return Ok(ModeledEvents::none("pull request merge already authorized"));
            }
            return Err(CommandError::ValidationError(
                "development_system.merge_pr_operation_reused".to_string(),
            ));
        }
        if state.as_ref().receipt.is_some() {
            return Err(CommandError::ValidationError(
                "development_system.pull_request_already_merged".to_string(),
            ));
        }
        let state = state.as_ref();
        let assignment = state.delivery_gate.assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        let opened = state.opened.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.open_pr_receipt_unknown".to_string())
        })?;
        let update_is_consistent = state.updated.as_ref().is_none_or(|updated| {
            updated.assignment_id == self.intent.assignment_id
                && updated.pull_request_url == opened.pull_request_url
        });
        if !state.delivery_gate.delivering
            || !state.delivery_gate.clean_review_observed
            || self.intent.expected_state_epoch != state.state_epoch
            || assignment.state_epoch != state.state_epoch
            || assignment.configuration_digest != self.intent.configuration_digest
            || assignment.expires_at < self.intent.authorized_at
            || opened.assignment_id != self.intent.assignment_id
            || !update_is_consistent
        {
            return Err(CommandError::ValidationError(
                "development_system.merge_pr_assignment_denied".to_string(),
            ));
        }
        if opened.provider == ForgeProvider::GitLab && self.intent.method == MergeMethod::Rebase {
            return Err(CommandError::ValidationError(
                "development_system.forge_merge_method_unsupported".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(MergePullRequestStreamToEvent::apply(self))
                .fact(MergePullRequestToFact::apply((
                    self, state, state, state, state, state, state,
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct RecordPullRequestMergedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt: MergePullRequestReceipt,
}
#[derive(ModelCommand)]
struct RecordPullRequestMerged {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt: MergePullRequestReceipt,
}
mapping! { RecordPullRequestMergedRequestToStream: RecordPullRequestMergedRequest.stream => RecordPullRequestMerged.stream using clone; }
mapping! { RecordPullRequestMergedRequestToReceipt: RecordPullRequestMergedRequest.receipt => RecordPullRequestMerged.receipt using clone; }
mapping! { RecordPullRequestMergedStreamToEvent: RecordPullRequestMerged.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn pull_request_merged_fact(receipt: &MergePullRequestReceipt) -> WorkflowFact {
    WorkflowFact::PullRequestMerged {
        receipt: receipt.clone(),
    }
}
mapping! { RecordPullRequestMergedToFact: RecordPullRequestMerged.receipt => WorkflowAuthorityEvent.fact using pull_request_merged_fact; }
impl ModelCommandLogic for RecordPullRequestMerged {
    type Event = WorkflowAuthorityEvent;
    type State = MergePullRequestState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        MergePullRequestState::model_builder()
            .authorization(RecordPullRequestMergedEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .receipt(RecordPullRequestMergedEventToReceipt::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let operation = state.as_ref().authorization.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.merge_pr_not_authorized".to_string())
        })?;
        if operation.assignment_id != self.receipt.assignment_id
            || operation.open_operation_id != self.receipt.open_operation_id
            || operation.pull_request_url != self.receipt.pull_request_url
            || operation.method != self.receipt.method
        {
            return Err(CommandError::ValidationError(
                "development_system.merge_pr_receipt_mismatch".to_string(),
            ));
        }
        if let Some(existing) = &state.as_ref().receipt {
            if existing == &self.receipt {
                return Ok(ModeledEvents::none(
                    "identical pull request merge receipt already recorded",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.merge_pr_receipt_reused".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(RecordPullRequestMergedStreamToEvent::apply(self))
                .fact(RecordPullRequestMergedToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeFileWriteRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    operation: WorkspaceFileWrite,
}

#[derive(ModelCommand)]
struct AuthorizeFileWrite {
    #[stream]
    stream: WorkflowAuthorityStream,
    operation: WorkspaceFileWrite,
}
mapping! { AuthorizeFileWriteRequestToStream: AuthorizeFileWriteRequest.stream => AuthorizeFileWrite.stream using clone; }
mapping! { AuthorizeFileWriteRequestToOperation: AuthorizeFileWriteRequest.operation => AuthorizeFileWrite.operation using clone; }
mapping! { AuthorizeFileWriteStreamToEvent: AuthorizeFileWrite.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn file_write_authorized_fact(operation: &WorkspaceFileWrite) -> WorkflowFact {
    WorkflowFact::FileWriteAuthorized {
        operation: operation.clone(),
    }
}
mapping! { AuthorizeFileWriteToFact: AuthorizeFileWrite.operation => WorkflowAuthorityEvent.fact using file_write_authorized_fact; }

impl ModelCommandLogic for AuthorizeFileWrite {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeFileWriteState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeFileWriteState::model_builder()
            .state_epoch(AuthorizeFileWriteEventToEpoch::apply((
                event,
                state.as_ref(),
            )))
            .assignment(AuthorizeFileWriteEventToAssignment::apply((
                event,
                self,
                state.as_ref(),
            )))
            .authorization(AuthorizeFileWriteEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .completed(AuthorizeFileWriteEventToCompleted::apply((
                event,
                self,
                state.as_ref(),
            )))
            .initial_dirty_paths(AuthorizeFileWriteEventToInitialDirtyPaths::apply((
                event,
                state.as_ref(),
            )))
            .build()
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let assignment = state.as_ref().assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        let initial_dirty_paths = state.as_ref().initial_dirty_paths.as_ref().ok_or_else(|| {
            CommandError::ValidationError(
                "development_system.initial_repository_baseline_required=true".to_string(),
            )
        })?;
        if initial_dirty_paths.contains(&self.operation.path) {
            return Err(CommandError::ValidationError(
                "development_system.file_write_overlaps_initial_user_change=true".to_string(),
            ));
        }
        if self.operation.state_epoch != state.as_ref().state_epoch
            || assignment.state_epoch != self.operation.state_epoch
            || !assignment.scope_ids.contains(&self.operation.scope_id)
        {
            return Err(CommandError::ValidationError(
                "development_system.file_write_authorization_stale=true".to_string(),
            ));
        }
        if let Some(existing) = state.as_ref().authorization.as_ref() {
            if existing == &self.operation {
                return Ok(ModeledEvents::none(
                    "identical file write is already authorized",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.file_write_operation_reused=true".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeFileWriteStreamToEvent::apply(self))
                .fact(AuthorizeFileWriteToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct ConfirmFileWrittenRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    operation: WorkspaceFileWrite,
}

#[derive(ModelCommand)]
struct ConfirmFileWritten {
    #[stream]
    stream: WorkflowAuthorityStream,
    operation: WorkspaceFileWrite,
}
mapping! { ConfirmFileWrittenRequestToStream: ConfirmFileWrittenRequest.stream => ConfirmFileWritten.stream using clone; }
mapping! { ConfirmFileWrittenRequestToOperation: ConfirmFileWrittenRequest.operation => ConfirmFileWritten.operation using clone; }
mapping! { ConfirmFileWrittenStreamToEvent: ConfirmFileWritten.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn file_written_fact(operation: &WorkspaceFileWrite) -> WorkflowFact {
    WorkflowFact::FileWritten {
        operation: operation.clone(),
    }
}
mapping! { ConfirmFileWrittenToFact: ConfirmFileWritten.operation => WorkflowAuthorityEvent.fact using file_written_fact; }

impl ModelCommandLogic for ConfirmFileWritten {
    type Event = WorkflowAuthorityEvent;
    type State = ConfirmFileWrittenState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        ConfirmFileWrittenState::model_builder()
            .authorization(ConfirmFileWrittenEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .completed(ConfirmFileWrittenEventToCompleted::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().completed {
            return Ok(ModeledEvents::none("file write is already confirmed"));
        }
        if state.as_ref().authorization.as_ref() != Some(&self.operation) {
            return Err(CommandError::ValidationError(
                "development_system.file_write_not_authorized=true".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(ConfirmFileWrittenStreamToEvent::apply(self))
                .fact(ConfirmFileWrittenToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeFileDeleteRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    operation: WorkspaceFileDeletion,
}
#[derive(ModelCommand)]
struct AuthorizeFileDelete {
    #[stream]
    stream: WorkflowAuthorityStream,
    operation: WorkspaceFileDeletion,
}
mapping! { AuthorizeFileDeleteRequestToStream: AuthorizeFileDeleteRequest.stream => AuthorizeFileDelete.stream using clone; }
mapping! { AuthorizeFileDeleteRequestToOperation: AuthorizeFileDeleteRequest.operation => AuthorizeFileDelete.operation using clone; }
mapping! { AuthorizeFileDeleteStreamToEvent: AuthorizeFileDelete.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn file_delete_authorized_fact(operation: &WorkspaceFileDeletion) -> WorkflowFact {
    WorkflowFact::FileDeleteAuthorized {
        operation: operation.clone(),
    }
}
mapping! { AuthorizeFileDeleteToFact: AuthorizeFileDelete.operation => WorkflowAuthorityEvent.fact using file_delete_authorized_fact; }

impl ModelCommandLogic for AuthorizeFileDelete {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeFileDeleteState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeFileDeleteState::model_builder()
            .state_epoch(AuthorizeFileDeleteEventToEpoch::apply((
                event,
                state.as_ref(),
            )))
            .assignment(AuthorizeFileDeleteEventToAssignment::apply((
                event,
                self,
                state.as_ref(),
            )))
            .authorization(AuthorizeFileDeleteEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .initial_dirty_paths(AuthorizeFileDeleteEventToInitialDirtyPaths::apply((
                event,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let assignment = state.as_ref().assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        let initial_dirty_paths = state.as_ref().initial_dirty_paths.as_ref().ok_or_else(|| {
            CommandError::ValidationError(
                "development_system.initial_repository_baseline_required=true".to_string(),
            )
        })?;
        if initial_dirty_paths.contains(&self.operation.path) {
            return Err(CommandError::ValidationError(
                "development_system.file_delete_overlaps_initial_user_change=true".to_string(),
            ));
        }
        if self.operation.state_epoch != state.as_ref().state_epoch
            || assignment.state_epoch != self.operation.state_epoch
            || !assignment.scope_ids.contains(&self.operation.scope_id)
        {
            return Err(CommandError::ValidationError(
                "development_system.file_delete_authorization_stale=true".to_string(),
            ));
        }
        if let Some(existing) = state.as_ref().authorization.as_ref() {
            if existing == &self.operation {
                return Ok(ModeledEvents::none(
                    "identical file deletion is already authorized",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.file_delete_operation_reused=true".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeFileDeleteStreamToEvent::apply(self))
                .fact(AuthorizeFileDeleteToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct ConfirmFileDeletedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    operation: WorkspaceFileDeletion,
}
#[derive(ModelCommand)]
struct ConfirmFileDeleted {
    #[stream]
    stream: WorkflowAuthorityStream,
    operation: WorkspaceFileDeletion,
}
mapping! { ConfirmFileDeletedRequestToStream: ConfirmFileDeletedRequest.stream => ConfirmFileDeleted.stream using clone; }
mapping! { ConfirmFileDeletedRequestToOperation: ConfirmFileDeletedRequest.operation => ConfirmFileDeleted.operation using clone; }
mapping! { ConfirmFileDeletedStreamToEvent: ConfirmFileDeleted.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn file_deleted_fact(operation: &WorkspaceFileDeletion) -> WorkflowFact {
    WorkflowFact::FileDeleted {
        operation: operation.clone(),
    }
}
mapping! { ConfirmFileDeletedToFact: ConfirmFileDeleted.operation => WorkflowAuthorityEvent.fact using file_deleted_fact; }

impl ModelCommandLogic for ConfirmFileDeleted {
    type Event = WorkflowAuthorityEvent;
    type State = ConfirmFileDeletedState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        ConfirmFileDeletedState::model_builder()
            .authorization(ConfirmFileDeletedEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .completed(ConfirmFileDeletedEventToCompleted::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().completed {
            return Ok(ModeledEvents::none("file deletion is already confirmed"));
        }
        if state.as_ref().authorization.as_ref() != Some(&self.operation) {
            return Err(CommandError::ValidationError(
                "development_system.file_delete_not_authorized=true".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(ConfirmFileDeletedStreamToEvent::apply(self))
                .fact(ConfirmFileDeletedToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeFileMoveRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    operation: WorkspaceFileMove,
}
#[derive(ModelCommand)]
struct AuthorizeFileMove {
    #[stream]
    stream: WorkflowAuthorityStream,
    operation: WorkspaceFileMove,
}
mapping! { AuthorizeFileMoveRequestToStream: AuthorizeFileMoveRequest.stream => AuthorizeFileMove.stream using clone; }
mapping! { AuthorizeFileMoveRequestToOperation: AuthorizeFileMoveRequest.operation => AuthorizeFileMove.operation using clone; }
mapping! { AuthorizeFileMoveStreamToEvent: AuthorizeFileMove.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn file_move_authorized_fact(operation: &WorkspaceFileMove) -> WorkflowFact {
    WorkflowFact::FileMoveAuthorized {
        operation: operation.clone(),
    }
}
mapping! { AuthorizeFileMoveToFact: AuthorizeFileMove.operation => WorkflowAuthorityEvent.fact using file_move_authorized_fact; }
impl ModelCommandLogic for AuthorizeFileMove {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeFileMoveState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        AuthorizeFileMoveState::model_builder()
            .state_epoch(AuthorizeFileMoveEventToEpoch::apply((
                event,
                state.as_ref(),
            )))
            .assignment(AuthorizeFileMoveEventToAssignment::apply((
                event,
                self,
                state.as_ref(),
            )))
            .authorization(AuthorizeFileMoveEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .initial_dirty_paths(AuthorizeFileMoveEventToInitialDirtyPaths::apply((
                event,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let assignment = state.as_ref().assignment.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.assignment_unknown".to_string())
        })?;
        let initial_dirty_paths = state.as_ref().initial_dirty_paths.as_ref().ok_or_else(|| {
            CommandError::ValidationError(
                "development_system.initial_repository_baseline_required=true".to_string(),
            )
        })?;
        if initial_dirty_paths.contains(&self.operation.from)
            || initial_dirty_paths.contains(&self.operation.to)
        {
            return Err(CommandError::ValidationError(
                "development_system.file_move_overlaps_initial_user_change=true".to_string(),
            ));
        }
        if self.operation.state_epoch != state.as_ref().state_epoch
            || assignment.state_epoch != self.operation.state_epoch
            || !assignment.scope_ids.contains(&self.operation.scope_id)
        {
            return Err(CommandError::ValidationError(
                "development_system.file_move_authorization_stale=true".to_string(),
            ));
        }
        if let Some(existing) = state.as_ref().authorization.as_ref() {
            if existing == &self.operation {
                return Ok(ModeledEvents::none(
                    "identical file move is already authorized",
                ));
            }
            return Err(CommandError::ValidationError(
                "development_system.file_move_operation_reused=true".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeFileMoveStreamToEvent::apply(self))
                .fact(AuthorizeFileMoveToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct ConfirmFileMovedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    operation: WorkspaceFileMove,
}
#[derive(ModelCommand)]
struct ConfirmFileMoved {
    #[stream]
    stream: WorkflowAuthorityStream,
    operation: WorkspaceFileMove,
}
mapping! { ConfirmFileMovedRequestToStream: ConfirmFileMovedRequest.stream => ConfirmFileMoved.stream using clone; }
mapping! { ConfirmFileMovedRequestToOperation: ConfirmFileMovedRequest.operation => ConfirmFileMoved.operation using clone; }
mapping! { ConfirmFileMovedStreamToEvent: ConfirmFileMoved.stream => WorkflowAuthorityEvent.stream using workflow_event_stream; }
fn file_moved_fact(operation: &WorkspaceFileMove) -> WorkflowFact {
    WorkflowFact::FileMoved {
        operation: operation.clone(),
    }
}
mapping! { ConfirmFileMovedToFact: ConfirmFileMoved.operation => WorkflowAuthorityEvent.fact using file_moved_fact; }
impl ModelCommandLogic for ConfirmFileMoved {
    type Event = WorkflowAuthorityEvent;
    type State = ConfirmFileMovedState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        ConfirmFileMovedState::model_builder()
            .authorization(ConfirmFileMovedEventToAuthorization::apply((
                event,
                self,
                state.as_ref(),
            )))
            .completed(ConfirmFileMovedEventToCompleted::apply((
                event,
                self,
                state.as_ref(),
            )))
            .build()
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().completed {
            return Ok(ModeledEvents::none("file move is already confirmed"));
        }
        if state.as_ref().authorization.as_ref() != Some(&self.operation) {
            return Err(CommandError::ValidationError(
                "development_system.file_move_not_authorized=true".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(ConfirmFileMovedStreamToEvent::apply(self))
                .fact(ConfirmFileMovedToFact::apply(self))
                .build(),
        ))
    }
}

impl ProjectConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text)
            .map_err(|error| format!("development_system.config_invalid source={error}"))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err("development_system.config_schema_unsupported".to_string());
        }
        if self.scopes.is_empty() {
            return Err("development_system.config_scope_required".to_string());
        }
        for (id, scope) in &self.scopes {
            valid_identifier(id, "scope")?;
            if scope.include.is_empty() {
                return Err(format!("development_system.scope_include_required id={id}"));
            }
            for pattern in scope.include.iter().chain(&scope.exclude) {
                validate_glob(pattern)?;
            }
        }
        for (id, command) in &self.commands {
            valid_identifier(id, "command")?;
            command.validate(self)?;
        }
        if let Some(forge) = &self.forge {
            validate_forge_repository(&forge.repository)?;
        }
        Ok(())
    }

    pub fn scope_allows(
        &self,
        scope_id: &str,
        root: &Path,
        relative: &Path,
    ) -> Result<bool, String> {
        let scope = self
            .scopes
            .get(scope_id)
            .ok_or_else(|| format!("development_system.scope_unknown id={scope_id}"))?;
        let relative = normalize_relative(relative)?;
        let slash_path = path_as_slashes(&relative);
        if is_protected(&slash_path) {
            return Ok(false);
        }
        let resolved = resolve_existing_prefix(&root.join(&relative))?;
        if !resolved.starts_with(root) {
            return Ok(false);
        }
        Ok(scope
            .include
            .iter()
            .any(|glob| glob_matches(glob, &slash_path))
            && !scope
                .exclude
                .iter()
                .any(|glob| glob_matches(glob, &slash_path)))
    }

    pub fn digest(&self) -> String {
        // Stable, bounded configuration identity.  This detects stale
        // assignments without treating serialization order as authority.
        let serialized = toml::to_string(self).unwrap_or_default();
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in serialized.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    }
}

fn validate_forge_repository(repository: &str) -> Result<(), String> {
    let mut parts = repository.split('/');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) if valid_part(owner) && valid_part(name) => Ok(()),
        _ => Err("development_system.forge_repository_invalid".to_string()),
    }
}

impl ProjectCommand {
    fn validate(&self, config: &ProjectConfig) -> Result<(), String> {
        if self.argv.is_empty() || self.argv.iter().any(|part| part.is_empty()) {
            return Err("development_system.command_argv_required".to_string());
        }
        let program = Path::new(&self.argv[0])
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if matches!(
            program,
            "sh" | "bash" | "zsh" | "fish" | "dash" | "pwsh" | "powershell" | "git" | "gh" | "glab"
        ) {
            return Err(format!(
                "development_system.command_program_forbidden program={program}"
            ));
        }
        if self.argv.iter().any(|part| {
            part.contains('\n') || part.contains('\r') || part.contains("$(") || part.contains('`')
        }) {
            return Err("development_system.command_argv_invalid".to_string());
        }
        for parameter in self.parameters.keys() {
            valid_identifier(parameter, "parameter")?;
        }
        for argument in &self.argv {
            if argument.contains('{') || argument.contains('}') {
                let parameter = argument
                    .strip_prefix('{')
                    .and_then(|value| value.strip_suffix('}'))
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        "development_system.command_parameter_reference_invalid".to_string()
                    })?;
                if !self.parameters.contains_key(parameter) {
                    return Err("development_system.command_parameter_unknown".to_string());
                }
            }
        }
        for scope_id in &self.output_scopes {
            if !config.scopes.contains_key(scope_id) {
                return Err(format!(
                    "development_system.command_output_scope_unknown id={scope_id}"
                ));
            }
        }
        if self.environment.iter().any(|name| {
            name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'_' || byte.is_ascii_digit())
        }) {
            return Err("development_system.command_environment_invalid".to_string());
        }
        if let Some(service) = &self.service {
            valid_identifier(&service.readiness_command, "service_readiness_command")?;
            valid_identifier(&service.shutdown_command, "service_shutdown_command")?;
            if !config.commands.contains_key(&service.readiness_command)
                || !config.commands.contains_key(&service.shutdown_command)
            {
                return Err("development_system.service_command_unknown".to_string());
            }
        }
        Ok(())
    }

    fn resolved_argv(&self, parameters: &BTreeMap<String, Value>) -> Result<Vec<String>, String> {
        if parameters.len() != self.parameters.len()
            || parameters
                .keys()
                .any(|key| !self.parameters.contains_key(key))
        {
            return Err("development_system.command_parameter_set_invalid".to_string());
        }
        let typed = self
            .parameters
            .iter()
            .map(|(name, kind)| {
                let value = parameters
                    .get(name)
                    .ok_or_else(|| "development_system.command_parameter_missing".to_string())?;
                let rendered = match kind {
                    ParameterType::String => {
                        value.as_str().map(str::to_string).ok_or_else(|| {
                            "development_system.command_parameter_type_invalid".to_string()
                        })?
                    }
                    ParameterType::Integer => value
                        .as_i64()
                        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
                        .map(|value| value.to_string())
                        .ok_or_else(|| {
                            "development_system.command_parameter_type_invalid".to_string()
                        })?,
                    ParameterType::Boolean => value
                        .as_bool()
                        .map(|value| value.to_string())
                        .ok_or_else(|| {
                            "development_system.command_parameter_type_invalid".to_string()
                        })?,
                };
                Ok((name.as_str(), rendered))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        self.argv
            .iter()
            .map(|argument| {
                if let Some(name) = argument
                    .strip_prefix('{')
                    .and_then(|value| value.strip_suffix('}'))
                {
                    typed
                        .get(name)
                        .cloned()
                        .ok_or_else(|| "development_system.command_parameter_missing".to_string())
                } else {
                    Ok(argument.clone())
                }
            })
            .collect()
    }
}

impl Assignment {
    pub fn authorize(
        &self,
        role: Role,
        state_epoch: u64,
        scope_id: Option<&str>,
        command_id: Option<&str>,
        config: &ProjectConfig,
        now: u64,
    ) -> Result<(), String> {
        if self.role != role {
            return Err("development_system.assignment_role_denied".to_string());
        }
        if self.state_epoch != state_epoch {
            return Err("development_system.assignment_stale_epoch".to_string());
        }
        if now >= self.expires_at {
            return Err("development_system.assignment_expired".to_string());
        }
        if self.configuration_digest != config.digest() {
            return Err("development_system.assignment_stale_configuration".to_string());
        }
        if let Some(scope_id) = scope_id {
            if !self.scope_ids.contains(scope_id) {
                return Err("development_system.assignment_scope_denied".to_string());
            }
            let scope = config
                .scopes
                .get(scope_id)
                .ok_or_else(|| "development_system.assignment_scope_denied".to_string())?;
            if !role_allows_scope(&self.role, &scope.category) {
                return Err("development_system.assignment_scope_role_denied".to_string());
            }
        }
        if let Some(command_id) = command_id {
            if !self.command_ids.contains(command_id) {
                return Err("development_system.assignment_command_denied".to_string());
            }
            let command = config
                .commands
                .get(command_id)
                .ok_or_else(|| "development_system.assignment_command_denied".to_string())?;
            if !role_allows_command(&self.role, &command.capability) {
                return Err("development_system.assignment_command_role_denied".to_string());
            }
        }
        Ok(())
    }
}

fn workflow_stream_id_read(root: &Path) -> Result<StreamId, String> {
    crate::workflow::workflow_authority_stream_id(root)
}

fn workflow_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    crate::workflow::lifecycle_runtime()
}

fn legacy_semantic_import_at(root: &Path) -> Result<Option<LegacySemanticImport>, String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.legacy_event_store_open_failed source={error}")
            })?;
    let stream = crate::workflow::legacy_semantic_stream_id(root)?;
    let facts = workflow_runtime()?.block_on(async move {
        let mut events = store
            .read_stream::<LegacySemanticEvent>(stream)
            .await
            .map_err(|error| {
                format!("development_system.legacy_event_store_read_failed source={error}")
            })?;
        let mut facts = Vec::new();
        while let Some(event) = events.next().await {
            facts.push(
                event
                    .map_err(|error| {
                        format!("development_system.legacy_event_store_read_failed source={error}")
                    })?
                    .fact,
            );
            if facts.len() > MAX_CHECKPOINTS {
                return Err("development_system.legacy_import_too_large".to_string());
            }
        }
        Ok::<_, String>(facts)
    })?;
    if facts.is_empty() {
        return Ok(None);
    }
    let digest = content_digest(&serde_json::to_vec(&facts).map_err(|error| {
        format!("development_system.legacy_import_encode_failed source={error}")
    })?);
    Ok(Some(LegacySemanticImport {
        source_id: format!("git-semantic-v1:{digest}"),
        facts,
    }))
}

/// The retired semantic stream is imported only immediately before a
/// capability mutation. Read-only projection calls intentionally bypass it.
pub(crate) fn ensure_legacy_semantic_imported_at(root: &Path) -> Result<(), String> {
    let Some(import) = legacy_semantic_import_at(root)? else {
        return Ok(());
    };
    let request = ImportLegacySemanticRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id_read(root)?))
        .import(import)
        .build();
    let command = ImportLegacySemantic::model_builder()
        .stream(ImportLegacySemanticRequestToStream::apply(request.as_ref()))
        .import(ImportLegacySemanticRequestToImport::apply(request.as_ref()))
        .build();
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.legacy_event_store_open_failed source={error}")
            })?;
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.legacy_import_failed source={error}"))
}

fn workflow_stream_id(root: &Path) -> Result<StreamId, String> {
    crate::workflow::ensure_legacy_lifecycle_imported_at(root)?;
    ensure_legacy_semantic_imported_at(root)?;
    workflow_stream_id_read(root)
}

fn workflow_projection_at(root: &Path) -> Result<WorkflowProjection, String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let runtime = workflow_runtime()?;
    let stream = workflow_stream_id_read(root)?;
    runtime.block_on(async move {
        let mut events = store
            .read_stream::<WorkflowAuthorityEvent>(stream)
            .await
            .map_err(|error| {
                format!("development_system.workflow_event_store_read_failed source={error}")
            })?;
        let mut projection = WorkflowProjection::default();
        while let Some(event) = events.next().await {
            let event = event.map_err(|error| {
                format!("development_system.workflow_event_store_read_failed source={error}")
            })?;
            projection = projection.apply_fact(&event.fact);
        }
        Ok(projection)
    })
}

fn issue_assignment_command_at(root: &Path, assignment: Assignment) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = IssueAssignmentRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .assignment_id(assignment.id)
        .role(assignment.role)
        .expected_state_epoch(assignment.state_epoch)
        .scope_ids(assignment.scope_ids)
        .command_ids(assignment.command_ids)
        .expires_at(assignment.expires_at)
        .configuration_digest(assignment.configuration_digest)
        .build();
    let command = IssueAssignment::model_builder()
        .stream(IssueAssignmentRequestToStream::apply(request.as_ref()))
        .assignment_id(IssueAssignmentRequestToId::apply(request.as_ref()))
        .role(IssueAssignmentRequestToRole::apply(request.as_ref()))
        .expected_state_epoch(IssueAssignmentRequestToExpectedEpoch::apply(
            request.as_ref(),
        ))
        .scope_ids(IssueAssignmentRequestToScopes::apply(request.as_ref()))
        .command_ids(IssueAssignmentRequestToCommands::apply(request.as_ref()))
        .expires_at(IssueAssignmentRequestToExpiry::apply(request.as_ref()))
        .configuration_digest(IssueAssignmentRequestToDigest::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn record_command_receipt_command_at(root: &Path, receipt: ReceiptIntent) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordCommandReceiptRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordCommandReceipt::model_builder()
        .stream(RecordReceiptRequestToStream::apply(request.as_ref()))
        .receipt(RecordReceiptRequestToIntent::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn capture_checkpoint_command_at(root: &Path, intent: CheckpointIntent) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = CaptureCheckpointRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(intent)
        .build();
    let command = CaptureCheckpoint::model_builder()
        .stream(CaptureCheckpointRequestToStream::apply(request.as_ref()))
        .intent(CaptureCheckpointRequestToIntent::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_signed_commit_command_at(
    root: &Path,
    intent: SignedCommitIntent,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeSignedCommitRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(intent)
        .build();
    let command = AuthorizeSignedCommit::model_builder()
        .stream(AuthorizeSignedCommitRequestToStream::apply(
            request.as_ref(),
        ))
        .intent(AuthorizeSignedCommitRequestToIntent::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn record_signed_commit_command_at(
    root: &Path,
    receipt: SignedCommitReceipt,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordSignedCommitRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordSignedCommit::model_builder()
        .stream(RecordSignedCommitRequestToStream::apply(request.as_ref()))
        .receipt(RecordSignedCommitRequestToReceipt::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_signed_tag_command_at(root: &Path, intent: SignedTagIntent) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeSignedTagRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(intent)
        .build();
    let command = AuthorizeSignedTag::model_builder()
        .stream(AuthorizeSignedTagRequestToStream::apply(request.as_ref()))
        .intent(AuthorizeSignedTagRequestToIntent::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn record_signed_tag_command_at(root: &Path, receipt: SignedTagReceipt) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordSignedTagRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordSignedTag::model_builder()
        .stream(RecordSignedTagRequestToStream::apply(request.as_ref()))
        .receipt(RecordSignedTagRequestToReceipt::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_fetch_ref_command_at(root: &Path, intent: FetchRefIntent) -> Result<(), String> {
    let configuration_digest = match config_at(root) {
        ConfigState::Valid(config) => config.digest(),
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeRemoteRefFetchRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(FetchRefIntent {
            configuration_digest,
            ..intent
        })
        .build();
    let command = AuthorizeRemoteRefFetch::model_builder()
        .stream(AuthorizeRemoteRefFetchRequestToStream::apply(
            request.as_ref(),
        ))
        .intent(AuthorizeRemoteRefFetchRequestToIntent::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}
fn record_fetch_ref_command_at(root: &Path, receipt: FetchRefReceipt) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordRemoteRefFetchedRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordRemoteRefFetched::model_builder()
        .stream(RecordRemoteRefFetchedRequestToStream::apply(
            request.as_ref(),
        ))
        .receipt(RecordRemoteRefFetchedRequestToReceipt::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_push_ref_command_at(
    root: &Path,
    intent: PushRefIntent,
) -> Result<PushRefOperation, String> {
    let configuration_digest = match config_at(root) {
        ConfigState::Valid(config) => config.digest(),
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeRemoteRefPushRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(PushRefIntent {
            configuration_digest,
            ..intent
        })
        .build();
    let command = AuthorizeRemoteRefPush::model_builder()
        .stream(AuthorizeRemoteRefPushRequestToStream::apply(
            request.as_ref(),
        ))
        .intent(AuthorizeRemoteRefPushRequestToIntent::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map_err(|error| {
            format!("development_system.workflow_event_command_failed source={error}")
        })?;
    let operation_id = request.as_ref().intent.operation_id.clone();
    workflow_projection_at(root)?
        .push_ref_authorizations
        .get(&operation_id)
        .cloned()
        .ok_or_else(|| "development_system.push_ref_authorization_missing".to_string())
}

fn record_push_ref_command_at(root: &Path, receipt: PushRefReceipt) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordRemoteRefPushedRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordRemoteRefPushed::model_builder()
        .stream(RecordRemoteRefPushedRequestToStream::apply(
            request.as_ref(),
        ))
        .receipt(RecordRemoteRefPushedRequestToReceipt::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn open_pull_request_command_at(root: &Path, intent: OpenPullRequestIntent) -> Result<(), String> {
    let configuration_digest = match config_at(root) {
        ConfigState::Valid(config) => config.digest(),
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = OpenPullRequestRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(OpenPullRequestIntent {
            configuration_digest,
            ..intent
        })
        .build();
    let command = OpenPullRequest::model_builder()
        .stream(OpenPullRequestRequestToStream::apply(request.as_ref()))
        .intent(OpenPullRequestRequestToIntent::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn record_pull_request_opened_command_at(
    root: &Path,
    receipt: OpenPullRequestReceipt,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordPullRequestOpenedRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordPullRequestOpened::model_builder()
        .stream(RecordPullRequestOpenedRequestToStream::apply(
            request.as_ref(),
        ))
        .receipt(RecordPullRequestOpenedRequestToReceipt::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn update_pull_request_command_at(
    root: &Path,
    intent: UpdatePullRequestIntent,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = UpdatePullRequestRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(intent)
        .build();
    let command = UpdatePullRequest::model_builder()
        .stream(UpdatePullRequestRequestToStream::apply(request.as_ref()))
        .intent(UpdatePullRequestRequestToIntent::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn record_pull_request_updated_command_at(
    root: &Path,
    receipt: UpdatePullRequestReceipt,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordPullRequestUpdatedRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordPullRequestUpdated::model_builder()
        .stream(RecordPullRequestUpdatedRequestToStream::apply(
            request.as_ref(),
        ))
        .receipt(RecordPullRequestUpdatedRequestToReceipt::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn merge_pull_request_command_at(
    root: &Path,
    intent: MergePullRequestIntent,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = MergePullRequestRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .intent(intent)
        .build();
    let command = MergePullRequest::model_builder()
        .stream(MergePullRequestRequestToStream::apply(request.as_ref()))
        .intent(MergePullRequestRequestToIntent::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}
fn record_pull_request_merged_command_at(
    root: &Path,
    receipt: MergePullRequestReceipt,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = RecordPullRequestMergedRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = RecordPullRequestMerged::model_builder()
        .stream(RecordPullRequestMergedRequestToStream::apply(
            request.as_ref(),
        ))
        .receipt(RecordPullRequestMergedRequestToReceipt::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_file_write_command_at(
    root: &Path,
    operation: WorkspaceFileWrite,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeFileWriteRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .operation(operation)
        .build();
    let command = AuthorizeFileWrite::model_builder()
        .stream(AuthorizeFileWriteRequestToStream::apply(request.as_ref()))
        .operation(AuthorizeFileWriteRequestToOperation::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn confirm_file_written_command_at(
    root: &Path,
    operation: WorkspaceFileWrite,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = ConfirmFileWrittenRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .operation(operation)
        .build();
    let command = ConfirmFileWritten::model_builder()
        .stream(ConfirmFileWrittenRequestToStream::apply(request.as_ref()))
        .operation(ConfirmFileWrittenRequestToOperation::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_checkpoint_abort_command_at(
    root: &Path,
    operation: CheckpointAbortOperation,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeCheckpointAbortRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .operation(operation)
        .build();
    let command = AuthorizeCheckpointAbort::model_builder()
        .stream(AuthorizeCheckpointAbortRequestToStream::apply(
            request.as_ref(),
        ))
        .operation(AuthorizeCheckpointAbortRequestToOperation::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn complete_checkpoint_abort_command_at(
    root: &Path,
    receipt: CheckpointAbortReceipt,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = CompleteCheckpointAbortRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .receipt(receipt)
        .build();
    let command = CompleteCheckpointAbort::model_builder()
        .stream(CompleteCheckpointAbortRequestToStream::apply(
            request.as_ref(),
        ))
        .receipt(CompleteCheckpointAbortRequestToReceipt::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_file_delete_command_at(
    root: &Path,
    operation: WorkspaceFileDeletion,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeFileDeleteRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .operation(operation)
        .build();
    let command = AuthorizeFileDelete::model_builder()
        .stream(AuthorizeFileDeleteRequestToStream::apply(request.as_ref()))
        .operation(AuthorizeFileDeleteRequestToOperation::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn confirm_file_deleted_command_at(
    root: &Path,
    operation: WorkspaceFileDeletion,
) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = ConfirmFileDeletedRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .operation(operation)
        .build();
    let command = ConfirmFileDeleted::model_builder()
        .stream(ConfirmFileDeletedRequestToStream::apply(request.as_ref()))
        .operation(ConfirmFileDeletedRequestToOperation::apply(
            request.as_ref(),
        ))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn authorize_file_move_command_at(root: &Path, operation: WorkspaceFileMove) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = AuthorizeFileMoveRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .operation(operation)
        .build();
    let command = AuthorizeFileMove::model_builder()
        .stream(AuthorizeFileMoveRequestToStream::apply(request.as_ref()))
        .operation(AuthorizeFileMoveRequestToOperation::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

fn confirm_file_moved_command_at(root: &Path, operation: WorkspaceFileMove) -> Result<(), String> {
    let store =
        GitEventStore::open_for_authority(root, GitEventStoreAuthority::DevelopmentWorkflow)
            .map_err(|error| {
                format!("development_system.workflow_event_store_open_failed source={error}")
            })?;
    let request = ConfirmFileMovedRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_stream_id(root)?))
        .operation(operation)
        .build();
    let command = ConfirmFileMoved::model_builder()
        .stream(ConfirmFileMovedRequestToStream::apply(request.as_ref()))
        .operation(ConfirmFileMovedRequestToOperation::apply(request.as_ref()))
        .build();
    workflow_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_system.workflow_event_command_failed source={error}"))
}

pub fn workflow_state_epoch_at(root: &Path) -> Result<u64, String> {
    crate::workflow::state_epoch_at(root)
}

pub fn issue_assignment_at(root: &Path, assignment: Assignment) -> Result<(), String> {
    issue_assignment_command_at(root, assignment)
}

pub fn capture_checkpoint_at(
    root: &Path,
    assignment_id: &str,
    role: Role,
    now: u64,
) -> Result<Checkpoint, String> {
    let _assignment = authorize_assignment_at(root, assignment_id, role, None, None, now)?;
    capture_current_checkpoint_at(root, now)
}

fn capture_current_checkpoint_at(root: &Path, now: u64) -> Result<Checkpoint, String> {
    let state_epoch = workflow_state_epoch_at(root)?;
    let projection = workflow_projection_at(root)?;
    let checkpoint = observe_current_checkpoint_at(
        root,
        now,
        state_epoch,
        projection.accepted_evidence_ids.clone(),
        projection.last_checkpoint_id.clone(),
    )?;
    let intent = CheckpointIntent {
        id: checkpoint.id.clone(),
        expected_state_epoch: checkpoint.state_epoch,
        index_tree: checkpoint.index_tree.clone(),
        owned_paths: checkpoint.owned_paths.clone(),
        authorized_scope_ids: checkpoint.authorized_scope_ids.clone(),
        command_policy_digest: checkpoint.command_policy_digest.clone(),
        evidence_ids: checkpoint.evidence_ids.clone(),
        expected_predecessor: checkpoint.predecessor.clone(),
        created_at: checkpoint.created_at,
    };
    capture_checkpoint_command_at(root, intent)?;
    Ok(checkpoint)
}

fn observe_current_checkpoint_at(
    root: &Path,
    now: u64,
    state_epoch: u64,
    evidence_ids: BTreeSet<String>,
    predecessor: Option<String>,
) -> Result<Checkpoint, String> {
    let output = Command::new("git")
        .args(["write-tree"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.checkpoint_git_unavailable source={error}"))?;
    if !output.status.success() {
        return Err("development_system.checkpoint_index_unavailable".to_string());
    }
    let index_tree = String::from_utf8(output.stdout)
        .map_err(|_| "development_system.checkpoint_tree_invalid".to_string())?
        .trim()
        .to_string();
    if !index_tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("development_system.checkpoint_tree_invalid".to_string());
    }
    let staged = Command::new("git")
        .args(["diff", "--cached", "--name-only", "-z"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.checkpoint_git_unavailable source={error}"))?;
    if !staged.status.success() {
        return Err("development_system.checkpoint_index_unavailable".to_string());
    }
    let config = match config_at(root) {
        ConfigState::Valid(config) => config,
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    let staged_paths = staged
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map_err(|_| "development_system.checkpoint_path_invalid".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let projection = workflow_projection_at(root)?;
    let mut owned_paths = BTreeSet::new();
    for path in staged_paths {
        if projection.workflow_owned_paths.contains(path) {
            owned_paths.insert(path.to_string());
        }
    }
    let authorized_scope_ids = projection
        .file_write_authorizations
        .values()
        .filter(|operation| owned_paths.contains(&operation.path))
        .map(|operation| operation.scope_id.clone())
        .collect();
    Ok(Checkpoint {
        id: format!(
            "checkpoint-{}-{}",
            now,
            CHECKPOINT_OPERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ),
        state_epoch,
        index_tree,
        owned_paths,
        authorized_scope_ids,
        command_policy_digest: config.digest(),
        evidence_ids,
        predecessor,
        created_at: now,
    })
}

fn stage_paths_owned_by_role(
    root: &Path,
    role: Role,
) -> Result<(String, BTreeSet<String>), String> {
    let projection = workflow_projection_at(root)?;
    let owned_paths = projection
        .workflow_path_owners
        .iter()
        .filter_map(|(path, assignment_id)| {
            projection
                .assignments
                .get(assignment_id)
                .filter(|assignment| assignment.role == role)
                .map(|_| path.clone())
        })
        .collect::<BTreeSet<_>>();
    let paths = owned_paths
        .into_iter()
        .filter(|path| {
            root.join(path).exists()
                || Command::new("git")
                    .args(["ls-files", "--error-unmatch", "--", path])
                    .current_dir(root)
                    .output()
                    .is_ok_and(|output| output.status.success())
        })
        .collect::<BTreeSet<_>>();
    if paths.is_empty() {
        return Err("development_system.checkpoint_owned_delta_required=true".to_string());
    }
    let before = Command::new("git")
        .args(["write-tree"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.checkpoint_git_unavailable source={error}"))?;
    if !before.status.success() {
        return Err("development_system.checkpoint_index_unavailable".to_string());
    }
    let before_tree = String::from_utf8(before.stdout)
        .map_err(|_| "development_system.checkpoint_tree_invalid".to_string())?
        .trim()
        .to_string();
    let mut command = Command::new("git");
    command.arg("add").arg("-A").arg("--");
    command.args(paths.iter());
    let status = command
        .current_dir(root)
        .status()
        .map_err(|error| format!("development_system.checkpoint_git_unavailable source={error}"))?;
    if !status.success() {
        return Err("development_system.checkpoint_stage_failed=true".to_string());
    }
    Ok((before_tree, paths))
}

fn restore_index_tree(root: &Path, tree: &str) -> Result<(), String> {
    let status = Command::new("git")
        .args(["read-tree", tree])
        .current_dir(root)
        .status()
        .map_err(|error| format!("development_system.checkpoint_git_unavailable source={error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("development_system.checkpoint_index_restore_failed=true".to_string())
    }
}

fn current_index_tree(root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["write-tree"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.checkpoint_git_unavailable source={error}"))?;
    if !output.status.success() {
        return Err("development_system.checkpoint_index_unavailable".to_string());
    }
    String::from_utf8(output.stdout)
        .map(|tree| tree.trim().to_string())
        .map_err(|_| "development_system.checkpoint_tree_invalid".to_string())
}

fn current_path_digests(
    root: &Path,
    paths: &BTreeSet<String>,
) -> Result<BTreeMap<String, Option<String>>, String> {
    paths
        .iter()
        .map(|path| {
            let absolute = root.join(path);
            match fs::read(&absolute) {
                Ok(bytes) => Ok((path.clone(), Some(content_digest(&bytes)))),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok((path.clone(), None))
                }
                Err(error) => Err(format!(
                    "development_system.checkpoint_abort_path_unreadable path={path} source={error}"
                )),
            }
        })
        .collect()
}

pub fn preview_checkpoint_abort_at(
    root: &Path,
    now: u64,
) -> Result<CheckpointAbortOperation, String> {
    let projection = workflow_projection_at(root)?;
    let checkpoint_id = projection
        .last_checkpoint_id
        .as_ref()
        .ok_or_else(|| "development_system.checkpoint_abort_checkpoint_required".to_string())?;
    let checkpoint = projection
        .checkpoints
        .get(checkpoint_id)
        .ok_or_else(|| "development_system.checkpoint_abort_checkpoint_required".to_string())?;
    if projection.paths_changed_since_checkpoint.is_empty() {
        return Err("development_system.checkpoint_abort_delta_required".to_string());
    }
    let expected_index_tree = current_index_tree(root)?;
    let path_digests = current_path_digests(root, &projection.paths_changed_since_checkpoint)?;
    let sequence = CHECKPOINT_ABORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let identity = serde_json::to_vec(&(
        checkpoint_id,
        &checkpoint.index_tree,
        &expected_index_tree,
        &projection.paths_changed_since_checkpoint,
        &path_digests,
        now,
        sequence,
    ))
    .map_err(|error| format!("development_system.checkpoint_abort_encode_failed source={error}"))?;
    Ok(CheckpointAbortOperation {
        operation_id: format!("checkpoint-abort-{}", content_digest(&identity)),
        checkpoint_id: checkpoint_id.clone(),
        checkpoint_tree: checkpoint.index_tree.clone(),
        expected_index_tree,
        affected_paths: projection.paths_changed_since_checkpoint.clone(),
        path_digests,
        authorized_at: now,
    })
}

fn git_common_directory(root: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.checkpoint_git_unavailable source={error}"))?;
    if !output.status.success() {
        return Err("development_system.checkpoint_git_common_directory_unavailable".to_string());
    }
    String::from_utf8(output.stdout)
        .map(|path| PathBuf::from(path.trim()))
        .map_err(|_| "development_system.checkpoint_git_common_directory_invalid".to_string())
}

fn archive_checkpoint_abort(
    root: &Path,
    operation: &CheckpointAbortOperation,
) -> Result<String, String> {
    let relative = format!("development-workflow/recovery/{}", operation.operation_id);
    let archive = git_common_directory(root)?.join(&relative);
    fs::create_dir_all(archive.join("files")).map_err(|error| {
        format!("development_system.checkpoint_abort_archive_failed source={error}")
    })?;
    let manifest = serde_json::to_vec_pretty(operation).map_err(|error| {
        format!("development_system.checkpoint_abort_archive_failed source={error}")
    })?;
    fs::write(archive.join("manifest.json"), manifest).map_err(|error| {
        format!("development_system.checkpoint_abort_archive_failed source={error}")
    })?;
    for path in &operation.affected_paths {
        let source = root.join(path);
        if source.is_file() {
            let destination = archive.join("files").join(path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("development_system.checkpoint_abort_archive_failed source={error}")
                })?;
            }
            fs::copy(source, destination).map_err(|error| {
                format!("development_system.checkpoint_abort_archive_failed source={error}")
            })?;
        }
    }
    Ok(relative)
}

fn restore_checkpoint_paths(
    root: &Path,
    operation: &CheckpointAbortOperation,
) -> Result<(), String> {
    let mut present = Vec::new();
    let mut absent = Vec::new();
    for path in &operation.affected_paths {
        let object = format!("{}:{path}", operation.checkpoint_tree);
        let exists = Command::new("git")
            .args(["cat-file", "-e", &object])
            .current_dir(root)
            .output()
            .map_err(|error| {
                format!("development_system.checkpoint_git_unavailable source={error}")
            })?
            .status
            .success();
        if exists {
            present.push(path.clone());
        } else {
            absent.push(path.clone());
        }
    }
    if !present.is_empty() {
        let mut command = Command::new("git");
        command
            .arg("restore")
            .arg(format!("--source={}", operation.checkpoint_tree))
            .args(["--staged", "--worktree", "--"])
            .args(&present)
            .current_dir(root);
        if !command
            .status()
            .map_err(|error| {
                format!("development_system.checkpoint_git_unavailable source={error}")
            })?
            .success()
        {
            return Err("development_system.checkpoint_abort_restore_failed".to_string());
        }
    }
    if !absent.is_empty() {
        let mut command = Command::new("git");
        command
            .args(["rm", "-f", "--ignore-unmatch", "--"])
            .args(&absent)
            .current_dir(root);
        if !command
            .status()
            .map_err(|error| {
                format!("development_system.checkpoint_git_unavailable source={error}")
            })?
            .success()
        {
            return Err("development_system.checkpoint_abort_restore_failed".to_string());
        }
        for path in absent {
            let absolute = root.join(path);
            if absolute.exists() {
                fs::remove_file(absolute).map_err(|error| {
                    format!("development_system.checkpoint_abort_restore_failed source={error}")
                })?;
            }
        }
    }
    Ok(())
}

pub fn apply_checkpoint_abort_at(
    root: &Path,
    operation: CheckpointAbortOperation,
    now: u64,
) -> Result<CheckpointAbortReceipt, String> {
    let projection = workflow_projection_at(root)?;
    if let Some(receipt) = projection
        .checkpoint_abort_receipts
        .get(&operation.operation_id)
    {
        return Ok(receipt.clone());
    }
    let authorized = projection
        .checkpoint_abort_authorizations
        .get(&operation.operation_id)
        .cloned();
    if let Some(existing) = &authorized {
        if existing != &operation {
            return Err("development_system.checkpoint_abort_operation_mismatch".to_string());
        }
    } else {
        if current_index_tree(root)? != operation.expected_index_tree
            || current_path_digests(root, &operation.affected_paths)? != operation.path_digests
        {
            return Err("development_system.checkpoint_abort_concurrent_change".to_string());
        }
        authorize_checkpoint_abort_command_at(root, operation.clone())?;
    }
    let archive_relative_path = archive_checkpoint_abort(root, &operation)?;
    restore_checkpoint_paths(root, &operation)?;
    let receipt = CheckpointAbortReceipt {
        operation_id: operation.operation_id.clone(),
        archive_relative_path,
        restored_index_tree: current_index_tree(root)?,
        completed_at: now,
    };
    complete_checkpoint_abort_command_at(root, receipt.clone())?;
    Ok(receipt)
}

pub fn accept_red_and_checkpoint_at(
    root: &Path,
    receipt_id: &str,
    now: u64,
) -> Result<crate::workflow::Workflow, String> {
    let receipt = command_receipt_at(root, receipt_id, false, now)?;
    let projection = workflow_projection_at(root)?;
    let assignment = projection
        .assignments
        .get(&receipt.assignment_id)
        .ok_or_else(|| "development_system.assignment_unknown".to_string())?;
    if assignment.role != Role::TestAuthor {
        return Err("development_system.red_evidence_role_invalid=true".to_string());
    }
    let (before_tree, _) = stage_paths_owned_by_role(root, Role::TestAuthor)?;
    let checkpoint = observe_current_checkpoint_at(
        root,
        now,
        projection.state_epoch.saturating_add(1),
        [receipt_id.to_string()].into_iter().collect(),
        projection.last_checkpoint_id.clone(),
    )?;
    match crate::workflow::accept_red_evidence_at(root, receipt_id, checkpoint) {
        Ok(workflow) => Ok(workflow),
        Err(error) => {
            restore_index_tree(root, &before_tree)?;
            Err(error)
        }
    }
}

pub fn accept_green_and_checkpoint_at(
    root: &Path,
    receipt_id: &str,
    now: u64,
) -> Result<crate::workflow::Workflow, String> {
    let receipt = command_receipt_at(root, receipt_id, true, now)?;
    let projection = workflow_projection_at(root)?;
    let assignment = projection
        .assignments
        .get(&receipt.assignment_id)
        .ok_or_else(|| "development_system.assignment_unknown".to_string())?;
    if assignment.role != Role::Implementer {
        return Err("development_system.green_evidence_role_invalid=true".to_string());
    }
    let (before_tree, _) = stage_paths_owned_by_role(root, Role::Implementer)?;
    let mut evidence_ids = projection.accepted_evidence_ids.clone();
    evidence_ids.insert(receipt_id.to_string());
    let checkpoint = observe_current_checkpoint_at(
        root,
        now,
        projection.state_epoch.saturating_add(1),
        evidence_ids,
        projection.last_checkpoint_id.clone(),
    )?;
    match crate::workflow::accept_green_evidence_at(root, receipt_id, checkpoint) {
        Ok(workflow) => Ok(workflow),
        Err(error) => {
            restore_index_tree(root, &before_tree)?;
            Err(error)
        }
    }
}

pub fn inspect_remote_at(root: &Path, remote: &str) -> Result<serde_json::Value, String> {
    valid_identifier(remote, "remote")?;
    let output = Command::new("git")
        .args(["ls-remote", "--heads", remote])
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.remote_git_unavailable source={error}"))?;
    if !output.status.success() {
        return Err("development_system.remote_inspection_failed".to_string());
    }
    let heads = String::from_utf8(output.stdout)
        .map_err(|_| "development_system.remote_output_invalid".to_string())?
        .lines()
        .filter_map(|line| {
            line.split_once('\t')
                .map(|(_, reference)| reference.to_string())
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "remote": remote,
        "head_count": heads.len(),
        "heads": heads,
    }))
}

pub fn fetch_ref_at(
    root: &Path,
    assignment_id: &str,
    remote: &str,
    remote_ref: &str,
    now: u64,
) -> Result<FetchRefReceipt, String> {
    valid_identifier(remote, "remote")?;
    if !(remote_ref.starts_with("refs/heads/") || remote_ref.starts_with("refs/tags/"))
        || remote_ref.len() > 512
    {
        return Err("development_system.remote_ref_invalid".to_string());
    }
    git_stdout(
        root,
        &["check-ref-format".to_string(), remote_ref.to_string()],
        None,
    )
    .map_err(|_| "development_system.remote_ref_invalid".to_string())?;
    git_stdout(
        root,
        &[
            "remote".to_string(),
            "get-url".to_string(),
            remote.to_string(),
        ],
        None,
    )
    .map_err(|_| "development_system.remote_unknown".to_string())?;
    let projection = workflow_projection_at(root)?;
    let operation_id = format!(
        "fetch-ref-{}",
        content_digest(
            format!(
                "{assignment_id}\0{}\0{remote}\0{remote_ref}",
                projection.state_epoch
            )
            .as_bytes()
        )
    );
    if let Some(receipt) = projection.fetch_ref_receipts.get(&operation_id) {
        return Ok(receipt.clone());
    }
    authorize_fetch_ref_command_at(
        root,
        FetchRefIntent {
            operation_id: operation_id.clone(),
            assignment_id: assignment_id.to_string(),
            expected_state_epoch: projection.state_epoch,
            remote: remote.to_string(),
            remote_ref: remote_ref.to_string(),
            configuration_digest: String::new(),
            authorized_at: now,
        },
    )?;
    git_stdout(
        root,
        &[
            "fetch".to_string(),
            "--no-tags".to_string(),
            remote.to_string(),
            remote_ref.to_string(),
        ],
        None,
    )
    .map_err(|error| format!("development_system.remote_fetch_failed source={error}"))?;
    let object_id = git_stdout(
        root,
        &["rev-parse".to_string(), "FETCH_HEAD".to_string()],
        None,
    )?;
    if !matches!(object_id.len(), 40 | 64)
        || !object_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("development_system.remote_fetch_object_invalid".to_string());
    }
    let receipt = FetchRefReceipt {
        operation_id,
        assignment_id: assignment_id.to_string(),
        remote: remote.to_string(),
        remote_ref: remote_ref.to_string(),
        object_id,
        fetched_at: now,
    };
    record_fetch_ref_command_at(root, receipt.clone())?;
    Ok(receipt)
}

fn remote_ref_object_at(
    root: &Path,
    remote: &str,
    remote_ref: &str,
) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .args(["ls-remote", "--refs", remote, remote_ref])
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.remote_git_unavailable source={error}"))?;
    if !output.status.success() {
        return Err("development_system.remote_authentication_failed".to_string());
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "development_system.remote_output_invalid".to_string())?;
    let mut matches = stdout.lines().filter_map(|line| line.split_once('\t'));
    let object = matches.next().map(|(object, reference)| {
        if reference != remote_ref
            || !matches!(object.len(), 40 | 64)
            || !object.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("development_system.remote_ref_advertisement_invalid".to_string());
        }
        Ok(object.to_string())
    });
    if matches.next().is_some() {
        return Err("development_system.remote_ref_advertisement_ambiguous".to_string());
    }
    object.transpose()
}

pub fn push_ref_at(
    root: &Path,
    assignment_id: &str,
    remote: &str,
    remote_ref: &str,
    source_operation_id: &str,
    expected_remote_object: Option<&str>,
    now: u64,
) -> Result<PushRefReceipt, String> {
    valid_identifier(remote, "remote")?;
    let source_kind = if remote_ref.starts_with("refs/heads/") {
        PushSourceKind::Commit
    } else if remote_ref.starts_with("refs/tags/") {
        PushSourceKind::Tag
    } else {
        return Err("development_system.remote_ref_invalid".to_string());
    };
    if remote_ref.len() > 512 {
        return Err("development_system.remote_ref_invalid".to_string());
    }
    git_stdout(
        root,
        &["check-ref-format".to_string(), remote_ref.to_string()],
        None,
    )
    .map_err(|_| "development_system.remote_ref_invalid".to_string())?;
    git_stdout(
        root,
        &[
            "remote".to_string(),
            "get-url".to_string(),
            remote.to_string(),
        ],
        None,
    )
    .map_err(|_| "development_system.remote_unknown".to_string())?;
    let projection = workflow_projection_at(root)?;
    let expected_remote_object = expected_remote_object.map(str::to_string);
    let operation_id = format!(
        "push-ref-{}",
        content_digest(
            format!(
                "{assignment_id}\0{}\0{remote}\0{remote_ref}\0{source_operation_id}\0{}",
                projection.state_epoch,
                expected_remote_object.as_deref().unwrap_or("<absent>")
            )
            .as_bytes()
        )
    );
    if let Some(receipt) = projection.push_ref_receipts.get(&operation_id) {
        return Ok(receipt.clone());
    }
    let observed_remote_object = remote_ref_object_at(root, remote, remote_ref)?;
    if observed_remote_object != expected_remote_object {
        return Err("development_system.remote_state_stale".to_string());
    }
    let operation = authorize_push_ref_command_at(
        root,
        PushRefIntent {
            operation_id: operation_id.clone(),
            assignment_id: assignment_id.to_string(),
            expected_state_epoch: projection.state_epoch,
            remote: remote.to_string(),
            remote_ref: remote_ref.to_string(),
            source_kind,
            source_operation_id: source_operation_id.to_string(),
            expected_remote_object: expected_remote_object.clone(),
            configuration_digest: String::new(),
            authorized_at: now,
        },
    )?;
    let source_object = operation.source_object;
    let push_result = git_stdout(
        root,
        &[
            "push".to_string(),
            "--porcelain".to_string(),
            remote.to_string(),
            format!("{source_object}:{remote_ref}"),
        ],
        None,
    );
    let resulting_remote_object = remote_ref_object_at(root, remote, remote_ref)?;
    if resulting_remote_object.as_deref() != Some(source_object.as_str()) {
        return Err(match push_result {
            Ok(_) => "development_system.remote_push_ambiguous".to_string(),
            Err(error) => format!("development_system.remote_push_failed source={error}"),
        });
    }
    let receipt = PushRefReceipt {
        operation_id,
        assignment_id: assignment_id.to_string(),
        remote: remote.to_string(),
        remote_ref: remote_ref.to_string(),
        source_object,
        previous_remote_object: expected_remote_object,
        pushed_at: now,
    };
    record_push_ref_command_at(root, receipt.clone())?;
    Ok(receipt)
}

fn forge_stdout(root: &Path, program: &str, arguments: &[String]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| format!("development_system.forge_cli_unavailable source={error}"))?;
    if !output.status.success() {
        return Err(format!(
            "development_system.forge_operation_failed detail={}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| "development_system.forge_output_invalid".to_string())
}

pub fn open_pull_request_at(
    root: &Path,
    assignment_id: &str,
    push_operation_id: &str,
    title: &str,
    body: &str,
    now: u64,
) -> Result<OpenPullRequestReceipt, String> {
    if title.trim().is_empty() || title.len() > 256 || body.len() > 65_536 {
        return Err("development_system.pull_request_text_invalid".to_string());
    }
    if crate::workflow::status_at(root)?.phase_name() != "delivering" {
        return Err("development_system.open_pr_phase_denied".to_string());
    }
    let assignment = authorize_assignment_at(root, assignment_id, Role::Delivery, None, None, now)?;
    let config = configured_project(root)?;
    let forge = config
        .forge
        .ok_or_else(|| "development_system.forge_not_configured".to_string())?;
    let base_branch = config
        .delivery
        .and_then(|delivery| delivery.trunk_branch)
        .ok_or_else(|| "development_system.delivery_trunk_branch_required".to_string())?;
    valid_identifier(&base_branch, "base_branch")?;
    let projection = workflow_projection_at(root)?;
    let pushed = projection
        .push_ref_receipts
        .get(push_operation_id)
        .ok_or_else(|| "development_system.push_receipt_unknown".to_string())?;
    if pushed.assignment_id != assignment_id || !pushed.remote_ref.starts_with("refs/heads/") {
        return Err("development_system.pull_request_head_invalid".to_string());
    }
    let head_ref = pushed
        .remote_ref
        .strip_prefix("refs/heads/")
        .ok_or_else(|| "development_system.pull_request_head_invalid".to_string())?
        .to_string();
    let operation_id = format!(
        "open-pr-{}",
        content_digest(
            format!(
                "{assignment_id}\0{}\0{}\0{push_operation_id}\0{head_ref}\0{base_branch}\0{title}\0{body}",
                assignment.state_epoch,
                forge.repository
            )
            .as_bytes()
        )
    );
    if let Some(receipt) = projection.pull_request_open_receipts.get(&operation_id) {
        return Ok(receipt.clone());
    }
    let intent = OpenPullRequestIntent {
        operation_id: operation_id.clone(),
        assignment_id: assignment_id.to_string(),
        expected_state_epoch: assignment.state_epoch,
        provider: forge.provider.clone(),
        repository: forge.repository.clone(),
        push_operation_id: push_operation_id.to_string(),
        head_ref: head_ref.clone(),
        base_branch: base_branch.clone(),
        title: title.to_string(),
        body: body.to_string(),
        configuration_digest: String::new(),
        authorized_at: now,
    };
    open_pull_request_command_at(root, intent)?;
    let (program, arguments) = match forge.provider {
        ForgeProvider::GitHub => (
            "gh",
            vec![
                "pr",
                "create",
                "--repo",
                &forge.repository,
                "--head",
                &head_ref,
                "--base",
                &base_branch,
                "--title",
                title,
                "--body",
                body,
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        ),
        ForgeProvider::GitLab => (
            "glab",
            vec![
                "mr",
                "create",
                "--repo",
                &forge.repository,
                "--source-branch",
                &head_ref,
                "--target-branch",
                &base_branch,
                "--title",
                title,
                "--description",
                body,
                "--yes",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        ),
    };
    let pull_request_url = forge_stdout(root, program, &arguments)?;
    if !pull_request_url.starts_with("https://")
        || pull_request_url
            .bytes()
            .any(|byte| byte.is_ascii_whitespace())
    {
        return Err("development_system.forge_receipt_url_invalid".to_string());
    }
    let receipt = OpenPullRequestReceipt {
        operation_id,
        assignment_id: assignment_id.to_string(),
        provider: forge.provider,
        repository: forge.repository,
        push_operation_id: push_operation_id.to_string(),
        pull_request_url,
        opened_at: now,
    };
    record_pull_request_opened_command_at(root, receipt.clone())?;
    Ok(receipt)
}

pub fn update_pull_request_at(
    root: &Path,
    assignment_id: &str,
    open_operation_id: &str,
    title: &str,
    body: &str,
    now: u64,
) -> Result<UpdatePullRequestReceipt, String> {
    if title.trim().is_empty() || title.len() > 256 || body.len() > 65_536 {
        return Err("development_system.pull_request_text_invalid".to_string());
    }
    if crate::workflow::status_at(root)?.phase_name() != "delivering" {
        return Err("development_system.update_pr_phase_denied".to_string());
    }
    let assignment = authorize_assignment_at(root, assignment_id, Role::Delivery, None, None, now)?;
    let projection = workflow_projection_at(root)?;
    let opened = projection
        .pull_request_open_receipts
        .get(open_operation_id)
        .ok_or_else(|| "development_system.open_pr_receipt_unknown".to_string())?;
    if opened.assignment_id != assignment_id {
        return Err("development_system.pull_request_assignment_mismatch".to_string());
    }
    let operation_id = format!(
        "update-pr-{}",
        content_digest(
            format!(
                "{assignment_id}\0{}\0{open_operation_id}\0{title}\0{body}",
                assignment.state_epoch
            )
            .as_bytes()
        )
    );
    if let Some(receipt) = projection.pull_request_update_receipts.get(&operation_id) {
        return Ok(receipt.clone());
    }
    update_pull_request_command_at(
        root,
        UpdatePullRequestIntent {
            operation_id: operation_id.clone(),
            assignment_id: assignment_id.to_string(),
            expected_state_epoch: assignment.state_epoch,
            open_operation_id: open_operation_id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            configuration_digest: assignment.configuration_digest.clone(),
            authorized_at: now,
        },
    )?;
    let (program, arguments) = match opened.provider {
        ForgeProvider::GitHub => (
            "gh",
            vec![
                "pr",
                "edit",
                &opened.pull_request_url,
                "--repo",
                &opened.repository,
                "--title",
                title,
                "--body",
                body,
            ],
        ),
        ForgeProvider::GitLab => (
            "glab",
            vec![
                "mr",
                "update",
                &opened.pull_request_url,
                "--repo",
                &opened.repository,
                "--title",
                title,
                "--description",
                body,
                "--yes",
            ],
        ),
    };
    let arguments = arguments
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    forge_stdout(root, program, &arguments)?;
    let receipt = UpdatePullRequestReceipt {
        operation_id,
        assignment_id: assignment_id.to_string(),
        open_operation_id: open_operation_id.to_string(),
        pull_request_url: opened.pull_request_url.clone(),
        updated_at: now,
    };
    record_pull_request_updated_command_at(root, receipt.clone())?;
    Ok(receipt)
}

pub fn merge_pull_request_at(
    root: &Path,
    assignment_id: &str,
    open_operation_id: &str,
    now: u64,
) -> Result<MergePullRequestReceipt, String> {
    let config = configured_project(root)?;
    let method = config
        .delivery
        .as_ref()
        .map_or(MergeMethod::Merge, |delivery| delivery.merge_method);
    let expected_state_epoch = workflow_state_epoch_at(root)?;
    let operation_id = format!(
        "merge-pr-{}",
        content_digest(
            format!(
                "{assignment_id}\0{}\0{open_operation_id}\0{method:?}",
                expected_state_epoch
            )
            .as_bytes()
        )
    );
    merge_pull_request_command_at(
        root,
        MergePullRequestIntent {
            operation_id: operation_id.clone(),
            assignment_id: assignment_id.to_string(),
            expected_state_epoch,
            open_operation_id: open_operation_id.to_string(),
            method,
            configuration_digest: config.digest(),
            authorized_at: now,
        },
    )?;
    let projection = workflow_projection_at(root)?;
    if let Some(receipt) = projection.pull_request_merge_receipts.get(&operation_id) {
        return Ok(receipt.clone());
    }
    let operation = projection
        .pull_request_merge_authorizations
        .get(&operation_id)
        .ok_or_else(|| "development_system.merge_pr_not_authorized".to_string())?;
    let (program, mut arguments) = match operation.provider {
        ForgeProvider::GitHub => (
            "gh",
            vec![
                "pr".to_string(),
                "merge".to_string(),
                operation.pull_request_url.clone(),
                "--repo".to_string(),
                operation.repository.clone(),
            ],
        ),
        ForgeProvider::GitLab => (
            "glab",
            vec![
                "mr".to_string(),
                "merge".to_string(),
                operation.pull_request_url.clone(),
                "--repo".to_string(),
                operation.repository.clone(),
                "--yes".to_string(),
            ],
        ),
    };
    match (operation.provider.clone(), operation.method) {
        (ForgeProvider::GitHub, MergeMethod::Merge) => arguments.push("--merge".to_string()),
        (ForgeProvider::GitHub, MergeMethod::Squash) => arguments.push("--squash".to_string()),
        (ForgeProvider::GitHub, MergeMethod::Rebase) => arguments.push("--rebase".to_string()),
        (ForgeProvider::GitLab, MergeMethod::Squash) => arguments.push("--squash".to_string()),
        (ForgeProvider::GitLab, MergeMethod::Merge) => {}
        (ForgeProvider::GitLab, MergeMethod::Rebase) => {
            unreachable!("rebase rejected before authorization")
        }
    }
    forge_stdout(root, program, &arguments)?;
    let receipt = MergePullRequestReceipt {
        operation_id,
        assignment_id: assignment_id.to_string(),
        open_operation_id: open_operation_id.to_string(),
        pull_request_url: operation.pull_request_url.clone(),
        method: operation.method,
        merged_at: now,
    };
    record_pull_request_merged_command_at(root, receipt.clone())?;
    Ok(receipt)
}

fn git_stdout(root: &Path, arguments: &[String], index: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(root);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command
        .output()
        .map_err(|error| format!("development_system.repository_git_unavailable source={error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "development_system.repository_git_failed detail={}",
            stderr.trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| "development_system.repository_git_output_invalid".to_string())
}

fn workflow_commit_tree(
    root: &Path,
    checkpoint: &Checkpoint,
    operation_id: &str,
    parent: &str,
) -> Result<String, String> {
    let temporary_index = git_common_directory(root)?
        .join("development-workflow")
        .join("tmp")
        .join(format!("{operation_id}.index"));
    if let Some(directory) = temporary_index.parent() {
        fs::create_dir_all(directory).map_err(|error| {
            format!("development_system.repository_temporary_index_failed source={error}")
        })?;
    }
    if temporary_index.exists() {
        fs::remove_file(&temporary_index).map_err(|error| {
            format!("development_system.repository_temporary_index_failed source={error}")
        })?;
    }
    let result = (|| {
        git_stdout(
            root,
            &["read-tree".to_string(), parent.to_string()],
            Some(&temporary_index),
        )?;
        for path in &checkpoint.owned_paths {
            let entry = git_stdout(
                root,
                &[
                    "ls-tree".to_string(),
                    checkpoint.index_tree.clone(),
                    "--".to_string(),
                    path.clone(),
                ],
                None,
            )?;
            if entry.is_empty() {
                git_stdout(
                    root,
                    &[
                        "update-index".to_string(),
                        "--remove".to_string(),
                        "--".to_string(),
                        path.clone(),
                    ],
                    Some(&temporary_index),
                )?;
                continue;
            }
            let (metadata, recorded_path) = entry.split_once('\t').ok_or_else(|| {
                "development_system.repository_checkpoint_entry_invalid".to_string()
            })?;
            if recorded_path != path {
                return Err("development_system.repository_checkpoint_entry_mismatch".to_string());
            }
            let mut fields = metadata.split_whitespace();
            let mode = fields.next().ok_or_else(|| {
                "development_system.repository_checkpoint_entry_invalid".to_string()
            })?;
            let kind = fields.next().ok_or_else(|| {
                "development_system.repository_checkpoint_entry_invalid".to_string()
            })?;
            let object = fields.next().ok_or_else(|| {
                "development_system.repository_checkpoint_entry_invalid".to_string()
            })?;
            if kind != "blob" || fields.next().is_some() {
                return Err("development_system.repository_checkpoint_entry_invalid".to_string());
            }
            git_stdout(
                root,
                &[
                    "update-index".to_string(),
                    "--add".to_string(),
                    "--cacheinfo".to_string(),
                    mode.to_string(),
                    object.to_string(),
                    path.clone(),
                ],
                Some(&temporary_index),
            )?;
        }
        git_stdout(root, &["write-tree".to_string()], Some(&temporary_index))
    })();
    let _ = fs::remove_file(&temporary_index);
    result
}

pub fn create_signed_commit_at(
    root: &Path,
    assignment_id: &str,
    message: &str,
    now: u64,
) -> Result<SignedCommitReceipt, String> {
    let config = match config_at(root) {
        ConfigState::Valid(config) => config,
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    if config
        .signing
        .as_ref()
        .is_none_or(|signing| !signing.commit)
    {
        return Err("development_system.commit_signing_required".to_string());
    }
    let mut lines = message.lines();
    let subject = lines.next().unwrap_or_default();
    let body = lines.collect::<Vec<_>>().join("\n");
    if subject.trim().is_empty() || body.trim().is_empty() || message.contains("Co-Authored-By:") {
        return Err("development_system.commit_message_invalid".to_string());
    }
    let projection = workflow_projection_at(root)?;
    let checkpoint_id = projection
        .last_checkpoint_id
        .ok_or_else(|| "development_system.checkpoint_required".to_string())?;
    let checkpoint = projection
        .checkpoints
        .get(&checkpoint_id)
        .cloned()
        .ok_or_else(|| "development_system.checkpoint_required".to_string())?;
    if current_index_tree(root)? != checkpoint.index_tree {
        return Err("development_system.checkpoint_stale".to_string());
    }
    let parent = git_stdout(root, &["rev-parse".to_string(), "HEAD".to_string()], None)?;
    let message_digest = content_digest(message.as_bytes());
    let operation_id = format!(
        "signed-commit-{}",
        content_digest(
            format!("{assignment_id}\0{checkpoint_id}\0{parent}\0{message_digest}").as_bytes()
        )
    );
    if let Some(receipt) = projection.signed_commit_receipts.get(&operation_id) {
        return Ok(receipt.clone());
    }
    let intent = SignedCommitIntent {
        operation_id: operation_id.clone(),
        assignment_id: assignment_id.to_string(),
        expected_state_epoch: projection.state_epoch,
        checkpoint_id: checkpoint_id.clone(),
        parent_commit: parent.clone(),
        message: message.to_string(),
        message_digest: message_digest.clone(),
        configuration_digest: config.digest(),
        authorized_at: now,
    };
    authorize_signed_commit_command_at(root, intent)?;
    let tree = workflow_commit_tree(root, &checkpoint, &operation_id, &parent)?;
    let mut command = Command::new("git");
    command
        .args(["commit-tree", &tree, "-S", "-p", &parent])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("development_system.signer_unavailable source={error}"))?;
    use std::io::Write as _;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "development_system.signer_unavailable".to_string())?
        .write_all(message.as_bytes())
        .map_err(|error| format!("development_system.signer_unavailable source={error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("development_system.signer_unavailable source={error}"))?;
    if !output.status.success() {
        return Err(format!(
            "development_system.signature_rejected detail={}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let commit = String::from_utf8(output.stdout)
        .map_err(|_| "development_system.repository_git_output_invalid".to_string())?
        .trim()
        .to_string();
    git_stdout(root, &["verify-commit".to_string(), commit.clone()], None)?;
    let receipt = SignedCommitReceipt {
        operation_id,
        assignment_id: assignment_id.to_string(),
        checkpoint_id,
        parent_commit: parent,
        tree,
        commit,
        message_digest,
        created_at: now,
    };
    record_signed_commit_command_at(root, receipt.clone())?;
    Ok(receipt)
}

pub fn create_signed_tag_at(
    root: &Path,
    assignment_id: &str,
    commit_operation_id: &str,
    tag_name: &str,
    message: &str,
    now: u64,
) -> Result<SignedTagReceipt, String> {
    let config = match config_at(root) {
        ConfigState::Valid(config) => config,
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    if config.signing.as_ref().is_none_or(|signing| !signing.tag) {
        return Err("development_system.tag_signing_required".to_string());
    }
    if tag_name.is_empty()
        || tag_name.len() > 128
        || message.trim().is_empty()
        || message.len() > 16 * 1024
    {
        return Err("development_system.signed_tag_input_invalid".to_string());
    }
    git_stdout(
        root,
        &[
            "check-ref-format".to_string(),
            format!("refs/tags/{tag_name}"),
        ],
        None,
    )
    .map_err(|_| "development_system.signed_tag_name_invalid".to_string())?;
    let projection = workflow_projection_at(root)?;
    let commit_receipt = projection
        .signed_commit_receipts
        .get(commit_operation_id)
        .ok_or_else(|| "development_system.signed_commit_receipt_unknown".to_string())?;
    let target_commit = commit_receipt.commit.clone();
    let message_digest = content_digest(message.as_bytes());
    let operation_id = format!("signed-tag-{}", content_digest(format!("{assignment_id}\0{commit_operation_id}\0{target_commit}\0{tag_name}\0{message_digest}").as_bytes()));
    if let Some(receipt) = projection.signed_tag_receipts.get(&operation_id) {
        return Ok(receipt.clone());
    }
    let intent = SignedTagIntent {
        operation_id: operation_id.clone(),
        assignment_id: assignment_id.to_string(),
        expected_state_epoch: projection.state_epoch,
        commit_operation_id: commit_operation_id.to_string(),
        target_commit: target_commit.clone(),
        tag_name: tag_name.to_string(),
        message: message.to_string(),
        message_digest: message_digest.clone(),
        configuration_digest: config.digest(),
        authorized_at: now,
    };
    authorize_signed_tag_command_at(root, intent)?;
    let reference = format!("refs/tags/{tag_name}");
    let existing = git_stdout(
        root,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            reference.clone(),
        ],
        None,
    );
    match existing {
        Ok(_) => {
            let peeled = git_stdout(
                root,
                &["rev-parse".to_string(), format!("{reference}^{{}}")],
                None,
            )?;
            if peeled != target_commit {
                return Err("development_system.signed_tag_ref_conflict".to_string());
            }
        }
        Err(_) => {
            git_stdout(
                root,
                &[
                    "tag".to_string(),
                    "-s".to_string(),
                    "--cleanup=verbatim".to_string(),
                    "-m".to_string(),
                    message.to_string(),
                    tag_name.to_string(),
                    target_commit.clone(),
                ],
                None,
            )
            .map_err(|error| format!("development_system.signature_rejected source={error}"))?;
        }
    }
    git_stdout(root, &["verify-tag".to_string(), reference.clone()], None)
        .map_err(|error| format!("development_system.signature_rejected source={error}"))?;
    let tag_object = git_stdout(root, &["rev-parse".to_string(), reference], None)?;
    let receipt = SignedTagReceipt {
        operation_id,
        assignment_id: assignment_id.to_string(),
        commit_operation_id: commit_operation_id.to_string(),
        target_commit,
        tag_name: tag_name.to_string(),
        tag_object,
        message_digest,
        created_at: now,
    };
    record_signed_tag_command_at(root, receipt.clone())?;
    Ok(receipt)
}

pub fn authorize_assignment_at(
    root: &Path,
    assignment_id: &str,
    role: Role,
    scope_id: Option<&str>,
    command_id: Option<&str>,
    now: u64,
) -> Result<Assignment, String> {
    let config = match config_at(root) {
        ConfigState::Valid(config) => config,
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    let projection = workflow_projection_at(root)?;
    let assignment = projection
        .assignments
        .get(assignment_id)
        .ok_or_else(|| "development_system.assignment_unknown".to_string())?
        .clone();
    assignment.authorize(
        role,
        workflow_state_epoch_at(root)?,
        scope_id,
        command_id,
        &config,
        now,
    )?;
    Ok(assignment)
}

fn command_receipt_at(
    root: &Path,
    receipt_id: &str,
    expected_succeeded: bool,
    now: u64,
) -> Result<CommandReceipt, String> {
    let config = match config_at(root) {
        ConfigState::Valid(config) => config,
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    let projection = workflow_projection_at(root)?;
    let receipt = projection
        .command_receipts
        .get(receipt_id)
        .ok_or_else(|| "development_system.command_receipt_unknown".to_string())?
        .clone();
    if receipt.state_epoch != workflow_state_epoch_at(root)? {
        return Err("development_system.command_receipt_stale_epoch".to_string());
    }
    if receipt.configuration_digest != config.digest() {
        return Err("development_system.command_receipt_stale_configuration".to_string());
    }
    if receipt.succeeded != expected_succeeded {
        return Err("development_system.command_receipt_outcome_invalid".to_string());
    }
    let assignment = projection
        .assignments
        .get(&receipt.assignment_id)
        .ok_or_else(|| "development_system.assignment_unknown".to_string())?;
    assignment.authorize(
        assignment.role.clone(),
        workflow_state_epoch_at(root)?,
        None,
        Some(&receipt.command_id),
        &config,
        now,
    )?;
    Ok(receipt)
}

pub fn content_digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub struct ReplaceFileRequest<'a> {
    pub assignment_id: &'a str,
    pub role: Role,
    pub scope_id: &'a str,
    pub relative: &'a Path,
    pub expected_digest: &'a str,
    pub replacement: &'a [u8],
}

pub struct DeleteFileRequest<'a> {
    pub assignment_id: &'a str,
    pub role: Role,
    pub scope_id: &'a str,
    pub relative: &'a Path,
    pub expected_digest: &'a str,
}

pub struct MoveFileRequest<'a> {
    pub assignment_id: &'a str,
    pub role: Role,
    pub scope_id: &'a str,
    pub from: &'a Path,
    pub to: &'a Path,
    pub expected_source_digest: &'a str,
    pub expected_destination_digest: &'a str,
}

pub fn replace_file_at(
    root: &Path,
    request: ReplaceFileRequest<'_>,
    now: u64,
) -> Result<String, String> {
    let assignment = authorize_assignment_at(
        root,
        request.assignment_id,
        request.role,
        Some(request.scope_id),
        None,
        now,
    )?;
    let config = match config_at(root) {
        ConfigState::Valid(config) => config,
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    if !config.scope_allows(request.scope_id, root, request.relative)? {
        return Err("development_system.editor_path_denied".to_string());
    }
    let relative = normalize_relative(request.relative)?;
    let target = root.join(&relative);
    let current = match fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(format!(
                "development_system.editor_read_failed source={error}"
            ));
        }
    };
    let relative_path = relative
        .to_str()
        .ok_or_else(|| "development_system.editor_path_denied".to_string())?
        .replace('\\', "/");
    let after_digest = content_digest(request.replacement);
    let operation_id = format!(
        "file-write-{}",
        content_digest(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                request.assignment_id,
                request.scope_id,
                relative_path,
                request.expected_digest,
                after_digest
            )
            .as_bytes()
        )
    );
    let projection = workflow_projection_at(root)?;
    let operation = projection
        .file_write_authorizations
        .get(&operation_id)
        .cloned()
        .unwrap_or(WorkspaceFileWrite {
            operation_id,
            assignment_id: request.assignment_id.to_string(),
            state_epoch: assignment.state_epoch,
            scope_id: request.scope_id.to_string(),
            path: relative_path,
            before_digest: request.expected_digest.to_string(),
            after_digest: after_digest.clone(),
            authorized_at: now,
        });
    let current_digest = content_digest(&current);
    let retrying_authorized_write = current_digest == after_digest
        && projection
            .file_write_authorizations
            .get(&operation.operation_id)
            == Some(&operation);
    if current_digest != request.expected_digest && !retrying_authorized_write {
        return Err("development_system.editor_preimage_mismatch".to_string());
    }
    authorize_file_write_command_at(root, operation.clone())?;
    let parent = target
        .parent()
        .ok_or_else(|| "development_system.editor_path_denied".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("development_system.editor_directory_failed source={error}"))?;
    let temporary = target.with_extension(format!("development-system-tmp-{}", std::process::id()));
    if !retrying_authorized_write {
        fs::write(&temporary, request.replacement)
            .map_err(|error| format!("development_system.editor_write_failed source={error}"))?;
        fs::rename(&temporary, &target)
            .map_err(|error| format!("development_system.editor_publish_failed source={error}"))?;
    }
    confirm_file_written_command_at(root, operation)?;
    Ok(after_digest)
}

pub fn create_file_at(
    root: &Path,
    request: ReplaceFileRequest<'_>,
    now: u64,
) -> Result<String, String> {
    if request.expected_digest != content_digest(b"") {
        return Err("development_system.editor_create_preimage_invalid".to_string());
    }
    let target = root.join(normalize_relative(request.relative)?);
    if target.exists() {
        return Err("development_system.editor_create_exists".to_string());
    }
    replace_file_at(root, request, now)
}

pub fn patch_file_at(
    root: &Path,
    request: ReplaceFileRequest<'_>,
    now: u64,
) -> Result<String, String> {
    let target = root.join(normalize_relative(request.relative)?);
    if !target.is_file() {
        return Err("development_system.editor_patch_missing".to_string());
    }
    replace_file_at(root, request, now)
}

pub fn delete_file_at(root: &Path, request: DeleteFileRequest<'_>, now: u64) -> Result<(), String> {
    let assignment = authorize_assignment_at(
        root,
        request.assignment_id,
        request.role,
        Some(request.scope_id),
        None,
        now,
    )?;
    let config = configured_project(root)?;
    if !config.scope_allows(request.scope_id, root, request.relative)? {
        return Err("development_system.editor_path_denied".to_string());
    }
    let relative = normalize_relative(request.relative)?;
    let relative_path = relative
        .to_str()
        .ok_or_else(|| "development_system.editor_path_denied".to_string())?
        .replace('\\', "/");
    let operation_id = format!(
        "file-delete-{}",
        content_digest(
            format!(
                "{}\0{}\0{}\0{}",
                request.assignment_id, request.scope_id, relative_path, request.expected_digest
            )
            .as_bytes()
        )
    );
    let projection = workflow_projection_at(root)?;
    let operation = projection
        .file_delete_authorizations
        .get(&operation_id)
        .cloned()
        .unwrap_or(WorkspaceFileDeletion {
            operation_id,
            assignment_id: request.assignment_id.to_string(),
            state_epoch: assignment.state_epoch,
            scope_id: request.scope_id.to_string(),
            path: relative_path,
            before_digest: request.expected_digest.to_string(),
            authorized_at: now,
        });
    let target = root.join(relative);
    match fs::read(&target) {
        Ok(bytes) => {
            if content_digest(&bytes) != request.expected_digest {
                return Err("development_system.editor_preimage_mismatch".to_string());
            }
            authorize_file_delete_command_at(root, operation.clone())?;
            fs::remove_file(target).map_err(|error| {
                format!("development_system.editor_delete_failed source={error}")
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if projection
                .file_delete_authorizations
                .get(&operation.operation_id)
                != Some(&operation)
            {
                return Err("development_system.editor_delete_missing".to_string());
            }
        }
        Err(error) => {
            return Err(format!(
                "development_system.editor_read_failed source={error}"
            ));
        }
    }
    confirm_file_deleted_command_at(root, operation)
}

pub fn move_file_at(root: &Path, request: MoveFileRequest<'_>, now: u64) -> Result<String, String> {
    let assignment = authorize_assignment_at(
        root,
        request.assignment_id,
        request.role,
        Some(request.scope_id),
        None,
        now,
    )?;
    let config = configured_project(root)?;
    if !config.scope_allows(request.scope_id, root, request.from)?
        || !config.scope_allows(request.scope_id, root, request.to)?
    {
        return Err("development_system.editor_path_denied".to_string());
    }
    let from_relative = normalize_relative(request.from)?;
    let to_relative = normalize_relative(request.to)?;
    let from_path = from_relative
        .to_str()
        .ok_or_else(|| "development_system.editor_path_denied".to_string())?
        .replace('\\', "/");
    let to_path = to_relative
        .to_str()
        .ok_or_else(|| "development_system.editor_path_denied".to_string())?
        .replace('\\', "/");
    let operation_id = format!(
        "file-move-{}",
        content_digest(
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}",
                request.assignment_id,
                request.scope_id,
                from_path,
                to_path,
                request.expected_source_digest,
                request.expected_destination_digest
            )
            .as_bytes()
        )
    );
    let projection = workflow_projection_at(root)?;
    let operation = projection
        .file_move_authorizations
        .get(&operation_id)
        .cloned()
        .unwrap_or(WorkspaceFileMove {
            operation_id,
            assignment_id: request.assignment_id.to_string(),
            state_epoch: assignment.state_epoch,
            scope_id: request.scope_id.to_string(),
            from: from_path,
            to: to_path,
            source_digest: request.expected_source_digest.to_string(),
            destination_digest: request.expected_destination_digest.to_string(),
            authorized_at: now,
        });
    let from = root.join(from_relative);
    let to = root.join(to_relative);
    let source = fs::read(&from);
    let retrying_authorized_move = source
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && fs::read(&to)
            .is_ok_and(|bytes| content_digest(&bytes) == request.expected_source_digest)
        && projection
            .file_move_authorizations
            .get(&operation.operation_id)
            == Some(&operation);
    if !retrying_authorized_move {
        let source = source.map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                "development_system.editor_move_source_missing".to_string()
            }
            _ => format!("development_system.editor_read_failed source={error}"),
        })?;
        if content_digest(&source) != request.expected_source_digest {
            return Err("development_system.editor_preimage_mismatch".to_string());
        }
        let destination = match fs::read(&to) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(format!(
                    "development_system.editor_read_failed source={error}"
                ));
            }
        };
        if content_digest(&destination) != request.expected_destination_digest {
            return Err("development_system.editor_destination_mismatch".to_string());
        }
        authorize_file_move_command_at(root, operation.clone())?;
    }
    let parent = to
        .parent()
        .ok_or_else(|| "development_system.editor_path_denied".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("development_system.editor_directory_failed source={error}"))?;
    if !retrying_authorized_move {
        fs::rename(&from, &to)
            .map_err(|error| format!("development_system.editor_move_failed source={error}"))?;
    }
    confirm_file_moved_command_at(root, operation)?;
    Ok(request.expected_source_digest.to_string())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CommandResult {
    pub evidence_id: String,
    pub command_id: String,
    pub exit_code: Option<i32>,
    pub succeeded: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_named_command_at(
    root: &Path,
    assignment_id: &str,
    role: Role,
    command_id: &str,
    parameters: &BTreeMap<String, Value>,
    now: u64,
) -> Result<CommandResult, String> {
    let assignment =
        authorize_assignment_at(root, assignment_id, role, None, Some(command_id), now)?;
    let config = match config_at(root) {
        ConfigState::Valid(config) => config,
        ConfigState::Absent => return Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => return Err(error),
    };
    let command = config
        .commands
        .get(command_id)
        .ok_or_else(|| "development_system.command_unknown".to_string())?;
    let argv = command.resolved_argv(parameters)?;
    let executable = resolve_runner_executable(&argv[0])?;
    let writable_roots = runner_writable_roots(root, &config, command)?;
    let outputs_before = runner_output_snapshot(root, &writable_roots)?;
    let runner_scratch = runner_scratch_directory()?;
    let sandbox = std::env::var_os("AI_PLUGINS_BWRAP_BIN")
        .map(PathBuf::from)
        .or_else(|| resolve_executable_on_path("bwrap"))
        .filter(|path| cfg!(target_os = "linux") && path.is_file())
        .ok_or_else(|| "development_system.runner_boundary_unavailable".to_string())?;
    let mut process = Command::new(sandbox);
    process
        .args(["--die-with-parent", "--new-session", "--ro-bind", "/", "/"])
        .args(["--dev", "/dev", "--proc", "/proc"])
        .arg("--bind")
        .arg(&runner_scratch.0)
        .arg(&runner_scratch.0);
    if command.network != Some(NetworkPolicy::Allowed) {
        process.arg("--unshare-net");
    }
    for writable in &writable_roots {
        process.arg("--bind").arg(writable).arg(writable);
    }
    process
        .arg("--chdir")
        .arg(root)
        .arg("--")
        .arg(executable)
        .args(&argv[1..])
        .current_dir(root)
        .env_clear()
        .env("HOME", &runner_scratch.0)
        .env("TMPDIR", &runner_scratch.0)
        .env("CARGO_TARGET_DIR", runner_scratch.0.join("cargo-target"))
        .env("GIT_AUTHOR_EMAIL", "runner@development-system.invalid")
        .env("GIT_AUTHOR_NAME", "Development System Runner")
        .env("GIT_COMMITTER_EMAIL", "runner@development-system.invalid")
        .env("GIT_COMMITTER_NAME", "Development System Runner")
        .env("NPM_CONFIG_CACHE", runner_scratch.0.join("npm-cache"))
        .env("NPM_CONFIG_PREFIX", runner_scratch.0.join("npm-prefix"))
        .env("XDG_CACHE_HOME", runner_scratch.0.join("cache"))
        .env("XDG_CONFIG_HOME", runner_scratch.0.join("config"))
        .env("XDG_STATE_HOME", runner_scratch.0.join("state"));
    for name in &command.environment {
        if let Some(value) = std::env::var_os(name) {
            process.env(name, value);
        }
    }
    let output = process
        .output()
        .map_err(|error| format!("development_system.runner_launch_failed source={error}"))?;
    const MAX_OUTPUT: usize = 64 * 1024;
    let bounded = |bytes: Vec<u8>| {
        String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_OUTPUT)]).into_owned()
    };
    let succeeded = output.status.success();
    let state_epoch = workflow_state_epoch_at(root)?;
    if state_epoch != assignment.state_epoch {
        return Err("development_system.assignment_stale_epoch".to_string());
    }
    let mut output_bytes = output.stdout.clone();
    output_bytes.push(0);
    output_bytes.extend_from_slice(&output.stderr);
    let output_digest = content_digest(&output_bytes);
    let outputs_after = runner_output_snapshot(root, &writable_roots)?;
    let mut observed_output_digests = BTreeMap::new();
    for path in outputs_before.keys().chain(outputs_after.keys()) {
        if outputs_before.get(path) != outputs_after.get(path) {
            observed_output_digests.insert(path.clone(), outputs_after.get(path).cloned());
        }
    }
    let output_files_digest = content_digest(
        &serde_json::to_vec(&observed_output_digests).map_err(|error| {
            format!("development_system.runner_output_evidence_encode_failed source={error}")
        })?,
    );
    let evidence_id = format!(
        "command-{}",
        content_digest(
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}",
                assignment.id,
                command_id,
                assignment.state_epoch,
                now,
                output_digest,
                output_files_digest
            )
            .as_bytes()
        )
    );
    record_command_receipt_command_at(
        root,
        ReceiptIntent {
            id: evidence_id.clone(),
            assignment_id: assignment.id,
            command_id: command_id.to_string(),
            expected_state_epoch: state_epoch,
            configuration_digest: assignment.configuration_digest,
            succeeded,
            output_digest,
            observed_output_digests,
            created_at: now,
        },
    )?;
    Ok(CommandResult {
        evidence_id,
        command_id: command_id.to_string(),
        exit_code: output.status.code(),
        succeeded,
        stdout: bounded(output.stdout),
        stderr: bounded(output.stderr),
    })
}

fn resolve_executable_on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(program))
            .find(|candidate| candidate.is_file())
    })
}

struct RunnerScratch(PathBuf);

impl Drop for RunnerScratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn runner_scratch_directory() -> Result<RunnerScratch, String> {
    let path = std::env::temp_dir().join(format!(
        "development-system-runner-{}-{}",
        std::process::id(),
        RUNNER_SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path)
        .map_err(|error| format!("development_system.runner_scratch_unavailable source={error}"))?;
    Ok(RunnerScratch(path))
}

fn resolve_runner_executable(program: &str) -> Result<PathBuf, String> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path
            .is_absolute()
            .then(|| path.to_path_buf())
            .filter(|candidate| candidate.is_file())
            .ok_or_else(|| "development_system.runner_executable_unavailable".to_string());
    }
    resolve_executable_on_path(program)
        .ok_or_else(|| "development_system.runner_executable_unavailable".to_string())
}

fn runner_writable_roots(
    root: &Path,
    config: &ProjectConfig,
    command: &ProjectCommand,
) -> Result<Vec<PathBuf>, String> {
    let mut writable = BTreeSet::new();
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        format!("development_system.runner_project_root_unavailable source={error}")
    })?;
    for scope_id in &command.output_scopes {
        let scope = config
            .scopes
            .get(scope_id)
            .ok_or_else(|| "development_system.command_output_scope_unknown".to_string())?;
        if !scope.exclude.is_empty() {
            return Err("development_system.runner_output_scope_not_mountable".to_string());
        }
        for include in &scope.include {
            let Some(prefix) = include.strip_suffix("/**") else {
                return Err("development_system.runner_output_scope_not_mountable".to_string());
            };
            if prefix.is_empty() || prefix.bytes().any(|byte| matches!(byte, b'*' | b'?')) {
                return Err("development_system.runner_output_scope_not_mountable".to_string());
            }
            let relative = normalize_relative(Path::new(prefix))?;
            let absolute = root.join(relative);
            fs::create_dir_all(&absolute).map_err(|error| {
                format!("development_system.runner_output_scope_unavailable source={error}")
            })?;
            let canonical = fs::canonicalize(&absolute).map_err(|error| {
                format!("development_system.runner_output_scope_unavailable source={error}")
            })?;
            if !canonical.starts_with(&canonical_root) {
                return Err("development_system.runner_output_scope_escape".to_string());
            }
            writable.insert(canonical);
        }
    }
    Ok(writable.into_iter().collect())
}

fn runner_output_snapshot(
    project_root: &Path,
    writable_roots: &[PathBuf],
) -> Result<BTreeMap<String, String>, String> {
    const MAX_OUTPUT_FILES: usize = 4096;
    let mut pending = writable_roots.to_vec();
    let mut files = BTreeMap::new();
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!("development_system.runner_output_inspection_failed source={error}")
        })?;
        if metadata.file_type().is_symlink() {
            return Err("development_system.runner_output_symlink_denied".to_string());
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(|error| {
                format!("development_system.runner_output_inspection_failed source={error}")
            })? {
                pending.push(
                    entry
                        .map_err(|error| {
                            format!(
                                "development_system.runner_output_inspection_failed source={error}"
                            )
                        })?
                        .path(),
                );
            }
            continue;
        }
        if !metadata.is_file() {
            return Err("development_system.runner_output_type_denied".to_string());
        }
        if files.len() >= MAX_OUTPUT_FILES {
            return Err("development_system.runner_output_file_limit".to_string());
        }
        let relative = path
            .strip_prefix(project_root)
            .map_err(|_| "development_system.runner_output_scope_escape".to_string())?
            .to_str()
            .ok_or_else(|| "development_system.runner_output_path_invalid".to_string())?
            .to_string();
        let bytes = fs::read(&path).map_err(|error| {
            format!("development_system.runner_output_inspection_failed source={error}")
        })?;
        files.insert(relative, content_digest(&bytes));
    }
    Ok(files)
}

pub fn config_at(root: &Path) -> ConfigState {
    let path = root.join(CONFIG_FILE);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ConfigState::Absent,
        Err(error) => {
            return ConfigState::Invalid(format!(
                "development_system.config_read_failed source={error}"
            ));
        }
    };
    if metadata.len() > MAX_CONFIG_BYTES {
        return ConfigState::Invalid("development_system.config_too_large".to_string());
    }
    match fs::read_to_string(path) {
        Ok(text) => {
            ProjectConfig::parse(&text).map_or_else(ConfigState::Invalid, ConfigState::Valid)
        }
        Err(error) => ConfigState::Invalid(format!(
            "development_system.config_read_failed source={error}"
        )),
    }
}

fn configured_project(root: &Path) -> Result<ProjectConfig, String> {
    match config_at(root) {
        ConfigState::Valid(config) => Ok(config),
        ConfigState::Absent => Err("development_system.configuration_required".to_string()),
        ConfigState::Invalid(error) => Err(error),
    }
}

pub fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn valid_identifier(value: &str, kind: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 80
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(format!("development_system.{kind}_identifier_invalid"));
    }
    Ok(())
}

fn validate_glob(glob: &str) -> Result<(), String> {
    if glob.is_empty() || glob.starts_with('/') || glob.contains("..") || glob.contains('\\') {
        return Err("development_system.scope_glob_invalid".to_string());
    }
    if is_protected(glob) {
        return Err("development_system.scope_protected_path".to_string());
    }
    Ok(())
}

fn normalize_relative(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Err("development_system.path_absolute_denied".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("development_system.path_escape_denied".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("development_system.path_empty_denied".to_string());
    }
    Ok(normalized)
}

fn resolve_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    let mut candidate = path;
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(candidate) {
            Ok(mut resolved) => {
                for component in suffix.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                suffix.push(
                    candidate
                        .file_name()
                        .ok_or_else(|| "development_system.path_escape_denied".to_string())?
                        .to_os_string(),
                );
                candidate = candidate
                    .parent()
                    .ok_or_else(|| "development_system.path_escape_denied".to_string())?;
            }
            Err(error) => {
                return Err(format!(
                    "development_system.path_resolve_failed source={error}"
                ));
            }
        }
    }
}

fn path_as_slashes(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn is_protected(path: &str) -> bool {
    PROTECTED_PATHS
        .iter()
        .any(|protected| path == protected.trim_end_matches('/') || path.starts_with(protected))
}

// Small glob matcher for repository-relative policy patterns. `*` never spans
// a slash; `**` does. This keeps policy deterministic without accepting a shell
// or a platform-dependent glob implementation.
fn glob_matches(pattern: &str, path: &str) -> bool {
    fn matches(pattern: &[u8], path: &[u8]) -> bool {
        match (pattern.first(), path.first()) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(b'*'), _) if pattern.get(1) == Some(&b'*') => {
                matches(&pattern[2..], path) || (!path.is_empty() && matches(pattern, &path[1..]))
            }
            (Some(b'*'), _) => {
                matches(&pattern[1..], path)
                    || (!path.is_empty() && path[0] != b'/' && matches(pattern, &path[1..]))
            }
            (Some(byte), Some(actual)) if byte == actual => matches(&pattern[1..], &path[1..]),
            _ => false,
        }
    }
    matches(pattern.as_bytes(), path.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_executable(program: &str) -> PathBuf {
        resolve_executable_on_path(program)
            .unwrap_or_else(|| panic!("test executable {program} should be available on PATH"))
    }

    #[test]
    fn complete_domain_model_has_no_unconsumed_provenance() {
        let report = eventcore::model::check().expect("complete EventCore assignment model");
        assert_eq!(report.status, eventcore::model::CheckStatus::Verified);
        assert!(
            report.warnings.is_empty(),
            "verified model still has unconsumed provenance: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn signed_delivery_authorization_facts_use_the_folded_epoch() {
        let commit = SignedCommitIntent {
            operation_id: "commit-op".to_string(),
            assignment_id: "delivery".to_string(),
            expected_state_epoch: 3,
            checkpoint_id: "checkpoint".to_string(),
            parent_commit: "parent".to_string(),
            message: "feat: signed delivery\n\nWhy this change is needed.".to_string(),
            message_digest: "message-digest".to_string(),
            configuration_digest: "configuration-digest".to_string(),
            authorized_at: 10,
        };
        let tag = SignedTagIntent {
            operation_id: "tag-op".to_string(),
            assignment_id: "delivery".to_string(),
            expected_state_epoch: 3,
            commit_operation_id: "commit-op".to_string(),
            target_commit: "commit".to_string(),
            tag_name: "v1.0.0".to_string(),
            message: "Release v1.0.0".to_string(),
            message_digest: "tag-digest".to_string(),
            configuration_digest: "configuration-digest".to_string(),
            authorized_at: 11,
        };

        let no_commit_operation = None;
        let no_commit_receipt = None;
        let no_tag_operation = None;
        let no_tag_receipt = None;
        let no_assignment = None;
        let no_checkpoint = None;
        let no_signed_commit = None;
        let false_value = false;
        assert!(matches!(
            signed_commit_authorized_fact(
                &commit,
                &9,
                &no_commit_operation,
                &no_commit_receipt,
                &no_assignment,
                &no_checkpoint,
                &false_value,
                &false_value,
            ),
            WorkflowFact::SignedCommitAuthorized { operation } if operation.state_epoch == 9
        ));
        assert!(matches!(
            signed_tag_authorized_fact(
                &tag,
                &9,
                &no_tag_operation,
                &no_tag_receipt,
                &no_assignment,
                &no_signed_commit,
                &false_value,
                &false_value,
            ),
            WorkflowFact::SignedTagAuthorized { operation } if operation.state_epoch == 9
        ));
    }

    #[test]
    fn delivery_review_fold_is_invalidated_by_a_return_to_red() {
        let mut delivering = false;
        let mut reviewed = false;
        fold_delivery_lifecycle(
            &crate::workflow::LifecycleFact::CleanReviewAccepted,
            &mut delivering,
            &mut reviewed,
        );
        fold_delivery_lifecycle(
            &crate::workflow::LifecycleFact::DeliveryAuthorized,
            &mut delivering,
            &mut reviewed,
        );
        assert!(delivering && reviewed);

        fold_delivery_lifecycle(
            &crate::workflow::LifecycleFact::ReturnedToRed,
            &mut delivering,
            &mut reviewed,
        );
        assert!(!delivering && !reviewed);
    }

    #[test]
    fn checkpoint_evidence_fold_includes_the_accepted_clean_review() {
        let prior = BTreeSet::from([
            "red-receipt".to_string(),
            "green-receipt".to_string(),
            "verification-receipt".to_string(),
        ]);
        let evidence = folded_accepted_evidence_ids(
            &WorkflowFact::Lifecycle(
                crate::workflow::LifecycleFact::CleanReviewEvidenceAccepted {
                    evidence_id: "review-state-fingerprint".to_string(),
                },
            ),
            &prior,
        );

        assert_eq!(
            evidence,
            BTreeSet::from([
                "green-receipt".to_string(),
                "red-receipt".to_string(),
                "review-state-fingerprint".to_string(),
                "verification-receipt".to_string(),
            ])
        );
    }

    fn config() -> ProjectConfig {
        ProjectConfig::parse(
            r#"schema_version = 3

[scopes.source]
category = "source"
include = ["src/**"]
exclude = ["src/generated/**"]

[scopes.tests]
category = "tests"
include = ["tests/**"]

[commands.unit]
argv = ["just", "test"]
capability = "tests"
output_scopes = ["tests"]
network = "denied"

[commands.implementation]
argv = ["just", "build"]
capability = "implementation"
"#,
        )
        .expect("valid config")
    }

    fn commit_test_baseline(root: &Path) {
        assert!(Command::new("git")
            .args(["add", "-A"])
            .current_dir(root)
            .status()
            .expect("stage test baseline")
            .success());
        assert!(Command::new("git")
            .args([
                "-c",
                "user.name=Development System Test",
                "-c",
                "user.email=development-system@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "test: baseline",
            ])
            .current_dir(root)
            .status()
            .expect("commit test baseline")
            .success());
    }

    fn bare_remote_fixture() -> (TempDir, TempDir, String) {
        let remote = TempDir::new().expect("bare remote");
        assert!(Command::new("git")
            .args(["init", "--bare", "--quiet"])
            .current_dir(remote.path())
            .status()
            .expect("create bare remote")
            .success());
        let source = TempDir::new().expect("source repository");
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(source.path())
            .status()
            .expect("initialize source")
            .success());
        fs::write(source.path().join("README.md"), "fixture\n").expect("fixture file");
        commit_test_baseline(source.path());
        assert!(Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path")
            ])
            .current_dir(source.path())
            .status()
            .expect("add remote")
            .success());
        assert!(Command::new("git")
            .args(["push", "--quiet", "origin", "HEAD:refs/heads/main"])
            .current_dir(source.path())
            .status()
            .expect("push fixture")
            .success());
        let head = git_stdout(
            source.path(),
            &["rev-parse".to_string(), "HEAD".to_string()],
            None,
        )
        .expect("fixture head");
        (source, remote, head)
    }

    fn prepare_delivery_fixture(root: &Path) -> Checkpoint {
        activate_test_workflow(root, 2);
        let digest = config().digest();
        crate::workflow::authorize_implementation_at(root).expect("authorize implementation");
        issue_assignment_at(
            root,
            Assignment {
                id: "implementation".to_string(),
                role: Role::Implementer,
                state_epoch: 3,
                scope_ids: BTreeSet::new(),
                command_ids: BTreeSet::new(),
                expires_at: u64::MAX,
                configuration_digest: digest.clone(),
            },
        )
        .expect("issue implementation assignment");
        record_command_receipt_command_at(
            root,
            ReceiptIntent {
                id: "green".to_string(),
                assignment_id: "implementation".to_string(),
                command_id: "implementation".to_string(),
                expected_state_epoch: 3,
                configuration_digest: digest.clone(),
                succeeded: true,
                output_digest: "green".to_string(),
                observed_output_digests: BTreeMap::new(),
                created_at: 3,
            },
        )
        .expect("record green receipt");
        let before_green = workflow_projection_at(root).expect("green projection");
        let mut green_evidence = before_green.accepted_evidence_ids.clone();
        green_evidence.insert("green".to_string());
        let green_checkpoint = observe_current_checkpoint_at(
            root,
            3,
            4,
            green_evidence,
            before_green.last_checkpoint_id,
        )
        .expect("green checkpoint");
        crate::workflow::accept_green_evidence_at(root, "green", green_checkpoint)
            .expect("accept green");
        crate::workflow::begin_verification_at(root).expect("begin verification");
        issue_assignment_at(
            root,
            Assignment {
                id: "verification".to_string(),
                role: Role::Verifier,
                state_epoch: 5,
                scope_ids: BTreeSet::new(),
                command_ids: BTreeSet::new(),
                expires_at: u64::MAX,
                configuration_digest: digest.clone(),
            },
        )
        .expect("issue verifier assignment");
        record_command_receipt_command_at(
            root,
            ReceiptIntent {
                id: "verification".to_string(),
                assignment_id: "verification".to_string(),
                command_id: "verification".to_string(),
                expected_state_epoch: 5,
                configuration_digest: digest.clone(),
                succeeded: true,
                output_digest: "verification".to_string(),
                observed_output_digests: BTreeMap::new(),
                created_at: 5,
            },
        )
        .expect("record verification receipt");
        crate::workflow::accept_verification_at(root, "verification").expect("accept verification");
        crate::workflow::accept_clean_review_at(root, "clean-review").expect("accept review");
        crate::workflow::authorize_delivery_at(root).expect("authorize delivery");
        issue_assignment_at(
            root,
            Assignment {
                id: "delivery".to_string(),
                role: Role::Delivery,
                state_epoch: 8,
                scope_ids: BTreeSet::new(),
                command_ids: BTreeSet::new(),
                expires_at: u64::MAX,
                configuration_digest: digest,
            },
        )
        .expect("issue delivery assignment");
        let before_delivery = workflow_projection_at(root).expect("delivery projection");
        let mut delivery_evidence = before_delivery.accepted_evidence_ids.clone();
        delivery_evidence.insert("clean-review".to_string());
        let checkpoint = observe_current_checkpoint_at(
            root,
            8,
            workflow_state_epoch_at(root).expect("delivery epoch"),
            delivery_evidence,
            before_delivery.last_checkpoint_id,
        )
        .expect("delivery checkpoint observation");
        capture_checkpoint_command_at(
            root,
            CheckpointIntent {
                id: checkpoint.id.clone(),
                expected_state_epoch: checkpoint.state_epoch,
                index_tree: checkpoint.index_tree.clone(),
                owned_paths: checkpoint.owned_paths.clone(),
                authorized_scope_ids: checkpoint.authorized_scope_ids.clone(),
                command_policy_digest: checkpoint.command_policy_digest.clone(),
                evidence_ids: checkpoint.evidence_ids.clone(),
                expected_predecessor: checkpoint.predecessor.clone(),
                created_at: checkpoint.created_at,
            },
        )
        .expect("capture delivery checkpoint");
        checkpoint
    }

    #[test]
    fn bare_remote_fixture_exercises_ref_inspection() {
        let (source, _remote, head) = bare_remote_fixture();
        let observed = remote_ref_object_at(source.path(), "origin", "refs/heads/main")
            .expect("inspect fixture ref");
        assert_eq!(observed.as_deref(), Some(head.as_str()));
    }

    #[test]
    fn public_remote_ref_operations_reject_before_any_bare_remote_mutation() {
        let (source, _remote, head) = bare_remote_fixture();
        fs::write(
            source.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("fixture configuration"),
        )
        .expect("write fixture configuration");

        let fetch = fetch_ref_at(source.path(), "delivery", "origin", "refs/heads/main", 1)
            .expect_err("unassigned delivery cannot fetch");
        assert!(fetch.contains("development_system.assignment_unknown"));
        let push = push_ref_at(
            source.path(),
            "delivery",
            "origin",
            "refs/heads/main",
            "signed-commit",
            Some(&head),
            1,
        )
        .expect_err("unassigned delivery cannot push");
        assert!(push.contains("development_system.assignment_unknown"));
        assert_eq!(
            remote_ref_object_at(source.path(), "origin", "refs/heads/main")
                .expect("inspect unchanged remote"),
            Some(head),
        );
    }

    #[test]
    fn public_remote_ref_operations_fetch_and_push_through_delivery_authority() {
        let (source, _remote, head) = bare_remote_fixture();
        fs::write(
            source.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("fixture configuration"),
        )
        .expect("write fixture configuration");
        let checkpoint = prepare_delivery_fixture(source.path());

        let fetched = fetch_ref_at(source.path(), "delivery", "origin", "refs/heads/main", 8)
            .expect("authorized semantic fetch");
        assert_eq!(fetched.object_id, head);

        let operation_id = "signed-commit-for-push".to_string();
        let message = "test: delivery source\n\nExercise semantic push.".to_string();
        let message_digest = content_digest(message.as_bytes());
        let parent = git_stdout(
            source.path(),
            &["rev-parse".to_string(), "HEAD".to_string()],
            None,
        )
        .expect("head");
        authorize_signed_commit_command_at(
            source.path(),
            SignedCommitIntent {
                operation_id: operation_id.clone(),
                assignment_id: "delivery".to_string(),
                expected_state_epoch: 8,
                checkpoint_id: checkpoint.id.clone(),
                parent_commit: parent.clone(),
                message: message.clone(),
                message_digest: message_digest.clone(),
                configuration_digest: config().digest(),
                authorized_at: 8,
            },
        )
        .expect("authorize signed source");
        record_signed_commit_command_at(
            source.path(),
            SignedCommitReceipt {
                operation_id: operation_id.clone(),
                assignment_id: "delivery".to_string(),
                checkpoint_id: checkpoint.id,
                parent_commit: parent.clone(),
                tree: current_index_tree(source.path()).expect("tree"),
                commit: parent.clone(),
                message_digest,
                created_at: 8,
            },
        )
        .expect("record signed source");
        let pushed = push_ref_at(
            source.path(),
            "delivery",
            "origin",
            "refs/heads/main",
            &operation_id,
            Some(&head),
            8,
        )
        .expect("authorized semantic push");
        assert_eq!(pushed.source_object, parent);
        assert_eq!(
            remote_ref_object_at(source.path(), "origin", "refs/heads/main")
                .expect("inspect pushed remote"),
            Some(head),
        );
    }

    #[test]
    fn pull_request_authorizations_derive_authoritative_delivery_prerequisites() {
        let (source, _remote, head) = bare_remote_fixture();
        fs::write(
            source.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("config"),
        )
        .expect("write config");
        let checkpoint = prepare_delivery_fixture(source.path());
        let signed_operation = "signed-open-pr-source".to_string();
        let parent = git_stdout(
            source.path(),
            &["rev-parse".to_string(), "HEAD".to_string()],
            None,
        )
        .expect("head");
        let message = "test: pr source\n\nExercise open PR authority.".to_string();
        authorize_signed_commit_command_at(
            source.path(),
            SignedCommitIntent {
                operation_id: signed_operation.clone(),
                assignment_id: "delivery".to_string(),
                expected_state_epoch: 8,
                checkpoint_id: checkpoint.id.clone(),
                parent_commit: parent.clone(),
                message: message.clone(),
                message_digest: content_digest(message.as_bytes()),
                configuration_digest: config().digest(),
                authorized_at: 8,
            },
        )
        .expect("authorize source");
        record_signed_commit_command_at(
            source.path(),
            SignedCommitReceipt {
                operation_id: signed_operation.clone(),
                assignment_id: "delivery".to_string(),
                checkpoint_id: checkpoint.id,
                parent_commit: parent.clone(),
                tree: current_index_tree(source.path()).expect("tree"),
                commit: parent,
                message_digest: content_digest(message.as_bytes()),
                created_at: 8,
            },
        )
        .expect("record source");
        let pushed = push_ref_at(
            source.path(),
            "delivery",
            "origin",
            "refs/heads/main",
            &signed_operation,
            Some(&head),
            8,
        )
        .expect("push");
        let intent = OpenPullRequestIntent {
            operation_id: "open-pr-authority".to_string(),
            assignment_id: "delivery".to_string(),
            expected_state_epoch: 8,
            provider: ForgeProvider::GitHub,
            repository: "owner/repository".to_string(),
            push_operation_id: pushed.operation_id.clone(),
            head_ref: "caller-must-not-control-this".to_string(),
            base_branch: "main".to_string(),
            title: "test: open PR".to_string(),
            body: "body".to_string(),
            configuration_digest: String::new(),
            authorized_at: 8,
        };
        open_pull_request_command_at(source.path(), intent).expect("authorize open PR");
        let authorization = workflow_projection_at(source.path())
            .expect("projection")
            .pull_request_open_authorizations
            .remove("open-pr-authority")
            .expect("authorization");
        assert_eq!(authorization.head_ref, "main");
        assert!(merge_pull_request_command_at(
            source.path(),
            MergePullRequestIntent {
                operation_id: "merge-pr-authority".to_string(),
                assignment_id: "delivery".to_string(),
                expected_state_epoch: 8,
                open_operation_id: "open-pr-authority".to_string(),
                method: MergeMethod::Squash,
                configuration_digest: config().digest(),
                authorized_at: 8,
            },
        )
        .is_err());
        record_pull_request_opened_command_at(
            source.path(),
            OpenPullRequestReceipt {
                operation_id: "open-pr-authority".to_string(),
                assignment_id: "delivery".to_string(),
                provider: ForgeProvider::GitHub,
                repository: "owner/repository".to_string(),
                push_operation_id: pushed.operation_id.clone(),
                pull_request_url: "https://example.invalid/owner/repository/pull/1".to_string(),
                opened_at: 8,
            },
        )
        .expect("record open receipt");
        update_pull_request_command_at(
            source.path(),
            UpdatePullRequestIntent {
                operation_id: "update-pr-authority".to_string(),
                assignment_id: "delivery".to_string(),
                expected_state_epoch: 8,
                open_operation_id: "open-pr-authority".to_string(),
                title: "updated title".to_string(),
                body: "updated body".to_string(),
                configuration_digest: config().digest(),
                authorized_at: 8,
            },
        )
        .expect("authorize update");
        let update = workflow_projection_at(source.path())
            .expect("projection")
            .pull_request_update_authorizations
            .remove("update-pr-authority")
            .expect("update authorization");
        assert_eq!(update.repository, "owner/repository");
        assert_eq!(
            update.pull_request_url,
            "https://example.invalid/owner/repository/pull/1"
        );
        record_pull_request_updated_command_at(
            source.path(),
            UpdatePullRequestReceipt {
                operation_id: "update-pr-authority".to_string(),
                assignment_id: "delivery".to_string(),
                open_operation_id: "open-pr-authority".to_string(),
                pull_request_url: "https://example.invalid/owner/repository/pull/1".to_string(),
                updated_at: 8,
            },
        )
        .expect("record update receipt");
        merge_pull_request_command_at(
            source.path(),
            MergePullRequestIntent {
                operation_id: "merge-pr-authority".to_string(),
                assignment_id: "delivery".to_string(),
                expected_state_epoch: 8,
                open_operation_id: "open-pr-authority".to_string(),
                method: MergeMethod::Squash,
                configuration_digest: config().digest(),
                authorized_at: 8,
            },
        )
        .expect("authorize merge");
        let merge = workflow_projection_at(source.path())
            .expect("projection")
            .pull_request_merge_authorizations
            .remove("merge-pr-authority")
            .expect("merge authorization");
        assert_eq!(merge.state_epoch, 8);
        assert_eq!(merge.provider, ForgeProvider::GitHub);
        assert_eq!(merge.repository, "owner/repository");
        assert_eq!(
            merge.pull_request_url,
            "https://example.invalid/owner/repository/pull/1"
        );
        assert_eq!(merge.method, MergeMethod::Squash);
    }

    #[test]
    fn checkpoint_path_restore_handles_same_file_delete_and_move_deltas() {
        let root = TempDir::new().expect("repository");
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .expect("initialize repository")
            .success());
        for (path, content) in [
            ("src/same.rs", "accepted red content\n"),
            ("src/deleted.rs", "restore deleted content\n"),
            ("src/moved.rs", "restore moved content\n"),
        ] {
            let absolute = root.path().join(path);
            fs::create_dir_all(absolute.parent().expect("parent")).expect("source directory");
            fs::write(absolute, content).expect("fixture content");
        }
        commit_test_baseline(root.path());
        let checkpoint_tree = current_index_tree(root.path()).expect("checkpoint tree");

        fs::write(
            root.path().join("src/same.rs"),
            "discarded implementation\n",
        )
        .expect("same-file implementation");
        fs::remove_file(root.path().join("src/deleted.rs")).expect("implementation deletion");
        fs::rename(
            root.path().join("src/moved.rs"),
            root.path().join("src/renamed.rs"),
        )
        .expect("implementation move");
        let affected_paths = [
            "src/deleted.rs".to_string(),
            "src/moved.rs".to_string(),
            "src/renamed.rs".to_string(),
            "src/same.rs".to_string(),
        ]
        .into_iter()
        .collect();
        let operation = CheckpointAbortOperation {
            operation_id: "checkpoint-abort-0000000000000000".to_string(),
            checkpoint_id: "red-checkpoint".to_string(),
            checkpoint_tree,
            expected_index_tree: current_index_tree(root.path()).expect("current index"),
            path_digests: current_path_digests(root.path(), &affected_paths).expect("path digests"),
            affected_paths,
            authorized_at: 1,
        };

        restore_checkpoint_paths(root.path(), &operation).expect("restore checkpoint paths");

        assert_eq!(
            fs::read_to_string(root.path().join("src/same.rs")).expect("same file"),
            "accepted red content\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("src/deleted.rs")).expect("deleted file"),
            "restore deleted content\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("src/moved.rs")).expect("moved source"),
            "restore moved content\n"
        );
        assert!(!root.path().join("src/renamed.rs").exists());
    }

    #[test]
    fn signed_commit_tree_uses_only_workflow_owned_checkpoint_entries() {
        let root = TempDir::new().expect("repository");
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .expect("initialize repository")
            .success());
        fs::create_dir_all(root.path().join("src")).expect("source directory");
        fs::write(root.path().join("src/lib.rs"), "baseline source\n").expect("source");
        fs::write(root.path().join("user.txt"), "baseline user\n").expect("user");
        commit_test_baseline(root.path());
        fs::write(root.path().join("src/lib.rs"), "approved source\n").expect("source change");
        fs::write(root.path().join("user.txt"), "unrelated staged user\n").expect("user change");
        assert!(Command::new("git")
            .args(["add", "--", "src/lib.rs", "user.txt"])
            .current_dir(root.path())
            .status()
            .expect("stage fixture")
            .success());
        let index_before = current_index_tree(root.path()).expect("index tree");
        let parent = git_stdout(
            root.path(),
            &["rev-parse".to_string(), "HEAD".to_string()],
            None,
        )
        .expect("parent");
        let checkpoint = Checkpoint {
            id: "checkpoint-green".to_string(),
            state_epoch: 1,
            index_tree: index_before.clone(),
            owned_paths: ["src/lib.rs".to_string()].into_iter().collect(),
            authorized_scope_ids: BTreeSet::new(),
            command_policy_digest: "policy".to_string(),
            evidence_ids: BTreeSet::new(),
            predecessor: None,
            created_at: 1,
        };
        let tree = workflow_commit_tree(root.path(), &checkpoint, "signed-commit-test", &parent)
            .expect("workflow tree");
        assert_eq!(
            git_stdout(
                root.path(),
                &["show".to_string(), format!("{tree}:src/lib.rs")],
                None
            )
            .expect("workflow content"),
            "approved source"
        );
        assert_eq!(
            git_stdout(
                root.path(),
                &["show".to_string(), format!("{tree}:user.txt")],
                None
            )
            .expect("parent user content"),
            "baseline user"
        );
        assert_eq!(
            current_index_tree(root.path()).expect("preserved index"),
            index_before
        );
    }

    fn activate_test_workflow(root: &Path, epoch: u64) {
        if !root.join(CONFIG_FILE).exists() {
            fs::write(
                root.join(CONFIG_FILE),
                r#"schema_version = 3
[scopes.source]
category = "source"
include = ["src/**"]
[scopes.tests]
category = "tests"
include = ["tests/**"]
[commands.unit]
argv = ["just", "test"]
capability = "tests"
output_scopes = ["tests"]
network = "denied"
[commands.implementation]
argv = ["just", "build"]
capability = "implementation"
"#,
            )
            .expect("write fixture semantic configuration");
        }
        crate::workflow::start_at(root, crate::workflow::ChangeKind::Production)
            .expect("start workflow");
        if epoch >= 2 {
            let configuration_digest = config().digest();
            issue_assignment_at(
                root,
                Assignment {
                    id: "workflow-fixture".to_string(),
                    role: Role::TestAuthor,
                    state_epoch: 1,
                    scope_ids: ["tests".to_string()].into_iter().collect(),
                    command_ids: ["unit".to_string()].into_iter().collect(),
                    expires_at: u64::MAX,
                    configuration_digest: configuration_digest.clone(),
                },
            )
            .expect("issue fixture assignment");
            record_command_receipt_command_at(
                root,
                ReceiptIntent {
                    id: "workflow-fixture-red".to_string(),
                    assignment_id: "workflow-fixture".to_string(),
                    command_id: "unit".to_string(),
                    expected_state_epoch: 1,
                    configuration_digest,
                    succeeded: false,
                    output_digest: "fixture".to_string(),
                    observed_output_digests: BTreeMap::new(),
                    created_at: 0,
                },
            )
            .expect("record fixture RED receipt");
            let checkpoint = observe_current_checkpoint_at(
                root,
                0,
                2,
                ["workflow-fixture-red".to_string()].into_iter().collect(),
                None,
            )
            .expect("observe fixture RED checkpoint");
            crate::workflow::accept_red_evidence_at(root, "workflow-fixture-red", checkpoint)
                .expect("record red");
        }
        if epoch >= 3 {
            crate::workflow::authorize_implementation_at(root).expect("authorize implementation");
        }
        assert_eq!(crate::workflow::state_epoch_at(root).expect("epoch"), epoch);
    }

    #[test]
    fn config_rejects_shell_git_protected_and_escape_surfaces() {
        for text in [
            "schema_version=3\n[scopes.source]\ncategory='source'\ninclude=['../src/**']",
            "schema_version=3\n[scopes.source]\ncategory='source'\ninclude=['.git/**']",
            "schema_version=3\n[scopes.source]\ncategory='source'\ninclude=['src/**']\n[commands.x]\nargv=['bash','-c','x']\ncapability='tests'",
            "schema_version=3\n[scopes.source]\ncategory='source'\ninclude=['src/**']\n[commands.x]\nargv=['git','status']\ncapability='tests'",
        ] {
            assert!(ProjectConfig::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn scope_containment_rejects_symlink_and_protected_paths() {
        let root = TempDir::new().expect("root");
        fs::create_dir_all(root.path().join("src")).expect("src");
        let outside = TempDir::new().expect("outside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("src/link")).expect("link");
        let config = config();
        assert!(config
            .scope_allows("source", root.path(), Path::new("src/lib.rs"))
            .expect("allow"));
        assert!(!config
            .scope_allows("source", root.path(), Path::new(".development-system.toml"))
            .expect("protected"));
        assert!(config
            .scope_allows("source", root.path(), Path::new("../outside"))
            .is_err());
        #[cfg(unix)]
        assert!(!config
            .scope_allows("source", root.path(), Path::new("src/link/file"))
            .expect("symlink denied"));
    }

    #[test]
    fn conventional_nested_scopes_cover_plugin_source_and_tests_without_broad_write_access() {
        let root = TempDir::new().expect("root");
        let config = ProjectConfig::parse(
            r#"schema_version = 3
[scopes.source]
category = "source"
include = ["src/**", "**/src/**"]
[scopes.tests]
category = "tests"
include = ["tests/**", "**/tests/**"]
"#,
        )
        .expect("config");
        assert!(config
            .scope_allows(
                "source",
                root.path(),
                Path::new("plugins/example/rust/src/lib.rs"),
            )
            .expect("nested source"));
        assert!(config
            .scope_allows(
                "tests",
                root.path(),
                Path::new("plugins/example/rust/tests/behavior.rs"),
            )
            .expect("nested tests"));
        assert!(!config
            .scope_allows(
                "source",
                root.path(),
                Path::new("plugins/example/rust/tests/behavior.rs"),
            )
            .expect("source excludes tests"));
    }

    #[test]
    fn assignment_fences_role_epoch_scope_command_expiry_and_config() {
        let config = config();
        let assignment = Assignment {
            id: "a".to_string(),
            role: Role::Implementer,
            state_epoch: 7,
            scope_ids: ["source".to_string()].into_iter().collect(),
            command_ids: ["implementation".to_string()].into_iter().collect(),
            expires_at: 100,
            configuration_digest: config.digest(),
        };
        assert!(assignment
            .authorize(
                Role::Implementer,
                7,
                Some("source"),
                Some("implementation"),
                &config,
                99
            )
            .is_ok());
        assert!(assignment
            .authorize(
                Role::TestAuthor,
                7,
                Some("source"),
                Some("implementation"),
                &config,
                99
            )
            .is_err());
        assert!(assignment
            .authorize(Role::Implementer, 8, Some("source"), None, &config, 99)
            .is_err());
        assert!(assignment
            .authorize(Role::Implementer, 7, Some("tests"), None, &config, 99)
            .is_err());
        assert!(assignment
            .authorize(Role::Implementer, 7, None, None, &config, 100)
            .is_err());
    }

    #[test]
    fn workflow_events_persist_assignments_and_reject_stale_or_reused_assignments() {
        let directory = TempDir::new().expect("repository");
        Command::new("git")
            .args(["init", "--quiet", directory.path().to_str().expect("path")])
            .status()
            .expect("git init");
        let config = config();
        activate_test_workflow(directory.path(), 3);
        let assignment = Assignment {
            id: "assignment-1".to_string(),
            role: Role::TestAuthor,
            state_epoch: 3,
            scope_ids: ["tests".to_string()].into_iter().collect(),
            command_ids: BTreeSet::new(),
            expires_at: 100,
            configuration_digest: config.digest(),
        };
        issue_assignment_at(directory.path(), assignment.clone()).expect("issue");
        assert!(issue_assignment_at(directory.path(), assignment).is_err());
        let workflow_ref = Command::new("git")
            .args(["rev-parse", "--verify", "refs/heads/development-workflow"])
            .current_dir(directory.path())
            .output()
            .expect("inspect workflow authority");
        assert!(
            workflow_ref.status.success(),
            "the checked workflow command state must publish through its own Git authority"
        );
        assert!(!directory
            .path()
            .join(".git/development-system/workflow-events.sqlite")
            .exists());
        assert_eq!(workflow_state_epoch_at(directory.path()).expect("epoch"), 3);
        assert_eq!(
            workflow_projection_at(directory.path())
                .expect("projection")
                .assignments
                .len(),
            2
        );
    }

    #[test]
    fn durable_assignment_lookup_revalidates_current_configuration_and_epoch() {
        let root = TempDir::new().expect("repository");
        Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("config"),
        )
        .expect("write config");
        activate_test_workflow(root.path(), 2);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "source-edit".to_string(),
                role: Role::Implementer,
                state_epoch: 2,
                scope_ids: ["source".to_string()].into_iter().collect(),
                command_ids: BTreeSet::new(),
                expires_at: 10,
                configuration_digest: config().digest(),
            },
        )
        .expect("issue");
        assert!(authorize_assignment_at(
            root.path(),
            "source-edit",
            Role::Implementer,
            Some("source"),
            None,
            9
        )
        .is_ok());
        assert!(authorize_assignment_at(
            root.path(),
            "source-edit",
            Role::TestAuthor,
            Some("source"),
            None,
            9
        )
        .is_err());
        crate::workflow::authorize_implementation_at(root.path()).expect("advance");
        assert!(authorize_assignment_at(
            root.path(),
            "source-edit",
            Role::Implementer,
            Some("source"),
            None,
            9
        )
        .is_err());
    }

    #[test]
    fn editor_replacement_requires_current_assignment_scope_and_preimage() {
        let root = TempDir::new().expect("repository");
        Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        fs::create_dir_all(root.path().join("src")).expect("source");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("config"),
        )
        .expect("write config");
        fs::write(root.path().join("src/lib.rs"), "before").expect("source");
        commit_test_baseline(root.path());
        activate_test_workflow(root.path(), 2);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "edit".to_string(),
                role: Role::Implementer,
                state_epoch: 2,
                scope_ids: ["source".to_string()].into_iter().collect(),
                command_ids: BTreeSet::new(),
                expires_at: 10,
                configuration_digest: config().digest(),
            },
        )
        .expect("issue");
        let old = content_digest(b"before");
        assert_eq!(
            replace_file_at(
                root.path(),
                ReplaceFileRequest {
                    assignment_id: "edit",
                    role: Role::Implementer,
                    scope_id: "source",
                    relative: Path::new("src/lib.rs"),
                    expected_digest: &old,
                    replacement: b"after",
                },
                9
            )
            .expect("edit"),
            content_digest(b"after")
        );
        assert_eq!(
            fs::read(root.path().join("src/lib.rs")).expect("read"),
            b"after"
        );
        assert_eq!(
            replace_file_at(
                root.path(),
                ReplaceFileRequest {
                    assignment_id: "edit",
                    role: Role::Implementer,
                    scope_id: "source",
                    relative: Path::new("src/lib.rs"),
                    expected_digest: &old,
                    replacement: b"after",
                },
                9,
            )
            .expect("idempotent interrupted-write retry"),
            content_digest(b"after")
        );
        let projection = workflow_projection_at(root.path()).expect("mutation projection");
        assert_eq!(projection.file_write_authorizations.len(), 1);
        assert_eq!(projection.completed_file_write_ids.len(), 1);
        assert_eq!(
            projection.workflow_owned_paths,
            ["src/lib.rs".to_string()].into_iter().collect()
        );
        assert!(replace_file_at(
            root.path(),
            ReplaceFileRequest {
                assignment_id: "edit",
                role: Role::Implementer,
                scope_id: "source",
                relative: Path::new("src/lib.rs"),
                expected_digest: &old,
                replacement: b"again",
            },
            9
        )
        .is_err());
        assert!(replace_file_at(
            root.path(),
            ReplaceFileRequest {
                assignment_id: "edit",
                role: Role::Implementer,
                scope_id: "source",
                relative: Path::new(".development-system.toml"),
                expected_digest: &content_digest(
                    &fs::read(root.path().join(CONFIG_FILE)).expect("config")
                ),
                replacement: b"bad",
            },
            9
        )
        .is_err());
    }

    #[test]
    fn editor_rejects_a_path_dirty_when_the_workflow_started() {
        let root = TempDir::new().expect("repository");
        assert!(Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init")
            .success());
        fs::create_dir_all(root.path().join("src")).expect("source directory");
        fs::write(root.path().join("src/user.rs"), b"user work").expect("user change");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("config"),
        )
        .expect("write config");
        activate_test_workflow(root.path(), 2);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "editor".to_string(),
                role: Role::Implementer,
                state_epoch: 2,
                scope_ids: ["source".to_string()].into_iter().collect(),
                command_ids: BTreeSet::new(),
                expires_at: 10,
                configuration_digest: config().digest(),
            },
        )
        .expect("assignment");
        let error = replace_file_at(
            root.path(),
            ReplaceFileRequest {
                assignment_id: "editor",
                role: Role::Implementer,
                scope_id: "source",
                relative: Path::new("src/user.rs"),
                expected_digest: &content_digest(b"user work"),
                replacement: b"workflow overwrite",
            },
            9,
        )
        .expect_err("initial user change must remain untouched");
        assert!(error.contains("file_write_overlaps_initial_user_change=true"));
        assert_eq!(
            fs::read(root.path().join("src/user.rs")).expect("preserved user change"),
            b"user work"
        );
        assert!(workflow_projection_at(root.path())
            .expect("projection")
            .file_write_authorizations
            .is_empty());
    }

    #[test]
    fn editor_moves_and_deletes_only_hash_checked_assigned_paths() {
        let root = TempDir::new().expect("repository");
        Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        fs::create_dir_all(root.path().join("src")).expect("source directory");
        fs::write(root.path().join("src/old.rs"), b"old").expect("source file");
        let config = config();
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config).expect("config"),
        )
        .expect("write config");
        commit_test_baseline(root.path());
        activate_test_workflow(root.path(), 2);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "editor".to_string(),
                role: Role::Implementer,
                state_epoch: 2,
                scope_ids: ["source".to_string()].into_iter().collect(),
                command_ids: BTreeSet::new(),
                expires_at: 10,
                configuration_digest: config.digest(),
            },
        )
        .expect("assignment");
        let empty = content_digest(b"");
        assert_eq!(
            patch_file_at(
                root.path(),
                ReplaceFileRequest {
                    assignment_id: "editor",
                    role: Role::Implementer,
                    scope_id: "source",
                    relative: Path::new("src/old.rs"),
                    expected_digest: &content_digest(b"old"),
                    replacement: b"patched",
                },
                9,
            )
            .expect("patch"),
            content_digest(b"patched")
        );
        assert_eq!(
            move_file_at(
                root.path(),
                MoveFileRequest {
                    assignment_id: "editor",
                    role: Role::Implementer,
                    scope_id: "source",
                    from: Path::new("src/old.rs"),
                    to: Path::new("src/new.rs"),
                    expected_source_digest: &content_digest(b"patched"),
                    expected_destination_digest: &empty,
                },
                9,
            )
            .expect("move"),
            content_digest(b"patched")
        );
        assert!(!root.path().join("src/old.rs").exists());
        assert_eq!(
            fs::read(root.path().join("src/new.rs")).expect("moved"),
            b"patched"
        );
        move_file_at(
            root.path(),
            MoveFileRequest {
                assignment_id: "editor",
                role: Role::Implementer,
                scope_id: "source",
                from: Path::new("src/old.rs"),
                to: Path::new("src/new.rs"),
                expected_source_digest: &content_digest(b"patched"),
                expected_destination_digest: &empty,
            },
            9,
        )
        .expect("idempotent interrupted-move retry");
        delete_file_at(
            root.path(),
            DeleteFileRequest {
                assignment_id: "editor",
                role: Role::Implementer,
                scope_id: "source",
                relative: Path::new("src/new.rs"),
                expected_digest: &content_digest(b"patched"),
            },
            9,
        )
        .expect("delete");
        assert!(!root.path().join("src/new.rs").exists());
        delete_file_at(
            root.path(),
            DeleteFileRequest {
                assignment_id: "editor",
                role: Role::Implementer,
                scope_id: "source",
                relative: Path::new("src/new.rs"),
                expected_digest: &content_digest(b"patched"),
            },
            9,
        )
        .expect("idempotent interrupted-delete retry");
        create_file_at(
            root.path(),
            ReplaceFileRequest {
                assignment_id: "editor",
                role: Role::Implementer,
                scope_id: "source",
                relative: Path::new("src/fresh.rs"),
                expected_digest: &empty,
                replacement: b"fresh",
            },
            9,
        )
        .expect("create");
        assert_eq!(
            fs::read(root.path().join("src/fresh.rs")).expect("fresh"),
            b"fresh"
        );
        let projection = workflow_projection_at(root.path()).expect("mutation projection");
        assert_eq!(projection.file_move_authorizations.len(), 1);
        assert_eq!(projection.completed_file_move_ids.len(), 1);
        assert_eq!(projection.file_delete_authorizations.len(), 1);
        assert_eq!(projection.completed_file_delete_ids.len(), 1);
        assert_eq!(
            projection.workflow_owned_paths,
            [
                "src/fresh.rs".to_string(),
                "src/new.rs".to_string(),
                "src/old.rs".to_string(),
            ]
            .into_iter()
            .collect()
        );
        stage_paths_owned_by_role(root.path(), Role::Implementer)
            .expect("stage move/delete/create delta");
        let staged = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(root.path())
            .output()
            .expect("inspect staged mutation paths");
        assert_eq!(
            String::from_utf8(staged.stdout).expect("staged paths"),
            "src/fresh.rs\nsrc/old.rs\n"
        );
    }

    #[test]
    fn runner_executes_only_assigned_named_argv() {
        let root = TempDir::new().expect("repository");
        Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        let false_executable = test_executable("false");
        let config = ProjectConfig::parse(&format!(
            r#"schema_version=3
[scopes.tests]
category="tests"
include=["tests/**"]
[commands.probe]
argv=["{}"]
capability="tests"
"#,
            false_executable.display()
        ))
        .expect("config");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config).expect("config"),
        )
        .expect("write");
        activate_test_workflow(root.path(), 1);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "runner".to_string(),
                role: Role::TestAuthor,
                state_epoch: 1,
                scope_ids: BTreeSet::new(),
                command_ids: ["probe".to_string()].into_iter().collect(),
                expires_at: 10,
                configuration_digest: config.digest(),
            },
        )
        .expect("issue");
        let result = run_named_command_at(
            root.path(),
            "runner",
            Role::TestAuthor,
            "probe",
            &BTreeMap::new(),
            9,
        )
        .expect("run");
        assert!(!result.succeeded);
        assert_eq!(result.exit_code, Some(1));
        let receipt = command_receipt_at(root.path(), &result.evidence_id, false, 9)
            .expect("durable failed-command evidence");
        assert_eq!(receipt.command_id, "probe");
        assert_eq!(receipt.state_epoch, 1);
        assert!(!receipt.output_digest.is_empty());
        assert!(run_named_command_at(
            root.path(),
            "runner",
            Role::TestAuthor,
            "unknown",
            &BTreeMap::new(),
            9,
        )
        .is_err());
    }

    #[test]
    fn runner_isolates_home_and_xdg_state_in_its_writable_scratch() {
        let root = TempDir::new().expect("repository");
        Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        let config = ProjectConfig::parse(
            r#"schema_version=3
[scopes.tests]
category="tests"
include=["tests/**"]
[commands.environment]
argv=["/usr/bin/env"]
capability="tests"
"#,
        )
        .expect("config");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config).expect("config"),
        )
        .expect("write");
        activate_test_workflow(root.path(), 1);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "runner-environment".to_string(),
                role: Role::TestAuthor,
                state_epoch: 1,
                scope_ids: BTreeSet::new(),
                command_ids: ["environment".to_string()].into_iter().collect(),
                expires_at: 10,
                configuration_digest: config.digest(),
            },
        )
        .expect("issue");

        let result = run_named_command_at(
            root.path(),
            "runner-environment",
            Role::TestAuthor,
            "environment",
            &BTreeMap::new(),
            9,
        )
        .expect("run");

        assert!(result.succeeded, "{}", result.stderr);
        let values = result
            .stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect::<BTreeMap<_, _>>();
        let home = values.get("HOME").expect("isolated HOME");
        assert_eq!(values.get("TMPDIR"), Some(home));
        assert_eq!(
            values.get("CARGO_TARGET_DIR"),
            Some(&format!("{home}/cargo-target").as_str())
        );
        assert_eq!(
            values.get("NPM_CONFIG_CACHE"),
            Some(&format!("{home}/npm-cache").as_str())
        );
        assert_eq!(
            values.get("NPM_CONFIG_PREFIX"),
            Some(&format!("{home}/npm-prefix").as_str())
        );
        assert_eq!(
            values.get("GIT_AUTHOR_NAME"),
            Some(&"Development System Runner")
        );
        assert_eq!(
            values.get("GIT_AUTHOR_EMAIL"),
            Some(&"runner@development-system.invalid")
        );
        assert_eq!(
            values.get("GIT_COMMITTER_NAME"),
            Some(&"Development System Runner")
        );
        assert_eq!(
            values.get("GIT_COMMITTER_EMAIL"),
            Some(&"runner@development-system.invalid")
        );
        assert_eq!(
            values.get("XDG_CACHE_HOME"),
            Some(&format!("{home}/cache").as_str())
        );
        assert_eq!(
            values.get("XDG_CONFIG_HOME"),
            Some(&format!("{home}/config").as_str())
        );
        assert_eq!(
            values.get("XDG_STATE_HOME"),
            Some(&format!("{home}/state").as_str())
        );
        assert_ne!(*home, std::env::var("HOME").unwrap_or_default());
    }

    #[test]
    fn runner_accepts_only_declared_typed_whole_argument_parameters() {
        let root = TempDir::new().expect("repository");
        Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        let echo_executable = test_executable("echo");
        let config = ProjectConfig::parse(&format!(
            r#"schema_version=3
[scopes.tests]
category="tests"
include=["tests/**"]
[commands.probe]
argv=["{}", "{{label}}", "{{count}}", "{{enabled}}"]
capability="tests"
[commands.probe.parameters]
label="string"
count="integer"
enabled="boolean"
"#,
            echo_executable.display()
        ))
        .expect("config");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config).expect("config"),
        )
        .expect("write");
        activate_test_workflow(root.path(), 1);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "runner".to_string(),
                role: Role::TestAuthor,
                state_epoch: 1,
                scope_ids: BTreeSet::new(),
                command_ids: ["probe".to_string()].into_iter().collect(),
                expires_at: 10,
                configuration_digest: config.digest(),
            },
        )
        .expect("assignment");
        let parameters = BTreeMap::from([
            ("label".to_string(), serde_json::json!("typed")),
            ("count".to_string(), serde_json::json!(7)),
            ("enabled".to_string(), serde_json::json!(true)),
        ]);
        let result = run_named_command_at(
            root.path(),
            "runner",
            Role::TestAuthor,
            "probe",
            &parameters,
            9,
        )
        .expect("run");
        assert!(result.succeeded);
        assert_eq!(result.stdout, "typed 7 true\n");
        assert!(run_named_command_at(
            root.path(),
            "runner",
            Role::TestAuthor,
            "probe",
            &BTreeMap::from([("label".to_string(), serde_json::json!("missing"))]),
            9,
        )
        .is_err());
        assert!(ProjectConfig::parse(&format!(
            r#"schema_version=3
[scopes.tests]
category="tests"
include=["tests/**"]
[commands.invalid]
argv=["{}", "prefix-{{value}}"]
capability="tests"
[commands.invalid.parameters]
value="string"
"#,
            echo_executable.display()
        ))
        .is_err());
    }

    #[test]
    fn runner_os_boundary_allows_only_declared_output_scope() {
        let root = TempDir::new().expect("repository");
        assert!(Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init")
            .success());
        let touch_executable = test_executable("touch");
        let config = ProjectConfig::parse(&format!(
            r#"schema_version=3
[scopes.tests]
category="tests"
include=["tests/**"]
[scopes.source]
category="source"
include=["src/**"]
[commands.touch]
argv=["{}", "{{path}}"]
capability="tests"
output_scopes=["tests"]
network="denied"
[commands.touch.parameters]
path="string"
"#,
            touch_executable.display()
        ))
        .expect("config");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config).expect("config"),
        )
        .expect("write config");
        activate_test_workflow(root.path(), 1);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "bounded-runner".to_string(),
                role: Role::TestAuthor,
                state_epoch: 1,
                scope_ids: BTreeSet::new(),
                command_ids: ["touch".to_string()].into_iter().collect(),
                expires_at: 10,
                configuration_digest: config.digest(),
            },
        )
        .expect("assignment");
        let allowed = root.path().join("tests/allowed.snapshot");
        let allowed_result = run_named_command_at(
            root.path(),
            "bounded-runner",
            Role::TestAuthor,
            "touch",
            &BTreeMap::from([(
                "path".to_string(),
                serde_json::json!(allowed.to_string_lossy()),
            )]),
            8,
        )
        .expect("run allowed output");
        assert!(allowed_result.succeeded, "{allowed_result:?}");
        assert!(allowed.exists());
        let receipt = command_receipt_at(root.path(), &allowed_result.evidence_id, true, 8)
            .expect("output-bound command receipt");
        assert_eq!(
            receipt.observed_output_digests,
            BTreeMap::from([(
                "tests/allowed.snapshot".to_string(),
                Some(content_digest(b"")),
            )])
        );

        let denied = root.path().join("src/denied.rs");
        fs::create_dir_all(denied.parent().expect("source parent")).expect("source directory");
        let denied_result = run_named_command_at(
            root.path(),
            "bounded-runner",
            Role::TestAuthor,
            "touch",
            &BTreeMap::from([(
                "path".to_string(),
                serde_json::json!(denied.to_string_lossy()),
            )]),
            9,
        )
        .expect("record denied command result");
        assert!(!denied_result.succeeded);
        assert!(!denied.exists());
    }

    #[test]
    fn runner_network_namespace_follows_declared_policy() {
        let root = TempDir::new().expect("repository");
        assert!(Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init")
            .success());
        let readlink_executable = test_executable("readlink");
        let config = ProjectConfig::parse(&format!(
            r#"schema_version=3
[scopes.tests]
category="tests"
include=["tests/**"]
[commands.denied]
argv=["{}", "/proc/self/ns/net"]
capability="tests"
network="denied"
[commands.allowed]
argv=["{}", "/proc/self/ns/net"]
capability="tests"
network="allowed"
"#,
            readlink_executable.display(),
            readlink_executable.display()
        ))
        .expect("config");
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config).expect("config"),
        )
        .expect("write config");
        activate_test_workflow(root.path(), 1);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "network-runner".to_string(),
                role: Role::TestAuthor,
                state_epoch: 1,
                scope_ids: BTreeSet::new(),
                command_ids: ["allowed".to_string(), "denied".to_string()]
                    .into_iter()
                    .collect(),
                expires_at: 10,
                configuration_digest: config.digest(),
            },
        )
        .expect("assignment");
        let denied = run_named_command_at(
            root.path(),
            "network-runner",
            Role::TestAuthor,
            "denied",
            &BTreeMap::new(),
            8,
        )
        .expect("denied-network receipt");
        assert!(denied.succeeded, "{}", denied.stderr);
        let allowed = run_named_command_at(
            root.path(),
            "network-runner",
            Role::TestAuthor,
            "allowed",
            &BTreeMap::new(),
            9,
        )
        .expect("allowed-network receipt");
        assert!(allowed.succeeded, "{}", allowed.stderr);
        let parent_namespace = fs::read_link("/proc/self/ns/net")
            .expect("parent network namespace")
            .to_string_lossy()
            .into_owned();
        assert_eq!(allowed.stdout.trim(), parent_namespace);
        assert_ne!(denied.stdout.trim(), parent_namespace);
    }

    #[test]
    fn checkpoint_captures_the_index_tree_under_a_current_delivery_assignment() {
        let root = TempDir::new().expect("repository");
        let status = Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        assert!(status.success());
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("config"),
        )
        .expect("write config");
        assert!(Command::new("git")
            .args(["add", "--", CONFIG_FILE])
            .current_dir(root.path())
            .status()
            .expect("stage baseline config")
            .success());
        assert!(Command::new("git")
            .args([
                "-c",
                "user.name=Development System Test",
                "-c",
                "user.email=development-system@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "test: baseline",
            ])
            .current_dir(root.path())
            .status()
            .expect("commit baseline")
            .success());
        activate_test_workflow(root.path(), 2);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "implementation".to_string(),
                role: Role::Implementer,
                state_epoch: 2,
                scope_ids: ["source".to_string()].into_iter().collect(),
                command_ids: BTreeSet::new(),
                expires_at: 10,
                configuration_digest: config().digest(),
            },
        )
        .expect("issue implementation assignment");
        replace_file_at(
            root.path(),
            ReplaceFileRequest {
                assignment_id: "implementation",
                role: Role::Implementer,
                scope_id: "source",
                relative: Path::new("src/new.rs"),
                expected_digest: &content_digest(b""),
                replacement: b"pub fn new() {}\n",
            },
            9,
        )
        .expect("workflow-owned source write");
        assert!(Command::new("git")
            .args(["add", "--", "src/new.rs"])
            .current_dir(root.path())
            .status()
            .expect("stage workflow file")
            .success());
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "delivery".to_string(),
                role: Role::Delivery,
                state_epoch: 2,
                scope_ids: BTreeSet::new(),
                command_ids: BTreeSet::new(),
                expires_at: 10,
                configuration_digest: config().digest(),
            },
        )
        .expect("issue");

        let checkpoint =
            capture_checkpoint_at(root.path(), "delivery", Role::Delivery, 9).expect("checkpoint");
        assert!(checkpoint.id.starts_with("checkpoint-9-"));
        assert!(matches!(checkpoint.index_tree.len(), 40 | 64));
        assert_eq!(
            checkpoint.owned_paths,
            ["src/new.rs".to_string()].into_iter().collect()
        );
        assert_eq!(
            checkpoint.authorized_scope_ids,
            ["source".to_string()].into_iter().collect()
        );
        assert_eq!(
            checkpoint.evidence_ids,
            ["workflow-fixture-red".to_string()].into_iter().collect()
        );
        assert_eq!(
            workflow_projection_at(root.path())
                .expect("projection")
                .checkpoints
                .get(&checkpoint.id),
            Some(&checkpoint)
        );
    }

    #[test]
    fn checkpoint_lineage_follows_capture_order_after_double_digit_identifiers() {
        let root = TempDir::new().expect("repository");
        let status = Command::new("git")
            .args(["init", "--quiet", root.path().to_str().expect("path")])
            .status()
            .expect("git init");
        assert!(status.success());
        fs::write(
            root.path().join(CONFIG_FILE),
            toml::to_string(&config()).expect("config"),
        )
        .expect("write config");
        activate_test_workflow(root.path(), 1);
        issue_assignment_at(
            root.path(),
            Assignment {
                id: "delivery".to_string(),
                role: Role::Delivery,
                state_epoch: 1,
                scope_ids: BTreeSet::new(),
                command_ids: BTreeSet::new(),
                expires_at: 100,
                configuration_digest: config().digest(),
            },
        )
        .expect("issue");

        let mut previous = None;
        let mut final_predecessor = None;
        let mut checkpoint = None;
        for now in 1..=11 {
            let captured = capture_checkpoint_at(root.path(), "delivery", Role::Delivery, now)
                .expect("checkpoint");
            assert_eq!(captured.predecessor, previous);
            final_predecessor = previous;
            previous = Some(captured.id.clone());
            checkpoint = Some(captured);
        }

        let checkpoint = checkpoint.expect("eleventh checkpoint");
        assert!(checkpoint.id.starts_with("checkpoint-11-"));
        assert_eq!(checkpoint.predecessor, final_predecessor);
    }

    #[test]
    fn remote_inspection_uses_a_named_remote_without_exposing_its_url() {
        let root = TempDir::new().expect("repository");
        let remote = TempDir::new().expect("remote");
        for (path, arguments) in [
            (root.path(), vec!["init", "--quiet"]),
            (remote.path(), vec!["init", "--bare", "--quiet"]),
        ] {
            let status = Command::new("git")
                .args(arguments)
                .current_dir(path)
                .status()
                .expect("git init");
            assert!(status.success());
        }
        let status = Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path"),
            ])
            .current_dir(root.path())
            .status()
            .expect("remote add");
        assert!(status.success());

        let inspected = inspect_remote_at(root.path(), "origin").expect("inspect");
        assert_eq!(inspected["remote"], "origin");
        assert_eq!(inspected["head_count"], 0);
        assert!(inspected.get("url").is_none());
    }

    #[test]
    fn forge_configuration_accepts_only_provider_native_repository_identity() {
        let mut valid = config();
        valid.forge = Some(Box::new(ForgePolicy {
            provider: ForgeProvider::GitHub,
            repository: "owner/project".to_string(),
        }));
        valid.validate().expect("provider identity");

        valid.forge = Some(Box::new(ForgePolicy {
            provider: ForgeProvider::GitHub,
            repository: "https://github.com/owner/project".to_string(),
        }));
        assert_eq!(
            valid.validate().expect_err("raw URL must fail closed"),
            "development_system.forge_repository_invalid"
        );
    }
}
