#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    clippy::std_instead_of_core,
    reason = "the ignored live fixture uses idiomatic Result propagation and small lifecycle helpers while keeping scrubbed failure codes explicit"
)]
mod tests {
    use std::{env, ffi::OsStr, time::Duration};

    use tiber_hindsight_http::{HindsightEndpoint, HindsightHttp, HindsightSetupError};
    use tiber_memory_core::{
        AgentId, ForgetOutcome, ForgetRequest, MemoryBackend as _, MemoryBackendError,
        MemoryCancellation, MemoryContractError, MemoryDeadline, MemoryDocumentId, MemoryKind,
        MemoryOperationState, MemoryRequestOptions, MemoryScope, MemoryText,
        OperationStatusRequest, OwnerId, RecallItemBudget, RecallRequest, RecallTokenBudget,
        ReconcileOutcome, ReconcileRequest, RepositoryId, RetainRequest, SessionId, TaskId, TurnId,
    };
    use tokio::time::{sleep, timeout};
    use uuid::Uuid;

    const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);
    const LIVE_ENDPOINT_ENV: &str = "TIBER_HINDSIGHT_ENDPOINT";
    const LIVE_RUN_ENV: &str = "TIBER_RUN_LIVE_HINDSIGHT";
    const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(90);
    const OPERATION_DEADLINE: Duration = Duration::from_secs(15);
    const POLL_INTERVAL: Duration = Duration::from_millis(500);
    const RECONCILIATION_ATTEMPTS: usize = 8;
    const STATUS_ATTEMPTS: usize = 24;

    type LiveResult<T> = Result<T, LiveFailure>;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct LiveFailure(&'static str);

    fn setup_error(error: HindsightSetupError) -> LiveFailure {
        LiveFailure(match error {
            HindsightSetupError::InvalidEndpoint => "hindsight_invalid_endpoint",
            HindsightSetupError::InsecureEndpoint => "hindsight_insecure_endpoint",
            HindsightSetupError::CredentialsUnsupported => "hindsight_credentials_unsupported",
            HindsightSetupError::EndpointMustBeOrigin => "hindsight_endpoint_must_be_origin",
            HindsightSetupError::ClientConstruction => "hindsight_client_construction",
            _ => "hindsight_setup_unknown",
        })
    }

    const fn contract_error(error: MemoryContractError) -> LiveFailure {
        LiveFailure(error.code())
    }

    fn backend_error(error: &MemoryBackendError) -> LiveFailure {
        LiveFailure(error.code())
    }

    fn configured_endpoint(
        run_live: Option<&OsStr>,
        endpoint: Option<&OsStr>,
    ) -> LiveResult<Option<HindsightEndpoint>> {
        if run_live != Some(OsStr::new("1")) {
            return Ok(None);
        }
        let Some(endpoint) = endpoint else {
            return Ok(None);
        };
        if endpoint.is_empty() {
            return Ok(None);
        }
        let endpoint = endpoint
            .to_str()
            .ok_or(LiveFailure("hindsight_live_endpoint_not_utf8"))?;
        HindsightEndpoint::parse(endpoint)
            .map(Some)
            .map_err(setup_error)
    }

    fn live_endpoint() -> LiveResult<Option<HindsightEndpoint>> {
        let run_live = env::var_os(LIVE_RUN_ENV);
        let endpoint = env::var_os(LIVE_ENDPOINT_ENV);
        configured_endpoint(run_live.as_deref(), endpoint.as_deref())
    }

    fn parsed<T>(value: &str, parser: fn(&str) -> Result<T, MemoryContractError>) -> LiveResult<T> {
        parser(value).map_err(contract_error)
    }

    fn options() -> LiveResult<MemoryRequestOptions> {
        let deadline = MemoryDeadline::new(OPERATION_DEADLINE).map_err(contract_error)?;
        Ok(MemoryRequestOptions::new(
            deadline,
            MemoryCancellation::default(),
        ))
    }

    async fn reconcile_retain(client: &HindsightHttp, request: &RetainRequest) -> LiveResult<()> {
        let reconcile = ReconcileRequest::new(request.reconciliation_handle());
        for _attempt in 0..RECONCILIATION_ATTEMPTS {
            match client.reconcile(&reconcile, &options()?).await {
                Ok(ReconcileOutcome::Applied) => return Ok(()),
                Ok(ReconcileOutcome::NotApplied) => {
                    return Err(LiveFailure("hindsight_live_retain_not_applied"));
                }
                Ok(ReconcileOutcome::Pending | ReconcileOutcome::StillUnknown) => {
                    sleep(POLL_INTERVAL).await;
                }
                Err(error) => return Err(backend_error(&error)),
            }
        }
        Err(LiveFailure("hindsight_live_reconciliation_exhausted"))
    }

    async fn await_retain(client: &HindsightHttp, request: &RetainRequest) -> LiveResult<()> {
        let outcome = match client.retain(request, &options()?).await {
            Ok(outcome) => outcome,
            Err(error) if error.reconciliation().is_some() => {
                return reconcile_retain(client, request).await;
            }
            Err(error) => return Err(backend_error(&error)),
        };
        let status_request = OperationStatusRequest::new(outcome.operation().clone());
        for _attempt in 0..STATUS_ATTEMPTS {
            match client.operation_status(&status_request, &options()?).await {
                Ok(status) if status.state() == MemoryOperationState::Completed => {
                    return reconcile_retain(client, request).await;
                }
                Ok(status)
                    if matches!(
                        status.state(),
                        MemoryOperationState::Pending | MemoryOperationState::Processing
                    ) =>
                {
                    sleep(POLL_INTERVAL).await;
                }
                Ok(_) | Err(_) => return reconcile_retain(client, request).await,
            }
        }
        reconcile_retain(client, request).await
    }

    async fn exercise_lifecycle(
        client: &HindsightHttp,
        retain: &RetainRequest,
        recall: &RecallRequest,
    ) -> LiveResult<()> {
        await_retain(client, retain).await?;
        let _bounded_result = client
            .recall(recall, &options()?)
            .await
            .map_err(|error| backend_error(&error))?;
        Ok(())
    }

    async fn forget_document(client: &HindsightHttp, request: &ForgetRequest) -> LiveResult<()> {
        match client.forget(request, &options()?).await {
            Ok(ForgetOutcome::Forgotten | ForgetOutcome::AlreadyAbsent) => Ok(()),
            Err(error) => Err(backend_error(&error)),
        }
    }

    #[test]
    fn live_configuration_requires_both_explicit_opt_ins() {
        let endpoint = OsStr::new("https://example.invalid/");

        assert!(matches!(
            configured_endpoint(None, Some(endpoint)),
            Ok(None)
        ));
        assert!(matches!(
            configured_endpoint(Some(OsStr::new("true")), Some(endpoint)),
            Ok(None)
        ));
        assert!(matches!(
            configured_endpoint(Some(OsStr::new("1")), None),
            Ok(None)
        ));
        assert!(matches!(
            configured_endpoint(Some(OsStr::new("1")), Some(OsStr::new(""))),
            Ok(None)
        ));
    }

    #[tokio::test]
    #[ignore = "requires explicit TIBER_RUN_LIVE_HINDSIGHT=1 and TIBER_HINDSIGHT_ENDPOINT"]
    async fn opt_in_live_hindsight_completes_bounded_isolated_lifecycle() -> LiveResult<()> {
        let Some(endpoint) = live_endpoint()? else {
            return Ok(());
        };
        let client = HindsightHttp::new(endpoint).map_err(setup_error)?;
        let nonce = Uuid::new_v4().to_string();
        let scope = MemoryScope::repository(
            parsed(&format!("owner-{nonce}"), OwnerId::parse)?,
            parsed(&format!("repository-{nonce}"), RepositoryId::parse)?,
            parsed(&format!("agent-{nonce}"), AgentId::parse)?,
            parsed(&format!("session-{nonce}"), SessionId::parse)?,
            parsed(&format!("task-{nonce}"), TaskId::parse)?,
            parsed(&format!("live-validation-{nonce}"), MemoryKind::parse)?,
        );
        let document_id = parsed(&format!("document-{nonce}"), MemoryDocumentId::parse)?;
        let retain = RetainRequest::new(
            scope.clone(),
            document_id.clone(),
            parsed(&format!("retained-turn-{nonce}"), TurnId::parse)?,
            parsed(
                &format!(
                    "Synthetic Tiber live validation marker {nonce}; this contains no user data"
                ),
                MemoryText::parse,
            )?,
            parsed(
                "Ignored opt-in Tiber adapter integration validation",
                MemoryText::parse,
            )?,
        );
        let recall = RecallRequest::new(
            scope.clone(),
            parsed(&format!("recall-turn-{nonce}"), TurnId::parse)?,
            parsed(&format!("recall-document-{nonce}"), MemoryDocumentId::parse)?,
            parsed(
                &format!("synthetic Tiber live validation marker {nonce}"),
                MemoryText::parse,
            )?,
            RecallItemBudget::new(4).map_err(contract_error)?,
            RecallTokenBudget::new(256).map_err(contract_error)?,
        )
        .map_err(contract_error)?;
        let forget = ForgetRequest::new(scope, document_id);

        let lifecycle = match timeout(
            LIFECYCLE_TIMEOUT,
            exercise_lifecycle(&client, &retain, &recall),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(LiveFailure("hindsight_live_lifecycle_timeout")),
        };
        let cleanup = match timeout(CLEANUP_TIMEOUT, forget_document(&client, &forget)).await {
            Ok(result) => result,
            Err(_elapsed) => Err(LiveFailure("hindsight_live_cleanup_timeout")),
        };

        match cleanup {
            Ok(()) => lifecycle,
            Err(error) => Err(error),
        }
    }
}
