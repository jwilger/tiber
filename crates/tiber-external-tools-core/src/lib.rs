//! Pure authority contracts for configured third-party external tools.
//!
//! This crate deliberately contains no RMCP, runtime, network, process, or
//! `EventCore` dependency. It turns trusted integration configuration and six
//! explicit policy layers into opaque requests that an imperative adapter may
//! execute.  Server-provided descriptions, schemas, and results never grant
//! authority.

extern crate alloc;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};
use core::{error::Error, fmt};
use std::path::{Path, PathBuf};

use serde::Serialize;
use url::Url;

/// Defines a canonical text identity at the pure authority boundary.
macro_rules! semantic_text {
    ($name:ident) => {
        #[doc = "A validated external-tool semantic identity."]
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        #[expect(
            clippy::implicit_return,
            reason = "the generated semantic parser and accessor use idiomatic tail expressions"
        )]
        impl $name {
            /// Returns this identity's canonical text.
            #[must_use]
            #[inline]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Parses and canonically trims one external-tool semantic identity.
            ///
            /// # Errors
            ///
            /// Returns a stable [`ExternalToolError`] when the value is empty,
            /// contains a control character, or exceeds the fixed text bound.
            #[inline]
            pub fn parse(value: &str) -> Result<Self, ExternalToolError> {
                let value = value.trim();
                if value.is_empty() {
                    return Err(ExternalToolError::EmptySemanticValue);
                }
                if value.len() > MAX_SEMANTIC_TEXT_BYTES || value.chars().any(char::is_control) {
                    return Err(ExternalToolError::InvalidSemanticValue);
                }
                Ok(Self(value.to_owned()))
            }
        }
    };
}

/// Generates the identical safe provenance inspectors for reconciliation outcomes and failures.
macro_rules! reconciliation_provenance_accessors {
    () => {
        /// Returns the owner approval bound to the originating mutation.
        #[must_use]
        #[inline]
        pub fn approval(&self) -> Option<&OwnerApprovalId> {
            self.provenance.approval.as_ref()
        }

        /// Returns the exact authority tuple bound by pure authorization.
        #[must_use]
        #[inline]
        pub fn authorization(&self) -> &AuthorizationContext {
            &self.provenance.authorization
        }

        /// Returns the stable mutation identity checked by reconciliation.
        #[must_use]
        #[inline]
        pub fn idempotency_key(&self) -> &IdempotencyKey {
            &self.provenance.idempotency_key
        }

        /// Returns the trusted integration identity without configuration.
        #[must_use]
        #[inline]
        pub fn integration_id(&self) -> &IntegrationId {
            &self.provenance.integration_id
        }

        /// Returns the exact mutating tool whose outcome was checked.
        #[must_use]
        #[inline]
        pub fn originating_tool(&self) -> &ToolName {
            &self.provenance.originating_tool
        }

        /// Returns the exact configured read-only status tool used by reconciliation.
        #[must_use]
        #[inline]
        pub fn status_tool(&self) -> &ToolName {
            &self.provenance.status_tool
        }
    };
}

/// Largest UTF-8 byte length accepted for a semantic external-tool value.
pub const MAX_SEMANTIC_TEXT_BYTES: usize = 256;
/// Largest raw JSON argument payload accepted at the external-tool boundary.
pub const MAX_TOOL_ARGUMENT_BYTES: usize = 64 * 1024;
/// Largest individual untrusted metadata or result payload retained by this boundary.
pub const MAX_UNTRUSTED_PAYLOAD_BYTES: usize = 64 * 1024;
/// Largest configured number of tools for one integration.
pub const MAX_CONFIGURED_TOOLS: usize = 64;
/// Largest number of trusted filesystem roots disclosed to one integration.
pub const MAX_TIBER_OWNED_ROOTS: usize = 16;

/// Reserved JSON member that binds a mutating tool invocation to reconciliation.
const IDEMPOTENCY_KEY_ARGUMENT: &str = "idempotencyKey";

/// Stable external-tool policy and configuration failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "callers must handle every stable policy refusal deliberately; variants remain in policy-flow order"
)]
pub enum ExternalToolError {
    /// A required semantic value was empty after canonical trimming.
    EmptySemanticValue,
    /// A semantic value was malformed, too long, or contained a control character.
    InvalidSemanticValue,
    /// A configured stdio executable path was not absolute.
    ProgramNotAbsolute,
    /// A configured literal argv item was malformed.
    InvalidLiteralArgument,
    /// A configured Tiber-owned root path was not absolute.
    RootNotAbsolute,
    /// A configured Tiber-owned root was malformed or could not become a file URI.
    InvalidTiberRoot,
    /// The configured Tiber-owned root catalog exceeded its fixed bound.
    InvalidTiberRootCatalog,
    /// More than one trusted configuration entry declared the same Tiber-owned root.
    DuplicateTiberRoot,
    /// The configured endpoint could not be parsed as a supported URL.
    InvalidEndpoint,
    /// The configured endpoint did not resolve to an allowed loopback host spelling.
    EndpointNotLoopback,
    /// The configured endpoint used an unsupported scheme.
    UnsupportedEndpointScheme,
    /// The configured tool catalog was empty or exceeded its fixed bound.
    InvalidToolCatalog,
    /// More than one trusted configuration entry declared the same tool.
    DuplicateConfiguredTool,
    /// A configured reconciliation tool was absent or was not read-only.
    InvalidReconciliationTool,
    /// The invocation argument payload was not bounded valid JSON.
    InvalidToolArguments,
    /// The requested resource URI was not a bounded absolute URI.
    InvalidResourceUri,
    /// The requested prompt argument payload was not a bounded JSON object.
    InvalidPromptArguments,
    /// Server-provided data crossed the fixed Tiber input bound.
    UntrustedPayloadTooLarge,
    /// The requested tool was not present in the trusted integration configuration.
    UnknownTool,
    /// A policy layer did not bind to the request's current authority context.
    PolicyContextMismatch,
    /// The complete policy intersection was not issued for the configured integration.
    PolicyIntegrationMismatch,
    /// A policy layer did not permit a required operation capability.
    CapabilityDenied,
    /// A policy layer did not permit the requested configured tool.
    ToolDenied,
    /// A mutating configured tool lacked an explicit owner approval identity.
    MutationApprovalRequired,
    /// A mutating configured tool lacked a stable idempotency identity.
    MutationIdempotencyRequired,
    /// Caller-supplied tool arguments conflicted with the typed mutation idempotency identity.
    MutationIdempotencyConflict,
    /// A mutating configured tool lacked its configured read-only reconciliation operation.
    MutationReconciliationRequired,
}

impl ExternalToolError {
    /// Returns the stable external code for this failure.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the total stable-code table is clearest as the function tail expression"
    )]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EmptySemanticValue => "external_tools_empty_semantic_value",
            Self::InvalidSemanticValue => "external_tools_invalid_semantic_value",
            Self::ProgramNotAbsolute => "external_tools_program_not_absolute",
            Self::InvalidLiteralArgument => "external_tools_invalid_literal_argument",
            Self::RootNotAbsolute => "external_tools_root_not_absolute",
            Self::InvalidTiberRoot => "external_tools_invalid_tiber_root",
            Self::InvalidTiberRootCatalog => "external_tools_invalid_tiber_root_catalog",
            Self::DuplicateTiberRoot => "external_tools_duplicate_tiber_root",
            Self::InvalidEndpoint => "external_tools_invalid_endpoint",
            Self::EndpointNotLoopback => "external_tools_endpoint_not_loopback",
            Self::UnsupportedEndpointScheme => "external_tools_unsupported_endpoint_scheme",
            Self::InvalidToolCatalog => "external_tools_invalid_tool_catalog",
            Self::DuplicateConfiguredTool => "external_tools_duplicate_configured_tool",
            Self::InvalidReconciliationTool => "external_tools_invalid_reconciliation_tool",
            Self::InvalidToolArguments => "external_tools_invalid_tool_arguments",
            Self::InvalidResourceUri => "external_tools_invalid_resource_uri",
            Self::InvalidPromptArguments => "external_tools_invalid_prompt_arguments",
            Self::UntrustedPayloadTooLarge => "external_tools_untrusted_payload_too_large",
            Self::UnknownTool => "external_tools_unknown_tool",
            Self::PolicyContextMismatch => "external_tools_policy_context_mismatch",
            Self::PolicyIntegrationMismatch => "external_tools_policy_integration_mismatch",
            Self::CapabilityDenied => "external_tools_capability_denied",
            Self::ToolDenied => "external_tools_tool_denied",
            Self::MutationApprovalRequired => "external_tools_mutation_approval_required",
            Self::MutationIdempotencyRequired => "external_tools_mutation_idempotency_required",
            Self::MutationIdempotencyConflict => "external_tools_mutation_idempotency_conflict",
            Self::MutationReconciliationRequired => {
                "external_tools_mutation_reconciliation_required"
            }
        }
    }
}

impl fmt::Display for ExternalToolError {
    #[expect(
        clippy::implicit_return,
        reason = "the display implementation directly delegates to the stable code table"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "stable external-tool failures carry no nested causal source"
)]
impl Error for ExternalToolError {}

semantic_text!(AgentRole);
semantic_text!(AssignmentId);
semantic_text!(IdempotencyKey);
semantic_text!(IntegrationId);
semantic_text!(OwnerApprovalId);
semantic_text!(PolicyDecisionId);
semantic_text!(PromptName);
semantic_text!(SessionId);
semantic_text!(ToolName);
semantic_text!(WorkflowMode);

/// An absolute executable selected by trusted integration configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AbsoluteProgram(PathBuf);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the parser precedes the accessor to make the trusted configuration boundary readable"
)]
impl AbsoluteProgram {
    /// Parses an absolute direct-argv executable path.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalToolError::ProgramNotAbsolute`] when the path is not
    /// absolute or cannot be represented as bounded non-control UTF-8 text.
    #[inline]
    pub fn parse<P>(path: P) -> Result<Self, ExternalToolError>
    where
        P: AsRef<Path>,
    {
        let parsed_path = path.as_ref();
        let Some(text) = parsed_path.to_str() else {
            return Err(ExternalToolError::ProgramNotAbsolute);
        };
        if !parsed_path.is_absolute()
            || text.is_empty()
            || text.len() > MAX_SEMANTIC_TEXT_BYTES
            || text.chars().any(char::is_control)
        {
            return Err(ExternalToolError::ProgramNotAbsolute);
        }
        Ok(Self(parsed_path.to_owned()))
    }

    /// Returns the direct executable path without a shell interpretation step.
    #[must_use]
    #[inline]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A trusted local filesystem root that Tiber may disclose to an authorized MCP server.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TiberOwnedRoot(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the trusted path parser precedes the protocol-URI accessor at this authority boundary"
)]
impl TiberOwnedRoot {
    /// Parses one trusted absolute local path into the exact file URI disclosed to a server.
    ///
    /// # Errors
    ///
    /// Returns a stable root error when the path is relative, non-UTF-8,
    /// oversized, contains a control character, or cannot become a file URI.
    #[inline]
    pub fn from_absolute_path<P>(path: P) -> Result<Self, ExternalToolError>
    where
        P: AsRef<Path>,
    {
        let parsed_path = path.as_ref();
        if !parsed_path.is_absolute() {
            return Err(ExternalToolError::RootNotAbsolute);
        }
        let Some(text) = parsed_path.to_str() else {
            return Err(ExternalToolError::InvalidTiberRoot);
        };
        if text.is_empty()
            || text.len() > MAX_SEMANTIC_TEXT_BYTES
            || text.chars().any(char::is_control)
        {
            return Err(ExternalToolError::InvalidTiberRoot);
        }
        let root_uri = match Url::from_file_path(parsed_path) {
            Ok(root_uri) => root_uri.to_string(),
            Err(()) => return Err(ExternalToolError::InvalidTiberRoot),
        };
        if root_uri.len() > MAX_SEMANTIC_TEXT_BYTES {
            return Err(ExternalToolError::InvalidTiberRoot);
        }
        Ok(Self(root_uri))
    }

    /// Returns the trusted local file URI sent only by an authorized roots callback.
    #[must_use]
    #[inline]
    pub fn as_uri(&self) -> &str {
        &self.0
    }
}

