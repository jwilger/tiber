#![forbid(unsafe_code)]
#![expect(
    clippy::arbitrary_source_item_ordering,
    clippy::pub_use,
    reason = "the public protocol checker precedes the isolated runtime module while the runtime implementation remains privately scoped"
)]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::{error::Error, fmt};

/// SHA-256 of the reviewed full Codex 0.147.0 V2 schema.
const CODEX_0_147_SCHEMA_SHA256: &str =
    "ff10829cd75b67297019b39ab508ac699198574663579aa18336b7dc55ea178f";
/// Ordered discriminators retained in the reviewed authority-surface projection.
const CODEX_0_147_THREAD_ITEM_TYPES: [&str; 18] = [
    "userMessage",
    "hookPrompt",
    "agentMessage",
    "plan",
    "reasoning",
    "commandExecution",
    "fileChange",
    "mcpToolCall",
    "dynamicToolCall",
    "collabAgentToolCall",
    "subAgentActivity",
    "webSearch",
    "imageView",
    "sleep",
    "imageGeneration",
    "enteredReviewMode",
    "exitedReviewMode",
    "contextCompaction",
];
/// Complete ordered field set from the reviewed `ThreadStartParams` projection.
const CODEX_0_147_THREAD_START_FIELDS: [&str; 25] = [
    "allowProviderModelFallback",
    "approvalPolicy",
    "approvalsReviewer",
    "baseInstructions",
    "config",
    "cwd",
    "developerInstructions",
    "dynamicTools",
    "environments",
    "ephemeral",
    "experimentalRawEvents",
    "historyMode",
    "mockExperimentalField",
    "model",
    "modelProvider",
    "multiAgentMode",
    "permissions",
    "personality",
    "runtimeWorkspaceRoots",
    "sandbox",
    "selectedCapabilityRoots",
    "serviceName",
    "serviceTier",
    "sessionStartSource",
    "threadSource",
];

/// Result of checking whether an app-server protocol exposes the controls used by Tiber.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatibilityReport {
    /// Operation item types whose effects must remain denied or Tiber-mediated at runtime.
    controlled_operations: Vec<String>,
}

impl CompatibilityReport {
    /// Returns protocol item types covered by the runtime effective-authority probe.
    #[expect(
        clippy::implicit_return,
        reason = "a single-expression accessor is clearer than an explicit return"
    )]
    #[inline]
    #[must_use]
    pub fn controlled_operations(&self) -> &[String] {
        &self.controlled_operations
    }

    /// Reports whether the exact reviewed protocol exposes Tiber's required control surface.
    #[expect(
        clippy::implicit_return,
        reason = "a single-expression predicate is clearer than an explicit return"
    )]
    #[inline]
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        true
    }
}

/// A typed failure produced while inspecting the app-server protocol schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatibilityError {
    /// Stable machine-readable classification.
    code: &'static str,
    /// Actionable human-readable diagnostic.
    message: String,
}

impl CompatibilityError {
    /// Stable machine-readable error code.
    #[expect(
        clippy::implicit_return,
        reason = "a single-expression accessor is clearer than an explicit return"
    )]
    #[inline]
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for CompatibilityError {
    #[expect(
        clippy::implicit_return,
        reason = "the formatter directly returns the delegated formatting result"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the Error defaults correctly express a leaf diagnostic with no source"
)]
impl Error for CompatibilityError {}

/// Inspects a provenance-bound app-server authority-surface projection.
///
/// # Errors
///
/// Returns a typed error when the schema is not valid JSON.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "iterator predicates and the final Result use idiomatic expression returns"
)]
pub fn inspect_protocol_schema(schema: &str) -> Result<CompatibilityReport, CompatibilityError> {
    let document = match serde_json::from_str::<serde_json::Value>(schema) {
        Ok(document) => document,
        Err(error) => {
            return Err(CompatibilityError {
                code: "app_server_schema_invalid",
                message: format!("app-server protocol schema is invalid: {error}"),
            });
        }
    };

    if document.get("title").and_then(serde_json::Value::as_str) != Some("CodexAppServerProtocolV2")
    {
        return Err(unrecognized_schema(
            "expected the CodexAppServerProtocolV2 schema title",
        ));
    }
    if document
        .pointer("/_provenance/codexVersion")
        .and_then(serde_json::Value::as_str)
        != Some("0.147.0")
        || document
            .pointer("/_provenance/schemaSha256")
            .and_then(serde_json::Value::as_str)
            != Some(CODEX_0_147_SCHEMA_SHA256)
    {
        return Err(unrecognized_schema(
            "expected the verified Codex 0.147.0 authority-surface projection",
        ));
    }
    let thread_start_properties = match document
        .pointer("/definitions/ThreadStartParams/properties")
        .and_then(serde_json::Value::as_object)
    {
        Some(properties) => properties,
        None => {
            return Err(unrecognized_schema("missing ThreadStartParams.properties"));
        }
    };
    if thread_start_properties.len() != CODEX_0_147_THREAD_START_FIELDS.len()
        || !CODEX_0_147_THREAD_START_FIELDS
            .into_iter()
            .all(|field| thread_start_properties.contains_key(field))
    {
        return Err(unrecognized_schema(
            "ThreadStartParams differs from the verified 0.147 authority surface",
        ));
    }
    let thread_item_types = match parse_thread_item_types(&document) {
        Ok(item_types) => item_types,
        Err(error) => return Err(error),
    };
    if thread_item_types != CODEX_0_147_THREAD_ITEM_TYPES {
        return Err(unrecognized_schema(
            "ThreadItem differs from the verified 0.147 authority surface",
        ));
    }
    let controlled_operations = ["commandExecution", "fileChange"]
        .into_iter()
        .map(|operation| format!("thread-item:{operation}:runtime-policy-controlled"))
        .collect::<Vec<_>>();

    Ok(CompatibilityReport {
        controlled_operations,
    })
}

