//! Pure authority boundary for configured process execution.
//!
//! Requests carry only semantic intent and trusted workflow provenance. A
//! trusted catalog resolves that intent without minting adapter authority.
//! Durable service history is required before an adapter can receive a plan.

extern crate alloc;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};
use core::{error::Error, fmt, time::Duration};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize, de::Error as _};
use tiber_workflow_core::{AssignmentId, EffectId, WorkflowId};

/// Maximum UTF-8 byte length of a configured command identity.
pub const MAX_COMMAND_ID_BYTES: usize = 128;
/// Maximum UTF-8 byte length of one app-server invocation correlation.
pub const MAX_INVOCATION_ID_BYTES: usize = 320;
/// Maximum number of literal arguments in one configured command.
pub const MAX_ARGUMENTS: usize = 64;
/// Maximum number of entries in the trusted configured-command catalog.
pub const MAX_CONFIGURED_COMMANDS: usize = 128;
/// Maximum UTF-8 byte length of one literal argument.
pub const MAX_ARGUMENT_BYTES: usize = 4_096;
/// Maximum UTF-8 byte length of one trusted absolute executable path.
pub const MAX_PROGRAM_PATH_BYTES: usize = 4_096;
/// Maximum number of exact environment entries in one configured command.
pub const MAX_ENVIRONMENT_ENTRIES: usize = 64;
/// Maximum UTF-8 byte length of one fixed environment value.
pub const MAX_ENVIRONMENT_VALUE_BYTES: usize = 4_096;
/// Maximum allowed process timeout.
pub const MAX_TIMEOUT: Duration = Duration::from_hours(1);
/// Maximum retained bytes for either output stream.
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Stable configured-process construction and policy refusals.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "stable policy refusals remain grouped by configuration lifecycle while callers exhaustively handle the closed set"
)]
pub enum ProcessPolicyError {
    /// A semantic value was empty, oversized, or malformed.
    InvalidSemanticValue,
    /// A configured executable was not an absolute path.
    ProgramNotAbsolute,
    /// A configured executable path was malformed or exceeded its fixed bound.
    InvalidProgramPath,
    /// A literal argv vector or item crossed its fixed bound.
    InvalidLiteralArguments,
    /// A configured working directory was absolute, escaping, or malformed.
    InvalidWorkingDirectory,
    /// A fixed environment map was malformed, duplicated, or oversized.
    InvalidFixedEnvironment,
    /// The timeout was zero or exceeded its fixed bound.
    InvalidTimeout,
    /// A stdout or stderr bound was zero or exceeded its fixed bound.
    InvalidOutputBounds,
    /// The trusted catalog was empty, oversized, or contained a duplicate ID.
    InvalidCatalog,
    /// The requested semantic ID was absent from trusted configuration.
    UnknownConfiguredCommand,
}

impl ProcessPolicyError {
    /// Returns the stable sanitized machine-readable failure code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidSemanticValue => "process_policy_invalid_semantic_value",
            Self::ProgramNotAbsolute => "process_policy_program_not_absolute",
            Self::InvalidProgramPath => "process_policy_invalid_program_path",
            Self::InvalidLiteralArguments => "process_policy_invalid_literal_arguments",
            Self::InvalidWorkingDirectory => "process_policy_invalid_working_directory",
            Self::InvalidFixedEnvironment => "process_policy_invalid_fixed_environment",
            Self::InvalidTimeout => "process_policy_invalid_timeout",
            Self::InvalidOutputBounds => "process_policy_invalid_output_bounds",
            Self::InvalidCatalog => "process_policy_invalid_catalog",
            Self::UnknownConfiguredCommand => "process_policy_unknown_configured_command",
        }
    }
}

impl fmt::Display for ProcessPolicyError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "sanitized process-policy refusals retain no causal source"
)]
impl Error for ProcessPolicyError {}

/// A validated semantic identity resolved only by trusted configuration.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ConfiguredCommandId(String);

/// Exact Tiber/app-server correlation for one configured-command invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProcessInvocationId(String);