/// A bounded, absolute resource identifier selected for one explicit resource-read proposal.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ResourceUri(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the parser precedes the adapter accessor to keep the resource authority boundary readable"
)]
impl ResourceUri {
    /// Parses one bounded absolute resource URI without granting it authority.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalToolError::InvalidResourceUri`] for empty,
    /// control-containing, oversized, or non-absolute values.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ExternalToolError> {
        let trimmed_value = value.trim();
        if trimmed_value.is_empty()
            || trimmed_value.len() > MAX_SEMANTIC_TEXT_BYTES
            || trimmed_value.chars().any(char::is_control)
        {
            return Err(ExternalToolError::InvalidResourceUri);
        }
        let parsed = match Url::parse(trimmed_value) {
            Ok(parsed) => parsed,
            Err(_) => return Err(ExternalToolError::InvalidResourceUri),
        };
        Ok(Self(parsed.into()))
    }

    /// Returns the canonical URI selected for the bounded protocol request.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One literal direct-argv argument from trusted integration configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LiteralArgument(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the parser precedes the accessor to make the trusted configuration boundary readable"
)]
impl LiteralArgument {
    /// Parses one literal argv argument.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalToolError::InvalidLiteralArgument`] for an embedded
    /// NUL, control character, or oversized argument.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ExternalToolError> {
        if value.len() > MAX_SEMANTIC_TEXT_BYTES
            || value.chars().any(char::is_control)
            || value.contains('\0')
        {
            return Err(ExternalToolError::InvalidLiteralArgument);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the literal value passed to `Command::arg`.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated localhost Streamable HTTP endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LoopbackEndpoint(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the parser precedes the accessor to make the trusted configuration boundary readable"
)]
impl LoopbackEndpoint {
    /// Parses an explicit loopback-only Streamable HTTP endpoint.
    ///
    /// # Errors
    ///
    /// Returns a stable endpoint error when the scheme is unsupported, the URL
    /// has user credentials or a fragment, or its host is not a loopback form.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ExternalToolError> {
        let parsed = match Url::parse(value) {
            Ok(parsed) => parsed,
            Err(_) => return Err(ExternalToolError::InvalidEndpoint),
        };
        if parsed.scheme() != "http" {
            return Err(ExternalToolError::UnsupportedEndpointScheme);
        }
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ExternalToolError::InvalidEndpoint);
        }
        let Some(host) = parsed.host_str() else {
            return Err(ExternalToolError::InvalidEndpoint);
        };
        if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
            return Err(ExternalToolError::EndpointNotLoopback);
        }
        Ok(Self(parsed.into()))
    }

    /// Returns the canonical URL used by the Streamable HTTP adapter.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The only transports admitted by the first third-party MCP slice.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "new transports require an explicit authority and test-contract decision"
)]
pub enum McpTransport {
    /// A direct trusted executable and literal argument vector over stdio.
    Stdio {
        /// Absolute program to execute.
        program: AbsoluteProgram,
        /// Literal arguments passed directly to the program.
        arguments: Vec<LiteralArgument>,
    },
    /// A Streamable HTTP endpoint bound to the local machine.
    StreamableHttp {
        /// Loopback-only endpoint.
        endpoint: LoopbackEndpoint,
    },
}

/// Trusted policy classification for one configured tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "a new effect class changes the authorization and reconciliation contract; variants are ordered by authority risk"
)]
pub enum ToolClass {
    /// A call that has no external mutation authority.
    Observe,
    /// A call that may mutate an external system.
    Mutate,
}

/// Closed policy capabilities for the initial third-party MCP boundary.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "excluded MCP protocol features must be explicit rather than silently admitted; variants follow protocol flow"
)]
pub enum ExternalToolCapability {
    /// Discover configured server tools.
    DiscoverTools,
    /// Invoke one configured and authorized tool.
    InvokeTools,
    /// Receive a server tool-list-change notification as untrusted data.
    ObserveToolListChanges,
    /// Receive server progress notifications as untrusted data.
    ObserveProgress,
    /// Receive server logging notifications as untrusted data.
    ObserveLogging,
    /// Receive server resource-change notifications as untrusted data.
    ObserveResourceChanges,
    /// Receive server prompt-list-change notifications as untrusted data.
    ObservePromptChanges,
    /// Declare Tiber-owned roots to a configured server.
    DeclareRoots,
    /// Read optional MCP resources as untrusted data.
    ReadResources,
    /// Read optional MCP prompts as untrusted data.
    ReadPrompts,
    /// Ask the configured read-only mutation-status tool to reconcile an outcome.
    ReconcileMutations,
}

/// Trusted externally configured classification for one server tool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConfiguredTool {
    /// Trusted configured side-effect classification.
    class: ToolClass,
    /// Trusted configured protocol tool name.
    name: ToolName,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "constructor and accessors are ordered by authority use rather than alphabetically"
)]
impl ConfiguredTool {
    /// Creates a trusted tool classification independent of server metadata.
    #[must_use]
    #[inline]
    pub fn new(name: ToolName, class: ToolClass) -> Self {
        Self { class, name }
    }

    /// Returns the trusted effect classification.
    #[must_use]
    #[inline]
    pub const fn class(&self) -> ToolClass {
        self.class
    }

    /// Returns the configured tool name.
    #[must_use]
    #[inline]
    pub fn name(&self) -> &ToolName {
        &self.name
    }
}

/// One explicit, trusted third-party MCP integration configuration.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct McpIntegration {
    /// Stable configured integration identity.
    id: IntegrationId,
    /// Trusted configured read-only status tool for ambiguous mutations.
    reconciliation_tool: Option<ToolName>,
    /// Trusted fixed filesystem roots that only an authorized roots callback may disclose.
    #[serde(skip)]
    roots: Vec<TiberOwnedRoot>,
    /// Trusted configured tool classes keyed by exact name.
    tools: BTreeMap<ToolName, ToolClass>,
    /// Trusted direct-argv or loopback transport configuration.
    transport: McpTransport,
}

#[expect(
    clippy::implicit_return,
    reason = "the redacted trusted-integration formatter uses an idiomatic tail expression"
)]
impl fmt::Debug for McpIntegration {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpIntegration")
            .field("id", &self.id)
            .field("reconciliation_tool", &self.reconciliation_tool)
            .field("tools", &self.tools)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "constructor and accessors are ordered by authority use rather than alphabetically"
)]
impl McpIntegration {
    /// Builds a bounded trusted integration catalog.
    ///
    /// # Errors
    ///
    /// Returns a stable error for an empty/oversized catalog, duplicated tool,
    /// or a reconciliation tool that is absent or mutating.
    #[inline]
    pub fn new<Tools>(
        id: IntegrationId,
        transport: McpTransport,
        tools: Tools,
        reconciliation_tool: Option<ToolName>,
    ) -> Result<Self, ExternalToolError>
    where
        Tools: IntoIterator<Item = ConfiguredTool>,
    {
        let mut configured_tools = BTreeMap::new();
        for tool in tools {
            if configured_tools.insert(tool.name, tool.class).is_some() {
                return Err(ExternalToolError::DuplicateConfiguredTool);
            }
            if configured_tools.len() > MAX_CONFIGURED_TOOLS {
                return Err(ExternalToolError::InvalidToolCatalog);
            }
        }
        if configured_tools.is_empty() {
            return Err(ExternalToolError::InvalidToolCatalog);
        }
        if let Some(configured_reconciliation_tool) = reconciliation_tool.as_ref()
            && configured_tools.get(configured_reconciliation_tool) != Some(&ToolClass::Observe)
        {
            return Err(ExternalToolError::InvalidReconciliationTool);
        }
        Ok(Self {
            id,
            reconciliation_tool,
            roots: Vec::new(),
            tools: configured_tools,
            transport,
        })
    }

    /// Replaces this integration's trusted Tiber-owned root catalog.
    ///
    /// An empty catalog is valid: the caller may still explicitly authorize a
    /// roots capability without disclosing a filesystem location.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the catalog exceeds the fixed bound or
    /// contains a duplicate trusted root.
    #[inline]
    pub fn with_tiber_roots<Roots>(mut self, roots: Roots) -> Result<Self, ExternalToolError>
    where
        Roots: IntoIterator<Item = TiberOwnedRoot>,
    {
        let mut configured_roots = Vec::new();
        let mut seen_roots = BTreeSet::new();
        for root in roots {
            if !seen_roots.insert(root.clone()) {
                return Err(ExternalToolError::DuplicateTiberRoot);
            }
            if configured_roots.len() == MAX_TIBER_OWNED_ROOTS {
                return Err(ExternalToolError::InvalidTiberRootCatalog);
            }
            configured_roots.push(root);
        }
        self.roots = configured_roots;
        Ok(self)
    }

    /// Returns the trusted integration identifier.
    #[must_use]
    #[inline]
    pub fn id(&self) -> &IntegrationId {
        &self.id
    }

    /// Returns the configured classification for a known tool.
    #[must_use]
    #[inline]
    pub fn tool_class(&self, name: &ToolName) -> Option<ToolClass> {
        self.tools.get(name).copied()
    }

    /// Returns the configured read-only reconciliation tool, if this integration has one.
    #[must_use]
    #[inline]
    pub fn reconciliation_tool(&self) -> Option<&ToolName> {
        self.reconciliation_tool.as_ref()
    }

    /// Returns the fixed trusted roots for a root-declaration authorization only.
    #[inline]
    fn tiber_roots(&self) -> &[TiberOwnedRoot] {
        &self.roots
    }

    /// Returns the trusted transport configuration.
    #[must_use]
    #[inline]
    pub fn transport(&self) -> &McpTransport {
        &self.transport
    }
}

/// Validated bounded JSON input for a tool invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ToolArguments(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the parser precedes the adapter accessor to make the untrusted-input boundary readable"
)]
impl ToolArguments {
    /// Parses a bounded JSON argument payload without granting it authority.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalToolError::InvalidToolArguments`] if the payload is
    /// oversized, not valid JSON, or not a JSON object.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ExternalToolError> {
        if value.len() > MAX_TOOL_ARGUMENT_BYTES {
            return Err(ExternalToolError::InvalidToolArguments);
        }
        let parsed = match serde_json::from_str::<serde_json::Value>(value) {
            Ok(parsed) => parsed,
            Err(_) => return Err(ExternalToolError::InvalidToolArguments),
        };
        if !parsed.is_object() {
            return Err(ExternalToolError::InvalidToolArguments);
        }
        Ok(Self(value.to_owned()))
    }

    /// Adds the typed mutation idempotency identity to the initial tool-call wire object.
    fn with_idempotency_key(
        self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Self, ExternalToolError> {
        let mut parsed = match serde_json::from_str::<serde_json::Value>(&self.0) {
            Ok(parsed) => parsed,
            Err(_) => return Err(ExternalToolError::InvalidToolArguments),
        };
        let Some(arguments) = parsed.as_object_mut() else {
            return Err(ExternalToolError::InvalidToolArguments);
        };
        let canonical_value = serde_json::Value::String(idempotency_key.as_str().to_owned());
        if let Some(existing) = arguments.get(IDEMPOTENCY_KEY_ARGUMENT)
            && existing != &canonical_value
        {
            return Err(ExternalToolError::MutationIdempotencyConflict);
        }
        arguments.insert(IDEMPOTENCY_KEY_ARGUMENT.to_owned(), canonical_value);
        let serialized = match serde_json::to_string(&parsed) {
            Ok(serialized) => serialized,
            Err(_) => return Err(ExternalToolError::InvalidToolArguments),
        };
        if serialized.len() > MAX_TOOL_ARGUMENT_BYTES {
            return Err(ExternalToolError::InvalidToolArguments);
        }
        Ok(Self(serialized))
    }

    /// Returns the validated JSON selected for the protocol adapter wire call.
    #[must_use]
    #[inline]
    pub fn as_json(&self) -> &str {
        &self.0
    }
}

/// Validated bounded JSON input for one prompt retrieval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PromptArguments(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the parser precedes the adapter accessor to keep prompt inputs separate from tool invocation semantics"
)]
impl PromptArguments {
    /// Parses bounded JSON object arguments without granting prompt authority.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalToolError::InvalidPromptArguments`] when the payload
    /// is oversized, not valid JSON, or is not a JSON object.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ExternalToolError> {
        if value.len() > MAX_TOOL_ARGUMENT_BYTES {
            return Err(ExternalToolError::InvalidPromptArguments);
        }
        let parsed = match serde_json::from_str::<serde_json::Value>(value) {
            Ok(parsed) => parsed,
            Err(_) => return Err(ExternalToolError::InvalidPromptArguments),
        };
        if !parsed.is_object() {
            return Err(ExternalToolError::InvalidPromptArguments);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the validated JSON object selected for the prompt protocol request.
    #[must_use]
    #[inline]
    pub fn as_json(&self) -> &str {
        &self.0
    }
}

/// An untrusted server-provided tool description, schema, or result payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UntrustedPayload(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the bounded constructor precedes the accessor to make the untrusted-input boundary readable"
)]
impl UntrustedPayload {
    /// Bounds one untrusted protocol value before it reaches a projection or model context.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalToolError::UntrustedPayloadTooLarge`] when the value
    /// exceeds the fixed boundary. The payload remains untrusted on success.
    #[inline]
    pub fn bounded(value: &str) -> Result<Self, ExternalToolError> {
        if value.len() > MAX_UNTRUSTED_PAYLOAD_BYTES {
            return Err(ExternalToolError::UntrustedPayloadTooLarge);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the bounded untrusted protocol data.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One policy layer's explicitly configured tools and protocol capabilities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PermissionGrant {
    /// Protocol operations permitted by this policy layer.
    capabilities: BTreeSet<ExternalToolCapability>,
    /// Exact configured tools permitted by this policy layer.
    tools: BTreeSet<ToolName>,
}

#[expect(
    clippy::implicit_return,
    reason = "grant constructors and predicates use idiomatic tail expressions"
)]
impl PermissionGrant {
    /// Creates one deny-by-default authority grant.
    #[must_use]
    #[inline]
    pub fn new<Tools, Capabilities>(tools: Tools, capabilities: Capabilities) -> Self
    where
        Tools: IntoIterator<Item = ToolName>,
        Capabilities: IntoIterator<Item = ExternalToolCapability>,
    {
        Self {
            capabilities: capabilities.into_iter().collect(),
            tools: tools.into_iter().collect(),
        }
    }

