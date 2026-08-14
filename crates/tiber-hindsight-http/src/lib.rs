//! Bounded HTTP adapter for Hindsight 0.8.3.
//!
//! Provider DTOs remain private. The public surface accepts only the
//! provider-independent, provenance-carrying contracts from
//! `tiber-memory-core`.

extern crate alloc;

use core::{
    fmt,
    future::{Future as _, poll_fn},
    net::IpAddr,
    str::FromStr as _,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use futures::StreamExt as _;
use reqwest::{StatusCode, Url, header, redirect::Policy, retry};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tiber_memory_core::{
    AgentId, CancelOutcome, CancelRequest, ForgetOutcome, ForgetRequest, MemoryBackend,
    MemoryBackendError, MemoryContractError, MemoryDocumentId, MemoryFuture, MemoryId, MemoryKind,
    MemoryOperationId, MemoryOperationKind, MemoryOperationState, MemoryOperationStatus,
    MemoryReconciliationHandle, MemoryRequestOptions, MemoryRetryability, MemorySafeCause,
    MemoryScope, MemoryTag, MemoryText, OperationStatusRequest, OwnerId, RecallCandidate,
    RecallRequest, RecallResult, ReconcileOutcome, ReconcileRequest, ReconcileTarget, RepositoryId,
    RetainOutcome, RetainRequest, SessionId, TaskId, TurnId,
};
use tokio::time::{Instant, sleep, sleep_until};

/// Exact vendor API version owned by these private DTOs.
const HINDSIGHT_API_VERSION: &str = "0.8.3";
/// Maximum untrusted response bytes retained before decoding.
const MAX_RESPONSE_BODY_BYTES: usize = 128 * 1024;
/// Response bound represented in the HTTP content-length domain.
const MAX_RESPONSE_BODY_BYTES_U64: u64 = 128 * 1024;
/// Cooperative polling interval for the runtime-neutral core cancellation flag.
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Hindsight version-probe response.
#[derive(Deserialize)]
struct VersionResponseDto {
    /// Exact API version string.
    api_version: String,
    /// Exact required 0.8.3 capability fields.
    features: FeaturesInfoDto,
}

/// Required Hindsight 0.8.3 feature shape.
#[derive(Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the private DTO mirrors Hindsight's required closed 0.8.3 boolean feature schema"
)]
struct FeaturesInfoDto {
    /// Audit-log capability, decoded to require the exact schema.
    #[serde(rename = "audit_log")]
    _audit_log: bool,
    /// Bank-configuration capability, decoded to require the exact schema.
    #[serde(rename = "bank_config_api")]
    _bank_config_api: bool,
    /// Bank-health capability, decoded to require the exact schema.
    #[serde(rename = "bank_llm_health")]
    _bank_llm_health: bool,
    /// Document-export capability, decoded to require the exact schema.
    #[serde(rename = "document_export_api")]
    _document_export_api: bool,
    /// Document-import capability, decoded to require the exact schema.
    #[serde(rename = "document_import_api")]
    _document_import_api: bool,
    /// File-upload capability, decoded to require the exact schema.
    #[serde(rename = "file_upload_api")]
    _file_upload_api: bool,
    /// LLM-trace capability, decoded to require the exact schema.
    #[serde(rename = "llm_trace")]
    _llm_trace: bool,
    /// MCP capability, decoded to require the exact schema.
    #[serde(rename = "mcp")]
    _mcp: bool,
    /// Observation capability, decoded to require the exact schema.
    #[serde(rename = "observations")]
    _observations: bool,
    /// Source-text storage capability, decoded to require the exact schema.
    /// Required source-text persistence for document reconciliation evidence.
    store_document_text: bool,
    /// Required background worker for asynchronous retain.
    worker: bool,
}

/// Hindsight asynchronous single-item retain request.
#[derive(Serialize)]
struct RetainRequestDto<'request> {
    /// Required asynchronous mode flag.
    #[serde(rename = "async")]
    asynchronous: bool,
    /// Exactly one stable document item.
    items: [RetainItemDto<'request>; 1],
}

/// One Hindsight retain item with Tiber-owned provenance.
#[derive(Serialize)]
struct RetainItemDto<'request> {
    /// Bounded retained content.
    content: &'request str,
    /// Bounded source context.
    context: &'request str,
    /// Stable upsert document identity.
    document_id: &'request str,
    /// Opaque effect witness stored with the backend document.
    metadata: RetainMetadataDto<'request>,
    /// Fixed observation scoping strategy.
    observation_scopes: &'static str,
    /// Complete strict Tiber provenance tags.
    tags: Vec<&'request str>,
    /// Fixed stable-document replacement behavior.
    update_mode: &'static str,
}

/// Tiber-owned document metadata used only for exact retain reconciliation.
#[derive(Serialize)]
struct RetainMetadataDto<'value> {
    /// Domain-separated evidence for the exact retained content and context.
    #[serde(rename = "tiber_retain_evidence_v1")]
    evidence: &'value str,
}

/// Hindsight asynchronous retain acknowledgement.
#[derive(Deserialize)]
struct RetainResponseDto {
    /// Echoed asynchronous mode.
    #[serde(rename = "async")]
    asynchronous: bool,
    /// Echoed bank identity.
    bank_id: String,
    /// Number of accepted items.
    items_count: usize,
    /// Reconciliation identity for the queued operation.
    operation_id: Option<String>,
    /// Vendor success flag.
    success: bool,
}

/// Hindsight recall request with strict tag filtering.
#[derive(Serialize)]
struct RecallRequestDto<'request> {
    /// Fixed low-latency retrieval budget.
    budget: &'static str,
    /// Explicitly disables every supplementary result facet.
    include: RecallIncludeDto,
    /// Caller-owned output token budget.
    max_tokens: usize,
    /// Bounded query.
    query: &'request str,
    /// Strict Tiber scope combined with current-turn exclusion.
    tag_groups: [RecallTagGroupDto<'request>; 2],
    /// Trace payloads remain disabled.
    trace: bool,
    /// Document-bounded fact kinds; observations have ambiguous provenance.
    types: [&'static str; 2],
}

/// Hindsight supplementary recall facets, all explicitly disabled.
#[derive(Serialize)]
struct RecallIncludeDto {
    /// Raw chunks remain disabled.
    chunks: Option<DisabledRecallFacetDto>,
    /// Entity summaries remain disabled.
    entities: Option<DisabledRecallFacetDto>,
    /// Observation source facts remain disabled.
    source_facts: Option<DisabledRecallFacetDto>,
}

/// Uninhabited-in-practice marker for an optional recall facet.
#[derive(Serialize)]
struct DisabledRecallFacetDto;

/// One supported Hindsight compound tag expression.
#[derive(Serialize)]
#[serde(untagged)]
enum RecallTagGroupDto<'request> {
    /// Direct strict tag match.
    Leaf(RecallTagLeafDto<'request>),
    /// Negated strict tag match.
    Not(RecallTagNotDto<'request>),
}

/// Hindsight tag-filter leaf.
#[derive(Serialize)]
struct RecallTagLeafDto<'request> {
    /// Match strategy, serialized under Hindsight's reserved-key spelling.
    #[serde(rename = "match")]
    match_mode: &'static str,
    /// Exact tag operands.
    tags: Vec<&'request str>,
}

/// Hindsight negated tag-filter expression.
#[derive(Serialize)]
struct RecallTagNotDto<'request> {
    /// Leaf that must not match.
    not: RecallTagLeafDto<'request>,
}

/// Hindsight recall response.
#[derive(Deserialize)]
struct RecallResponseDto {
    /// Ranked untrusted candidate memories.
    results: Vec<RecallResultDto>,
}

/// One untrusted Hindsight recall candidate.
#[derive(Deserialize)]
struct RecallResultDto {
    /// Stable source document identity.
    document_id: String,
    /// Closed Hindsight fact kind.
    #[serde(rename = "type")]
    fact_type: RecallFactTypeDto,
    /// Backend memory identity.
    id: String,
    /// Backend-provided provenance tags.
    tags: Vec<String>,
    /// Advisory memory text.
    text: String,
}

/// Closed Hindsight recall fact discriminator.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RecallFactTypeDto {
    /// Document-bounded experiential fact.
    Experience,
    /// Potentially multi-document synthesized observation.
    Observation,
    /// Document-bounded world fact.
    World,
}

/// Hindsight document deletion acknowledgement.
#[derive(Deserialize)]
struct DeleteDocumentResponseDto {
    /// Count of associated memories removed by the server.
    #[serde(rename = "memory_units_deleted")]
    _memory_units_deleted: usize,
    /// Human-readable vendor acknowledgement, decoded but never surfaced.
    #[serde(rename = "message")]
    _message: String,
    /// Echoed document identity.
    document_id: String,
    /// Vendor success flag.
    success: bool,
}

/// Hindsight asynchronous operation status.
#[derive(Deserialize)]
struct OperationStatusResponseDto {
    /// Echoed operation identity.
    operation_id: String,
    /// Closed lifecycle status.
    status: MemoryOperationState,
}

/// Hindsight operation-cancellation acknowledgement.
#[derive(Deserialize)]
struct CancelResponseDto {
    /// Human-readable vendor acknowledgement, decoded but never surfaced.
    #[serde(rename = "message")]
    _message: String,
    /// Echoed operation identity.
    operation_id: String,
    /// Vendor success flag.
    success: bool,
}

/// Minimal document evidence used only for read-only reconciliation.
#[derive(Deserialize)]
struct DocumentEvidenceDto {
    /// Echoed bank identity.
    bank_id: String,
    /// Tiber-owned effect witness returned with the stored document.
    document_metadata: Option<DocumentMetadataDto>,
    /// Echoed collision-free document identity.
    id: String,
    /// Backend-provided provenance tags.
    tags: Vec<String>,
}

/// Owned response form of Tiber's retain reconciliation metadata.
#[derive(Deserialize)]
struct DocumentMetadataDto {
    /// Domain-separated evidence for the exact retained content and context.
    #[serde(rename = "tiber_retain_evidence_v1")]
    evidence: Option<String>,
}