/// Parses every V2 `ThreadItem` discriminator without searching unrelated schema text.
#[expect(
    clippy::implicit_return,
    reason = "the parser returns its collected semantic values as a final expression"
)]
#[expect(
    clippy::single_call_fn,
    reason = "a named parser keeps schema-shape validation separate and independently reviewable"
)]
fn parse_thread_item_types(document: &serde_json::Value) -> Result<Vec<&str>, CompatibilityError> {
    let variants = match document
        .pointer("/definitions/ThreadItem/oneOf")
        .and_then(serde_json::Value::as_array)
    {
        Some(variants) if !variants.is_empty() => variants,
        Some(_) | None => return Err(unrecognized_schema("missing ThreadItem.oneOf variants")),
    };
    let mut item_types = Vec::with_capacity(variants.len());
    for variant in variants {
        let Some(item_type) = variant
            .pointer("/properties/type/enum/0")
            .and_then(serde_json::Value::as_str)
        else {
            return Err(unrecognized_schema(
                "ThreadItem variant lacks a string type discriminator",
            ));
        };
        item_types.push(item_type);
    }
    Ok(item_types)
}

#[expect(
    clippy::absolute_paths,
    clippy::doc_markdown,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    clippy::missing_trait_methods,
    clippy::needless_pass_by_value,
    clippy::question_mark_used,
    clippy::renamed_function_params,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::single_call_fn,
    clippy::unused_trait_names,
    reason = "the isolated process adapter follows JSON-RPC wire order, uses typed early propagation, and keeps transport implementation details together"
)]
/// Isolated app-server transport implementation.
mod runtime {
    use alloc::{collections::VecDeque, string::String, sync::Arc, vec::Vec};
    use core::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    use std::{
        fs,
        io::{self, BufRead, BufReader, Write},
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Child, ChildStdin, Command, Stdio},
        sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        thread,
        time::Instant,
    };

    use super::{Error, fmt};

    /// Stable permission-profile name whose effective authority is probe-covered.
    pub const INFERENCE_PERMISSION_PROFILE: &str = "tiber-inference";
    /// Exact Codex protocol implementation covered by the reviewed adapter.
    pub const SUPPORTED_CODEX_VERSION: &str = "0.147.0";
    /// Maximum out-of-order envelopes retained during a request.
    const MAX_QUEUED_MESSAGES: usize = 256;
    /// Maximum one-line protocol envelope, kept below the Linux pipe capacity.
    const MAX_MESSAGE_BYTES: usize = 32 * 1024;
    /// Maximum user prompt bytes reserved within the protocol envelope.
    const MAX_PROMPT_BYTES: usize = 16 * 1024;
    /// Maximum assistant bytes accumulated by the convenience collector.
    const MAX_COLLECTED_TEXT_BYTES: usize = 256 * 1024;
    /// Maximum inert tool requests accumulated by the convenience collector.
    const MAX_COLLECTED_TOOL_REQUESTS: usize = 256;
    /// Maximum wait before rechecking cooperative cancellation.
    const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(25);

    /// Configuration for one isolated app-server child process.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AppServerConfig {
        /// Direct executable path.
        executable: PathBuf,
        /// Direct argument vector.
        arguments: Vec<String>,
        /// Tiber-owned Codex home.
        codex_home: PathBuf,
        /// Repository observation root.
        workspace: PathBuf,
        /// Whole-operation deadline.
        request_timeout: Duration,
    }

    impl AppServerConfig {
        /// Creates a process configuration after checking its semantic invariants.
        ///
        /// # Errors
        ///
        /// Returns a typed error when a path is not absolute or the timeout is zero.
        pub fn new(
            executable: PathBuf,
            arguments: Vec<String>,
            codex_home: PathBuf,
            workspace: PathBuf,
            request_timeout: Duration,
        ) -> Result<Self, AppServerError> {
            if !executable.is_absolute() || !codex_home.is_absolute() || !workspace.is_absolute() {
                return Err(AppServerError::new(
                    "app_server_path_not_absolute",
                    "app-server executable, Codex home, and workspace paths must be absolute",
                    false,
                ));
            }
            if request_timeout.is_zero() {
                return Err(AppServerError::new(
                    "app_server_timeout_invalid",
                    "app-server request timeout must be greater than zero",
                    false,
                ));
            }
            Ok(Self {
                executable,
                arguments,
                codex_home,
                workspace,
                request_timeout,
            })
        }

        /// Prepares the isolated Codex home used by every Tiber-owned Codex child.
        ///
        /// # Errors
        ///
        /// Returns a typed filesystem or configuration-rendering error.
        pub fn prepare_isolated_home(&self, isolated_config: &str) -> Result<(), AppServerError> {
            prepare_isolated_home(&self.codex_home, &self.executable, isolated_config)
        }

        /// Runs Codex's API-key login command without observing its stdin.
        ///
        /// The child alone reads the owner-provided credential from inherited stdin.
        /// Tiber removes ambient API-key variables, suppresses child output, and
        /// returns only a stable completion result.
        ///
        /// # Errors
        ///
        /// Returns a typed preparation, startup, timeout, or unsuccessful-login error.
        pub fn login_with_api_key_from_stdin(
            &self,
            isolated_config: &str,
        ) -> Result<(), AppServerError> {
            self.prepare_isolated_home(isolated_config)?;
            let deadline = deadline_after(self.request_timeout)?;
            let mut child = Command::new(&self.executable)
                .args(["login", "--with-api-key"])
                .current_dir(&self.workspace)
                .env("CODEX_HOME", &self.codex_home)
                .env_remove("OPENAI_API_KEY")
                .env_remove("ANTHROPIC_API_KEY")
                .stdin(Stdio::inherit())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_error| {
                    AppServerError::new(
                        "app_server_api_key_login_start_failed",
                        "Codex API-key login could not start",
                        true,
                    )
                })?;
            loop {
                let status = match child.try_wait() {
                    Ok(status) => status,
                    Err(_error) => {
                        let _ignored = child.kill();
                        let _ignored = child.wait();
                        return Err(AppServerError::new(
                            "app_server_api_key_login_wait_failed",
                            "Codex API-key login could not be observed",
                            true,
                        ));
                    }
                };
                if let Some(status) = status {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(AppServerError::new(
                            "app_server_api_key_login_failed",
                            "Codex API-key login did not complete",
                            false,
                        ))
                    };
                }
                if Instant::now() >= deadline {
                    let _ignored = child.kill();
                    let _ignored = child.wait();
                    return Err(AppServerError::new(
                        "app_server_api_key_login_timed_out",
                        "Codex API-key login exceeded the configured deadline",
                        true,
                    ));
                }
                thread::sleep(CANCELLATION_POLL_INTERVAL);
            }
        }
    }

    /// Stable typed app-server failure.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AppServerError {
        /// Stable classification.
        code: &'static str,
        /// Sanitized detail.
        message: String,
        /// Retry classification.
        retryable: bool,
    }

    impl AppServerError {
        /// Constructs a sanitized typed failure.
        fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
            Self {
                code,
                message: message.into(),
                retryable,
            }
        }

        /// Stable machine-readable classification.
        #[must_use]
        pub const fn code(&self) -> &'static str {
            self.code
        }

        /// Whether repeating the operation may succeed without owner intervention.
        #[must_use]
        pub const fn is_retryable(&self) -> bool {
            self.retryable
        }
    }

    impl fmt::Display for AppServerError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.message)
        }
    }

    impl Error for AppServerError {}

    /// Authentication state reported by app-server without exposing credentials.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum AccountStatus {
        /// No account is currently authenticated.
        SignedOut,
        /// ChatGPT subscription authentication is active.
        ChatGpt { email: Option<String> },
        /// App-server-managed API-key authentication is active.
        ///
        /// Tiber can verify this state after a direct stdin handoff to Codex; it
        /// never reads, copies, serializes, logs, or retains the API key.
        ApiKey,
    }

    /// Browser login handoff returned by app-server.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct LoginHandoff {
        /// Opaque login operation identity.
        pub login_id: String,
        /// URL the owner opens to authenticate.
        pub auth_url: String,
    }

    /// A model-requested tool call that Tiber deliberately did not execute.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct InertToolRequest {
        /// App-server call identity.
        pub call_id: String,
        /// Tiber-declared tool name.
        pub tool: String,
        /// Untrusted arguments supplied by the model.
        pub arguments: serde_json::Value,
    }

    /// Correlation and deadline state for one active inference turn.
    #[derive(Clone, Debug)]
    pub struct TurnHandle {
        /// Absolute deadline shared by every observation in this turn.
        deadline: Instant,
        /// App-server thread identity.
        thread_id: String,
        /// App-server turn identity.
        turn_id: String,
    }

    /// Cooperative cancellation shared with an application-shell owner.
    #[derive(Clone, Debug)]
    pub struct OperationCancellation {
        /// Cancellation flag observed by bounded transport waits.
        cancelled: Arc<AtomicBool>,
    }

    impl OperationCancellation {
        /// Requests cancellation of the current adapter operation.
        pub fn cancel(&self) {
            self.cancelled.store(true, Ordering::Release);
        }

        /// Reports whether the owner requested cancellation.
        #[must_use]
        pub fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Acquire)
        }
    }

    /// One typed observation from an active inference turn.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum TurnEvent {
        /// Assistant text arrived in delivery order.
        AssistantDelta(String),
        /// A model-requested tool was rejected and retained as inert data.
        InertToolRequested(InertToolRequest),
        /// The correlated turn completed successfully.
        Completed,
    }

    /// Completed minimal conversation observation.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct ConversationResult {
        /// Streamed assistant text in delivery order.
        pub text: String,
        /// Structured tool requests rejected by the adapter as inert data.
        pub inert_tool_requests: Vec<InertToolRequest>,
    }

    /// Stateful imperative-shell adapter for one app-server subprocess.
    pub struct AppServerClient {
        /// Owned child process.
        child: Child,
        /// Child input stream.
        input: ChildStdin,
        /// Decoded child output.
        output: Receiver<Result<serde_json::Value, AppServerError>>,
        /// Bounded out-of-order message buffer.
        queued: VecDeque<serde_json::Value>,
        /// Next client request identity.
        next_request_id: u64,
        /// Validated process configuration.
        config: AppServerConfig,
        /// Cooperative cancellation for application-shell responsiveness.
        cancellation: OperationCancellation,
    }

    impl AppServerClient {
        /// Returns the child process identity for lifecycle receipts and tests.
        #[must_use]
        pub fn child_process_id(&self) -> u32 {
            self.child.id()
        }

        /// Returns the cooperative cancellation handle for the application shell.
        #[must_use]
        pub fn cancellation_handle(&self) -> OperationCancellation {
            self.cancellation.clone()
        }

        /// Creates the isolated home, starts app-server, and completes initialization.
        ///
        /// # Errors
        ///
        /// Returns a typed startup, I/O, protocol, compatibility, or timeout error.
        pub fn start(
            config: AppServerConfig,
            isolated_config: &str,
        ) -> Result<Self, AppServerError> {
            config.prepare_isolated_home(isolated_config)?;
            let mut command = Command::new(&config.executable);
            command
                .args(&config.arguments)
                .env("CODEX_HOME", &config.codex_home)
                .env_remove("OPENAI_API_KEY")
                .env_remove("ANTHROPIC_API_KEY")
                .current_dir(&config.workspace)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = command.spawn().map_err(|error| {
                AppServerError::new(
                    "app_server_spawn_failed",
                    format!("failed to start app-server: {error}"),
                    true,
                )
            })?;
            let input = child.stdin.take().ok_or_else(|| {
                AppServerError::new(
                    "app_server_stdio_unavailable",
                    "app-server stdin was not piped",
                    false,
                )
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                AppServerError::new(
                    "app_server_stdio_unavailable",
                    "app-server stdout was not piped",
                    false,
                )
            })?;
            let (sender, output) = mpsc::sync_channel(MAX_QUEUED_MESSAGES);
            thread::spawn(move || read_messages(stdout, &sender));
            let mut client = Self {
                child,
                input,
                output,
                queued: VecDeque::new(),
                next_request_id: 1,
                config,
                cancellation: OperationCancellation {
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            };
            let initialized = client.request(
            "initialize",
            serde_json::json!({
                "capabilities": { "experimentalApi": true },
                "clientInfo": { "name": "tiber", "title": "Tiber", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
            let reported_home = initialized
                .get("codexHome")
                .and_then(serde_json::Value::as_str)
                .map(Path::new);
            if reported_home != Some(client.config.codex_home.as_path()) {
                return Err(AppServerError::new(
                    "app_server_isolation_mismatch",
                    "app-server did not report the Tiber-owned Codex home",
                    false,
                ));
            }
            let user_agent = initialized
                .get("userAgent")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if codex_version(user_agent) != Some(SUPPORTED_CODEX_VERSION) {
                return Err(AppServerError::new(
                    "app_server_version_incompatible",
                    format!(
                        "app-server must report reviewed Codex version {SUPPORTED_CODEX_VERSION}"
                    ),
                    false,
                ));
            }
            client.notify("initialized", serde_json::Value::Null)?;
            Ok(client)
        }

        /// Reads the current app-server-managed authentication state.
        ///
        /// # Errors
        ///
        /// Returns a typed transport or protocol error.
        pub fn account_status(&mut self) -> Result<AccountStatus, AppServerError> {
            let response =
                self.request("account/read", serde_json::json!({ "refreshToken": false }))?;
            let Some(account) = response.get("account") else {
                return Err(protocol_error("account/read omitted account"));
            };
            if account.is_null() {
                return Ok(AccountStatus::SignedOut);
            }
            match account.get("type").and_then(serde_json::Value::as_str) {
                Some("apiKey") => Ok(AccountStatus::ApiKey),
                Some("chatgpt") => Ok(AccountStatus::ChatGpt {
                    email: account
                        .get("email")
                        .and_then(serde_json::Value::as_str)
                        .map(String::from),
                }),
                _ => Err(protocol_error(
                    "account/read returned an unsupported account type",
                )),
            }
        }

        /// Requires app-server to confirm API-key authentication after the direct handoff.
        ///
        /// # Errors
        ///
        /// Returns a typed status-query or unverified-authentication error.
        pub fn require_api_key_account(&mut self) -> Result<(), AppServerError> {
            match self.account_status()? {
                AccountStatus::ApiKey => Ok(()),
                AccountStatus::SignedOut | AccountStatus::ChatGpt { .. } => {
                    Err(AppServerError::new(
                        "app_server_api_key_login_unverified",
                        "app-server did not report API-key authentication after login",
                        false,
                    ))
                }
            }
        }

        /// Starts app-server-managed ChatGPT subscription login.
        ///
        /// # Errors
        ///
        /// Returns a typed transport or protocol error.
        pub fn start_chatgpt_login(&mut self) -> Result<LoginHandoff, AppServerError> {
            let response = self.request(
                "account/login/start",
                serde_json::json!({ "type": "chatgpt" }),
            )?;
            let login_id = required_string(&response, "loginId")?;
            let auth_url = required_string(&response, "authUrl")?;
            Ok(LoginHandoff { login_id, auth_url })
        }

        /// Waits for app-server to finish a previously started `ChatGPT` login.
        ///
        /// # Errors
        ///
        /// Returns a typed timeout, stream, protocol, or authentication error.
        pub fn await_chatgpt_login(&mut self, login_id: &str) -> Result<(), AppServerError> {
            let deadline = deadline_after(self.config.request_timeout)?;
            loop {
                let message = self.receive_before(deadline)?;
                if message.get("method").and_then(serde_json::Value::as_str)
                    != Some("account/login/completed")
                {
                    continue;
                }
                let params = message
                    .get("params")
                    .ok_or_else(|| protocol_error("login completion omitted params"))?;
                let completed_login = params.get("loginId").and_then(serde_json::Value::as_str);
                if completed_login.is_some() && completed_login != Some(login_id) {
                    continue;
                }
                if params.get("success").and_then(serde_json::Value::as_bool) == Some(true) {
                    return Ok(());
                }
                let detail = params
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("authentication failed");
                return Err(AppServerError::new(
                    "app_server_authentication_failed",
                    detail,
                    false,
                ));
            }
        }

        /// Logs out through app-server.
        ///
        /// # Errors
        ///
        /// Returns a typed transport or protocol error.
        pub fn logout(&mut self) -> Result<(), AppServerError> {
            let _response = self.request("account/logout", serde_json::Value::Null)?;
            Ok(())
        }

        /// Runs one minimal inference turn and rejects every model-requested tool effect.
        ///
        /// # Errors
        ///
        /// Returns a typed transport, stream, protocol, or terminal-turn error.
        pub fn converse(&mut self, prompt: &str) -> Result<ConversationResult, AppServerError> {
            let turn = self.start_turn(prompt)?;
            let mut text = String::new();
            let mut inert_tool_requests = Vec::new();
            loop {
                match self.next_turn_event(&turn)? {
                    TurnEvent::AssistantDelta(delta) => {
                        if text.len().saturating_add(delta.len()) > MAX_COLLECTED_TEXT_BYTES {
                            return Err(AppServerError::new(
                                "app_server_turn_output_too_large",
                                "assistant output exceeded the bounded turn collector",
                                false,
                            ));
                        }
                        text.push_str(&delta);
                    }
                    TurnEvent::InertToolRequested(request) => {
                        if inert_tool_requests.len() >= MAX_COLLECTED_TOOL_REQUESTS {
                            return Err(AppServerError::new(
                                "app_server_turn_tool_limit_exceeded",
                                "model tool requests exceeded the bounded turn collector",
                                false,
                            ));
                        }
                        inert_tool_requests.push(request);
                    }
                    TurnEvent::Completed => {
                        return Ok(ConversationResult {
                            text,
                            inert_tool_requests,
                        });
                    }
                }
            }
        }

        /// Starts one bounded inference turn without consuming its stream.
        ///
        /// # Errors
        ///
        /// Returns a typed transport, protocol, policy-profile, or input error.
        pub fn start_turn(&mut self, prompt: &str) -> Result<TurnHandle, AppServerError> {
            if prompt.is_empty() {
                return Err(AppServerError::new(
                    "app_server_prompt_empty",
                    "conversation prompt must not be empty",
                    false,
                ));
            }
            if prompt.len() > MAX_PROMPT_BYTES {
                return Err(AppServerError::new(
                    "app_server_prompt_too_large",
                    "conversation prompt exceeds the bounded app-server envelope",
                    false,
                ));
            }
            let started = self.request(
            "thread/start",
            serde_json::json!({
                "approvalPolicy": "never",
                "approvalsReviewer": "user",
                "cwd": self.config.workspace,
                "dynamicTools": [{
                    "description": "Requests a Tiber-owned effect; this spike records it as inert data.",
                    "inputSchema": { "type": "object" },
                    "name": "tiber_effect",
                    "type": "function"
                }],
                "environments": [],
                "ephemeral": true,
                "permissions": INFERENCE_PERMISSION_PROFILE
            }),
        )?;
            validate_effective_inference_profile(&started)?;
            let thread_id = required_string(
                started
                    .get("thread")
                    .ok_or_else(|| protocol_error("thread/start omitted thread"))?,
                "id",
            )?;
            let turn = self.request(
                "turn/start",
                serde_json::json!({
                    "environments": [],
                    "input": [{ "text": prompt, "type": "text" }],
                    "threadId": thread_id
                }),
            )?;
            let turn_id = required_string(
                turn.get("turn")
                    .ok_or_else(|| protocol_error("turn/start omitted turn"))?,
                "id",
            )?;
            Ok(TurnHandle {
                deadline: deadline_after(self.config.request_timeout)?,
                thread_id,
                turn_id,
            })
        }

        /// Returns the next presentation-safe observation for one active turn.
        ///
        /// Approval and permission requests are declined internally and are never
        /// surfaced as executable presentation actions.
        ///
        /// # Errors
        ///
        /// Returns a typed transport, stream, protocol, or terminal-turn error.
        pub fn next_turn_event(&mut self, turn: &TurnHandle) -> Result<TurnEvent, AppServerError> {
            self.next_turn_event_before(turn, turn.deadline)
        }

        /// Polls one active turn for at most `wait`, preserving its absolute deadline.
        ///
        /// `None` means no presentation observation arrived during this poll. It is
        /// not a terminal turn result.
        ///
        /// # Errors
        ///
        /// Returns a typed transport, stream, protocol, or terminal-turn error.
        pub fn poll_turn_event(
            &mut self,
            turn: &TurnHandle,
            wait: Duration,
        ) -> Result<Option<TurnEvent>, AppServerError> {
            let poll_deadline = Instant::now()
                .checked_add(wait)
                .map_or(turn.deadline, |deadline| deadline.min(turn.deadline));
            match self.next_turn_event_before(turn, poll_deadline) {
                Ok(event) => Ok(Some(event)),
                Err(error)
                    if error.code() == "app_server_timeout" && poll_deadline < turn.deadline =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        }

        /// Consumes messages for one turn until an observation or deadline.
        fn next_turn_event_before(
            &mut self,
            turn: &TurnHandle,
            deadline: Instant,
        ) -> Result<TurnEvent, AppServerError> {
            loop {
                self.ensure_not_cancelled()?;
                let message = self.receive_before(deadline)?;
                match message.get("method").and_then(serde_json::Value::as_str) {
                    Some("item/agentMessage/delta") => {
                        if !belongs_to_turn(&message, &turn.thread_id, &turn.turn_id) {
                            continue;
                        }
                        if let Some(delta) = message
                            .pointer("/params/delta")
                            .and_then(serde_json::Value::as_str)
                        {
                            return Ok(TurnEvent::AssistantDelta(delta.to_owned()));
                        }
                    }
                    Some("item/tool/call") => {
                        let request_id = message
                            .get("id")
                            .cloned()
                            .ok_or_else(|| protocol_error("tool call omitted request id"))?;
                        let params = message
                            .get("params")
                            .ok_or_else(|| protocol_error("tool call omitted params"))?;
                        let request = belongs_to_turn(&message, &turn.thread_id, &turn.turn_id)
                            .then(|| {
                                Ok::<_, AppServerError>(InertToolRequest {
                                    call_id: required_string(params, "callId")?,
                                    tool: required_string(params, "tool")?,
                                    arguments: params
                                        .get("arguments")
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null),
                                })
                            })
                            .transpose()?;
                        self.respond(request_id, serde_json::json!({
                        "contentItems": [{ "text": "Tiber spike records model-requested tools as inert data.", "type": "inputText" }],
                        "success": false
                    }))?;
                        if let Some(request) = request {
                            return Ok(TurnEvent::InertToolRequested(request));
                        }
                    }
                    Some(
                        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval",
                    ) => {
                        let request_id = message
                            .get("id")
                            .cloned()
                            .ok_or_else(|| protocol_error("approval request omitted request id"))?;
                        self.respond(request_id, serde_json::json!({ "decision": "decline" }))?;
                    }
                    Some("item/permissions/requestApproval") => {
                        let request_id = message.get("id").cloned().ok_or_else(|| {
                            protocol_error("permission request omitted request id")
                        })?;
                        self.respond(
                            request_id,
                            serde_json::json!({ "permissions": {}, "scope": "turn" }),
                        )?;
                    }
                    Some("turn/completed") => {
                        let completed_turn = message
                            .pointer("/params/turn/id")
                            .and_then(serde_json::Value::as_str);
                        let completed_thread = message
                            .pointer("/params/threadId")
                            .and_then(serde_json::Value::as_str);
                        if completed_thread == Some(turn.thread_id.as_str())
                            && completed_turn == Some(turn.turn_id.as_str())
                        {
                            let status = message
                                .pointer("/params/turn/status")
                                .and_then(serde_json::Value::as_str);
                            if status != Some("completed") {
                                return Err(AppServerError::new(
                                    "app_server_turn_failed",
                                    format!(
                                        "app-server turn ended with status {}",
                                        status.unwrap_or("unknown")
                                    ),
                                    status == Some("failed"),
                                ));
                            }
                            return Ok(TurnEvent::Completed);
                        }
                    }
                    Some("error") => {
                        return Err(protocol_error("app-server emitted an error notification"));
                    }
                    _ => {}
                }
            }
        }

        /// Sends one bounded client request and awaits its matching response.
        fn request(
            &mut self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, AppServerError> {
            let id = self.next_request_id;
            let deadline = deadline_after(self.config.request_timeout)?;
            self.next_request_id = self.next_request_id.checked_add(1).ok_or_else(|| {
                AppServerError::new(
                    "app_server_request_id_exhausted",
                    "request identity space exhausted",
                    false,
                )
            })?;
            self.send(&serde_json::json!({ "id": id, "method": method, "params": params }))?;
            loop {
                self.ensure_not_cancelled()?;
                let message = if let Some(position) = self.queued.iter().position(|message| {
                    message.get("method").is_none()
                        && message.get("id").and_then(serde_json::Value::as_u64) == Some(id)
                }) {
                    self.queued
                        .remove(position)
                        .ok_or_else(|| protocol_error("queued response disappeared"))?
                } else {
                    let slice_deadline = Instant::now()
                        .checked_add(CANCELLATION_POLL_INTERVAL)
                        .map_or(deadline, |candidate| candidate.min(deadline));
                    match self.receive_output_before(slice_deadline) {
                        Err(error)
                            if error.code() == "app_server_timeout"
                                && slice_deadline < deadline =>
                        {
                            continue;
                        }
                        result => result?,
                    }
                };
                if message.get("method").is_some() && message.get("id").is_some() {
                    self.reject_server_request(&message)?;
                    continue;
                }
                if message.get("method").is_none()
                    && message.get("id").and_then(serde_json::Value::as_u64) == Some(id)
                {
                    if let Some(error) = message.get("error") {
                        return Err(AppServerError::new(
                            "app_server_request_rejected",
                            if method == "account/login/start" {
                                "app-server rejected account login".to_owned()
                            } else {
                                format!("app-server rejected {method}: {error}")
                            },
                            false,
                        ));
                    }
                    return message
                        .get("result")
                        .cloned()
                        .ok_or_else(|| protocol_error("response omitted result"));
                }
                if self.queued.len() >= MAX_QUEUED_MESSAGES {
                    return Err(AppServerError::new(
                        "app_server_queue_exhausted",
                        "app-server exceeded the bounded pending-message queue",
                        false,
                    ));
                }
                self.queued.push_back(message);
            }
        }

        /// Rejects one server-originated request without executing an effect.
        fn reject_server_request(
            &mut self,
            message: &serde_json::Value,
        ) -> Result<(), AppServerError> {
            let request_id = message
                .get("id")
                .cloned()
                .ok_or_else(|| protocol_error("server request omitted request id"))?;
            match message.get("method").and_then(serde_json::Value::as_str) {
                Some(
                    "item/commandExecution/requestApproval"
                    | "item/fileChange/requestApproval",
                ) => self.respond(request_id, serde_json::json!({ "decision": "decline" })),
                Some("item/permissions/requestApproval") => self.respond(
                    request_id,
                    serde_json::json!({ "permissions": {}, "scope": "turn" }),
                ),
                Some("item/tool/call") => self.respond(
                    request_id,
                    serde_json::json!({
                        "contentItems": [{ "text": "Tiber rejected a tool request outside the active turn.", "type": "inputText" }],
                        "success": false
                    }),
                ),
                _ => Err(protocol_error("app-server emitted an unsupported server request")),
            }
        }

        /// Sends a client notification.
        fn notify(
            &mut self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<(), AppServerError> {
            self.send(&serde_json::json!({ "method": method, "params": params }))
        }

        /// Responds to a server-originated request.
        fn respond(
            &mut self,
            id: serde_json::Value,
            result: serde_json::Value,
        ) -> Result<(), AppServerError> {
            self.send(&serde_json::json!({ "id": id, "result": result }))
        }

        /// Encodes and flushes one protocol envelope.
        fn send(&mut self, message: &serde_json::Value) -> Result<(), AppServerError> {
            let mut encoded = serde_json::to_vec(message).map_err(|error| {
                AppServerError::new(
                    "app_server_write_failed",
                    format!("failed to encode app-server message: {error}"),
                    true,
                )
            })?;
            if encoded.len() >= MAX_MESSAGE_BYTES {
                return Err(AppServerError::new(
                    "app_server_message_too_large",
                    "app-server message exceeds the bounded transport envelope",
                    false,
                ));
            }
            encoded.push(b'\n');
            self.input.write_all(&encoded).map_err(|error| {
                AppServerError::new(
                    "app_server_write_failed",
                    format!("failed to write app-server message: {error}"),
                    true,
                )
            })
        }

        /// Receives the next queued or child message before one deadline.
        fn receive_before(
            &mut self,
            deadline: Instant,
        ) -> Result<serde_json::Value, AppServerError> {
            if let Some(message) = self.queued.pop_front() {
                return Ok(message);
            }
            self.receive_output_before(deadline)
        }

        /// Receives a child message before one absolute deadline.
        fn receive_output_before(
            &mut self,
            deadline: Instant,
        ) -> Result<serde_json::Value, AppServerError> {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AppServerError::new(
                    "app_server_timeout",
                    "app-server operation deadline elapsed",
                    true,
                ));
            }
            match self.output.recv_timeout(remaining) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => Err(AppServerError::new(
                    "app_server_timeout",
                    "app-server did not respond before the configured deadline",
                    true,
                )),
                Err(RecvTimeoutError::Disconnected) => Err(AppServerError::new(
                    "app_server_stream_closed",
                    "app-server output closed unexpectedly",
                    true,
                )),
            }
        }

        /// Fails the current operation after an owner-requested cancellation.
        fn ensure_not_cancelled(&self) -> Result<(), AppServerError> {
            if self.cancellation.cancelled.load(Ordering::Acquire) {
                return Err(AppServerError::new(
                    "app_server_cancelled",
                    "app-server operation cancelled by the owner",
                    false,
                ));
            }
            Ok(())
        }
    }

    impl Drop for AppServerClient {
        fn drop(&mut self) {
            let _ignored = self.child.kill();
            let _ignored = self.child.wait();
        }
    }

    /// Atomically renders the isolated runtime configuration.
    fn prepare_isolated_home(
        codex_home: &Path,
        executable: &Path,
        isolated_config: &str,
    ) -> Result<(), AppServerError> {
        let executable = executable.canonicalize().map_err(|error| {
            AppServerError::new(
                "app_server_executable_resolve_failed",
                format!("failed to resolve app-server executable: {error}"),
                false,
            )
        })?;
        let quoted_executable =
            serde_json::to_string(&executable.to_string_lossy()).map_err(|error| {
                AppServerError::new(
                    "app_server_config_render_failed",
                    format!("failed to quote app-server executable path: {error}"),
                    false,
                )
            })?;
        let rendered_config = isolated_config.replace(
            "# TIBER_CODEX_RUNTIME_READ_GRANT",
            &format!("{quoted_executable} = \"read\""),
        );
        let temporary_config = codex_home.join(format!(".config.toml.{}.tmp", std::process::id()));
        fs::create_dir_all(codex_home)
            .and_then(|()| fs::write(&temporary_config, rendered_config))
            .and_then(|()| {
                fs::set_permissions(&temporary_config, fs::Permissions::from_mode(0o600))
            })
            .and_then(|()| fs::rename(&temporary_config, codex_home.join("config.toml")))
            .map_err(|error| {
                AppServerError::new(
                    "app_server_home_prepare_failed",
                    format!("failed to prepare isolated Codex home: {error}"),
                    false,
                )
            })
    }

    /// Decodes newline-delimited server messages on the reader thread.
    fn read_messages(
        stdout: impl std::io::Read,
        sender: &SyncSender<Result<serde_json::Value, AppServerError>>,
    ) {
        let mut reader = BufReader::new(stdout);
        loop {
            let message = match read_bounded_line(&mut reader) {
                Ok(Some(line)) => serde_json::from_slice(&line).map_err(|error| {
                    AppServerError::new(
                        "app_server_message_invalid",
                        format!("app-server emitted invalid JSON: {error}"),
                        false,
                    )
                }),
                Ok(None) => break,
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    Err(AppServerError::new(
                        "app_server_message_too_large",
                        "app-server emitted a message larger than the bounded transport envelope",
                        false,
                    ))
                }
                Err(error) => Err(AppServerError::new(
                    "app_server_read_failed",
                    format!("failed to read app-server output: {error}"),
                    true,
                )),
            };
            let terminal = message.is_err();
            if sender.send(message).is_err() || terminal {
                break;
            }
        }
    }

    /// Reads one newline-delimited envelope without allocating past the wire bound.
    fn read_bounded_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
        let mut line = Vec::new();
        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(line))
                };
            }
            let available = buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .and_then(|position| position.checked_add(1))
                .unwrap_or(buffer.len());
            if line.len().saturating_add(available) > MAX_MESSAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "app-server message exceeds transport bound",
                ));
            }
            let chunk = buffer.get(..available).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid buffered line boundary")
            })?;
            line.extend_from_slice(chunk);
            reader.consume(available);
            if line.last() == Some(&b'\n') {
                line.pop();
                return Ok(Some(line));
            }
        }
    }

    /// Extracts one required string response field.
    fn required_string(value: &serde_json::Value, field: &str) -> Result<String, AppServerError> {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(String::from)
            .ok_or_else(|| {
                protocol_error(&format!("app-server response omitted string field {field}"))
            })
    }

    /// Constructs a stable protocol-shape failure.
    fn protocol_error(message: &str) -> AppServerError {
        AppServerError::new("app_server_protocol_invalid", message, false)
    }

    /// Fails closed when app-server's returned effective profile exceeds Tiber's fence.
    fn validate_effective_inference_profile(
        started: &serde_json::Value,
    ) -> Result<(), AppServerError> {
        if started
            .pointer("/activePermissionProfile/id")
            .and_then(serde_json::Value::as_str)
            != Some(INFERENCE_PERMISSION_PROFILE)
        {
            return Err(effective_profile_mismatch(
                "active permission profile must be tiber-inference",
            ));
        }
        if started
            .get("approvalPolicy")
            .and_then(serde_json::Value::as_str)
            != Some("never")
        {
            return Err(effective_profile_mismatch(
                "effective approval policy must be never",
            ));
        }
        if started
            .pointer("/sandbox/type")
            .and_then(serde_json::Value::as_str)
            != Some("readOnly")
        {
            return Err(effective_profile_mismatch(
                "effective sandbox type must be readOnly",
            ));
        }
        if started
            .pointer("/sandbox/networkAccess")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
        {
            return Err(effective_profile_mismatch(
                "effective sandbox network access must be disabled",
            ));
        }
        Ok(())
    }

    /// Creates the stable failure for any returned effective-profile mismatch.
    fn effective_profile_mismatch(detail: &str) -> AppServerError {
        AppServerError::new("app_server_effective_profile_mismatch", detail, false)
    }

    /// Checks correlation for a turn-scoped message.
    fn belongs_to_turn(message: &serde_json::Value, thread_id: &str, turn_id: &str) -> bool {
        message
            .pointer("/params/threadId")
            .and_then(serde_json::Value::as_str)
            == Some(thread_id)
            && message
                .pointer("/params/turnId")
                .and_then(serde_json::Value::as_str)
                == Some(turn_id)
    }

    /// Computes a bounded deadline without panicking on overflow.
    fn deadline_after(timeout: Duration) -> Result<Instant, AppServerError> {
        Instant::now().checked_add(timeout).ok_or_else(|| {
            AppServerError::new(
                "app_server_timeout_invalid",
                "app-server timeout exceeds the platform clock range",
                false,
            )
        })
    }

    /// Extracts an exact dotted numeric version token from the runtime user agent.
    fn codex_version(user_agent: &str) -> Option<&str> {
        user_agent
            .split(|character: char| !(character.is_ascii_digit() || character == '.'))
            .find(|token| {
                !token.is_empty()
                    && token.split('.').count() == 3
                    && token.split('.').all(|part| {
                        !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
                    })
            })
    }
}

pub use runtime::{
    AccountStatus, AppServerClient, AppServerConfig, AppServerError, ConversationResult,
    INFERENCE_PERMISSION_PROFILE, InertToolRequest, LoginHandoff, OperationCancellation,
    SUPPORTED_CODEX_VERSION, TurnEvent, TurnHandle,
};

/// Builds the stable fail-closed error for an unknown schema structure.
#[expect(
    clippy::implicit_return,
    reason = "the constructor is clearest as a single struct expression"
)]
fn unrecognized_schema(detail: &str) -> CompatibilityError {
    CompatibilityError {
        code: "app_server_schema_contract_unrecognized",
        message: format!("cannot verify app-server authority contract: {detail}"),
    }
}