    /// Returns whether this layer permits one exact tool invocation capability.
    #[inline]
    fn permits(&self, tool: &ToolName, capability: ExternalToolCapability) -> bool {
        self.tools.contains(tool) && self.capabilities.contains(&capability)
    }

    /// Returns whether this layer permits a capability regardless of tool name.
    #[inline]
    fn permits_capability(&self, capability: ExternalToolCapability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// A grant tied to one exact workflow authority identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScopedPermission<I> {
    /// Policy permission set bound to the scoped identity.
    grant: PermissionGrant,
    /// Exact identity this policy layer authorizes.
    subject: I,
}

#[expect(
    clippy::implicit_return,
    reason = "the scoped policy constructor uses an idiomatic tail expression"
)]
impl<I> ScopedPermission<I> {
    /// Creates a policy grant bound to one typed current-authority identity.
    #[must_use]
    #[inline]
    pub fn new(subject: I, grant: PermissionGrant) -> Self {
        Self { grant, subject }
    }
}

/// The caller-supplied current authority tuple, independent of server data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizationContext {
    /// Current role selected for the acting agent.
    agent_role: AgentRole,
    /// Current bounded work assignment identity.
    assignment: AssignmentId,
    /// Current durable policy decision identity.
    policy_decision: PolicyDecisionId,
    /// Current durable session identity.
    session: SessionId,
    /// Current workflow execution mode.
    workflow_mode: WorkflowMode,
}

#[expect(
    clippy::implicit_return,
    reason = "the context constructor uses an idiomatic tail expression"
)]
impl AuthorizationContext {
    /// Creates the full current authority tuple used by every policy intersection.
    #[must_use]
    #[inline]
    pub fn new(
        workflow_mode: WorkflowMode,
        agent_role: AgentRole,
        session: SessionId,
        assignment: AssignmentId,
        policy_decision: PolicyDecisionId,
    ) -> Self {
        Self {
            agent_role,
            assignment,
            policy_decision,
            session,
            workflow_mode,
        }
    }
}

/// The complete policy intersection required before a server may be contacted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PolicyIntersection {
    /// Agent-role policy layer.
    agent_role: ScopedPermission<AgentRole>,
    /// Assignment policy layer.
    assignment: ScopedPermission<AssignmentId>,
    /// Effect/policy-decision policy layer.
    effect: ScopedPermission<PolicyDecisionId>,
    /// Global policy layer.
    global: PermissionGrant,
    /// Stable textual identity of the trusted integration this policy may authorize.
    integration: IntegrationId,
    /// Complete immutable trusted configuration bound to this policy decision.
    #[serde(skip)]
    integration_configuration: McpIntegration,
    /// Session policy layer.
    session: ScopedPermission<SessionId>,
    /// Workflow-mode policy layer.
    workflow_mode: ScopedPermission<WorkflowMode>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the intersection helpers follow policy evaluation order rather than alphabetic order"
)]
impl PolicyIntersection {
    /// Creates all six independent policy layers.
    #[must_use]
    #[inline]
    pub fn new(
        integration: &McpIntegration,
        global: PermissionGrant,
        workflow_mode: ScopedPermission<WorkflowMode>,
        agent_role: ScopedPermission<AgentRole>,
        session: ScopedPermission<SessionId>,
        assignment: ScopedPermission<AssignmentId>,
        effect: ScopedPermission<PolicyDecisionId>,
    ) -> Self {
        Self {
            agent_role,
            assignment,
            effect,
            global,
            integration: integration.id().clone(),
            integration_configuration: integration.clone(),
            session,
            workflow_mode,
        }
    }

    /// Returns all six grants when every scoped layer exactly matches the current context.
    fn matching_grants<'grant>(
        &'grant self,
        context: &'grant AuthorizationContext,
    ) -> Result<[&'grant PermissionGrant; 6], ExternalToolError> {
        if self.workflow_mode.subject != context.workflow_mode
            || self.agent_role.subject != context.agent_role
            || self.session.subject != context.session
            || self.assignment.subject != context.assignment
            || self.effect.subject != context.policy_decision
        {
            return Err(ExternalToolError::PolicyContextMismatch);
        }
        Ok([
            &self.global,
            &self.workflow_mode.grant,
            &self.agent_role.grant,
            &self.session.grant,
            &self.assignment.grant,
            &self.effect.grant,
        ])
    }

    /// Requires the caller's integration to match the complete bound configuration.
    fn permit_integration(&self, integration: &McpIntegration) -> Result<(), ExternalToolError> {
        if &self.integration_configuration != integration {
            return Err(ExternalToolError::PolicyIntegrationMismatch);
        }
        Ok(())
    }

    /// Requires every policy layer to permit one capability.
    fn permit_capability(
        &self,
        context: &AuthorizationContext,
        integration: &McpIntegration,
        capability: ExternalToolCapability,
    ) -> Result<(), ExternalToolError> {
        match self.permit_integration(integration) {
            Ok(()) => {}
            Err(error) => return Err(error),
        }
        let grants = match self.matching_grants(context) {
            Ok(grants) => grants,
            Err(error) => return Err(error),
        };
        if grants
            .into_iter()
            .all(|grant| grant.permits_capability(capability))
        {
            return Ok(());
        }
        Err(ExternalToolError::CapabilityDenied)
    }

    /// Requires every policy layer to permit one exact configured tool capability.
    fn permit_tool(
        &self,
        context: &AuthorizationContext,
        integration: &McpIntegration,
        tool: &ToolName,
        capability: ExternalToolCapability,
    ) -> Result<(), ExternalToolError> {
        match self.permit_integration(integration) {
            Ok(()) => {}
            Err(error) => return Err(error),
        }
        let grants = match self.matching_grants(context) {
            Ok(grants) => grants,
            Err(error) => return Err(error),
        };
        if grants
            .into_iter()
            .all(|grant| grant.permits(tool, capability))
        {
            return Ok(());
        }
        Err(ExternalToolError::ToolDenied)
    }
}

/// A proposed tool call whose raw arguments have no authority by themselves.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolCallProposal {
    /// Untrusted bounded JSON arguments for the named tool proposal.
    arguments: ToolArguments,
    /// Stable deduplication identity offered for a potentially mutating call.
    idempotency_key: Option<IdempotencyKey>,
    /// Requested tool name; it has no effect class outside trusted configuration.
    tool: ToolName,
}

#[expect(
    clippy::implicit_return,
    reason = "the proposal constructor uses an idiomatic tail expression"
)]
impl ToolCallProposal {
    /// Creates an untrusted-proposal-shaped input for pure policy evaluation.
    #[must_use]
    #[inline]
    pub fn new(
        tool: ToolName,
        arguments: ToolArguments,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Self {
        Self {
            arguments,
            idempotency_key,
            tool,
        }
    }
}

/// A proposed read of one resource whose URI has no authority by itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceReadProposal {
    /// The bounded absolute resource URI selected for a possible read.
    uri: ResourceUri,
}

#[expect(
    clippy::implicit_return,
    reason = "the proposal constructor uses an idiomatic tail expression"
)]
impl ResourceReadProposal {
    /// Creates an untrusted-proposal-shaped resource read for pure policy evaluation.
    #[must_use]
    #[inline]
    pub fn new(uri: ResourceUri) -> Self {
        Self { uri }
    }
}

/// A proposed retrieval of one prompt whose name and arguments have no authority by themselves.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PromptGetProposal {
    /// Optional bounded JSON object supplied to the selected prompt template.
    arguments: Option<PromptArguments>,
    /// The bounded prompt name selected for a possible retrieval.
    name: PromptName,
}

#[expect(
    clippy::implicit_return,
    reason = "the proposal constructor uses an idiomatic tail expression"
)]
impl PromptGetProposal {
    /// Creates an untrusted-proposal-shaped prompt retrieval for pure policy evaluation.
    #[must_use]
    #[inline]
    pub fn new(name: PromptName, arguments: Option<PromptArguments>) -> Self {
        Self { arguments, name }
    }
}

/// Opaque authorization to discover the tools of one configured integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedToolListing {
    /// Integration selected by the complete pure policy decision.
    integration: McpIntegration,
}

#[expect(
    clippy::implicit_return,
    reason = "the opaque-listing accessor uses an idiomatic tail expression"
)]
impl AuthorizedToolListing {
    /// Returns the integration selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }
}

/// Opaque authorization to disclose the fixed Tiber-owned roots of one integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedRootDeclaration {
    /// Trusted integration selected by the complete pure policy decision.
    integration: McpIntegration,
    /// Immutable snapshot of the only roots an adapter may disclose for this authorization.
    roots: Vec<TiberOwnedRoot>,
}

#[expect(
    clippy::implicit_return,
    reason = "opaque roots authorization accessors use idiomatic tail expressions"
)]
impl AuthorizedRootDeclaration {
    /// Returns the trusted integration selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }

    /// Returns the fixed trusted roots that may be disclosed by this authorization only.
    #[must_use]
    #[inline]
    pub fn roots(&self) -> &[TiberOwnedRoot] {
        &self.roots
    }
}

/// Opaque authorization to list the untrusted resources of one configured integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedResourceListing {
    /// Trusted integration selected by the complete pure policy decision.
    integration: McpIntegration,
}

#[expect(
    clippy::implicit_return,
    reason = "opaque resource-listing accessors use idiomatic tail expressions"
)]
impl AuthorizedResourceListing {
    /// Returns the trusted integration selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }
}

/// Opaque authorization to read exactly one resource of one configured integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedResourceRead {
    /// Trusted integration selected by the complete pure policy decision.
    integration: McpIntegration,
    /// Exact bounded URI selected by the pure policy decision.
    uri: ResourceUri,
}

#[expect(
    clippy::implicit_return,
    reason = "opaque resource-read accessors use idiomatic tail expressions"
)]
impl AuthorizedResourceRead {
    /// Returns the trusted integration selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }

    /// Returns the one bounded resource URI selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn uri(&self) -> &ResourceUri {
        &self.uri
    }
}

/// Opaque authorization to list the untrusted prompts of one configured integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedPromptListing {
    /// Trusted integration selected by the complete pure policy decision.
    integration: McpIntegration,
}

#[expect(
    clippy::implicit_return,
    reason = "opaque prompt-listing accessors use idiomatic tail expressions"
)]
impl AuthorizedPromptListing {
    /// Returns the trusted integration selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }
}

/// Opaque authorization to retrieve exactly one prompt of one configured integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedPromptGet {
    /// Optional bounded JSON arguments selected by the pure policy decision.
    arguments: Option<PromptArguments>,
    /// Trusted integration selected by the complete pure policy decision.
    integration: McpIntegration,
    /// Exact bounded prompt name selected by the pure policy decision.
    name: PromptName,
}

#[expect(
    clippy::implicit_return,
    reason = "opaque prompt retrieval accessors use idiomatic tail expressions"
)]
impl AuthorizedPromptGet {
    /// Returns optional bounded JSON arguments selected for the prompt request.
    #[must_use]
    #[inline]
    pub fn arguments(&self) -> Option<&PromptArguments> {
        self.arguments.as_ref()
    }

    /// Returns the trusted integration selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }

    /// Returns the one bounded prompt name selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn name(&self) -> &PromptName {
        &self.name
    }
}

/// Opaque authorization to invoke exactly one configured tool.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedToolCall {
    /// Explicit owner approval for a mutating call.
    approval: Option<OwnerApprovalId>,
    /// Bounded raw JSON passed verbatim to the imperative adapter.
    arguments: ToolArguments,
    /// Exact trusted authority tuple that produced this opaque token.
    authorization: AuthorizationContext,
    /// Trusted configured side-effect classification.
    class: ToolClass,
    /// Stable deduplication identity for a mutating call.
    idempotency_key: Option<IdempotencyKey>,
    /// Trusted integration selected by the complete policy decision.
    integration: McpIntegration,
    /// Exact configured tool selected by the complete policy decision.
    tool: ToolName,
}

/// Exact sanitized transcript of one refused tool-call authorization decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolCallDenial {
    /// Exact trusted authority tuple evaluated by policy.
    authorization: AuthorizationContext,
    /// Configured effect class when the requested tool was known.
    class: Option<ToolClass>,
    /// Stable refusal produced by the authorization decision.
    error: ExternalToolError,
    /// Requested integration identity without transport or other configuration.
    integration_id: IntegrationId,
    /// Exact requested tool name.
    tool: ToolName,
}

#[expect(
    clippy::implicit_return,
    reason = "denial accessors use idiomatic tail expressions"
)]
impl ToolCallDenial {
    /// Returns the exact trusted authority tuple evaluated by policy.
    #[must_use]
    #[inline]
    pub fn authorization(&self) -> &AuthorizationContext {
        &self.authorization
    }

    /// Returns the configured effect class when the requested tool was known.
    #[must_use]
    #[inline]
    pub const fn class(&self) -> Option<ToolClass> {
        self.class
    }