/// A validated explicit Hindsight endpoint.
#[derive(Clone, Debug)]
pub struct HindsightEndpoint(Url);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    clippy::question_mark_used,
    reason = "endpoint parsing follows validation flow and exposes one small public boundary"
)]
impl HindsightEndpoint {
    /// Parses an HTTPS endpoint, or an HTTP endpoint whose host is an explicit
    /// loopback IP address. Credentials, query strings, fragments, and path
    /// prefixes are deliberately refused.
    ///
    /// # Errors
    ///
    /// Returns a stable setup error for an unsafe or malformed endpoint.
    pub fn parse(value: &str) -> Result<Self, HindsightSetupError> {
        let url = Url::parse(value).map_err(|_source| HindsightSetupError::InvalidEndpoint)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(HindsightSetupError::CredentialsUnsupported);
        }
        if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
            return Err(HindsightSetupError::EndpointMustBeOrigin);
        }
        let secure = url.scheme() == "https";
        let loopback_http = url.scheme() == "http"
            && url
                .host_str()
                .and_then(|host| {
                    let unbracketed = host
                        .strip_prefix('[')
                        .and_then(|bracketed_host| bracketed_host.strip_suffix(']'))
                        .unwrap_or(host);
                    IpAddr::from_str(unbracketed).ok()
                })
                .is_some_and(|address| address.is_loopback());
        if !secure && !loopback_http {
            return Err(HindsightSetupError::InsecureEndpoint);
        }
        Ok(Self(url))
    }

    /// Replaces the validated origin path with one adapter-owned API path.
    fn operation_url(&self, path: &str) -> Url {
        let mut url = self.0.clone();
        url.set_path(path);
        url
    }
}

/// Stable failures while constructing the Hindsight adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "setup failures follow endpoint validation and client construction flow"
)]
pub enum HindsightSetupError {
    /// The configured value was not an absolute URL.
    InvalidEndpoint,
    /// Plain HTTP was configured for a non-loopback host, or the scheme was unsupported.
    InsecureEndpoint,
    /// User information would implicitly manage or forward authentication.
    CredentialsUnsupported,
    /// The endpoint included a path, query string, or fragment.
    EndpointMustBeOrigin,
    /// The fixed no-proxy, no-redirect, no-retry client could not be built.
    ClientConstruction,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    clippy::pattern_type_mismatch,
    reason = "the stable setup-code projection is a total borrowed enum match"
)]
impl fmt::Display for HindsightSetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidEndpoint => "hindsight_invalid_endpoint",
            Self::InsecureEndpoint => "hindsight_insecure_endpoint",
            Self::CredentialsUnsupported => "hindsight_credentials_unsupported",
            Self::EndpointMustBeOrigin => "hindsight_endpoint_must_be_origin",
            Self::ClientConstruction => "hindsight_client_construction",
        })
    }
}

/// Direct, non-replaying Hindsight 0.8.3 HTTP adapter.
#[derive(Clone, Debug)]
pub struct HindsightHttp {
    /// Fixed no-proxy, no-redirect, no-retry HTTP handle.
    client: reqwest::Client,
    /// Explicit validated service origin.
    endpoint: HindsightEndpoint,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::result_large_err,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    reason = "adapter methods keep sequential transport state explicit while returning the core's provenance-rich typed failures by value"
)]
impl HindsightHttp {
    /// Builds the fixed HTTP client without probing or mutating the endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`HindsightSetupError::ClientConstruction`] when the fixed
    /// client configuration cannot be constructed.
    pub fn new(endpoint: HindsightEndpoint) -> Result<Self, HindsightSetupError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .retry(retry::never())
            .build()
            .map_err(|_source| HindsightSetupError::ClientConstruction)?;
        Ok(Self { client, endpoint })
    }

    /// Executes one version-gated asynchronous retain request.
    async fn retain_inner(
        &self,
        request: &RetainRequest,
        options: &MemoryRequestOptions,
    ) -> Result<RetainOutcome, MemoryBackendError> {
        let operation = MemoryOperationKind::Retain;
        let budget = OperationBudget::new(options);
        self.probe_version(operation, &budget).await?;
        budget.check_before_dispatch(operation)?;
        let bank = request.scope().bank();
        let document_id = request.backend_document_id();
        let tags = request.strict_tags();
        let evidence = request.expected_evidence();
        let dto = RetainRequestDto {
            asynchronous: true,
            items: [RetainItemDto {
                content: request.content().as_str(),
                context: request.context().as_str(),
                document_id: document_id.as_str(),
                metadata: RetainMetadataDto {
                    evidence: evidence.as_str(),
                },
                observation_scopes: "combined",
                tags: tags.as_slice().iter().map(MemoryTag::as_str).collect(),
                update_mode: "replace",
            }],
        };
        let body = serialize_request(&dto, operation)?;
        let path = format!("/v1/default/banks/{}/memories", bank.as_str());
        budget.check_before_dispatch(operation)?;
        let response = budget
            .send_mutation(
                self.client
                    .post(self.endpoint.operation_url(&path))
                    .header(header::ACCEPT, "application/json")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body),
            )
            .await
            .map_err(|failure| {
                mutation_send_error(operation, failure, request.reconciliation_handle())
            })?;
        if !response.status().is_success() {
            return mutation_status_error(
                response.status(),
                operation,
                request.reconciliation_handle(),
            );
        }
        let dto: RetainResponseDto = budget.read_json(response).await.map_err(|_failure| {
            MemoryBackendError::outcome_unknown(request.reconciliation_handle())
        })?;
        if !dto.success || !dto.asynchronous || dto.bank_id != bank.as_str() || dto.items_count != 1
        {
            return Err(MemoryBackendError::outcome_unknown(
                request.reconciliation_handle(),
            ));
        }
        let operation_id = dto
            .operation_id
            .ok_or_else(|| MemoryBackendError::outcome_unknown(request.reconciliation_handle()))?;
        let operation_id = MemoryOperationId::parse(&operation_id).map_err(|_error| {
            MemoryBackendError::outcome_unknown(request.reconciliation_handle())
        })?;
        if !operation_id_is_path_segment_safe(&operation_id) {
            return Err(MemoryBackendError::outcome_unknown(
                request.reconciliation_handle(),
            ));
        }
        Ok(RetainOutcome::accepted(request, operation_id))
    }

    /// Executes one version-gated strict advisory recall.
    async fn recall_inner(
        &self,
        request: &RecallRequest,
        options: &MemoryRequestOptions,
    ) -> Result<RecallResult, MemoryBackendError> {
        let operation = MemoryOperationKind::Recall;
        let budget = OperationBudget::new(options);
        self.probe_version(operation, &budget).await?;
        budget.check_before_dispatch(operation)?;
        let bank = request.scope().bank();
        let strict_tags = request.strict_tags();
        let excluded_turn_tag = request.excluded_turn_tag();
        let dto = RecallRequestDto {
            budget: "low",
            include: RecallIncludeDto {
                chunks: None,
                entities: None,
                source_facts: None,
            },
            max_tokens: request.token_budget().get(),
            query: request.query().as_str(),
            tag_groups: [
                RecallTagGroupDto::Leaf(RecallTagLeafDto {
                    match_mode: "all_strict",
                    tags: strict_tags
                        .as_slice()
                        .iter()
                        .map(MemoryTag::as_str)
                        .collect(),
                }),
                RecallTagGroupDto::Not(RecallTagNotDto {
                    not: RecallTagLeafDto {
                        match_mode: "all_strict",
                        tags: vec![excluded_turn_tag.as_str()],
                    },
                }),
            ],
            trace: false,
            types: ["world", "experience"],
        };
        let body = serialize_request(&dto, operation)?;
        let path = format!("/v1/default/banks/{}/memories/recall", bank.as_str());
        let response = budget
            .send(
                self.client
                    .post(self.endpoint.operation_url(&path))
                    .header(header::ACCEPT, "application/json")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body),
            )
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        if !response.status().is_success() {
            return Err(status_error(operation, response.status()));
        }
        let response: RecallResponseDto = budget
            .read_json(response)
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        let candidates = response
            .results
            .into_iter()
            .map(|candidate| parse_candidate(request, candidate))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok(RecallResult::from_candidates(request, candidates))
    }

    /// Executes one version-gated stable-document deletion.
    async fn forget_inner(
        &self,
        request: &ForgetRequest,
        options: &MemoryRequestOptions,
    ) -> Result<ForgetOutcome, MemoryBackendError> {
        let operation = MemoryOperationKind::Forget;
        let budget = OperationBudget::new(options);
        self.probe_version(operation, &budget).await?;
        budget.check_before_dispatch(operation)?;
        let bank = request.scope().bank();
        let document_id = request.backend_document_id();
        let path = format!(
            "/v1/default/banks/{}/documents/{}",
            bank.as_str(),
            document_id.as_str()
        );
        budget.check_before_dispatch(operation)?;
        let response = budget
            .send_mutation(
                self.client
                    .delete(self.endpoint.operation_url(&path))
                    .header(header::ACCEPT, "application/json"),
            )
            .await
            .map_err(|failure| {
                mutation_send_error(operation, failure, request.reconciliation_handle())
            })?;
        if !response.status().is_success() {
            return mutation_status_error(
                response.status(),
                operation,
                request.reconciliation_handle(),
            );
        }
        let dto: DeleteDocumentResponseDto =
            budget.read_json(response).await.map_err(|_failure| {
                MemoryBackendError::outcome_unknown(request.reconciliation_handle())
            })?;
        if dto.success && dto.document_id == document_id.as_str() {
            Ok(ForgetOutcome::Forgotten)
        } else {
            Err(MemoryBackendError::outcome_unknown(
                request.reconciliation_handle(),
            ))
        }
    }

    /// Executes one version-gated asynchronous-operation status read.
    async fn operation_status_inner(
        &self,
        request: &OperationStatusRequest,
        options: &MemoryRequestOptions,
    ) -> Result<MemoryOperationStatus, MemoryBackendError> {
        let operation = MemoryOperationKind::OperationStatus;
        let budget = OperationBudget::new(options);
        self.probe_version(operation, &budget).await?;
        budget.check_before_dispatch(operation)?;
        let bank = request.scope().bank();
        let path = operation_path(bank.as_str(), request.operation_id(), operation)?;
        let response = budget
            .send(
                self.client
                    .get(self.endpoint.operation_url(&path))
                    .header(header::ACCEPT, "application/json"),
            )
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        if !response.status().is_success() {
            return Err(status_error(operation, response.status()));
        }
        let dto: OperationStatusResponseDto = budget
            .read_json(response)
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        if dto.operation_id != request.operation_id().as_str() {
            return Err(protocol_error(operation));
        }
        Ok(MemoryOperationStatus::new(request, dto.status))
    }

    /// Reconciles one ambiguous mutation using read-only backend evidence.
    async fn reconcile_inner(
        &self,
        request: &ReconcileRequest,
        options: &MemoryRequestOptions,
    ) -> Result<ReconcileOutcome, MemoryBackendError> {
        let operation = reconciliation_operation(request.handle().target());
        let budget = OperationBudget::new(options);
        self.probe_version(operation, &budget).await?;
        budget.check_before_dispatch(operation)?;
        match request.handle().target() {
            ReconcileTarget::RetainDocument(target) => {
                self.reconcile_document(target.document_id(), Some(target), operation, &budget)
                    .await
            }
            ReconcileTarget::ForgetDocument(document) => {
                self.reconcile_document(document, None, operation, &budget)
                    .await
            }
            ReconcileTarget::CancelOperation(handle) => {
                self.reconcile_cancel(handle, operation, &budget).await
            }
        }
    }

    /// Inspects one exact document without replaying retain or forget.
    async fn reconcile_document(
        &self,
        document: &tiber_memory_core::ScopedMemoryDocumentId,
        expected_retain: Option<&tiber_memory_core::RetainReconciliationTarget>,
        operation: MemoryOperationKind,
        budget: &OperationBudget<'_>,
    ) -> Result<ReconcileOutcome, MemoryBackendError> {
        let bank = document.scope().bank();
        let path = format!(
            "/v1/default/banks/{}/documents/{}",
            bank.as_str(),
            document.as_str()
        );
        let response = budget
            .send(
                self.client
                    .get(self.endpoint.operation_url(&path))
                    .header(header::ACCEPT, "application/json"),
            )
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        if !response.status().is_success() {
            return Ok(ReconcileOutcome::StillUnknown);
        }
        let Some(expected_retain) = expected_retain else {
            return Ok(ReconcileOutcome::StillUnknown);
        };
        let dto: DocumentEvidenceDto = match budget.read_json(response).await {
            Ok(dto) => dto,
            Err(_failure) => return Ok(ReconcileOutcome::StillUnknown),
        };
        if !document_evidence_matches(document, expected_retain, &dto) {
            return Ok(ReconcileOutcome::StillUnknown);
        }
        Ok(ReconcileOutcome::Applied)
    }

    /// Inspects one exact operation without replaying its cancellation.
    async fn reconcile_cancel(
        &self,
        handle: &tiber_memory_core::MemoryOperationHandle,
        operation: MemoryOperationKind,
        budget: &OperationBudget<'_>,
    ) -> Result<ReconcileOutcome, MemoryBackendError> {
        let bank = handle.scope().bank();
        let path = operation_path(bank.as_str(), handle.operation_id(), operation)?;
        let response = budget
            .send(
                self.client
                    .get(self.endpoint.operation_url(&path))
                    .header(header::ACCEPT, "application/json"),
            )
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        if !response.status().is_success() {
            return Ok(ReconcileOutcome::StillUnknown);
        }
        let dto: OperationStatusResponseDto = match budget.read_json(response).await {
            Ok(dto) => dto,
            Err(_failure) => return Ok(ReconcileOutcome::StillUnknown),
        };
        if dto.operation_id != handle.operation_id().as_str() {
            return Ok(ReconcileOutcome::StillUnknown);
        }
        Ok(match dto.status {
            MemoryOperationState::Cancelled => ReconcileOutcome::Applied,
            MemoryOperationState::Completed | MemoryOperationState::Failed => {
                ReconcileOutcome::NotApplied
            }
            MemoryOperationState::Pending | MemoryOperationState::Processing => {
                ReconcileOutcome::Pending
            }
            MemoryOperationState::NotFound => ReconcileOutcome::StillUnknown,
        })
    }

    /// Executes one version-gated pending-operation cancellation.
    async fn cancel_inner(
        &self,
        request: &CancelRequest,
        options: &MemoryRequestOptions,
    ) -> Result<CancelOutcome, MemoryBackendError> {
        let operation = MemoryOperationKind::Cancel;
        let budget = OperationBudget::new(options);
        self.probe_version(operation, &budget).await?;
        budget.check_before_dispatch(operation)?;
        let bank = request.scope().bank();
        let path = operation_path(bank.as_str(), request.operation_id(), operation)?;
        budget.check_before_dispatch(operation)?;
        let response = budget
            .send_mutation(
                self.client
                    .delete(self.endpoint.operation_url(&path))
                    .header(header::ACCEPT, "application/json"),
            )
            .await
            .map_err(|failure| {
                mutation_send_error(operation, failure, request.reconciliation_handle())
            })?;
        if !response.status().is_success() {
            return mutation_status_error(
                response.status(),
                operation,
                request.reconciliation_handle(),
            );
        }
        let dto: CancelResponseDto = budget.read_json(response).await.map_err(|_failure| {
            MemoryBackendError::outcome_unknown(request.reconciliation_handle())
        })?;
        if dto.success && dto.operation_id == request.operation_id().as_str() {
            Ok(CancelOutcome::Cancelled)
        } else {
            Err(MemoryBackendError::outcome_unknown(
                request.reconciliation_handle(),
            ))
        }
    }

    /// Requires the exact DTO-owned API version within the operation budget.
    async fn probe_version(
        &self,
        operation: MemoryOperationKind,
        budget: &OperationBudget<'_>,
    ) -> Result<(), MemoryBackendError> {
        budget.check_before_dispatch(operation)?;
        let response = budget
            .send(
                self.client
                    .get(self.endpoint.operation_url("/version"))
                    .header(header::ACCEPT, "application/json"),
            )
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        if !response.status().is_success() {
            return Err(status_error(operation, response.status()));
        }
        let version: VersionResponseDto = budget
            .read_json(response)
            .await
            .map_err(|failure| read_failure(operation, failure))?;
        if version.api_version != HINDSIGHT_API_VERSION
            || !version.features.worker
            || !version.features.store_document_text
        {
            return Err(MemoryBackendError::unsupported(operation));
        }
        Ok(())
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    reason = "the object-safe port methods directly box their matching operation futures"
)]
impl MemoryBackend for HindsightHttp {
    fn retain<'request>(
        &'request self,
        request: &'request RetainRequest,
        options: &'request MemoryRequestOptions,
    ) -> MemoryFuture<'request, RetainOutcome> {
        Box::pin(self.retain_inner(request, options))
    }

    fn recall<'request>(
        &'request self,
        request: &'request RecallRequest,
        options: &'request MemoryRequestOptions,
    ) -> MemoryFuture<'request, RecallResult> {
        Box::pin(self.recall_inner(request, options))
    }

    fn forget<'request>(
        &'request self,
        request: &'request ForgetRequest,
        options: &'request MemoryRequestOptions,
    ) -> MemoryFuture<'request, ForgetOutcome> {
        Box::pin(self.forget_inner(request, options))
    }

    fn operation_status<'request>(
        &'request self,
        request: &'request OperationStatusRequest,
        options: &'request MemoryRequestOptions,
    ) -> MemoryFuture<'request, MemoryOperationStatus> {
        Box::pin(self.operation_status_inner(request, options))
    }

    fn reconcile<'request>(
        &'request self,
        request: &'request ReconcileRequest,
        options: &'request MemoryRequestOptions,
    ) -> MemoryFuture<'request, ReconcileOutcome> {
        Box::pin(self.reconcile_inner(request, options))
    }

    fn cancel<'request>(
        &'request self,
        request: &'request CancelRequest,
        options: &'request MemoryRequestOptions,
    ) -> MemoryFuture<'request, CancelOutcome> {
        Box::pin(self.cancel_inner(request, options))
    }
}