impl ProcessInvocationId {
    /// Returns the exact validated correlation.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses a bounded protocol correlation once at the app-server boundary.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal for empty, oversized, or control-bearing text.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ProcessPolicyError> {
        if value.is_empty()
            || value.len() > MAX_INVOCATION_ID_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(ProcessPolicyError::InvalidSemanticValue);
        }
        Ok(Self(value.to_owned()))
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the semantic parser is the sole durable construction boundary; deserialize_in_place cannot preserve it"
)]
impl<'de> Deserialize<'de> for ProcessInvocationId {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let decoded = String::deserialize(deserializer)?;
        Self::parse(&decoded).map_err(D::Error::custom)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the semantic parser is the sole durable construction boundary; deserialize_in_place cannot preserve it"
)]
impl<'de> Deserialize<'de> for ConfiguredCommandId {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let decoded = String::deserialize(deserializer)?;
        Self::parse(&decoded).map_err(D::Error::custom)
    }
}

impl ConfiguredCommandId {
    /// Returns the canonical semantic identity.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses a command identity once at its external boundary.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal for empty, oversized, or non-semantic text.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ProcessPolicyError> {
        let canonical = value.trim();
        if canonical.is_empty()
            || canonical.len() > MAX_COMMAND_ID_BYTES
            || !canonical.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(ProcessPolicyError::InvalidSemanticValue);
        }
        Ok(Self(canonical.to_owned()))
    }
}

/// Trusted workflow and assignment provenance carried by a process request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "workflow provenance precedes its assignment refinement at this authority boundary"
)]
pub struct AssignmentWorkflowProvenance {
    /// Exact originating workflow.
    workflow: WorkflowId,
    /// Exact assignment within the workflow.
    assignment: AssignmentId,
    /// Exact durable effect authorizing this request.
    effect: EffectId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the constructor is followed by provenance inspectors in conceptual order"
)]
impl AssignmentWorkflowProvenance {
    /// Binds one process intent to its workflow assignment and exact durable effect.
    #[must_use]
    #[inline]
    pub const fn new(
        workflow_id: WorkflowId,
        assignment_id: AssignmentId,
        effect_id: EffectId,
    ) -> Self {
        Self {
            workflow: workflow_id,
            assignment: assignment_id,
            effect: effect_id,
        }
    }

    /// Returns the originating workflow identity.
    #[must_use]
    #[inline]
    pub const fn workflow_id(&self) -> &WorkflowId {
        &self.workflow
    }

    /// Returns the originating assignment identity.
    #[must_use]
    #[inline]
    pub const fn assignment_id(&self) -> &AssignmentId {
        &self.assignment
    }

    /// Returns the exact durable effect authorizing this request.
    #[must_use]
    #[inline]
    pub const fn effect_id(&self) -> &EffectId {
        &self.effect
    }
}

/// Pure process intent containing no executable or operational configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessRequest {
    /// Requested trusted catalog entry.
    command_id: ConfiguredCommandId,
    /// Exact app-server invocation correlation bound into signed authority.
    invocation_id: ProcessInvocationId,
    /// Workflow authority context for the request.
    provenance: AssignmentWorkflowProvenance,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the constructor is followed by request inspectors in conceptual order"
)]
impl ProcessRequest {
    /// Constructs one invocation-correlated semantic process intent.
    #[must_use]
    #[inline]
    pub const fn for_invocation(
        command_id: ConfiguredCommandId,
        invocation_id: ProcessInvocationId,
        provenance: AssignmentWorkflowProvenance,
    ) -> Self {
        Self {
            command_id,
            invocation_id,
            provenance,
        }
    }

    /// Returns the configured command identity without resolving it.
    #[must_use]
    #[inline]
    pub const fn command_id(&self) -> &ConfiguredCommandId {
        &self.command_id
    }

    /// Returns the exact invocation correlation for this request.
    #[must_use]
    #[inline]
    pub const fn invocation_id(&self) -> &ProcessInvocationId {
        &self.invocation_id
    }

    /// Returns the trusted assignment/workflow provenance.
    #[must_use]
    #[inline]
    pub const fn provenance(&self) -> &AssignmentWorkflowProvenance {
        &self.provenance
    }
}

/// One bounded literal direct-argv item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiteralArgument(String);