    /// Returns the stable refusal code without serializing the error enum.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> &'static str {
        self.error.code()
    }

    /// Returns the stable policy or mutation-fence refusal.
    #[must_use]
    #[inline]
    pub const fn error(&self) -> ExternalToolError {
        self.error
    }

    /// Returns only the requested trusted integration identity.
    #[must_use]
    #[inline]
    pub fn integration_id(&self) -> &IntegrationId {
        &self.integration_id
    }

    /// Returns the exact requested tool name.
    #[must_use]
    #[inline]
    pub fn tool(&self) -> &ToolName {
        &self.tool
    }
}

/// Complete pure decision for one proposed configured tool invocation.
#[expect(
    clippy::exhaustive_enums,
    reason = "every tool-call authorization decision must remain explicit"
)]
pub enum ToolCallAuthorizationDecision {
    /// All policy and mutation fences produced an opaque invocation token.
    Authorized(AuthorizedToolCall),
    /// Authorization was refused with an exact sanitized decision transcript.
    Denied(ToolCallDenial),
}

#[expect(
    clippy::implicit_return,
    reason = "the compatibility projection is a direct total decision match"
)]
impl ToolCallAuthorizationDecision {
    /// Projects the complete decision into the original authorization API.
    ///
    /// # Errors
    ///
    /// Returns the exact stable refusal retained by a denied decision.
    #[inline]
    pub fn into_result(self) -> Result<AuthorizedToolCall, ExternalToolError> {
        match self {
            Self::Authorized(call) => Ok(call),
            Self::Denied(denial) => Err(denial.error),
        }
    }
}

/// Refusal to bind an adapter outcome to its exact originating authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "every provenance-binding failure must remain an explicit closed case"
)]
pub enum ToolCallOutcomeBindingError {
    /// An ambiguous recovery token did not originate from the supplied call authorization.
    ReconciliationMismatch,
    /// An observed payload carried no originating authorization identity.
    UnattributedObservation,
}

impl ToolCallOutcomeBindingError {
    /// Returns the stable code for this provenance-binding refusal.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the stable code is a direct total projection"
    )]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ReconciliationMismatch => "external_tools_reconciliation_mismatch",
            Self::UnattributedObservation => "external_tools_unattributed_observation",
        }
    }
}

impl fmt::Display for ToolCallOutcomeBindingError {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates to the stable closed error code"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the closed binding error has no lower-level cause"
)]
impl Error for ToolCallOutcomeBindingError {}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "accessors follow invocation and recovery flow rather than alphabetical order"
)]
impl AuthorizedToolCall {
    /// Returns the exact trusted authority tuple that produced this token.
    #[must_use]
    #[inline]
    pub fn authorization(&self) -> &AuthorizationContext {
        &self.authorization
    }

    /// Returns the approved invocation's JSON argument payload.
    ///
    /// Mutating calls contain the canonical reserved `idempotencyKey` member
    /// matching [`Self::idempotency_key`]. Read-only calls retain their
    /// validated caller-provided JSON object without a Tiber authority binding.
    #[must_use]
    #[inline]
    pub fn arguments(&self) -> &ToolArguments {
        &self.arguments
    }

    /// Returns the configured trusted effect class.
    #[must_use]
    #[inline]
    pub const fn class(&self) -> ToolClass {
        self.class
    }

    /// Returns the stable key required for a mutating invocation, if any.
    #[must_use]
    #[inline]
    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }

    /// Returns the trusted integration selected by the policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }

    /// Returns whether an explicit owner approval was bound to this invocation.
    #[must_use]
    #[inline]
    pub fn owner_approval(&self) -> Option<&OwnerApprovalId> {
        self.approval.as_ref()
    }

    /// Returns the one configured tool selected by the policy decision.
    #[must_use]
    #[inline]
    pub fn tool(&self) -> &ToolName {
        &self.tool
    }

    /// Builds the only permitted mutation-reconciliation request for this call.
    #[must_use]
    #[inline]
    pub fn reconciliation(&self) -> Option<AuthorizedReconciliation> {
        if self.class != ToolClass::Mutate {
            return None;
        }
        let idempotency_key = match self.idempotency_key.clone() {
            Some(key) => key,
            None => return None,
        };
        let status_tool = match self.integration.reconciliation_tool() {
            Some(tool) => tool.clone(),
            None => return None,
        };
        Some(AuthorizedReconciliation {
            authorization: Box::new(self.authorization.clone()),
            idempotency_key,
            integration: Box::new(self.integration.clone()),
            originating_tool: self.tool.clone(),
            owner_approval: self.approval.clone(),
            status_tool,
        })
    }

    /// Consumes this mutation token to record an ambiguous outcome that must be reconciled.
    #[must_use]
    #[inline]
    pub fn into_outcome_unknown(self) -> Option<ToolCallOutcome> {
        if self.class != ToolClass::Mutate {
            return None;
        }
        let idempotency_key = match self.idempotency_key {
            Some(key) => key,
            None => return None,
        };
        let status_tool = match self.integration.reconciliation_tool() {
            Some(tool) => tool.clone(),
            None => return None,
        };
        Some(ToolCallOutcome::OutcomeUnknown(AuthorizedReconciliation {
            authorization: Box::new(self.authorization),
            idempotency_key,
            integration: Box::new(self.integration),
            originating_tool: self.tool,
            owner_approval: self.approval,
            status_tool,
        }))
    }

    /// Binds an adapter result to this exact authorization for safe downstream projection.
    ///
    /// # Errors
    ///
    /// Refuses an ambiguous result whose reconciliation authority did not
    /// originate from this exact authorization context and mutation identity.
    #[inline]
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "borrowing the closed outcome is required before moving it into the bound transcript"
    )]
    pub fn bind_outcome(
        self,
        outcome: ToolCallOutcome,
    ) -> Result<BoundToolCallOutcome, ToolCallOutcomeBindingError> {
        match &outcome {
            ToolCallOutcome::Observed(_) => {
                return Err(ToolCallOutcomeBindingError::UnattributedObservation);
            }
            ToolCallOutcome::OutcomeUnknown(reconciliation)
                if self.reconciliation().as_ref() != Some(reconciliation) =>
            {
                return Err(ToolCallOutcomeBindingError::ReconciliationMismatch);
            }
            ToolCallOutcome::OutcomeUnknown(_) => {}
        }
        Ok(BoundToolCallOutcome {
            outcome,
            provenance: self.into_provenance(),
        })
    }

    /// Consumes this exact authorization and binds the payload observed by its adapter call.
    ///
    /// This is an adapter-implementation boundary, not independent proof that
    /// dispatch occurred. Application callers must use their configured adapter
    /// call API and retain only the bound outcome returned by that operation.
    #[must_use]
    #[inline]
    pub fn bind_observation(self, payload: UntrustedPayload) -> BoundToolCallOutcome {
        BoundToolCallOutcome {
            outcome: ToolCallOutcome::Observed(payload),
            provenance: self.into_provenance(),
        }
    }

    /// Consumes this mutation authorization into its only possible ambiguous transcript.
    ///
    /// # Errors
    ///
    /// Returns safe provenance without replay authority when the token is not a
    /// fully fenced mutating authorization.
    #[inline]
    #[expect(
        clippy::result_large_err,
        reason = "failure must return the complete safe provenance after consuming replay authority"
    )]
    pub fn bind_ambiguity(self) -> Result<BoundToolCallOutcome, BoundToolCallFailure> {
        let Some(reconciliation) = self.reconciliation() else {
            return Err(self.bind_failure());
        };
        Ok(BoundToolCallOutcome {
            outcome: ToolCallOutcome::OutcomeUnknown(reconciliation),
            provenance: self.into_provenance(),
        })
    }

    /// Consumes invocation authority into safe failure provenance with no replay route.
    #[must_use]
    #[inline]
    pub fn bind_failure(self) -> BoundToolCallFailure {
        BoundToolCallFailure {
            provenance: self.into_provenance(),
        }
    }

    /// Removes arguments and integration configuration from a consumed authorization.
    fn into_provenance(self) -> ToolCallProvenance {
        ToolCallProvenance {
            approval: self.approval,
            authorization: self.authorization,
            class: self.class,
            idempotency_key: self.idempotency_key,
            integration_id: self.integration.id().clone(),
            tool: self.tool,
        }
    }
}

/// Opaque authorization to reconcile one ambiguous mutating tool invocation.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedReconciliation {
    /// Exact trusted authority tuple that produced the original mutation token.
    authorization: Box<AuthorizationContext>,
    /// Stable deduplication identity whose outcome is being checked.
    idempotency_key: IdempotencyKey,
    /// Trusted integration selected by the original policy decision.
    integration: Box<McpIntegration>,
    /// Exact mutating tool whose outcome is being reconciled.
    originating_tool: ToolName,
    /// Explicit approval bound to the original mutating call.
    owner_approval: Option<OwnerApprovalId>,
    /// Exact configured read-only status tool.
    status_tool: ToolName,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "recovery binding methods remain next to the authority accessor that supplies their provenance"
)]
impl AuthorizedReconciliation {
    /// Returns the exact bounded JSON object passed to the configured status tool.
    #[must_use]
    #[inline]
    pub fn arguments(&self) -> ToolArguments {
        ToolArguments(format!(
            r#"{{"{}":{}}}"#,
            IDEMPOTENCY_KEY_ARGUMENT,
            serde_json::Value::String(self.idempotency_key.as_str().to_owned())
        ))
    }

    /// Returns the exact trusted authority tuple that produced the original mutation token.
    #[must_use]
    #[inline]
    pub fn authorization(&self) -> &AuthorizationContext {
        &self.authorization
    }

    /// Binds one observed status outcome to this exact recovery authorization.
    ///
    /// The returned transcript contains no integration configuration, status
    /// arguments, payload, or invocation replay authority.
    #[must_use]
    #[inline]
    pub fn bind_outcome(&self, outcome: ReconciliationOutcome) -> BoundReconciliationOutcome {
        BoundReconciliationOutcome {
            outcome,
            provenance: self.provenance(),
        }
    }

    /// Binds a sanitized adapter failure to this exact recovery authorization.
    ///
    /// The returned transcript contains no integration configuration, status
    /// arguments, payload, or invocation replay authority.
    #[must_use]
    #[inline]
    pub fn bind_failure(&self) -> BoundReconciliationFailure {
        BoundReconciliationFailure {
            provenance: self.provenance(),
        }
    }

    /// Returns the stable idempotency identity used by the status operation.
    #[must_use]
    #[inline]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// Returns the trusted integration selected by the original authorization.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }

    /// Returns the exact mutating tool whose outcome is being reconciled.
    #[must_use]
    #[inline]
    pub fn originating_tool(&self) -> &ToolName {
        &self.originating_tool
    }

    /// Returns the approval bound to the original mutation.
    #[must_use]
    #[inline]
    pub fn owner_approval(&self) -> Option<&OwnerApprovalId> {
        self.owner_approval.as_ref()
    }

    /// Returns the configured read-only status tool and not an arbitrary caller choice.
    #[must_use]
    #[inline]
    pub fn status_tool(&self) -> &ToolName {
        &self.status_tool
    }

    /// Copies only safe recovery provenance out of the opaque authorization.
    fn provenance(&self) -> ReconciliationProvenance {
        ReconciliationProvenance {
            approval: self.owner_approval.clone(),
            authorization: (*self.authorization).clone(),
            idempotency_key: self.idempotency_key.clone(),
            integration_id: self.integration.id().clone(),
            originating_tool: self.originating_tool.clone(),
            status_tool: self.status_tool.clone(),
        }
    }
}

/// Result states for a separately requested mutation reconciliation operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "ambiguous external mutations must be resolved explicitly by every caller"
)]
pub enum ReconciliationOutcome {
    /// The configured status operation proved the mutation committed.
    Committed,
    /// The configured status operation proved the mutation did not commit.
    NotCommitted,
    /// The configured status operation could not establish either terminal result.
    StillUnknown,
}

/// Safe provenance-bound result of one authorized read-only reconciliation.
///
/// This opaque transcript deliberately retains neither status arguments nor
/// integration transport configuration, and it carries no invocation replay
/// authority.
pub struct BoundReconciliationOutcome {
    /// Closed status observed by the concrete adapter.
    outcome: ReconciliationOutcome,
    /// Exact safe identity of the recovery authorization used by the adapter.
    provenance: ReconciliationProvenance,
}

/// Safe provenance for one authorized reconciliation attempt that failed.
///
/// This opaque transcript deliberately retains neither status arguments nor
/// integration transport configuration, and it carries no invocation replay
/// authority.
pub struct BoundReconciliationFailure {
    /// Exact safe identity of the recovery authorization used by the adapter.
    provenance: ReconciliationProvenance,
}

/// Shared safe identity extracted from one recovery authorization.
struct ReconciliationProvenance {
    /// Explicit owner approval identity from the originating mutation.
    approval: Option<OwnerApprovalId>,
    /// Exact trusted authority tuple that produced the mutation authorization.
    authorization: AuthorizationContext,
    /// Stable identity of the mutation whose outcome is being checked.
    idempotency_key: IdempotencyKey,
    /// Trusted integration identity without configuration.
    integration_id: IntegrationId,
    /// Exact mutating tool whose outcome is being checked.
    originating_tool: ToolName,
    /// Exact configured read-only status tool used for reconciliation.
    status_tool: ToolName,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the result state follows the shared provenance accessors for audit projection readability"
)]
impl BoundReconciliationOutcome {
    reconciliation_provenance_accessors!();