/// One absolute operation deadline plus cooperative caller cancellation.
struct OperationBudget<'request> {
    /// Runtime-neutral cancellation flag borrowed from the request.
    cancellation: &'request tiber_memory_core::MemoryCancellation,
    /// Absolute Tokio deadline shared across version probe and operation I/O.
    deadline: Instant,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::ignored_unit_patterns,
    clippy::implicit_return,
    clippy::integer_division_remainder_used,
    clippy::question_mark_used,
    clippy::result_large_err,
    clippy::shadow_reuse,
    reason = "the budget helper keeps deadline and cancellation selection local while returning the core's provenance-rich typed failures"
)]
impl<'request> OperationBudget<'request> {
    /// Starts one absolute budget shared across every phase.
    fn new(options: &'request MemoryRequestOptions) -> Self {
        let now = Instant::now();
        let deadline = now.checked_add(options.deadline().get()).unwrap_or(now);
        Self {
            cancellation: options.cancellation(),
            deadline,
        }
    }

    /// Refuses cancellation or deadline expiry before request dispatch.
    fn check_before_dispatch(
        &self,
        operation: MemoryOperationKind,
    ) -> Result<(), MemoryBackendError> {
        if self.cancellation.is_cancelled() {
            return Err(MemoryBackendError::cancelled(operation));
        }
        if Instant::now() >= self.deadline {
            return Err(MemoryBackendError::deadline_exceeded(
                operation,
                MemoryRetryability::Retryable,
            ));
        }
        Ok(())
    }

    /// Dispatches one HTTP request under the absolute budget.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, IoFailure> {
        let deadline = sleep_until(self.deadline);
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            _ = wait_for_cancellation(self.cancellation) => Err(IoFailure::Cancelled),
            _ = &mut deadline => Err(IoFailure::Deadline),
            response = request.send() => response.map_err(|_source| IoFailure::Transport),
        }
    }

    /// Dispatches a mutation while retaining the exact first-poll boundary.
    async fn send_mutation(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, MutationSendFailure> {
        let send_was_polled = AtomicBool::new(false);
        let send = request.send();
        tokio::pin!(send);
        let tracked_send = poll_fn(|context| {
            send_was_polled.store(true, Ordering::Relaxed);
            send.as_mut().poll(context)
        });
        tokio::pin!(tracked_send);
        let deadline = sleep_until(self.deadline);
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            _ = wait_for_cancellation(self.cancellation) => {
                Err(MutationSendFailure::interrupt(
                    send_was_polled.load(Ordering::Relaxed),
                    IoFailure::Cancelled,
                ))
            }
            _ = &mut deadline => {
                Err(MutationSendFailure::interrupt(
                    send_was_polled.load(Ordering::Relaxed),
                    IoFailure::Deadline,
                ))
            }
            response = &mut tracked_send => {
                response.map_err(|_source| MutationSendFailure::OutcomeUnknown)
            }
        }
    }

    /// Incrementally reads and decodes one bounded JSON response.
    async fn read_json<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, IoFailure> {
        let content_type_is_json = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media_type| media_type.trim() == "application/json");
        if !content_type_is_json {
            return Err(IoFailure::Response);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BODY_BYTES_U64)
        {
            return Err(IoFailure::Response);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        loop {
            let deadline = sleep_until(self.deadline);
            tokio::pin!(deadline);
            let next = tokio::select! {
                biased;
                _ = wait_for_cancellation(self.cancellation) => return Err(IoFailure::Cancelled),
                _ = &mut deadline => return Err(IoFailure::Deadline),
                item = stream.next() => item,
            };
            let Some(chunk) = next else {
                break;
            };
            let chunk = chunk.map_err(|_source| IoFailure::Transport)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY_BYTES {
                return Err(IoFailure::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        self.check_io_budget()?;
        serde_json::from_slice(&bytes).map_err(|_source| IoFailure::Response)
    }

    /// Rechecks the absolute budget immediately before synchronous decoding.
    fn check_io_budget(&self) -> Result<(), IoFailure> {
        if self.cancellation.is_cancelled() {
            return Err(IoFailure::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(IoFailure::Deadline);
        }
        Ok(())
    }
}

/// Internal transport outcomes before read-versus-mutation mapping.
#[derive(Clone, Copy)]
enum IoFailure {
    /// The caller cancellation flag fired.
    Cancelled,
    /// The operation's absolute deadline fired.
    Deadline,
    /// A response exceeded its bound or failed DTO decoding.
    Response,
    /// Reqwest could not produce or continue a response.
    Transport,
}

/// Mutation transport failures split at reqwest's first request-future poll.
#[derive(Clone, Copy)]
enum MutationSendFailure {
    /// Cancellation or expiry won before reqwest could begin dispatch.
    BeforeDispatch(IoFailure),
    /// The request future was polled, so remote mutation cannot be excluded.
    OutcomeUnknown,
}

impl MutationSendFailure {
    /// Classifies a budget interrupt using the captured request poll boundary.
    #[expect(
        clippy::implicit_return,
        reason = "the first-poll boundary has one direct conservative fallback"
    )]
    fn interrupt(send_was_polled: bool, failure: IoFailure) -> Self {
        if send_was_polled {
            return Self::OutcomeUnknown;
        }
        Self::BeforeDispatch(failure)
    }
}

/// Waits cooperatively for the runtime-neutral cancellation flag.
#[expect(
    clippy::implicit_return,
    reason = "the polling loop completes only when the borrowed cancellation flag is set"
)]
async fn wait_for_cancellation(cancellation: &tiber_memory_core::MemoryCancellation) {
    while !cancellation.is_cancelled() {
        sleep(CANCELLATION_POLL_INTERVAL).await;
    }
}