impl LiteralArgument {
    /// Parses one argv item without shell interpretation.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal when the item is oversized or contains NUL.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ProcessPolicyError> {
        if value.len() > MAX_ARGUMENT_BYTES || value.contains('\0') {
            return Err(ProcessPolicyError::InvalidLiteralArguments);
        }
        Ok(Self(value.to_owned()))
    }
}

/// A validated repository-relative working directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelativeWorkingDirectory(PathBuf);

impl RelativeWorkingDirectory {
    /// Parses a repository-relative path that cannot escape through `..`.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal for absolute, empty, or escaping paths.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, ProcessPolicyError> {
        let path = Path::new(value);
        if value.is_empty()
            || value.len() > MAX_ARGUMENT_BYTES
            || value.contains('\0')
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ProcessPolicyError::InvalidWorkingDirectory);
        }
        Ok(Self(path.to_path_buf()))
    }
}

/// Exact fixed environment supplied by trusted configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixedEnvironment(BTreeMap<String, String>);

impl FixedEnvironment {
    /// Builds an exact, deterministic environment map.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal for duplicate, malformed, or oversized entries.
    #[inline]
    pub fn new<K, V, I>(entries: I) -> Result<Self, ProcessPolicyError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut environment = BTreeMap::new();
        for (raw_key, raw_value) in entries {
            let key = raw_key.as_ref();
            let value = raw_value.as_ref();
            let valid_key = !key.is_empty()
                && key.len() <= MAX_COMMAND_ID_BYTES
                && key.chars().enumerate().all(|(index, character)| {
                    matches!(
                        (index, character),
                        (0, 'A'..='Z' | '_') | (_, 'A'..='Z' | '0'..='9' | '_')
                    )
                });
            if !valid_key
                || value.len() > MAX_ENVIRONMENT_VALUE_BYTES
                || value.contains('\0')
                || environment
                    .insert(key.to_owned(), value.to_owned())
                    .is_some()
                || environment.len() > MAX_ENVIRONMENT_ENTRIES
            {
                return Err(ProcessPolicyError::InvalidFixedEnvironment);
            }
        }
        Ok(Self(environment))
    }
}

/// Independent fixed capture limits for child stdout and stderr.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "stdout and stderr limits retain process stream order"
)]
pub struct OutputBounds {
    /// Maximum stdout bytes retained by the adapter.
    stdout_bytes: usize,
    /// Maximum stderr bytes retained by the adapter.
    stderr_bytes: usize,
}

impl OutputBounds {
    /// Constructs nonzero bounded output limits.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal when either limit is zero or too large.
    #[inline]
    pub const fn new(stdout_bytes: usize, stderr_bytes: usize) -> Result<Self, ProcessPolicyError> {
        if stdout_bytes == 0
            || stderr_bytes == 0
            || stdout_bytes > MAX_OUTPUT_BYTES
            || stderr_bytes > MAX_OUTPUT_BYTES
        {
            return Err(ProcessPolicyError::InvalidOutputBounds);
        }
        Ok(Self {
            stdout_bytes,
            stderr_bytes,
        })
    }
}

/// Trusted, fixed execution configuration for one command identity.
#[derive(Clone, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "trusted execution configuration follows direct process construction order"
)]
pub struct ConfiguredCommand {
    /// Absolute executable path.
    program: PathBuf,
    /// Literal direct argv.
    argv: Vec<LiteralArgument>,
    /// Repository-relative working directory.
    cwd: RelativeWorkingDirectory,
    /// Exact fixed child environment.
    environment: FixedEnvironment,
    /// Nonzero execution deadline.
    timeout: Duration,
    /// Independent output stream limits.
    output: OutputBounds,
}