    /// Returns the closed status observed by the concrete adapter.
    #[must_use]
    #[inline]
    pub const fn outcome(&self) -> ReconciliationOutcome {
        self.outcome
    }
}

#[expect(
    clippy::implicit_return,
    reason = "bound reconciliation failure accessors use idiomatic tail expressions"
)]
impl BoundReconciliationFailure {
    reconciliation_provenance_accessors!();
}

/// A bounded observation from one authorized tool invocation.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must explicitly distinguish observed data from an ambiguous mutation"
)]
pub enum ToolCallOutcome {
    /// The adapter observed bounded untrusted tool output.
    Observed(UntrustedPayload),
    /// The adapter cannot determine a mutation outcome and must reconcile it before any retry.
    OutcomeUnknown(AuthorizedReconciliation),
}

impl ToolCallOutcome {
    /// Returns the bounded untrusted payload when the adapter observed a response.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the read-only projection matches directly over the borrowed closed outcome"
    )]
    pub fn observed_payload(&self) -> Option<&UntrustedPayload> {
        match self {
            Self::Observed(payload) => Some(payload),
            Self::OutcomeUnknown(_) => None,
        }
    }
}

/// Safe provenance binding between one authorized call and its adapter outcome.
///
/// This opaque value deliberately retains neither invocation arguments nor
/// integration transport configuration. It also deliberately omits `Debug`
/// and `Serialize` because an observed outcome still contains untrusted payload.
pub struct BoundToolCallOutcome {
    /// Adapter result bound to the authorization.
    outcome: ToolCallOutcome,
    /// Safe originating identity with all invocation authority removed.
    provenance: ToolCallProvenance,
}

/// Safe provenance for one authorized call that failed before an outcome was observed.
///
/// This opaque value deliberately retains neither invocation arguments nor
/// integration transport configuration. It also deliberately omits `Debug`
/// and `Serialize` so failure handling cannot accidentally expose authority.
pub struct BoundToolCallFailure {
    /// Safe originating identity with all invocation authority removed.
    provenance: ToolCallProvenance,
}

/// Shared safe identity extracted by consuming one invocation authorization.
struct ToolCallProvenance {
    /// Explicit owner approval identity, when present on the authorization.
    approval: Option<OwnerApprovalId>,
    /// Exact trusted authority tuple that produced the authorization.
    authorization: AuthorizationContext,
    /// Trusted configured side-effect classification.
    class: ToolClass,
    /// Stable mutation idempotency identity, when present.
    idempotency_key: Option<IdempotencyKey>,
    /// Trusted integration identity without configuration.
    integration_id: IntegrationId,
    /// Exact trusted configured tool name.
    tool: ToolName,
}

#[expect(
    clippy::implicit_return,
    reason = "bound-outcome accessors use idiomatic tail expressions"
)]
impl BoundToolCallOutcome {
    /// Returns explicit owner approval identity when bound to the call.
    #[must_use]
    #[inline]
    pub fn approval(&self) -> Option<&OwnerApprovalId> {
        self.provenance.approval.as_ref()
    }

    /// Returns the exact authority tuple bound by pure authorization.
    #[must_use]
    #[inline]
    pub fn authorization(&self) -> &AuthorizationContext {
        &self.provenance.authorization
    }

    /// Returns the trusted configured effect class.
    #[must_use]
    #[inline]
    pub const fn class(&self) -> ToolClass {
        self.provenance.class
    }

    /// Returns the stable mutation idempotency identity when present.
    #[must_use]
    #[inline]
    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.provenance.idempotency_key.as_ref()
    }

    /// Returns the trusted integration identity without configuration.
    #[must_use]
    #[inline]
    pub fn integration_id(&self) -> &IntegrationId {
        &self.provenance.integration_id
    }

    /// Consumes an ambiguous outcome into its exact non-replayable recovery token.
    #[must_use]
    #[inline]
    pub fn into_reconciliation(self) -> Option<AuthorizedReconciliation> {
        match self.outcome {
            ToolCallOutcome::Observed(_) => None,
            ToolCallOutcome::OutcomeUnknown(reconciliation) => Some(reconciliation),
        }
    }

    /// Returns the adapter outcome for sanitized downstream projection.
    #[must_use]
    #[inline]
    pub fn outcome(&self) -> &ToolCallOutcome {
        &self.outcome
    }

    /// Returns the exact trusted configured tool name.
    #[must_use]
    #[inline]
    pub fn tool(&self) -> &ToolName {
        &self.provenance.tool
    }
}

#[expect(
    clippy::implicit_return,
    reason = "bound-failure accessors use idiomatic tail expressions"
)]
impl BoundToolCallFailure {
    /// Returns explicit owner approval identity when bound to the failed call.
    #[must_use]
    #[inline]
    pub fn approval(&self) -> Option<&OwnerApprovalId> {
        self.provenance.approval.as_ref()
    }

    /// Returns the exact authority tuple bound by pure authorization.
    #[must_use]
    #[inline]
    pub fn authorization(&self) -> &AuthorizationContext {
        &self.provenance.authorization
    }

    /// Returns the trusted configured effect class.
    #[must_use]
    #[inline]
    pub const fn class(&self) -> ToolClass {
        self.provenance.class
    }

    /// Returns the stable mutation idempotency identity when present.
    #[must_use]
    #[inline]
    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.provenance.idempotency_key.as_ref()
    }

    /// Returns the trusted integration identity without configuration.
    #[must_use]
    #[inline]
    pub fn integration_id(&self) -> &IntegrationId {
        &self.provenance.integration_id
    }

    /// Returns the exact trusted configured tool name.
    #[must_use]
    #[inline]
    pub fn tool(&self) -> &ToolName {
        &self.provenance.tool
    }
}

/// One server notification kind that may be exposed only with a matching policy capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "new server notification kinds require an explicit policy and projection decision"
)]
pub enum ServerObservationKind {
    /// The server emitted bounded untrusted logging data.
    Logging,
    /// The server emitted bounded untrusted progress data.
    Progress,
    /// The server announced that its untrusted prompt catalog may have changed.
    PromptListChanged,
    /// The server announced that its untrusted resource catalog may have changed.
    ResourceListChanged,
    /// The server announced an update for one untrusted resource identifier.
    ResourceUpdated,
    /// The server announced that its untrusted tool catalog may have changed.
    ToolListChanged,
}

#[expect(
    clippy::implicit_return,
    reason = "the closed notification-kind mapping uses an idiomatic total tail match"
)]
impl ServerObservationKind {
    /// Returns the exact policy capability required to expose this notification kind.
    #[must_use]
    #[inline]
    const fn capability(self) -> ExternalToolCapability {
        match self {
            Self::Logging => ExternalToolCapability::ObserveLogging,
            Self::Progress => ExternalToolCapability::ObserveProgress,
            Self::ResourceListChanged | Self::ResourceUpdated => {
                ExternalToolCapability::ObserveResourceChanges
            }
            Self::ToolListChanged => ExternalToolCapability::ObserveToolListChanges,
            Self::PromptListChanged => ExternalToolCapability::ObservePromptChanges,
        }
    }
}

/// Opaque authorization to expose one exact bounded server notification kind.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizedServerObservation {
    /// Trusted integration selected by the complete policy decision.
    integration: McpIntegration,
    /// Exact notification kind permitted by the complete policy decision.
    kind: ServerObservationKind,
}

#[expect(
    clippy::implicit_return,
    reason = "opaque observation-token accessors use idiomatic tail expressions"
)]
impl AuthorizedServerObservation {
    /// Returns the trusted integration selected by the pure policy decision.
    #[must_use]
    #[inline]
    pub fn integration(&self) -> &McpIntegration {
        &self.integration
    }

    /// Returns the exact notification kind this token may expose.
    #[must_use]
    #[inline]
    pub const fn kind(&self) -> ServerObservationKind {
        self.kind
    }
}

/// Authorizes disclosure of the exact fixed Tiber-owned roots for one integration.
///
/// # Errors
///
/// Returns a stable context, integration, or capability denial before a
/// protocol adapter may advertise or answer the MCP roots callback.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the opaque roots authorization is the function's direct final decision"
)]
pub fn authorize_root_declaration(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
) -> Result<AuthorizedRootDeclaration, ExternalToolError> {
    match policy.permit_capability(context, integration, ExternalToolCapability::DeclareRoots) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    Ok(AuthorizedRootDeclaration {
        integration: integration.clone(),
        roots: integration.tiber_roots().to_vec(),
    })
}

/// Authorizes discovery of the untrusted resources offered by one integration.
///
/// # Errors
///
/// Returns a stable context, integration, or capability denial before an
/// adapter may contact the configured server.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the opaque resource-list authorization is the function's direct final decision"
)]
pub fn authorize_resource_listing(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
) -> Result<AuthorizedResourceListing, ExternalToolError> {
    match policy.permit_capability(context, integration, ExternalToolCapability::ReadResources) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    Ok(AuthorizedResourceListing {
        integration: integration.clone(),
    })
}

/// Authorizes reading exactly one resource selected by a bounded proposal.
///
/// # Errors
///
/// Returns a stable context, integration, or capability denial before an
/// adapter may contact the configured server.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the opaque resource-read authorization is the function's direct final decision"
)]
pub fn authorize_resource_read(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
    proposal: ResourceReadProposal,
) -> Result<AuthorizedResourceRead, ExternalToolError> {
    match policy.permit_capability(context, integration, ExternalToolCapability::ReadResources) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    Ok(AuthorizedResourceRead {
        integration: integration.clone(),
        uri: proposal.uri,
    })
}

/// Authorizes discovery of the untrusted prompts offered by one integration.
///
/// # Errors
///
/// Returns a stable context, integration, or capability denial before an
/// adapter may contact the configured server.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the opaque prompt-list authorization is the function's direct final decision"
)]
pub fn authorize_prompt_listing(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
) -> Result<AuthorizedPromptListing, ExternalToolError> {
    match policy.permit_capability(context, integration, ExternalToolCapability::ReadPrompts) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    Ok(AuthorizedPromptListing {
        integration: integration.clone(),
    })
}

/// Authorizes retrieval of exactly one prompt selected by a bounded proposal.
///
/// # Errors
///
/// Returns a stable context, integration, or capability denial before an
/// adapter may contact the configured server.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the opaque prompt retrieval authorization is the function's direct final decision"
)]
pub fn authorize_prompt_get(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
    proposal: PromptGetProposal,
) -> Result<AuthorizedPromptGet, ExternalToolError> {
    match policy.permit_capability(context, integration, ExternalToolCapability::ReadPrompts) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    Ok(AuthorizedPromptGet {
        arguments: proposal.arguments,
        integration: integration.clone(),
        name: proposal.name,
    })
}

/// Authorizes exposure of one exact bounded server notification kind.
///
/// # Errors
///
/// Returns a stable integration, context, or capability denial before a caller
/// may receive the requested server notification.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the opaque observation authorization is the function's direct final decision"
)]
pub fn authorize_server_observation(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
    kind: ServerObservationKind,
) -> Result<AuthorizedServerObservation, ExternalToolError> {
    match policy.permit_capability(context, integration, kind.capability()) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    Ok(AuthorizedServerObservation {
        integration: integration.clone(),
        kind,
    })
}

/// Authorizes bounded tool discovery through all six policy layers.
///
/// # Errors
///
/// Returns a stable context or capability denial before an adapter can contact
/// the configured server.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the authorization result is the function's direct final decision"
)]
pub fn authorize_tool_listing(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
) -> Result<AuthorizedToolListing, ExternalToolError> {
    match policy.permit_capability(context, integration, ExternalToolCapability::DiscoverTools) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    Ok(AuthorizedToolListing {
        integration: integration.clone(),
    })
}

/// Intersects policy and trusted configuration to authorize one exact tool call.
///
/// # Errors
///
/// Returns a stable denial before an adapter can contact the configured server.
/// Mutating calls additionally require an owner approval, idempotency key, and
/// configured read-only reconciliation tool. Their authorized wire arguments
/// contain a canonical `idempotencyKey` member matching that typed key; a
/// conflicting caller-supplied member is refused before adapter contact.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the authorization result is the function's direct final decision"
)]
pub fn authorize_tool_call(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
    proposal: ToolCallProposal,
    approval: Option<OwnerApprovalId>,
) -> Result<AuthorizedToolCall, ExternalToolError> {
    decide_tool_call(integration, policy, context, proposal, approval).into_result()
}

/// Intersects policy and trusted configuration while retaining a sanitized exact denial.
///
/// Unlike [`authorize_tool_call`], this decision preserves the trusted context,
/// requested identities, configured class, and stable refusal for audit consumers.
/// It never retains proposal arguments or integration transport configuration in
/// the denial transcript.
#[must_use]
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the complete decision is the function's direct final projection"
)]
pub fn decide_tool_call(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
    proposal: ToolCallProposal,
    approval: Option<OwnerApprovalId>,
) -> ToolCallAuthorizationDecision {
    let requested_tool = proposal.tool.clone();
    let class = integration.tool_class(&requested_tool);
    match authorize_tool_call_inner(integration, policy, context, proposal, approval) {
        Ok(call) => ToolCallAuthorizationDecision::Authorized(call),
        Err(error) => ToolCallAuthorizationDecision::Denied(ToolCallDenial {
            authorization: context.clone(),
            class,
            error,
            integration_id: integration.id().clone(),
            tool: requested_tool,
        }),
    }
}