#[expect(
    clippy::implicit_return,
    reason = "the closed read-failure projection is clearest as a total match"
)]
/// Maps an internal I/O failure into a retryable read error.
fn read_failure(operation: MemoryOperationKind, failure: IoFailure) -> MemoryBackendError {
    match failure {
        IoFailure::Cancelled => MemoryBackendError::cancelled(operation),
        IoFailure::Deadline => {
            MemoryBackendError::deadline_exceeded(operation, MemoryRetryability::Retryable)
        }
        IoFailure::Transport => MemoryBackendError::transport(
            operation,
            MemoryRetryability::Retryable,
            Some(MemorySafeCause::Connection),
        ),
        IoFailure::Response => protocol_error(operation),
    }
}

/// Rejects the only URL dot-segment forms admitted by the core ID grammar.
#[expect(
    clippy::implicit_return,
    reason = "the closed path-segment predicate directly mirrors URL dot normalization"
)]
fn operation_id_is_path_segment_safe(operation_id: &MemoryOperationId) -> bool {
    !matches!(operation_id.as_str(), "." | "..")
}

#[expect(
    clippy::implicit_return,
    clippy::result_large_err,
    reason = "the closed operation-segment guard returns the core's typed protocol error"
)]
/// Constructs one exact operation path only after dot-segment validation.
fn operation_path(
    bank_id: &str,
    operation_id: &MemoryOperationId,
    operation: MemoryOperationKind,
) -> Result<String, MemoryBackendError> {
    if !operation_id_is_path_segment_safe(operation_id) {
        return Err(protocol_error(operation));
    }
    Ok(format!(
        "/v1/default/banks/{bank_id}/operations/{}",
        operation_id.as_str()
    ))
}

#[expect(
    clippy::implicit_return,
    reason = "mutation send failures map exhaustively across the first-poll dispatch boundary"
)]
/// Maps a mutation send failure without losing definitive pre-dispatch errors.
fn mutation_send_error(
    operation: MemoryOperationKind,
    send_failure: MutationSendFailure,
    reconciliation: MemoryReconciliationHandle,
) -> MemoryBackendError {
    match send_failure {
        MutationSendFailure::BeforeDispatch(io_failure) => read_failure(operation, io_failure),
        MutationSendFailure::OutcomeUnknown => MemoryBackendError::outcome_unknown(reconciliation),
    }
}

#[expect(
    clippy::implicit_return,
    reason = "the fixed protocol error constructor is a one-expression projection"
)]
/// Constructs one stable malformed-response error.
fn protocol_error(operation: MemoryOperationKind) -> MemoryBackendError {
    MemoryBackendError::protocol(operation, Some(MemorySafeCause::Response))
}

#[expect(
    clippy::implicit_return,
    reason = "status failures share one sanitized retryable read projection"
)]
/// Constructs one sanitized non-success read response.
fn status_error(operation: MemoryOperationKind, status: StatusCode) -> MemoryBackendError {
    if !status.is_client_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
    {
        MemoryBackendError::transport(
            operation,
            MemoryRetryability::Retryable,
            Some(MemorySafeCause::Status),
        )
    } else {
        MemoryBackendError::protocol(operation, Some(MemorySafeCause::Status))
    }
}

#[expect(
    clippy::implicit_return,
    clippy::result_large_err,
    reason = "mutation statuses split definitive rejection from the core's provenance-rich ambiguity error"
)]
/// Distinguishes definitive client rejection from ambiguous mutation status.
fn mutation_status_error<T>(
    status: StatusCode,
    operation: MemoryOperationKind,
    reconciliation: MemoryReconciliationHandle,
) -> Result<T, MemoryBackendError> {
    if matches!(operation, MemoryOperationKind::Retain)
        && status == StatusCode::UNPROCESSABLE_ENTITY
    {
        Err(MemoryBackendError::protocol(
            operation,
            Some(MemorySafeCause::Status),
        ))
    } else {
        Err(MemoryBackendError::outcome_unknown(reconciliation))
    }
}

#[expect(
    clippy::implicit_return,
    clippy::result_large_err,
    reason = "serialization maps directly into the core's provenance-rich typed protocol error"
)]
/// Serializes one private request DTO before dispatch.
fn serialize_request<T: Serialize>(
    value: &T,
    operation: MemoryOperationKind,
) -> Result<Vec<u8>, MemoryBackendError> {
    serde_json::to_vec(value).map_err(|_source| protocol_error(operation))
}

#[expect(
    clippy::implicit_return,
    clippy::needless_pass_by_value,
    clippy::question_mark_used,
    clippy::result_large_err,
    clippy::single_call_fn,
    reason = "one candidate decoder parses every field once and returns the core's provenance-rich typed error"
)]
/// Parses one untrusted recall DTO into core-owned semantic values.
fn parse_candidate(
    request: &RecallRequest,
    dto: RecallResultDto,
) -> Result<Option<RecallCandidate>, MemoryBackendError> {
    let operation = MemoryOperationKind::Recall;
    let document_id = match request.scope().parse_backend_document_id(&dto.document_id) {
        Ok(document_id) => document_id,
        Err(MemoryContractError::ScopeMismatch)
            if backend_document_identity_is_well_formed(&dto.document_id) =>
        {
            return Ok(None);
        }
        Err(_error) => return Err(protocol_error(operation)),
    };
    if matches!(dto.fact_type, RecallFactTypeDto::Observation) {
        return Ok(None);
    }
    let id = MemoryId::parse(&dto.id).map_err(|_error| protocol_error(operation))?;
    let text = MemoryText::parse(&dto.text).map_err(|_error| protocol_error(operation))?;
    let tags = dto
        .tags
        .iter()
        .map(|tag| MemoryTag::parse(tag).map_err(|_error| protocol_error(operation)))
        .collect::<Result<Vec<_>, _>>()?;
    let expected_tags = request.strict_tags();
    if tags.len() != expected_tags.as_slice().len().saturating_add(1)
        || !expected_tags
            .as_slice()
            .iter()
            .all(|expected| tags.contains(expected))
    {
        return Ok(None);
    }
    let mut turn_tags = tags
        .iter()
        .filter(|tag| !expected_tags.as_slice().contains(tag));
    let Some(turn_tag) = turn_tags.next() else {
        return Ok(None);
    };
    if turn_tags.next().is_some()
        || !memory_tag_is_canonical_turn(turn_tag)
        || turn_tag == &request.excluded_turn_tag()
    {
        return Ok(None);
    }
    Ok(Some(RecallCandidate::new(id, text, document_id, tags)))
}

/// Distinguishes canonical foreign identities from malformed colon-bearing input.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the canonical foreign-identity guard remains adjacent to its sole parsing boundary"
)]
fn backend_document_identity_is_well_formed(value: &str) -> bool {
    let mut parts = value.split(':');
    let (
        Some(namespace),
        Some(version),
        Some(bank_scope),
        Some(raw_owner),
        Some(raw_repository),
        Some(raw_agent),
        Some(raw_session),
        Some(raw_task),
        Some(raw_kind),
        Some(raw_document),
    ) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    )
    else {
        return false;
    };
    if namespace != "tiber" || version != "v1" || parts.next().is_some() {
        return false;
    }
    let (Ok(owner), Ok(repository), Ok(agent), Ok(session), Ok(task), Ok(kind), Ok(document)) = (
        OwnerId::parse(raw_owner),
        RepositoryId::parse(raw_repository),
        AgentId::parse(raw_agent),
        SessionId::parse(raw_session),
        TaskId::parse(raw_task),
        MemoryKind::parse(raw_kind),
        MemoryDocumentId::parse(raw_document),
    ) else {
        return false;
    };
    let scope = match bank_scope {
        "owner-global" => MemoryScope::owner_global(owner, repository, agent, session, task, kind),
        "repository" => MemoryScope::repository(owner, repository, agent, session, task, kind),
        _unsupported => return false,
    };
    scope.backend_document_id(&document).as_str() == value
}

/// Accepts only turn tags that can be reconstructed from a core semantic ID.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "canonical semantic-tag validation remains adjacent to its sole response boundary"
)]
fn memory_tag_is_canonical_turn(tag: &MemoryTag) -> bool {
    let Some(raw_turn_id) = tag.as_str().strip_prefix("turn:") else {
        return false;
    };
    let Ok(turn_id) = TurnId::parse(raw_turn_id) else {
        return false;
    };
    MemoryScope::turn_tag(&turn_id) == *tag
}

/// Returns the original mutation kind represented by one reconciliation target.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the closed target-to-operation projection remains explicit beside reconciliation"
)]
fn reconciliation_operation(target: &ReconcileTarget) -> MemoryOperationKind {
    match target {
        ReconcileTarget::RetainDocument(_) => MemoryOperationKind::Retain,
        ReconcileTarget::ForgetDocument(_) => MemoryOperationKind::Forget,
        ReconcileTarget::CancelOperation(_) => MemoryOperationKind::Cancel,
    }
}