impl fmt::Debug for ConfiguredCommand {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConfiguredCommand(<redacted>)")
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "constructor precedes adapter-data inspectors in direct process construction order"
)]
impl ConfiguredCommand {
    /// Constructs a direct-argv, network-denied configured command.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal when the program, argv, or bounds are invalid.
    #[inline]
    pub fn new(
        program: PathBuf,
        argv: Vec<LiteralArgument>,
        cwd: RelativeWorkingDirectory,
        environment: FixedEnvironment,
        timeout: Duration,
        output: OutputBounds,
    ) -> Result<Self, ProcessPolicyError> {
        if !program.is_absolute() {
            return Err(ProcessPolicyError::ProgramNotAbsolute);
        }
        let Some(program_text) = program.to_str() else {
            return Err(ProcessPolicyError::InvalidProgramPath);
        };
        let mut executable_components = program.components();
        let rooted = matches!(executable_components.next(), Some(Component::RootDir));
        let mut saw_normal_component = false;
        let syntactic = executable_components.all(|component| {
            if matches!(component, Component::Normal(_)) {
                saw_normal_component = true;
                true
            } else {
                false
            }
        });
        if program_text.len() > MAX_PROGRAM_PATH_BYTES
            || program_text.contains('\0')
            || !rooted
            || !syntactic
            || !saw_normal_component
        {
            return Err(ProcessPolicyError::InvalidProgramPath);
        }
        if argv.len() > MAX_ARGUMENTS {
            return Err(ProcessPolicyError::InvalidLiteralArguments);
        }
        if timeout.is_zero() || timeout > MAX_TIMEOUT {
            return Err(ProcessPolicyError::InvalidTimeout);
        }
        Ok(Self {
            program,
            argv,
            cwd,
            environment,
            timeout,
            output,
        })
    }

    /// Returns the trusted absolute executable path.
    #[must_use]
    #[inline]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Returns the trusted literal argv in order.
    #[must_use]
    #[inline]
    pub fn argv(&self) -> impl ExactSizeIterator<Item = &str> {
        self.argv.iter().map(|argument| argument.0.as_str())
    }

    /// Returns the trusted repository-relative working directory.
    #[must_use]
    #[inline]
    pub fn repository_relative_cwd(&self) -> &Path {
        &self.cwd.0
    }

    /// Returns the exact fixed child environment.
    #[must_use]
    #[inline]
    pub fn fixed_environment(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.environment
            .0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// Returns the trusted nonzero timeout.
    #[must_use]
    #[inline]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Returns the stdout capture limit.
    #[must_use]
    #[inline]
    pub const fn stdout_limit_bytes(&self) -> usize {
        self.output.stdout_bytes
    }

    /// Returns the stderr capture limit.
    #[must_use]
    #[inline]
    pub const fn stderr_limit_bytes(&self) -> usize {
        self.output.stderr_bytes
    }
}

/// Trusted mapping from semantic command identities to fixed execution plans.
#[derive(Clone)]
pub struct ConfiguredCommandCatalog(BTreeMap<ConfiguredCommandId, ConfiguredCommand>);

impl fmt::Debug for ConfiguredCommandCatalog {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConfiguredCommandCatalog(<redacted>)")
    }
}

impl ConfiguredCommandCatalog {
    /// Iterates the validated semantic identities available to a model-facing schema.
    ///
    /// Execution plans remain private to the trusted process boundary.
    #[inline]
    pub fn command_ids(&self) -> impl Iterator<Item = &ConfiguredCommandId> {
        self.0.keys()
    }

    /// Constructs a bounded catalog with unique identities.
    ///
    /// # Errors
    ///
    /// Returns a stable refusal for an empty, oversized, or duplicate catalog.
    #[inline]
    pub fn new<I>(commands: I) -> Result<Self, ProcessPolicyError>
    where
        I: IntoIterator<Item = (ConfiguredCommandId, ConfiguredCommand)>,
    {
        let mut catalog = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for (id, command) in commands {
            if !seen.insert(id.clone()) || catalog.len() >= MAX_CONFIGURED_COMMANDS {
                return Err(ProcessPolicyError::InvalidCatalog);
            }
            catalog.insert(id, command);
        }
        if catalog.is_empty() {
            return Err(ProcessPolicyError::InvalidCatalog);
        }
        Ok(Self(catalog))
    }

    /// Resolves semantic intent to trusted configuration without adapter authority.
    ///
    /// # Errors
    ///
    /// Returns a sanitized stable refusal when the identity is not configured.
    #[inline]
    pub fn resolve(
        &self,
        command_id: &ConfiguredCommandId,
    ) -> Result<&ConfiguredCommand, ProcessPolicyError> {
        self.0
            .get(command_id)
            .ok_or(ProcessPolicyError::UnknownConfiguredCommand)
    }
}