/// Performs the original ordered authorization checks for both public projections.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "one shared implementation preserves exact error precedence across the Result and transcript APIs"
)]
fn authorize_tool_call_inner(
    integration: &McpIntegration,
    policy: &PolicyIntersection,
    context: &AuthorizationContext,
    proposal: ToolCallProposal,
    approval: Option<OwnerApprovalId>,
) -> Result<AuthorizedToolCall, ExternalToolError> {
    match policy.permit_integration(integration) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    let Some(class) = integration.tool_class(&proposal.tool) else {
        return Err(ExternalToolError::UnknownTool);
    };
    match policy.permit_tool(
        context,
        integration,
        &proposal.tool,
        ExternalToolCapability::InvokeTools,
    ) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    if class == ToolClass::Mutate {
        let Some(status_tool) = integration.reconciliation_tool() else {
            return Err(ExternalToolError::MutationReconciliationRequired);
        };
        match policy.permit_tool(
            context,
            integration,
            status_tool,
            ExternalToolCapability::ReconcileMutations,
        ) {
            Ok(()) => {}
            Err(error) => return Err(error),
        }
        if approval.is_none() {
            return Err(ExternalToolError::MutationApprovalRequired);
        }
        if proposal.idempotency_key.is_none() {
            return Err(ExternalToolError::MutationIdempotencyRequired);
        }
    }
    let idempotency_key = if class == ToolClass::Mutate {
        proposal.idempotency_key
    } else {
        None
    };
    let arguments = match idempotency_key.as_ref() {
        Some(key) => match proposal.arguments.with_idempotency_key(key) {
            Ok(arguments) => arguments,
            Err(error) => return Err(error),
        },
        None => proposal.arguments,
    };
    Ok(AuthorizedToolCall {
        approval,
        arguments,
        authorization: context.clone(),
        class,
        idempotency_key,
        integration: integration.clone(),
        tool: proposal.tool,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "test fixture parsing must fail loudly and uses an idiomatic tail expression"
    )]
    fn text<T>(parse: impl FnOnce(&str) -> Result<T, ExternalToolError>, value: &str) -> T {
        parse(value).expect("fixture semantic text is valid")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the test fixture wrapper uses an idiomatic tail expression"
    )]
    fn tool(value: &str) -> ToolName {
        text(ToolName::parse, value)
    }

    #[expect(
        clippy::implicit_return,
        reason = "the named fixture makes the complete six-layer grant readable and uses an idiomatic tail expression"
    )]
    fn all_permissions() -> PermissionGrant {
        PermissionGrant::new(
            [
                tool("read_status"),
                tool("apply_change"),
                tool("mutation_status"),
            ],
            [
                ExternalToolCapability::DiscoverTools,
                ExternalToolCapability::InvokeTools,
                ExternalToolCapability::ReconcileMutations,
            ],
        )
    }

    #[expect(
        clippy::implicit_return,
        reason = "the test context fixture uses an idiomatic tail expression"
    )]
    fn context() -> AuthorizationContext {
        AuthorizationContext::new(
            text(WorkflowMode::parse, "review"),
            text(AgentRole::parse, "reviewer"),
            text(SessionId::parse, "session-1"),
            text(AssignmentId::parse, "assignment-1"),
            text(PolicyDecisionId::parse, "policy-1"),
        )
    }

    #[expect(
        clippy::implicit_return,
        reason = "the test policy fixture uses an idiomatic tail expression"
    )]
    fn policy(integration: &McpIntegration, context: &AuthorizationContext) -> PolicyIntersection {
        let permissions = all_permissions();
        PolicyIntersection::new(
            integration,
            permissions.clone(),
            ScopedPermission::new(context.workflow_mode.clone(), permissions.clone()),
            ScopedPermission::new(context.agent_role.clone(), permissions.clone()),
            ScopedPermission::new(context.session.clone(), permissions.clone()),
            ScopedPermission::new(context.assignment.clone(), permissions.clone()),
            ScopedPermission::new(context.policy_decision.clone(), permissions),
        )
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "trusted fixture construction must fail loudly and uses an idiomatic tail expression"
    )]
    fn integration() -> McpIntegration {
        McpIntegration::new(
            text(IntegrationId::parse, "local-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/example").expect("absolute fixture path"),
                arguments: vec![LiteralArgument::parse("--mcp").expect("literal fixture argument")],
            },
            [
                ConfiguredTool::new(tool("read_status"), ToolClass::Observe),
                ConfiguredTool::new(tool("apply_change"), ToolClass::Mutate),
                ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
            ],
            Some(tool("mutation_status")),
        )
        .expect("trusted fixture integration is valid")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "trusted collision fixtures must fail loudly and use an idiomatic tail expression"
    )]
    fn integration_variant(
        transport: McpTransport,
        tools: Vec<ConfiguredTool>,
        reconciliation_tool: Option<ToolName>,
        root: &str,
    ) -> McpIntegration {
        McpIntegration::new(
            text(IntegrationId::parse, "local-tools"),
            transport,
            tools,
            reconciliation_tool,
        )
        .expect("same-ID integration variant is valid")
        .with_tiber_roots([TiberOwnedRoot::from_absolute_path(root)
            .expect("same-ID absolute root fixture is valid")])
        .expect("same-ID integration root catalog is valid")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the collision fixture catalog uses an idiomatic tail expression"
    )]
    fn configured_tools() -> Vec<ConfiguredTool> {
        vec![
            ConfiguredTool::new(tool("read_status"), ToolClass::Observe),
            ConfiguredTool::new(tool("apply_change"), ToolClass::Mutate),
            ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
        ]
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "isolated collision fixtures must fail loudly and use an idiomatic tail expression"
    )]
    fn isolated_integration_variants(
        configured: &McpIntegration,
    ) -> Vec<(&'static str, McpIntegration)> {
        vec![
            (
                "transport kind",
                integration_variant(
                    McpTransport::StreamableHttp {
                        endpoint: LoopbackEndpoint::parse("http://127.0.0.1:8123/mcp")
                            .expect("substituted loopback endpoint is valid"),
                    },
                    configured_tools(),
                    Some(tool("mutation_status")),
                    "/workspace/configured",
                ),
            ),
            (
                "stdio program",
                integration_variant(
                    McpTransport::Stdio {
                        program: AbsoluteProgram::parse("/usr/bin/other-example")
                            .expect("substituted absolute program is valid"),
                        arguments: vec![
                            LiteralArgument::parse("--mcp")
                                .expect("configured literal argument is valid"),
                        ],
                    },
                    configured_tools(),
                    Some(tool("mutation_status")),
                    "/workspace/configured",
                ),
            ),
            (
                "stdio arguments",
                integration_variant(
                    McpTransport::Stdio {
                        program: AbsoluteProgram::parse("/usr/bin/example")
                            .expect("configured absolute program is valid"),
                        arguments: vec![
                            LiteralArgument::parse("--other-mode")
                                .expect("substituted literal argument is valid"),
                        ],
                    },
                    configured_tools(),
                    Some(tool("mutation_status")),
                    "/workspace/configured",
                ),
            ),
            (
                "tool name",
                integration_variant(
                    configured.transport().clone(),
                    vec![
                        ConfiguredTool::new(tool("other_status"), ToolClass::Observe),
                        ConfiguredTool::new(tool("apply_change"), ToolClass::Mutate),
                        ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
                    ],
                    Some(tool("mutation_status")),
                    "/workspace/configured",
                ),
            ),
            (
                "tool class",
                integration_variant(
                    configured.transport().clone(),
                    vec![
                        ConfiguredTool::new(tool("read_status"), ToolClass::Mutate),
                        ConfiguredTool::new(tool("apply_change"), ToolClass::Mutate),
                        ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
                    ],
                    Some(tool("mutation_status")),
                    "/workspace/configured",
                ),
            ),
            (
                "reconciliation tool",
                integration_variant(
                    configured.transport().clone(),
                    configured_tools(),
                    None,
                    "/workspace/configured",
                ),
            ),
            (
                "Tiber-owned roots",
                integration_variant(
                    configured.transport().clone(),
                    configured_tools(),
                    Some(tool("mutation_status")),
                    "/workspace/substituted",
                ),
            ),
        ]
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "JSON fixture parsing must fail loudly and uses an idiomatic tail expression"
    )]
    fn proposal(name: &str, idempotency_key: Option<IdempotencyKey>) -> ToolCallProposal {
        ToolCallProposal::new(
            tool(name),
            ToolArguments::parse(r#"{"fixture":true}"#).expect("JSON fixture is valid"),
            idempotency_key,
        )
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        reason = "the positive authorization fixture must fail loudly when its required opaque reconciliation outcome is absent"
    )]
    fn policy_intersection_authorizes_only_a_trusted_mutating_tool_with_all_fences() {
        let context = context();
        let integration = integration();
        let approved = authorize_tool_call(
            &integration,
            &policy(&integration, &context),
            &context,
            proposal(
                "apply_change",
                Some(text(IdempotencyKey::parse, "invocation-1")),
            ),
            Some(text(OwnerApprovalId::parse, "owner-approval-1")),
        )
        .expect("all six policy layers and mutation fences authorize the call");

        let serialized = serde_json::to_value(&approved).expect("authorization serializes");
        assert_eq!(
            serialized["authorization"],
            serde_json::json!({
                "agent_role": "reviewer",
                "assignment": "assignment-1",
                "policy_decision": "policy-1",
                "session": "session-1",
                "workflow_mode": "review"
            })
        );

        assert_eq!(approved.class(), ToolClass::Mutate);
        assert_eq!(approved.authorization(), &context);
        assert_eq!(approved.tool().as_str(), "apply_change");
        assert_eq!(
            approved.arguments().as_json(),
            r#"{"fixture":true,"idempotencyKey":"invocation-1"}"#
        );
        let preview_reconciliation = approved
            .reconciliation()
            .expect("mutations retain a reconciliation request");
        assert_eq!(preview_reconciliation.authorization(), &context);
        assert_eq!(
            preview_reconciliation.originating_tool().as_str(),
            "apply_change"
        );
        assert_eq!(
            preview_reconciliation
                .owner_approval()
                .map(OwnerApprovalId::as_str),
            Some("owner-approval-1")
        );
        assert_eq!(
            preview_reconciliation.status_tool().as_str(),
            "mutation_status"
        );
        let Some(ToolCallOutcome::OutcomeUnknown(reconciliation)) = approved.into_outcome_unknown()
        else {
            panic!("a mutating call with every fence must produce a reconciliation outcome");
        };
        assert_eq!(
            reconciliation.arguments().as_json(),
            r#"{"idempotencyKey":"invocation-1"}"#
        );
        assert_eq!(reconciliation.authorization(), &context);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "trusted provenance-binding fixtures must fail loudly"
    )]
    fn outcome_binding_refuses_reconciliation_from_another_authority_context() {
        let integration = integration();
        let first_context = context();
        let other_context = AuthorizationContext::new(
            text(WorkflowMode::parse, "review"),
            text(AgentRole::parse, "reviewer"),
            text(SessionId::parse, "session-other"),
            text(AssignmentId::parse, "assignment-1"),
            text(PolicyDecisionId::parse, "policy-1"),
        );
        let first_call = authorize_tool_call(
            &integration,
            &policy(&integration, &first_context),
            &first_context,
            proposal(
                "apply_change",
                Some(text(IdempotencyKey::parse, "invocation-1")),
            ),
            Some(text(OwnerApprovalId::parse, "approval-first")),
        )
        .expect("first authority permits its mutation");
        let other_outcome = authorize_tool_call(
            &integration,
            &policy(&integration, &other_context),
            &other_context,
            proposal(
                "apply_change",
                Some(text(IdempotencyKey::parse, "invocation-1")),
            ),
            Some(text(OwnerApprovalId::parse, "approval-other")),
        )
        .expect("other authority permits its own mutation")
        .into_outcome_unknown()
        .expect("mutation produces reconciliation authority");

        assert!(matches!(
            first_call.bind_outcome(other_outcome),
            Err(ToolCallOutcomeBindingError::ReconciliationMismatch)
        ));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the cohesive identity-dimension table uses trusted fixtures and a concise authorization closure"
    )]
    fn outcome_binding_refuses_each_mismatched_reconciliation_identity_dimension() {
        let integration = McpIntegration::new(
            text(IntegrationId::parse, "multi-mutation-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/example")
                    .expect("fixture program is absolute"),
                arguments: vec![],
            },
            [
                ConfiguredTool::new(tool("apply_change"), ToolClass::Mutate),
                ConfiguredTool::new(tool("apply_other"), ToolClass::Mutate),
                ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
            ],
            Some(tool("mutation_status")),
        )
        .expect("multi-mutation integration is valid");
        let current = context();
        let grant = PermissionGrant::new(
            [
                tool("apply_change"),
                tool("apply_other"),
                tool("mutation_status"),
            ],
            [
                ExternalToolCapability::InvokeTools,
                ExternalToolCapability::ReconcileMutations,
            ],
        );
        let policy = PolicyIntersection::new(
            &integration,
            grant.clone(),
            ScopedPermission::new(current.workflow_mode.clone(), grant.clone()),
            ScopedPermission::new(current.agent_role.clone(), grant.clone()),
            ScopedPermission::new(current.session.clone(), grant.clone()),
            ScopedPermission::new(current.assignment.clone(), grant.clone()),
            ScopedPermission::new(current.policy_decision.clone(), grant),
        );
        let authorized = |name: &str, key: &str, approval: &str| {
            authorize_tool_call(
                &integration,
                &policy,
                &current,
                proposal(name, Some(text(IdempotencyKey::parse, key))),
                Some(text(OwnerApprovalId::parse, approval)),
            )
            .expect("identity-dimension fixture is authorized")
        };

        for (dimension, other_tool, other_key, other_approval) in [
            (
                "owner approval",
                "apply_change",
                "invocation-exact",
                "approval-other",
            ),
            (
                "idempotency key",
                "apply_change",
                "invocation-other",
                "approval-exact",
            ),
            (
                "originating mutation tool",
                "apply_other",
                "invocation-exact",
                "approval-exact",
            ),
        ] {
            let exact_call = authorized("apply_change", "invocation-exact", "approval-exact");
            let mismatched_outcome = authorized(other_tool, other_key, other_approval)
                .into_outcome_unknown()
                .expect("mismatched mutation still has its own reconciliation token");

            assert!(
                matches!(
                    exact_call.bind_outcome(mismatched_outcome),
                    Err(ToolCallOutcomeBindingError::ReconciliationMismatch)
                ),
                "binding must refuse a mismatched {dimension}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "trusted bound-transcript fixtures must fail loudly"
    )]
    fn outcome_binding_exposes_only_safe_call_provenance_and_the_exact_outcome() {
        let integration = integration();
        let current = context();
        let call = authorize_tool_call(
            &integration,
            &policy(&integration, &current),
            &current,
            proposal(
                "apply_change",
                Some(text(IdempotencyKey::parse, "invocation-bound")),
            ),
            Some(text(OwnerApprovalId::parse, "approval-bound")),
        )
        .expect("authority permits the bound mutation");
        let outcome = ToolCallOutcome::OutcomeUnknown(
            call.reconciliation()
                .expect("mutation carries exact reconciliation identity"),
        );

        let bound = call
            .bind_outcome(outcome)
            .expect("the call accepts its own exact reconciliation identity");

        assert_eq!(bound.authorization(), &current);
        assert_eq!(bound.integration_id(), integration.id());
        assert_eq!(bound.tool().as_str(), "apply_change");
        assert_eq!(bound.class(), ToolClass::Mutate);
        assert_eq!(
            bound.approval().map(OwnerApprovalId::as_str),
            Some("approval-bound")
        );
        assert_eq!(
            bound.idempotency_key().map(IdempotencyKey::as_str),
            Some("invocation-bound")
        );
        assert!(matches!(
            bound.outcome(),
            ToolCallOutcome::OutcomeUnknown(reconciliation)
                if reconciliation.authorization() == &current
                    && reconciliation.originating_tool().as_str() == "apply_change"
                    && reconciliation.owner_approval().map(OwnerApprovalId::as_str)
                        == Some("approval-bound")
        ));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the observed-payload substitution fixture uses trusted bounded inputs"
    )]
    fn generic_outcome_binding_refuses_an_unattributed_observation() {
        let integration = integration();
        let current = context();
        let call = authorize_tool_call(
            &integration,
            &policy(&integration, &current),
            &current,
            proposal("read_status", None),
            None,
        )
        .expect("read-only call is authorized");
        let unattributed = ToolCallOutcome::Observed(
            UntrustedPayload::bounded("payload-from-another-call")
                .expect("observed payload is bounded"),
        );

        assert!(
            call.bind_outcome(unattributed).is_err(),
            "a generic outcome cannot prove which call observed its payload"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "trusted success and failure provenance fixtures must fail loudly"
    )]
    fn consumed_authorizations_bind_observation_and_failure_without_replay_authority() {
        let integration = integration();
        let current = context();
        let observed_call = authorize_tool_call(
            &integration,
            &policy(&integration, &current),
            &current,
            proposal("read_status", None),
            None,
        )
        .expect("read-only call is authorized");
        let bound = observed_call.bind_observation(
            UntrustedPayload::bounded("payload-observed-by-this-call")
                .expect("observed payload is bounded"),
        );

        assert_eq!(bound.authorization(), &current);
        assert_eq!(bound.integration_id(), integration.id());
        assert_eq!(bound.tool().as_str(), "read_status");
        assert!(matches!(
            bound.outcome(),
            ToolCallOutcome::Observed(payload)
                if payload.as_str() == "payload-observed-by-this-call"
        ));

        let failed_call = authorize_tool_call(
            &integration,
            &policy(&integration, &current),
            &current,
            proposal(
                "apply_change",
                Some(text(IdempotencyKey::parse, "invocation-failed")),
            ),
            Some(text(OwnerApprovalId::parse, "approval-failed")),
        )
        .expect("mutation call is authorized");
        let failure = failed_call.bind_failure();

        assert_eq!(failure.authorization(), &current);
        assert_eq!(failure.integration_id(), integration.id());
        assert_eq!(failure.tool().as_str(), "apply_change");
        assert_eq!(failure.class(), ToolClass::Mutate);
        assert_eq!(
            failure.approval().map(OwnerApprovalId::as_str),
            Some("approval-failed")
        );
        assert_eq!(
            failure.idempotency_key().map(IdempotencyKey::as_str),
            Some("invocation-failed")
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "the denial transcript fixture must fail loudly and remain exhaustive"
    )]
    fn complete_decision_retains_exact_safe_denial_and_result_error_precedence() {
        let integration = integration();
        let current = context();
        let secret_arguments =
            ToolArguments::parse(r#"{"secret_argument":"must-not-enter-denial-transcript"}"#)
                .expect("denial fixture arguments are valid JSON");
        let decision = decide_tool_call(
            &integration,
            &policy(&integration, &current),
            &current,
            ToolCallProposal::new(tool("not_configured"), secret_arguments, None),
            None,
        );
        let ToolCallAuthorizationDecision::Denied(denial) = decision else {
            panic!("unknown tool must produce a denial transcript");
        };

        assert_eq!(denial.authorization(), &current);
        assert_eq!(denial.integration_id(), integration.id());
        assert_eq!(denial.tool().as_str(), "not_configured");
        assert_eq!(denial.class(), None);
        assert_eq!(denial.error(), ExternalToolError::UnknownTool);
        assert_eq!(denial.code(), "external_tools_unknown_tool");
        let serialized = serde_json::to_string(&denial).expect("safe denial serializes");
        assert!(!serialized.contains("must-not-enter-denial-transcript"));
        assert!(!serialized.contains("/usr/bin/example"));

        assert_eq!(
            authorize_tool_call(
                &integration,
                &policy(&integration, &current),
                &current,
                proposal("not_configured", None),
                None,
            ),
            Err(ExternalToolError::UnknownTool)
        );
    }

    #[test]
    fn mutation_authorization_requires_the_configured_reconciliation_tool_in_every_layer() {
        let context = context();
        let permitted = all_permissions();
        let reconciliation_tool_denied = PermissionGrant::new(
            [tool("read_status"), tool("apply_change")],
            [
                ExternalToolCapability::DiscoverTools,
                ExternalToolCapability::InvokeTools,
                ExternalToolCapability::ReconcileMutations,
            ],
        );
        let policy = PolicyIntersection::new(
            &integration(),
            permitted.clone(),
            ScopedPermission::new(context.workflow_mode.clone(), permitted.clone()),
            ScopedPermission::new(context.agent_role.clone(), permitted.clone()),
            ScopedPermission::new(context.session.clone(), permitted.clone()),
            ScopedPermission::new(context.assignment.clone(), reconciliation_tool_denied),
            ScopedPermission::new(context.policy_decision.clone(), permitted),
        );

        assert_eq!(
            authorize_tool_call(
                &integration(),
                &policy,
                &context,
                proposal(
                    "apply_change",
                    Some(text(IdempotencyKey::parse, "invocation-1")),
                ),
                Some(text(OwnerApprovalId::parse, "owner-approval-1")),
            ),
            Err(ExternalToolError::ToolDenied)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the conflicting idempotency fixture must fail loudly"
    )]
    fn mutating_authorization_rejects_a_conflicting_wire_idempotency_key() {
        let context = context();
        let integration = integration();
        let conflicting_proposal = ToolCallProposal::new(
            tool("apply_change"),
            ToolArguments::parse(r#"{"idempotencyKey":"other-invocation"}"#)
                .expect("JSON fixture is valid"),
            Some(text(IdempotencyKey::parse, "invocation-1")),
        );

        assert!(matches!(
            authorize_tool_call(
                &integration,
                &policy(&integration, &context),
                &context,
                conflicting_proposal,
                Some(text(OwnerApprovalId::parse, "owner-approval-1")),
            ),
            Err(error) if error.code() == "external_tools_mutation_idempotency_conflict"
        ));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the read-only authorization fixture must fail loudly"
    )]
    fn observe_calls_never_mint_reconciliation_even_with_a_caller_supplied_key() {
        let context = context();
        let integration = integration();
        let observed = authorize_tool_call(
            &integration,
            &policy(&integration, &context),
            &context,
            proposal(
                "read_status",
                Some(text(IdempotencyKey::parse, "caller-supplied-key")),
            ),
            None,
        )
        .expect("a configured read-only tool is authorized without owner approval");

        assert_eq!(observed.idempotency_key(), None);
        assert_eq!(observed.reconciliation(), None);
        assert_eq!(observed.into_outcome_unknown(), None);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the read-only reserved-key fixture must fail loudly"
    )]
    fn observe_authorization_preserves_a_caller_supplied_schema_field_verbatim() {
        let context = context();
        let integration = integration();
        let observed = authorize_tool_call(
            &integration,
            &policy(&integration, &context),
            &context,
            ToolCallProposal::new(
                tool("read_status"),
                ToolArguments::parse(r#"{"fixture":true,"idempotencyKey":"caller-key"}"#)
                    .expect("JSON fixture is valid"),
                Some(text(IdempotencyKey::parse, "caller-key")),
            ),
            None,
        )
        .expect("a configured read-only tool is authorized without owner approval");

        assert_eq!(
            observed.arguments().as_json(),
            r#"{"fixture":true,"idempotencyKey":"caller-key"}"#
        );
    }

    #[test]
    fn policy_denies_unknown_unapproved_or_unbound_tool_calls_before_transport() {
        let context = context();
        let integration = integration();
        let policy = policy(&integration, &context);

        assert_eq!(
            authorize_tool_call(
                &integration,
                &policy,
                &context,
                proposal("not_configured", None),
                None,
            ),
            Err(ExternalToolError::UnknownTool)
        );
        assert_eq!(
            authorize_tool_call(
                &integration,
                &policy,
                &context,
                proposal("apply_change", None),
                None,
            ),
            Err(ExternalToolError::MutationApprovalRequired)
        );
        let mismatched_context = AuthorizationContext::new(
            text(WorkflowMode::parse, "review"),
            text(AgentRole::parse, "reviewer"),
            text(SessionId::parse, "session-2"),
            text(AssignmentId::parse, "assignment-1"),
            text(PolicyDecisionId::parse, "policy-1"),
        );
        assert_eq!(
            authorize_tool_listing(&integration, &policy, &mismatched_context),
            Err(ExternalToolError::PolicyContextMismatch)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the alternate trusted integration fixture must fail loudly"
    )]
    fn policy_intersection_cannot_authorize_a_different_integration_with_the_same_tool_name() {
        let context = context();
        let other_integration = McpIntegration::new(
            text(IntegrationId::parse, "other-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/other-example")
                    .expect("absolute fixture path"),
                arguments: vec![LiteralArgument::parse("--mcp").expect("literal fixture argument")],
            },
            [ConfiguredTool::new(tool("read_status"), ToolClass::Observe)],
            None,
        )
        .expect("alternate trusted fixture integration is valid");

        assert_eq!(
            authorize_tool_listing(
                &other_integration,
                &policy(&integration(), &context),
                &context,
            ),
            Err(ExternalToolError::PolicyIntegrationMismatch)
        );
        assert_eq!(
            authorize_tool_call(
                &other_integration,
                &policy(&integration(), &context),
                &context,
                proposal("read_status", None),
                None,
            ),
            Err(ExternalToolError::PolicyIntegrationMismatch)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "same-ID trusted integration fixtures must fail loudly"
    )]
    fn policy_intersection_rejects_a_different_configuration_with_the_same_textual_id() {
        let context = context();
        let configured = integration()
            .with_tiber_roots([TiberOwnedRoot::from_absolute_path("/workspace/configured")
                .expect("configured absolute root fixture is valid")])
            .expect("configured integration root catalog is valid");
        let configured_policy = policy(&configured, &context);

        for (dimension, substituted) in isolated_integration_variants(&configured) {
            assert_eq!(
                authorize_tool_listing(&substituted, &configured_policy, &context),
                Err(ExternalToolError::PolicyIntegrationMismatch),
                "policy must reject a same-ID difference in {dimension}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "same-ID tool and root substitution fixtures must fail loudly"
    )]
    fn same_id_substitution_precedes_tool_lookup_and_root_authority() {
        let context = context();
        let configured = integration()
            .with_tiber_roots([TiberOwnedRoot::from_absolute_path("/workspace/configured")
                .expect("configured absolute root fixture is valid")])
            .expect("configured integration root catalog is valid");
        let variants = isolated_integration_variants(&configured);
        let tool_name_variant = variants.get(3).expect("isolated tool-name variant exists");
        assert_eq!(tool_name_variant.0, "tool name");
        let tool_name = &tool_name_variant.1;
        let root_variant = variants.get(6).expect("isolated root variant exists");
        assert_eq!(root_variant.0, "Tiber-owned roots");
        let roots = &root_variant.1;
        let permissions = PermissionGrant::new(
            [tool("read_status")],
            [
                ExternalToolCapability::InvokeTools,
                ExternalToolCapability::DeclareRoots,
            ],
        );
        let configured_policy = PolicyIntersection::new(
            &configured,
            permissions.clone(),
            ScopedPermission::new(context.workflow_mode.clone(), permissions.clone()),
            ScopedPermission::new(context.agent_role.clone(), permissions.clone()),
            ScopedPermission::new(context.session.clone(), permissions.clone()),
            ScopedPermission::new(context.assignment.clone(), permissions.clone()),
            ScopedPermission::new(context.policy_decision.clone(), permissions),
        );

        assert_eq!(
            authorize_tool_call(
                tool_name,
                &configured_policy,
                &context,
                proposal("read_status", None),
                None,
            ),
            Err(ExternalToolError::PolicyIntegrationMismatch)
        );
        assert_eq!(
            authorize_root_declaration(roots, &configured_policy, &context),
            Err(ExternalToolError::PolicyIntegrationMismatch)
        );
    }

    #[test]
    fn tool_arguments_reject_non_object_json_before_an_adapter_connection() {
        assert_eq!(
            ToolArguments::parse("[]"),
            Err(ExternalToolError::InvalidToolArguments)
        );
        assert_eq!(
            ToolArguments::parse("null"),
            Err(ExternalToolError::InvalidToolArguments)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the capability-gated observation fixture must fail loudly"
    )]
    fn server_observations_require_a_matching_opaque_capability_token() {
        let context = context();
        let integration = integration();
        let permissions = PermissionGrant::new(
            [tool("read_status")],
            [ExternalToolCapability::ObserveProgress],
        );
        let progress_policy = PolicyIntersection::new(
            &integration,
            permissions.clone(),
            ScopedPermission::new(context.workflow_mode.clone(), permissions.clone()),
            ScopedPermission::new(context.agent_role.clone(), permissions.clone()),
            ScopedPermission::new(context.session.clone(), permissions.clone()),
            ScopedPermission::new(context.assignment.clone(), permissions.clone()),
            ScopedPermission::new(context.policy_decision.clone(), permissions),
        );

        let progress = authorize_server_observation(
            &integration,
            &progress_policy,
            &context,
            ServerObservationKind::Progress,
        )
        .expect("all six layers explicitly permit progress observation");
        assert_eq!(progress.integration(), &integration);
        assert_eq!(progress.kind(), ServerObservationKind::Progress);
        assert_eq!(
            authorize_server_observation(
                &integration,
                &progress_policy,
                &context,
                ServerObservationKind::Logging,
            ),
            Err(ExternalToolError::CapabilityDenied)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the valid loopback fixture must fail loudly"
    )]
    fn configured_transport_rejects_non_absolute_and_non_loopback_endpoints() {
        assert_eq!(
            AbsoluteProgram::parse("relative-program"),
            Err(ExternalToolError::ProgramNotAbsolute)
        );
        assert_eq!(
            LoopbackEndpoint::parse("https://localhost/mcp"),
            Err(ExternalToolError::UnsupportedEndpointScheme)
        );
        assert_eq!(
            LoopbackEndpoint::parse("http://example.test/mcp"),
            Err(ExternalToolError::EndpointNotLoopback)
        );
        let endpoint = LoopbackEndpoint::parse("http://127.0.0.1:3000/mcp")
            .expect("loopback fixture is valid");
        assert_eq!(endpoint.as_str(), "http://127.0.0.1:3000/mcp");
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the untrusted-payload fixture must fail loudly"
    )]
    fn untrusted_payloads_cannot_classify_or_authorize_a_tool() {
        let context = context();
        let integration = integration();
        let hostile_description = UntrustedPayload::bounded("apply_change").expect("small payload");
        assert_eq!(hostile_description.as_str(), "apply_change");
        assert_eq!(
            authorize_tool_call(
                &integration,
                &policy(&integration, &context),
                &context,
                proposal("not_configured", None),
                None,
            ),
            Err(ExternalToolError::UnknownTool)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "trusted root and policy fixtures must fail loudly"
    )]
    fn roots_are_trusted_bounded_configuration_and_require_declare_roots() {
        let configured = integration()
            .with_tiber_roots([TiberOwnedRoot::from_absolute_path("/workspace")
                .expect("absolute root fixture is valid")])
            .expect("one absolute Tiber root is valid");
        let context = context();
        let permissions = PermissionGrant::new([], [ExternalToolCapability::DeclareRoots]);
        let root_policy = PolicyIntersection::new(
            &configured,
            permissions.clone(),
            ScopedPermission::new(context.workflow_mode.clone(), permissions.clone()),
            ScopedPermission::new(context.agent_role.clone(), permissions.clone()),
            ScopedPermission::new(context.session.clone(), permissions.clone()),
            ScopedPermission::new(context.assignment.clone(), permissions.clone()),
            ScopedPermission::new(context.policy_decision.clone(), permissions),
        );

        let authorization = authorize_root_declaration(&configured, &root_policy, &context)
            .expect("all six layers explicitly permit Tiber root declaration");
        assert_eq!(authorization.integration(), &configured);
        assert_eq!(authorization.roots(), configured.tiber_roots());
        assert_eq!(
            authorization.roots().first().map(TiberOwnedRoot::as_uri),
            Some("file:///workspace")
        );
        let ordinary_listing =
            authorize_tool_listing(&configured, &policy(&configured, &context), &context)
                .expect("ordinary tool listing is permitted without root disclosure authority");
        let ordinary_token_json = serde_json::to_string(&ordinary_listing)
            .expect("ordinary authorization token serializes");
        let ordinary_integration_json = serde_json::to_string(ordinary_listing.integration())
            .expect("ordinary token integration serializes");
        let ordinary_token_debug = format!("{ordinary_listing:?}");
        let root_declaration_json = serde_json::to_string(&authorization)
            .expect("root declaration authorization serializes");
        let root_policy_json = serde_json::to_string(&root_policy).expect("root policy serializes");
        let root_policy_debug = format!("{root_policy:?}");
        assert!(!ordinary_token_json.contains("file:///workspace"));
        assert!(!ordinary_integration_json.contains("file:///workspace"));
        assert!(!ordinary_token_debug.contains("file:///workspace"));
        assert!(!root_policy_json.contains("file:///workspace"));
        assert!(!root_policy_debug.contains("file:///workspace"));
        assert!(root_declaration_json.contains("file:///workspace"));
        assert_eq!(
            configured.clone().with_tiber_roots([
                TiberOwnedRoot::from_absolute_path("/workspace")
                    .expect("absolute root fixture is valid"),
                TiberOwnedRoot::from_absolute_path("/workspace")
                    .expect("absolute root fixture is valid"),
            ]),
            Err(ExternalToolError::DuplicateTiberRoot)
        );
        let mut too_many_roots = Vec::new();
        for index in 0..=MAX_TIBER_OWNED_ROOTS {
            too_many_roots.push(
                TiberOwnedRoot::from_absolute_path(format!("/workspace/{index}"))
                    .expect("bounded absolute root fixture is valid"),
            );
        }
        assert_eq!(
            integration().with_tiber_roots(too_many_roots),
            Err(ExternalToolError::InvalidTiberRootCatalog)
        );
        assert_eq!(
            authorize_root_declaration(&configured, &policy(&configured, &context), &context),
            Err(ExternalToolError::CapabilityDenied)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "optional MCP request fixtures must fail loudly"
    )]
    fn optional_resource_and_prompt_authorizations_bind_exact_untrusted_requests() {
        let context = context();
        let integration = integration();
        let permissions = PermissionGrant::new(
            [],
            [
                ExternalToolCapability::ReadResources,
                ExternalToolCapability::ReadPrompts,
            ],
        );
        let optional_policy = PolicyIntersection::new(
            &integration,
            permissions.clone(),
            ScopedPermission::new(context.workflow_mode.clone(), permissions.clone()),
            ScopedPermission::new(context.agent_role.clone(), permissions.clone()),
            ScopedPermission::new(context.session.clone(), permissions.clone()),
            ScopedPermission::new(context.assignment.clone(), permissions.clone()),
            ScopedPermission::new(context.policy_decision.clone(), permissions),
        );
        let resource = ResourceReadProposal::new(
            ResourceUri::parse("tiber://fixture/resource").expect("absolute resource URI"),
        );
        let prompt = PromptGetProposal::new(
            text(PromptName::parse, "review-diff"),
            Some(PromptArguments::parse(r#"{"path":"src/lib.rs"}"#).expect("object arguments")),
        );

        let listing = authorize_resource_listing(&integration, &optional_policy, &context)
            .expect("resource list is explicitly permitted");
        let read = authorize_resource_read(&integration, &optional_policy, &context, resource)
            .expect("resource read is explicitly permitted");
        let prompt_listing = authorize_prompt_listing(&integration, &optional_policy, &context)
            .expect("prompt list is explicitly permitted");
        let get = authorize_prompt_get(&integration, &optional_policy, &context, prompt)
            .expect("prompt get is explicitly permitted");

        assert_eq!(listing.integration(), &integration);
        assert_eq!(read.uri().as_str(), "tiber://fixture/resource");
        assert_eq!(prompt_listing.integration(), &integration);
        assert_eq!(get.name().as_str(), "review-diff");
        assert_eq!(
            get.arguments().expect("arguments remain bound").as_json(),
            r#"{"path":"src/lib.rs"}"#
        );
        assert_eq!(
            authorize_resource_listing(&integration, &policy(&integration, &context), &context),
            Err(ExternalToolError::CapabilityDenied)
        );
        assert_eq!(
            authorize_prompt_listing(&integration, &policy(&integration, &context), &context),
            Err(ExternalToolError::CapabilityDenied)
        );
        let other_integration = McpIntegration::new(
            text(IntegrationId::parse, "other-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/other-example")
                    .expect("absolute fixture path"),
                arguments: vec![LiteralArgument::parse("--mcp").expect("literal fixture argument")],
            },
            [ConfiguredTool::new(tool("read_status"), ToolClass::Observe)],
            None,
        )
        .expect("alternate trusted fixture integration is valid");
        assert_eq!(
            authorize_resource_listing(&other_integration, &optional_policy, &context),
            Err(ExternalToolError::PolicyIntegrationMismatch)
        );
        assert_eq!(
            authorize_prompt_listing(&other_integration, &optional_policy, &context),
            Err(ExternalToolError::PolicyIntegrationMismatch)
        );
    }

    #[test]
    fn optional_request_values_reject_unbounded_or_nonsemantic_input_before_authorization() {
        assert_eq!(
            TiberOwnedRoot::from_absolute_path("relative-root"),
            Err(ExternalToolError::RootNotAbsolute)
        );
        assert_eq!(
            ResourceUri::parse("relative-resource"),
            Err(ExternalToolError::InvalidResourceUri)
        );
        assert_eq!(
            PromptArguments::parse("[]"),
            Err(ExternalToolError::InvalidPromptArguments)
        );
        assert_eq!(
            PromptArguments::parse("not-json"),
            Err(ExternalToolError::InvalidPromptArguments)
        );
        assert_eq!(
            PromptArguments::parse(&"x".repeat(MAX_TOOL_ARGUMENT_BYTES + 1)),
            Err(ExternalToolError::InvalidPromptArguments)
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "capability-specific notification fixtures must fail loudly"
    )]
    fn resource_and_prompt_change_observations_are_independently_capability_gated() {
        let context = context();
        let integration = integration();
        let permissions =
            PermissionGrant::new([], [ExternalToolCapability::ObserveResourceChanges]);
        let resource_policy = PolicyIntersection::new(
            &integration,
            permissions.clone(),
            ScopedPermission::new(context.workflow_mode.clone(), permissions.clone()),
            ScopedPermission::new(context.agent_role.clone(), permissions.clone()),
            ScopedPermission::new(context.session.clone(), permissions.clone()),
            ScopedPermission::new(context.assignment.clone(), permissions.clone()),
            ScopedPermission::new(context.policy_decision.clone(), permissions),
        );

        let updated = authorize_server_observation(
            &integration,
            &resource_policy,
            &context,
            ServerObservationKind::ResourceUpdated,
        )
        .expect("resource updates need only the matching observation grant");
        assert_eq!(updated.kind(), ServerObservationKind::ResourceUpdated);
        assert_eq!(
            authorize_server_observation(
                &integration,
                &resource_policy,
                &context,
                ServerObservationKind::PromptListChanged,
            ),
            Err(ExternalToolError::CapabilityDenied)
        );
    }
}