/// Requires exact bank, document, and retained provenance evidence.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the exact evidence predicate remains adjacent to its sole reconciliation boundary"
)]
fn document_evidence_matches(
    document: &tiber_memory_core::ScopedMemoryDocumentId,
    expected_retain: &tiber_memory_core::RetainReconciliationTarget,
    dto: &DocumentEvidenceDto,
) -> bool {
    if dto.bank_id != document.scope().bank().as_str() || dto.id != document.as_str() {
        return false;
    }
    let parsed_tags = dto
        .tags
        .iter()
        .map(|tag| MemoryTag::parse(tag))
        .collect::<Result<Vec<_>, _>>();
    let Ok(tags) = parsed_tags else {
        return false;
    };
    let evidence_matches = dto
        .document_metadata
        .as_ref()
        .and_then(|metadata| metadata.evidence.as_deref())
        .is_some_and(|evidence| evidence == expected_retain.expected_evidence().as_str());
    evidence_matches
        && tags.len() == expected_retain.expected_tags().as_slice().len()
        && expected_retain
            .expected_tags()
            .as_slice()
            .iter()
            .all(|expected_tag| tags.contains(expected_tag))
}

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::arithmetic_side_effects,
    clippy::default_numeric_fallback,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    clippy::pattern_type_mismatch,
    clippy::separated_literal_suffix,
    clippy::shadow_reuse,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core,
    reason = "the local black-box HTTP fixtures fail fast and inspect their bounded wire transcript directly"
)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use serde_json::{Value, json};
    use tiber_memory_core::{
        AgentId, CancelRequest, ForgetRequest, MemoryBackend as _, MemoryBackendErrorKind,
        MemoryCancellation, MemoryDeadline, MemoryDocumentId, MemoryKind, MemoryOperationId,
        MemoryOperationKind, MemoryRequestOptions, MemoryRetryability, MemoryScope, MemoryText,
        OperationStatusRequest, OwnerId, RecallItemBudget, RecallRequest, RecallTokenBudget,
        ReconcileOutcome, ReconcileRequest, RepositoryId, RetainOutcome, RetainRequest, SessionId,
        TaskId, TurnId,
    };
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        sync::{Mutex, mpsc, oneshot},
        task::JoinHandle,
        time::sleep,
    };

    use super::{HindsightEndpoint, HindsightHttp, OperationBudget, mutation_send_error};

    struct FixtureResponse {
        body: Vec<u8>,
        status: &'static str,
    }

    struct FixtureRequest {
        body: Value,
        method: String,
        path: String,
    }

    async fn server(
        responses: Vec<FixtureResponse>,
    ) -> (
        HindsightEndpoint,
        mpsc::Receiver<FixtureRequest>,
        JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds loopback");
        let address = listener.local_addr().expect("fixture has address");
        let endpoint = HindsightEndpoint::parse(&format!("http://{address}/"))
            .expect("fixture endpoint is safe");
        let (sender, receiver) = mpsc::channel(responses.len());
        let responses = Arc::new(Mutex::new(responses.into_iter()));
        let task = tokio::spawn(async move {
            loop {
                let response = responses.lock().await.next();
                let Some(response) = response else { break };
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts request");
                let request = read_request(&mut stream).await;
                sender.send(request).await.expect("fixture records request");
                write_response(&mut stream, response).await;
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err(),
                "fixture receives no unexpected replay"
            );
        });
        (endpoint, receiver, task)
    }

    async fn read_request(stream: &mut TcpStream) -> FixtureRequest {
        let mut raw = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 4096];
            let read = stream
                .read(&mut chunk)
                .await
                .expect("fixture reads request");
            assert_ne!(read, 0, "fixture receives complete request");
            raw.extend_from_slice(&chunk[..read]);
            let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            break index + 4;
        };
        let head = std::str::from_utf8(&raw[..header_end]).expect("headers are UTF-8");
        let mut lines = head.split("\r\n");
        let mut request_line = lines
            .next()
            .expect("request line exists")
            .split_whitespace();
        let method = request_line.next().expect("method exists").to_owned();
        let path = request_line.next().expect("path exists").to_owned();
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _value)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_name, value)| {
                value
                    .trim()
                    .parse::<usize>()
                    .expect("content length parses")
            });
        while raw.len().saturating_sub(header_end) < content_length {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.expect("fixture reads body");
            assert_ne!(read, 0, "fixture receives complete body");
            raw.extend_from_slice(&chunk[..read]);
        }
        let body = if content_length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&raw[header_end..header_end + content_length])
                .expect("fixture body is JSON")
        };
        FixtureRequest { body, method, path }
    }

    async fn write_response(stream: &mut TcpStream, response: FixtureResponse) {
        let head = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.status,
            response.body.len()
        );
        stream
            .write_all(head.as_bytes())
            .await
            .expect("fixture writes head");
        stream
            .write_all(&response.body)
            .await
            .expect("fixture writes body");
        stream.shutdown().await.expect("fixture closes response");
    }

    fn response(status: &'static str, body: Value) -> FixtureResponse {
        FixtureResponse {
            body: serde_json::to_vec(&body).expect("fixture serializes response"),
            status,
        }
    }

    fn version() -> FixtureResponse {
        version_response("0.8.3", true)
    }

    fn version_response(api_version: &str, worker: bool) -> FixtureResponse {
        version_response_with_storage(api_version, worker, true)
    }

    fn version_response_with_storage(
        api_version: &str,
        worker: bool,
        store_document_text: bool,
    ) -> FixtureResponse {
        response(
            "200 OK",
            json!({
                "api_version":api_version,
                "features":{
                    "audit_log":false,
                    "bank_config_api":true,
                    "bank_llm_health":true,
                    "document_export_api":true,
                    "document_import_api":true,
                    "file_upload_api":true,
                    "llm_trace":false,
                    "mcp":false,
                    "observations":true,
                    "store_document_text":store_document_text,
                    "worker":worker
                }
            }),
        )
    }

    fn parsed<T>(
        value: &str,
        parser: fn(&str) -> Result<T, tiber_memory_core::MemoryContractError>,
    ) -> T {
        parser(value).expect("fixture semantic value parses")
    }

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

    fn owner_global_scope(repository: &str) -> MemoryScope {
        MemoryScope::owner_global(
            parsed("owner", OwnerId::parse),
            parsed(repository, RepositoryId::parse),
            parsed("agent", AgentId::parse),
            parsed("session", SessionId::parse),
            parsed("task", TaskId::parse),
            parsed("turn-summary", MemoryKind::parse),
        )
    }

    fn options(milliseconds: u64) -> MemoryRequestOptions {
        MemoryRequestOptions::new(
            MemoryDeadline::new(Duration::from_millis(milliseconds)).expect("deadline is valid"),
            MemoryCancellation::default(),
        )
    }

    #[test]
    fn endpoint_accepts_explicit_ipv6_loopback_and_rejects_remote_plain_http() {
        HindsightEndpoint::parse("http://[::1]:8000/").expect("IPv6 loopback HTTP is safe");
        HindsightEndpoint::parse("http://[2001:db8::1]:8000/")
            .expect_err("remote IPv6 plain HTTP is unsafe");
        HindsightEndpoint::parse("https://example.com/").expect("remote HTTPS is safe");
    }

    fn response_tags(turn_id: Option<&str>) -> Value {
        let mut tags = vec![
            "owner:owner".to_owned(),
            "repository:repo".to_owned(),
            "agent:agent".to_owned(),
            "session:session".to_owned(),
            "task:task".to_owned(),
            "kind:turn-summary".to_owned(),
        ];
        if let Some(turn_id) = turn_id {
            tags.push(format!("turn:{turn_id}"));
        }
        json!(tags)
    }

    #[tokio::test]
    async fn retain_probes_083_and_sends_one_strict_async_document() {
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response(
                "200 OK",
                json!({"success":true,"bank_id":"tiber-repository-repo","items_count":1,"async":true,"operation_id":"op-1"}),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let outcome = client
            .retain(&request, &options(1_000))
            .await
            .expect("retain is accepted");
        assert_eq!(outcome.operation_id().as_str(), "op-1");
        let probe = requests.recv().await.expect("version request recorded");
        assert_eq!(
            (probe.method.as_str(), probe.path.as_str()),
            ("GET", "/version")
        );
        let retain = requests.recv().await.expect("retain request recorded");
        assert_eq!(retain.method, "POST");
        assert_eq!(
            retain.path,
            "/v1/default/banks/tiber-repository-repo/memories"
        );
        assert_eq!(retain.body["async"], true);
        assert_eq!(
            retain.body["items"][0]["metadata"]["tiber_retain_evidence_v1"],
            request.expected_evidence().as_str()
        );
        assert_eq!(
            retain.body["items"][0]["document_id"],
            request.backend_document_id().as_str()
        );
        assert_eq!(
            retain.body["items"][0]["tags"],
            json!([
                "owner:owner",
                "repository:repo",
                "agent:agent",
                "session:session",
                "task:task",
                "kind:turn-summary",
                "turn:turn-1"
            ])
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn owner_global_retain_and_forget_are_scoped_by_repository_on_the_wire() {
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response(
                "200 OK",
                json!({"success":true,"bank_id":"tiber-owner-owner","items_count":1,"async":true,"operation_id":"op-one"}),
            ),
            version(),
            response(
                "200 OK",
                json!({"success":true,"bank_id":"tiber-owner-owner","items_count":1,"async":true,"operation_id":"op-two"}),
            ),
            version(),
            response("404 Not Found", json!({"detail":"missing"})),
            version(),
            response("404 Not Found", json!({"detail":"missing"})),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let first = RetainRequest::new(
            owner_global_scope("repo-one"),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("first repository", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );
        let second = RetainRequest::new(
            owner_global_scope("repo-two"),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("second repository", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        client
            .retain(&first, &options(1_000))
            .await
            .expect("first retain is accepted");
        client
            .retain(&second, &options(1_000))
            .await
            .expect("second retain is accepted");
        let first_forget = ForgetRequest::new(first.scope().clone(), first.document_id().clone());
        let second_forget =
            ForgetRequest::new(second.scope().clone(), second.document_id().clone());
        let first_forget_error = client
            .forget(&first_forget, &options(1_000))
            .await
            .expect_err("undocumented missing response is ambiguous");
        assert_eq!(
            first_forget_error.reconciliation(),
            Some(&first_forget.reconciliation_handle())
        );
        let second_forget_error = client
            .forget(&second_forget, &options(1_000))
            .await
            .expect_err("undocumented missing response is ambiguous");
        assert_eq!(
            second_forget_error.reconciliation(),
            Some(&second_forget.reconciliation_handle())
        );

        let _first_probe = requests.recv().await.expect("first probe recorded");
        let first_wire = requests.recv().await.expect("first retain recorded");
        let _second_probe = requests.recv().await.expect("second probe recorded");
        let second_wire = requests.recv().await.expect("second retain recorded");
        let first_id = first.backend_document_id();
        let second_id = second.backend_document_id();
        assert_eq!(
            first_wire.body["items"][0]["document_id"],
            first_id.as_str()
        );
        assert_eq!(
            second_wire.body["items"][0]["document_id"],
            second_id.as_str()
        );
        assert_ne!(first_id, second_id);
        assert_eq!(first_wire.path, second_wire.path);
        let _first_forget_probe = requests.recv().await.expect("first forget probe recorded");
        let first_delete = requests.recv().await.expect("first delete recorded");
        let _second_forget_probe = requests.recv().await.expect("second forget probe recorded");
        let second_delete = requests.recv().await.expect("second delete recorded");
        assert_eq!(
            first_delete.path,
            format!(
                "/v1/default/banks/tiber-owner-owner/documents/{}",
                first_forget.backend_document_id().as_str()
            )
        );
        assert_eq!(
            second_delete.path,
            format!(
                "/v1/default/banks/tiber-owner-owner/documents/{}",
                second_forget.backend_document_id().as_str()
            )
        );
        assert_ne!(first_delete.path, second_delete.path);
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn version_mismatch_refuses_retain_before_dispatch() {
        let (endpoint, mut requests, task) = server(vec![version_response("0.8.2", true)]).await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("unsupported version is refused");
        assert_eq!(error.kind(), MemoryBackendErrorKind::Unsupported);
        let probe_request = requests.recv().await.expect("version request recorded");
        assert_eq!(probe_request.path, "/version");
        task.await.expect("fixture completes");
        assert!(requests.try_recv().is_err(), "retain was never dispatched");
    }

    #[tokio::test]
    async fn dot_segment_retain_operation_id_is_refused_as_unknown() {
        let (endpoint, _requests, task) = server(vec![
            version(),
            response(
                "200 OK",
                json!({"success":true,"bank_id":"tiber-repository-repo","items_count":1,"async":true,"operation_id":".."}),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("unsafe backend operation identity is never surfaced");
        assert_eq!(error.kind(), MemoryBackendErrorKind::OutcomeUnknown);
        assert_eq!(
            error.reconciliation(),
            Some(&request.reconciliation_handle())
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn worker_disabled_refuses_async_retain_before_dispatch() {
        let (endpoint, mut requests, task) = server(vec![version_response("0.8.3", false)]).await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("disabled worker is incompatible with async retain");
        assert_eq!(error.kind(), MemoryBackendErrorKind::Unsupported);
        let probe = requests.recv().await.expect("version request recorded");
        assert_eq!(
            (probe.method.as_str(), probe.path.as_str()),
            ("GET", "/version")
        );
        task.await.expect("fixture completes");
        assert!(requests.try_recv().is_err(), "retain was never dispatched");
    }

    #[tokio::test]
    async fn document_text_storage_disabled_refuses_retain_before_dispatch() {
        let (endpoint, mut requests, task) =
            server(vec![version_response_with_storage("0.8.3", true, false)]).await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("disabled document storage cannot preserve retain evidence");
        assert_eq!(error.kind(), MemoryBackendErrorKind::Unsupported);
        let probe = requests.recv().await.expect("version request recorded");
        assert_eq!(
            (probe.method.as_str(), probe.path.as_str()),
            ("GET", "/version")
        );
        task.await.expect("fixture completes");
        assert!(requests.try_recv().is_err(), "retain was never dispatched");
    }

    #[tokio::test]
    async fn recall_returns_only_prior_strictly_scoped_candidates() {
        let all_tags = response_tags(Some("prior"));
        let stripped_tags = response_tags(None);
        let current_tags = response_tags(Some("current"));
        let malformed_turn_tags = response_tags(Some("x:y"));
        let recall_scope = scope();
        let current_document = parsed("doc-current", MemoryDocumentId::parse);
        let current_backend_document = recall_scope.backend_document_id(&current_document);
        let prior_backend_document =
            recall_scope.backend_document_id(&parsed("doc-prior", MemoryDocumentId::parse));
        let cross_scope_document = owner_global_scope("other-repository")
            .backend_document_id(&parsed("doc-cross", MemoryDocumentId::parse));
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response("200 OK", json!({"results":[
                {"id":"cross","type":"world","text":"other scope","document_id":cross_scope_document.as_str(),"tags":all_tags.clone()},
                {"id":"observation","type":"observation","text":"ambiguous provenance","document_id":prior_backend_document.as_str(),"tags":all_tags.clone()},
                {"id":"stripped","type":"experience","text":"missing turn provenance","document_id":prior_backend_document.as_str(),"tags":stripped_tags},
                {"id":"current","type":"world","text":"current turn","document_id":current_backend_document.as_str(),"tags":current_tags},
                {"id":"malformed-turn","type":"world","text":"invalid turn provenance","document_id":prior_backend_document.as_str(),"tags":malformed_turn_tags},
                {"id":"prior","type":"experience","text":"prior useful fact","document_id":prior_backend_document.as_str(),"tags":all_tags}
            ]})),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RecallRequest::new(
            recall_scope,
            parsed("current", TurnId::parse),
            current_document,
            parsed("what changed", MemoryText::parse),
            RecallItemBudget::new(4).expect("item budget is valid"),
            RecallTokenBudget::new(64).expect("token budget is valid"),
        )
        .expect("recall request is valid");

        let result = client
            .recall(&request, &options(1_000))
            .await
            .expect("recall succeeds");
        assert_eq!(result.memories().len(), 1);
        assert_eq!(result.memories()[0].id().as_str(), "prior");
        let _probe = requests.recv().await.expect("version request recorded");
        let recall = requests.recv().await.expect("recall request recorded");
        assert_eq!(
            recall.body["tag_groups"],
            json!([
                {
                    "tags": [
                        "owner:owner",
                        "repository:repo",
                        "agent:agent",
                        "session:session",
                        "task:task",
                        "kind:turn-summary"
                    ],
                    "match": "all_strict"
                },
                {
                    "not": {
                        "tags": ["turn:current"],
                        "match": "all_strict"
                    }
                }
            ])
        );
        assert!(recall.body.get("tags").is_none());
        assert!(recall.body.get("tags_match").is_none());
        assert_eq!(
            recall.body["include"],
            json!({"entities":null,"chunks":null,"source_facts":null})
        );
        assert_eq!(recall.body["max_tokens"], 64);
        assert_eq!(recall.body["types"], json!(["world", "experience"]));
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn malformed_backend_document_identity_refuses_recall() {
        let scoped_tags = json!([
            "owner:owner",
            "repository:repo",
            "agent:agent",
            "session:session",
            "task:task",
            "kind:turn-summary"
        ]);
        let (endpoint, _requests, task) = server(vec![
            version(),
            response(
                "200 OK",
                json!({"results":[{
                    "id":"malformed",
                    "type":"world",
                    "text":"untrusted",
                    "document_id":"garbage:doc",
                    "tags":scoped_tags
                }]}),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RecallRequest::new(
            scope(),
            parsed("current", TurnId::parse),
            parsed("doc-current", MemoryDocumentId::parse),
            parsed("what changed", MemoryText::parse),
            RecallItemBudget::new(4).expect("item budget is valid"),
            RecallTokenBudget::new(64).expect("token budget is valid"),
        )
        .expect("recall request is valid");

        let error = client
            .recall(&request, &options(1_000))
            .await
            .expect_err("malformed backend provenance is refused");
        assert_eq!(error.kind(), MemoryBackendErrorKind::Protocol);
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn stalled_dispatched_retain_is_unknown_and_never_replayed() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds");
        let endpoint = HindsightEndpoint::parse(&format!(
            "http://{}/",
            listener.local_addr().expect("address")
        ))
        .expect("endpoint parses");
        let server = tokio::spawn(async move {
            let (mut probe, _) = listener.accept().await.expect("probe accepted");
            let _request = read_request(&mut probe).await;
            write_response(&mut probe, version()).await;
            let (mut retain, _) = listener.accept().await.expect("retain accepted");
            let request = read_request(&mut retain).await;
            assert_eq!(request.method, "POST");
            sleep(Duration::from_millis(100)).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err(),
                "mutation is not replayed"
            );
        });
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(30))
            .await
            .expect_err("deadline makes outcome ambiguous");
        assert_eq!(error.kind(), MemoryBackendErrorKind::OutcomeUnknown);
        server.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn ambiguous_async_retain_document_absence_stays_unknown_without_replay() {
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response("500 Internal Server Error", json!({"detail":"uncertain"})),
            version(),
            response("404 Not Found", json!({"detail":"not materialized"})),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("server failure leaves retain ambiguous");
        let reconciliation_handle = error
            .reconciliation()
            .expect("ambiguous retain exposes reconciliation")
            .clone();
        let outcome = client
            .reconcile(
                &ReconcileRequest::new(reconciliation_handle),
                &options(1_000),
            )
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(outcome, ReconcileOutcome::StillUnknown);

        let retain_probe = requests.recv().await.expect("retain probe recorded");
        let retain = requests.recv().await.expect("retain recorded");
        let reconcile_probe = requests.recv().await.expect("reconcile probe recorded");
        let reconciliation_request = requests.recv().await.expect("reconcile GET recorded");
        assert_eq!(
            (retain_probe.method.as_str(), retain_probe.path.as_str()),
            ("GET", "/version")
        );
        assert_eq!(retain.method, "POST");
        assert_eq!(
            (
                reconcile_probe.method.as_str(),
                reconcile_probe.path.as_str()
            ),
            ("GET", "/version")
        );
        assert_eq!(reconciliation_request.method, "GET");
        assert_eq!(
            reconciliation_request.path,
            format!(
                "/v1/default/banks/tiber-repository-repo/documents/{}",
                request.backend_document_id().as_str()
            )
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn unrecognized_post_dispatch_status_preserves_retain_reconciliation() {
        let (endpoint, _requests, task) = server(vec![
            version(),
            response("302 Found", json!({"detail":"unexpected redirect"})),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("unrecognized dispatched status is ambiguous");
        assert_eq!(error.kind(), MemoryBackendErrorKind::OutcomeUnknown);
        assert_eq!(
            error.reconciliation(),
            Some(&request.reconciliation_handle())
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn generic_client_status_preserves_retain_reconciliation() {
        let (endpoint, _requests, task) = server(vec![
            version(),
            response("400 Bad Request", json!({"detail":"ambiguous rejection"})),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("generic dispatched rejection is ambiguous");
        assert_eq!(error.kind(), MemoryBackendErrorKind::OutcomeUnknown);
        assert_eq!(
            error.reconciliation(),
            Some(&request.reconciliation_handle())
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn ambiguous_retain_reconciles_applied_from_exact_document_evidence() {
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );
        let document_id = request.backend_document_id();
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response("500 Internal Server Error", json!({"detail":"uncertain"})),
            version(),
            response(
                "200 OK",
                json!({
                    "bank_id":"tiber-repository-repo",
                    "document_metadata":{
                        "tiber_retain_evidence_v1":request.expected_evidence().as_str()
                    },
                    "id":document_id.as_str(),
                    "tags":[
                        "owner:owner",
                        "repository:repo",
                        "agent:agent",
                        "session:session",
                        "task:task",
                        "kind:turn-summary",
                        "turn:turn-1"
                    ]
                }),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("server failure leaves retain ambiguous");
        let reconciliation_handle = error
            .reconciliation()
            .expect("ambiguous retain exposes reconciliation")
            .clone();
        let outcome = client
            .reconcile(
                &ReconcileRequest::new(reconciliation_handle),
                &options(1_000),
            )
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(outcome, ReconcileOutcome::Applied);

        let _retain_probe = requests.recv().await.expect("retain probe recorded");
        let retain = requests.recv().await.expect("retain recorded");
        let _reconcile_probe = requests.recv().await.expect("reconcile probe recorded");
        let reconciliation_request = requests.recv().await.expect("reconcile GET recorded");
        assert_eq!(retain.method, "POST");
        assert_eq!(reconciliation_request.method, "GET");
        assert_eq!(
            reconciliation_request.path,
            format!(
                "/v1/default/banks/tiber-repository-repo/documents/{}",
                document_id.as_str()
            )
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn ambiguous_retain_rejects_same_turn_different_effect_evidence() {
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-current", TurnId::parse),
            parsed("new durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );
        let stale_request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-current", TurnId::parse),
            parsed("old durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );
        let document_id = request.backend_document_id();
        let (endpoint, _requests, task) = server(vec![
            version(),
            response("500 Internal Server Error", json!({"detail":"uncertain"})),
            version(),
            response(
                "200 OK",
                json!({
                    "bank_id":"tiber-repository-repo",
                    "document_metadata":{
                        "tiber_retain_evidence_v1":stale_request.expected_evidence().as_str()
                    },
                    "id":document_id.as_str(),
                    "tags":[
                        "owner:owner",
                        "repository:repo",
                        "agent:agent",
                        "session:session",
                        "task:task",
                        "kind:turn-summary",
                        "turn:turn-current"
                    ]
                }),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");

        let error = client
            .retain(&request, &options(1_000))
            .await
            .expect_err("server failure leaves retain ambiguous");
        let reconciliation_handle = error
            .reconciliation()
            .expect("ambiguous retain exposes reconciliation")
            .clone();
        let outcome = client
            .reconcile(
                &ReconcileRequest::new(reconciliation_handle),
                &options(1_000),
            )
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(outcome, ReconcileOutcome::StillUnknown);
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn ambiguous_forget_absence_stays_unknown_without_replay() {
        let request = ForgetRequest::new(scope(), parsed("document-1", MemoryDocumentId::parse));
        let document_id = request.backend_document_id();
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response("500 Internal Server Error", json!({"detail":"uncertain"})),
            version(),
            response("404 Not Found", json!({"detail":"absent"})),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");

        let error = client
            .forget(&request, &options(1_000))
            .await
            .expect_err("server failure leaves forget ambiguous");
        let reconciliation_handle = error
            .reconciliation()
            .expect("ambiguous forget exposes reconciliation")
            .clone();
        let outcome = client
            .reconcile(
                &ReconcileRequest::new(reconciliation_handle),
                &options(1_000),
            )
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(outcome, ReconcileOutcome::StillUnknown);

        let forget_probe = requests.recv().await.expect("forget probe recorded");
        let forget = requests.recv().await.expect("forget recorded");
        let reconcile_probe = requests.recv().await.expect("reconcile probe recorded");
        let reconciliation_request = requests.recv().await.expect("reconcile GET recorded");
        assert_eq!(
            (forget_probe.method.as_str(), forget_probe.path.as_str()),
            ("GET", "/version")
        );
        assert_eq!(forget.method, "DELETE");
        assert_eq!(
            (
                reconcile_probe.method.as_str(),
                reconcile_probe.path.as_str()
            ),
            ("GET", "/version")
        );
        assert_eq!(reconciliation_request.method, "GET");
        assert_eq!(
            reconciliation_request.path,
            format!(
                "/v1/default/banks/tiber-repository-repo/documents/{}",
                document_id.as_str()
            )
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn ambiguous_forget_recreated_document_stays_unknown_without_replay() {
        let request = ForgetRequest::new(scope(), parsed("document-1", MemoryDocumentId::parse));
        let document_id = request.backend_document_id();
        let (endpoint, _requests, task) = server(vec![
            version(),
            response("500 Internal Server Error", json!({"detail":"uncertain"})),
            version(),
            response(
                "200 OK",
                json!({
                    "bank_id":"tiber-repository-repo",
                    "id":document_id.as_str(),
                    "tags":[
                        "owner:owner",
                        "repository:repo",
                        "agent:agent",
                        "session:session",
                        "task:task",
                        "kind:turn-summary",
                        "turn:recreated"
                    ]
                }),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");

        let error = client
            .forget(&request, &options(1_000))
            .await
            .expect_err("server failure leaves forget ambiguous");
        let reconciliation_handle = error
            .reconciliation()
            .expect("ambiguous forget exposes reconciliation")
            .clone();
        let outcome = client
            .reconcile(
                &ReconcileRequest::new(reconciliation_handle),
                &options(1_000),
            )
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(outcome, ReconcileOutcome::StillUnknown);
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn ambiguous_forget_refuses_malformed_turn_document_evidence() {
        let request = ForgetRequest::new(scope(), parsed("document-1", MemoryDocumentId::parse));
        let document_id = request.backend_document_id();
        let (endpoint, _requests, task) = server(vec![
            version(),
            response("500 Internal Server Error", json!({"detail":"uncertain"})),
            version(),
            response(
                "200 OK",
                json!({
                    "bank_id":"tiber-repository-repo",
                    "id":document_id.as_str(),
                    "tags":[
                        "owner:owner",
                        "repository:repo",
                        "agent:agent",
                        "session:session",
                        "task:task",
                        "kind:turn-summary",
                        "turn:"
                    ]
                }),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");

        let error = client
            .forget(&request, &options(1_000))
            .await
            .expect_err("server failure leaves forget ambiguous");
        let reconciliation_handle = error
            .reconciliation()
            .expect("ambiguous forget exposes reconciliation")
            .clone();
        let outcome = client
            .reconcile(
                &ReconcileRequest::new(reconciliation_handle),
                &options(1_000),
            )
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(outcome, ReconcileOutcome::StillUnknown);
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn ambiguous_cancel_reconciles_applied_from_scoped_operation_status() {
        let retained = RetainRequest::new(
            scope(),
            parsed("status-document", MemoryDocumentId::parse),
            parsed("status-turn", TurnId::parse),
            parsed("status content", MemoryText::parse),
            parsed("status context", MemoryText::parse),
        );
        let operation = parsed("op-cancel", MemoryOperationId::parse);
        let operation_handle = RetainOutcome::accepted(&retained, operation)
            .operation()
            .clone();
        let request = CancelRequest::new(operation_handle);
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response("500 Internal Server Error", json!({"detail":"uncertain"})),
            version(),
            response(
                "200 OK",
                json!({"operation_id":"op-cancel","status":"cancelled"}),
            ),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");

        let error = client
            .cancel(&request, &options(1_000))
            .await
            .expect_err("server failure leaves cancellation ambiguous");
        let reconciliation_handle = error
            .reconciliation()
            .expect("ambiguous cancellation exposes reconciliation")
            .clone();
        let outcome = client
            .reconcile(
                &ReconcileRequest::new(reconciliation_handle),
                &options(1_000),
            )
            .await
            .expect("read-only reconciliation completes");
        assert_eq!(outcome, ReconcileOutcome::Applied);

        let cancel_probe = requests.recv().await.expect("cancel probe recorded");
        let cancel = requests.recv().await.expect("cancel recorded");
        let reconcile_probe = requests.recv().await.expect("reconcile probe recorded");
        let reconciliation_request = requests.recv().await.expect("reconcile GET recorded");
        assert_eq!(
            (cancel_probe.method.as_str(), cancel_probe.path.as_str()),
            ("GET", "/version")
        );
        assert_eq!(cancel.method, "DELETE");
        assert_eq!(
            (
                reconcile_probe.method.as_str(),
                reconcile_probe.path.as_str()
            ),
            ("GET", "/version")
        );
        assert_eq!(reconciliation_request.method, "GET");
        assert_eq!(cancel.path, reconciliation_request.path);
        assert_eq!(
            reconciliation_request.path,
            "/v1/default/banks/tiber-repository-repo/operations/op-cancel"
        );
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn cancel_reconciliation_maps_every_conservative_operation_state() {
        let cases = [
            ("pending", ReconcileOutcome::Pending),
            ("processing", ReconcileOutcome::Pending),
            ("completed", ReconcileOutcome::NotApplied),
            ("failed", ReconcileOutcome::NotApplied),
            ("not_found", ReconcileOutcome::StillUnknown),
        ];
        for (status, expected) in cases {
            let retained = RetainRequest::new(
                scope(),
                parsed("status-document", MemoryDocumentId::parse),
                parsed("status-turn", TurnId::parse),
                parsed("status content", MemoryText::parse),
                parsed("status context", MemoryText::parse),
            );
            let operation = parsed("op-cancel", MemoryOperationId::parse);
            let operation_handle = RetainOutcome::accepted(&retained, operation)
                .operation()
                .clone();
            let request = CancelRequest::new(operation_handle);
            let (endpoint, _requests, task) = server(vec![
                version(),
                response(
                    "200 OK",
                    json!({"operation_id":"op-cancel","status":status}),
                ),
            ])
            .await;
            let client = HindsightHttp::new(endpoint).expect("client builds");
            let outcome = client
                .reconcile(
                    &ReconcileRequest::new(request.reconciliation_handle()),
                    &options(1_000),
                )
                .await
                .expect("status reconciliation completes");
            assert_eq!(outcome, expected);
            task.await.expect("fixture completes");
        }

        for operation_response in [
            response("404 Not Found", json!({"detail":"missing"})),
            response(
                "200 OK",
                json!({"operation_id":"other-operation","status":"cancelled"}),
            ),
        ] {
            let retained = RetainRequest::new(
                scope(),
                parsed("status-document", MemoryDocumentId::parse),
                parsed("status-turn", TurnId::parse),
                parsed("status content", MemoryText::parse),
                parsed("status context", MemoryText::parse),
            );
            let operation = parsed("op-cancel", MemoryOperationId::parse);
            let operation_handle = RetainOutcome::accepted(&retained, operation)
                .operation()
                .clone();
            let request = CancelRequest::new(operation_handle);
            let (endpoint, _requests, task) = server(vec![version(), operation_response]).await;
            let client = HindsightHttp::new(endpoint).expect("client builds");
            let outcome = client
                .reconcile(
                    &ReconcileRequest::new(request.reconciliation_handle()),
                    &options(1_000),
                )
                .await
                .expect("conservative reconciliation completes");
            assert_eq!(outcome, ReconcileOutcome::StillUnknown);
            task.await.expect("fixture completes");
        }
    }

    #[tokio::test]
    async fn ready_cancellation_before_mutation_poll_is_definitive_without_dispatch() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds");
        let cancellation = MemoryCancellation::default();
        let options = MemoryRequestOptions::new(
            MemoryDeadline::new(Duration::from_secs(1)).expect("deadline is valid"),
            cancellation.clone(),
        );
        let budget = OperationBudget::new(&options);
        let url = format!(
            "http://{}/mutation",
            listener.local_addr().expect("fixture has address")
        );
        cancellation.cancel();

        let failure = budget
            .send_mutation(reqwest::Client::new().delete(url))
            .await
            .expect_err("ready cancellation wins before reqwest is polled");
        let error = mutation_send_error(
            MemoryOperationKind::Retain,
            failure,
            RetainRequest::new(
                scope(),
                parsed("document", MemoryDocumentId::parse),
                parsed("turn", TurnId::parse),
                parsed("content", MemoryText::parse),
                parsed("context", MemoryText::parse),
            )
            .reconciliation_handle(),
        );
        assert_eq!(error.kind(), MemoryBackendErrorKind::Cancelled);
        assert!(error.reconciliation().is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err(),
            "pre-dispatch cancellation opens no connection"
        );
    }

    #[tokio::test]
    async fn ready_deadline_before_mutation_poll_is_retryable_without_dispatch() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds");
        let options = MemoryRequestOptions::new(
            MemoryDeadline::new(Duration::from_millis(1)).expect("deadline is valid"),
            MemoryCancellation::default(),
        );
        let budget = OperationBudget::new(&options);
        sleep(Duration::from_millis(5)).await;
        let url = format!(
            "http://{}/mutation",
            listener.local_addr().expect("fixture has address")
        );

        let failure = budget
            .send_mutation(reqwest::Client::new().delete(url))
            .await
            .expect_err("ready deadline wins before reqwest is polled");
        let error = mutation_send_error(
            MemoryOperationKind::Retain,
            failure,
            RetainRequest::new(
                scope(),
                parsed("document", MemoryDocumentId::parse),
                parsed("turn", TurnId::parse),
                parsed("content", MemoryText::parse),
                parsed("context", MemoryText::parse),
            )
            .reconciliation_handle(),
        );
        assert_eq!(error.kind(), MemoryBackendErrorKind::DeadlineExceeded);
        assert_eq!(error.retryability(), MemoryRetryability::Retryable);
        assert!(error.reconciliation().is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err(),
            "pre-dispatch expiry opens no connection"
        );
    }

    #[tokio::test]
    async fn cancellation_after_retain_dispatch_is_unknown_and_never_replayed() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds");
        let endpoint = HindsightEndpoint::parse(&format!(
            "http://{}/",
            listener.local_addr().expect("address")
        ))
        .expect("endpoint parses");
        let (dispatched_sender, dispatched_receiver) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut probe, _) = listener.accept().await.expect("probe accepted");
            let _request = read_request(&mut probe).await;
            write_response(&mut probe, version()).await;
            let (mut retain, _) = listener.accept().await.expect("retain accepted");
            let request = read_request(&mut retain).await;
            assert_eq!(request.method, "POST");
            dispatched_sender
                .send(())
                .expect("fixture signals retain dispatch");
            sleep(Duration::from_millis(100)).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err(),
                "cancelled mutation is not replayed"
            );
        });
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let request = RetainRequest::new(
            scope(),
            parsed("document-1", MemoryDocumentId::parse),
            parsed("turn-1", TurnId::parse),
            parsed("durable decision", MemoryText::parse),
            parsed("Tiber turn transcript", MemoryText::parse),
        );
        let cancellation = MemoryCancellation::default();
        let options = MemoryRequestOptions::new(
            MemoryDeadline::new(Duration::from_secs(1)).expect("deadline is valid"),
            cancellation.clone(),
        );
        let canceller = tokio::spawn(async move {
            dispatched_receiver
                .await
                .expect("retain dispatch is observed before cancellation");
            cancellation.cancel();
        });

        let error = client
            .retain(&request, &options)
            .await
            .expect_err("post-dispatch cancellation is ambiguous");
        assert_eq!(error.kind(), MemoryBackendErrorKind::OutcomeUnknown);
        canceller.await.expect("canceller completes");
        server.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn status_cancel_and_forget_use_their_narrow_scoped_paths() {
        let (endpoint, mut requests, task) = server(vec![
            version(),
            response(
                "200 OK",
                json!({"operation_id":"op-1","status":"processing"}),
            ),
            version(),
            response("409 Conflict", json!({"detail":"already processing"})),
            version(),
            response("404 Not Found", json!({"detail":"missing"})),
        ])
        .await;
        let client = HindsightHttp::new(endpoint).expect("client builds");
        let operation = parsed("op-1", MemoryOperationId::parse);
        let retained = RetainRequest::new(
            scope(),
            parsed("status-document", MemoryDocumentId::parse),
            parsed("status-turn", TurnId::parse),
            parsed("status content", MemoryText::parse),
            parsed("status context", MemoryText::parse),
        );
        let operation_handle = RetainOutcome::accepted(&retained, operation)
            .operation()
            .clone();
        let forget_request =
            ForgetRequest::new(scope(), parsed("doc-old", MemoryDocumentId::parse));
        let status = client
            .operation_status(
                &OperationStatusRequest::new(operation_handle.clone()),
                &options(1_000),
            )
            .await
            .expect("status succeeds");
        assert_eq!(
            status.state(),
            tiber_memory_core::MemoryOperationState::Processing
        );
        let cancel_request = CancelRequest::new(operation_handle);
        let cancel_error = client
            .cancel(&cancel_request, &options(1_000))
            .await
            .expect_err("undocumented conflict is ambiguous");
        assert_eq!(
            cancel_error.reconciliation(),
            Some(&cancel_request.reconciliation_handle())
        );
        let forget_error = client
            .forget(&forget_request, &options(1_000))
            .await
            .expect_err("undocumented missing response is ambiguous");
        assert_eq!(
            forget_error.reconciliation(),
            Some(&forget_request.reconciliation_handle())
        );
        let mut paths = Vec::new();
        while let Some(request) = requests.recv().await {
            paths.push((request.method, request.path));
        }
        assert!(paths.contains(&(
            "GET".to_owned(),
            "/v1/default/banks/tiber-repository-repo/operations/op-1".to_owned()
        )));
        assert!(paths.contains(&(
            "DELETE".to_owned(),
            "/v1/default/banks/tiber-repository-repo/operations/op-1".to_owned()
        )));
        assert!(paths.contains(&(
            "DELETE".to_owned(),
            format!(
                "/v1/default/banks/tiber-repository-repo/documents/{}",
                forget_request.backend_document_id().as_str()
            )
        )));
        task.await.expect("fixture completes");
    }

    #[tokio::test]
    async fn dot_segment_operation_id_never_reaches_status_or_cancel_routes() {
        let retained = RetainRequest::new(
            scope(),
            parsed("status-document", MemoryDocumentId::parse),
            parsed("status-turn", TurnId::parse),
            parsed("status content", MemoryText::parse),
            parsed("status context", MemoryText::parse),
        );
        let operation_handle =
            RetainOutcome::accepted(&retained, parsed("..", MemoryOperationId::parse))
                .operation()
                .clone();

        let (status_endpoint, mut status_requests, status_task) = server(vec![version()]).await;
        let status_client = HindsightHttp::new(status_endpoint).expect("client builds");
        let status_error = status_client
            .operation_status(
                &OperationStatusRequest::new(operation_handle.clone()),
                &options(1_000),
            )
            .await
            .expect_err("dot segment is not a status path segment");
        assert_eq!(status_error.kind(), MemoryBackendErrorKind::Protocol);
        let status_probe = status_requests
            .recv()
            .await
            .expect("only version probe is recorded");
        assert_eq!(
            (status_probe.method.as_str(), status_probe.path.as_str()),
            ("GET", "/version")
        );
        status_task.await.expect("fixture sees no status route");

        let cancel_request = CancelRequest::new(operation_handle.clone());
        let (cancel_endpoint, mut cancel_requests, cancel_task) = server(vec![version()]).await;
        let cancel_client = HindsightHttp::new(cancel_endpoint).expect("client builds");
        let cancel_error = cancel_client
            .cancel(&cancel_request, &options(1_000))
            .await
            .expect_err("dot segment is not a cancellation path segment");
        assert_eq!(cancel_error.kind(), MemoryBackendErrorKind::Protocol);
        let cancel_probe = cancel_requests
            .recv()
            .await
            .expect("only version probe is recorded");
        assert_eq!(
            (cancel_probe.method.as_str(), cancel_probe.path.as_str()),
            ("GET", "/version")
        );
        cancel_task
            .await
            .expect("fixture sees no cancellation route");

        let (reconcile_endpoint, mut reconcile_requests, reconcile_task) =
            server(vec![version()]).await;
        let reconcile_client = HindsightHttp::new(reconcile_endpoint).expect("client builds");
        let reconcile_error = reconcile_client
            .reconcile(
                &ReconcileRequest::new(cancel_request.reconciliation_handle()),
                &options(1_000),
            )
            .await
            .expect_err("dot segment is not a reconciliation path segment");
        assert_eq!(reconcile_error.kind(), MemoryBackendErrorKind::Protocol);
        let reconcile_probe = reconcile_requests
            .recv()
            .await
            .expect("only version probe is recorded");
        assert_eq!(
            (
                reconcile_probe.method.as_str(),
                reconcile_probe.path.as_str()
            ),
            ("GET", "/version")
        );
        reconcile_task
            .await
            .expect("fixture sees no reconciliation route");
    }
}
