#[cfg(test)]
mod tests {
    use core::{fmt, iter, time::Duration};
    use std::path::PathBuf;

    use tiber_process_core::{
        AssignmentWorkflowProvenance, ConfiguredCommand, ConfiguredCommandCatalog,
        ConfiguredCommandId, FixedEnvironment, LiteralArgument, MAX_ARGUMENTS,
        MAX_CONFIGURED_COMMANDS, MAX_PROGRAM_PATH_BYTES, OutputBounds, ProcessInvocationId,
        ProcessPolicyError, ProcessRequest, RelativeWorkingDirectory,
    };
    use tiber_workflow_core::{AssignmentId, EffectId, WorkflowId};

    fn parsed<T, E: fmt::Debug>(result: Result<T, E>) -> T {
        result.expect("fixture value should satisfy the semantic boundary")
    }

    #[test]
    fn known_semantic_command_resolves_without_minting_adapter_authority() {
        let command_id = parsed(ConfiguredCommandId::parse("format-check"));
        let provenance = AssignmentWorkflowProvenance::new(
            parsed(WorkflowId::parse("workflow-3")),
            parsed(AssignmentId::parse("assignment-3")),
            parsed(EffectId::parse("effect-3")),
        );
        let invocation_id = parsed(ProcessInvocationId::parse("invocation-3"));
        let request = ProcessRequest::for_invocation(
            command_id.clone(),
            invocation_id.clone(),
            provenance.clone(),
        );

        assert_eq!(request.command_id(), &command_id);
        assert_eq!(request.invocation_id(), &invocation_id);
        assert_eq!(request.provenance(), &provenance);

        let command = parsed(ConfiguredCommand::new(
            PathBuf::from("/nix/store/example/bin/cargo"),
            vec![
                parsed(LiteralArgument::parse("test")),
                parsed(LiteralArgument::parse("--workspace")),
            ],
            parsed(RelativeWorkingDirectory::parse("crates/tiber-process-core")),
            parsed(FixedEnvironment::new([
                ("LANG", "C.UTF-8"),
                ("NO_COLOR", "1"),
            ])),
            Duration::from_secs(30),
            parsed(OutputBounds::new(0x4000, 0x2000)),
        ));
        let configured_diagnostic = format!("{command:?}");
        assert!(!configured_diagnostic.contains("/nix/store/example/bin/cargo"));
        assert!(!configured_diagnostic.contains("C.UTF-8"));
        let catalog = parsed(ConfiguredCommandCatalog::new([(command_id, command)]));
        let catalog_diagnostic = format!("{catalog:?}");
        assert!(!catalog_diagnostic.contains("/nix/store/example/bin/cargo"));
        assert!(!catalog_diagnostic.contains("C.UTF-8"));

        let _resolved = parsed(catalog.resolve(request.command_id()));
    }

    #[test]
    fn trusted_executable_path_is_absolute_syntactic_and_bounded() {
        let exact = format!("/{}", "a".repeat(MAX_PROGRAM_PATH_BYTES - 1));
        let command = ConfiguredCommand::new(
            PathBuf::from(exact),
            Vec::new(),
            parsed(RelativeWorkingDirectory::parse(".")),
            parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
            Duration::from_secs(1),
            parsed(OutputBounds::new(1, 1)),
        );
        let _command = parsed(command);

        for (program, expected) in [
            (
                PathBuf::from(format!("/{}", "a".repeat(MAX_PROGRAM_PATH_BYTES))),
                ProcessPolicyError::InvalidProgramPath,
            ),
            (
                PathBuf::from("/trusted/bin/\0hostile"),
                ProcessPolicyError::InvalidProgramPath,
            ),
            (
                PathBuf::from("/trusted/../hostile"),
                ProcessPolicyError::InvalidProgramPath,
            ),
            (
                PathBuf::from("relative/tool"),
                ProcessPolicyError::ProgramNotAbsolute,
            ),
        ] {
            let refusal = ConfiguredCommand::new(
                program,
                Vec::new(),
                parsed(RelativeWorkingDirectory::parse(".")),
                parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
                Duration::from_secs(1),
                parsed(OutputBounds::new(1, 1)),
            )
            .expect_err("malformed executable configuration must fail closed");
            assert_eq!(refusal, expected);
            assert_eq!(refusal.to_string(), refusal.code());
        }
    }

    #[test]
    fn trusted_execution_configuration_rejects_representative_bound_violations() {
        let too_many_arguments = iter::repeat_with(|| parsed(LiteralArgument::parse("literal")))
            .take(MAX_ARGUMENTS.saturating_add(1))
            .collect();
        assert_eq!(
            ConfiguredCommand::new(
                PathBuf::from("/trusted/bin/tool"),
                too_many_arguments,
                parsed(RelativeWorkingDirectory::parse(".")),
                parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
                Duration::from_secs(1),
                parsed(OutputBounds::new(1, 1)),
            )
            .expect_err("argv count must remain bounded"),
            ProcessPolicyError::InvalidLiteralArguments
        );
        assert_eq!(
            LiteralArgument::parse("literal\0argument"),
            Err(ProcessPolicyError::InvalidLiteralArguments)
        );
        assert_eq!(
            RelativeWorkingDirectory::parse("../escape"),
            Err(ProcessPolicyError::InvalidWorkingDirectory)
        );
        assert_eq!(
            RelativeWorkingDirectory::parse("repo\0hostile"),
            Err(ProcessPolicyError::InvalidWorkingDirectory)
        );
        assert_eq!(
            FixedEnvironment::new([("LANG", "C"), ("LANG", "hostile")]),
            Err(ProcessPolicyError::InvalidFixedEnvironment)
        );
        assert_eq!(
            OutputBounds::new(0, 1),
            Err(ProcessPolicyError::InvalidOutputBounds)
        );
        assert_eq!(
            ConfiguredCommand::new(
                PathBuf::from("/trusted/bin/tool"),
                Vec::new(),
                parsed(RelativeWorkingDirectory::parse(".")),
                parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
                Duration::ZERO,
                parsed(OutputBounds::new(1, 1)),
            )
            .expect_err("timeout must be positive"),
            ProcessPolicyError::InvalidTimeout
        );
    }

    #[test]
    fn configured_command_catalog_has_its_own_semantic_capacity_bound() {
        assert_ne!(
            MAX_CONFIGURED_COMMANDS, MAX_ARGUMENTS,
            "catalog capacity must not be coupled to per-command argv capacity"
        );
        let command = parsed(ConfiguredCommand::new(
            PathBuf::from("/trusted/bin/tool"),
            Vec::new(),
            parsed(RelativeWorkingDirectory::parse(".")),
            parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
            Duration::from_secs(1),
            parsed(OutputBounds::new(1, 1)),
        ));
        let at_capacity = (0..MAX_CONFIGURED_COMMANDS).map(|index| {
            let raw_id = format!("command-{index}");
            (parsed(ConfiguredCommandId::parse(&raw_id)), command.clone())
        });
        let catalog = ConfiguredCommandCatalog::new(at_capacity)
            .expect("the documented catalog capacity must be accepted");
        assert_eq!(
            catalog
                .resolve(&parsed(ConfiguredCommandId::parse("command-0")))
                .expect("first command must resolve"),
            &command
        );
        let over_capacity = (0..MAX_CONFIGURED_COMMANDS.saturating_add(1)).map(|index| {
            let raw_id = format!("overflow-command-{index}");
            (parsed(ConfiguredCommandId::parse(&raw_id)), command.clone())
        });
        assert_eq!(
            ConfiguredCommandCatalog::new(over_capacity)
                .expect_err("catalog entries above the semantic capacity must fail closed"),
            ProcessPolicyError::InvalidCatalog
        );
    }

    #[test]
    fn durable_request_values_round_trip_through_validated_semantic_deserialization() {
        let request = ProcessRequest::for_invocation(
            parsed(ConfiguredCommandId::parse("format-check")),
            parsed(ProcessInvocationId::parse("invocation-3")),
            AssignmentWorkflowProvenance::new(
                parsed(WorkflowId::parse("workflow-3")),
                parsed(AssignmentId::parse("assignment-3")),
                parsed(EffectId::parse("effect-3")),
            ),
        );
        let encoded = parsed(serde_json::to_string(&request));
        let decoded: ProcessRequest = parsed(serde_json::from_str(&encoded));
        assert_eq!(decoded, request);

        let refusal = ProcessPolicyError::UnknownConfiguredCommand;
        let encoded_refusal = parsed(serde_json::to_string(&refusal));
        let decoded_refusal: ProcessPolicyError = parsed(serde_json::from_str(&encoded_refusal));
        assert_eq!(decoded_refusal, refusal);

        let hostile = "../hostile-command";
        let invalid = serde_json::from_str::<ConfiguredCommandId>(&format!("\"{hostile}\""))
            .expect_err("durable decoding must preserve semantic validation");
        assert!(
            invalid
                .to_string()
                .contains("process_policy_invalid_semantic_value")
        );
        assert!(!invalid.to_string().contains(hostile));
    }

    #[test]
    fn durable_invocation_id_deserialization_preserves_semantic_validation() {
        let hostile = "invocation\0secret";
        let encoded = serde_json::to_string(hostile).expect("fixture text must serialize");
        let invalid = serde_json::from_str::<ProcessInvocationId>(&encoded)
            .expect_err("durable decoding must reject control-bearing invocation IDs");

        assert!(
            invalid
                .to_string()
                .contains("process_policy_invalid_semantic_value")
        );
        assert!(!invalid.to_string().contains("secret"));
    }

    #[test]
    fn durable_process_request_requires_an_invocation_correlation() {
        let request = ProcessRequest::for_invocation(
            parsed(ConfiguredCommandId::parse("format-check")),
            parsed(ProcessInvocationId::parse("invocation-3")),
            AssignmentWorkflowProvenance::new(
                parsed(WorkflowId::parse("workflow-3")),
                parsed(AssignmentId::parse("assignment-3")),
                parsed(EffectId::parse("effect-3")),
            ),
        );
        let mut encoded = serde_json::to_value(request).expect("fixture request must serialize");
        encoded
            .as_object_mut()
            .expect("request must serialize as an object")
            .remove("invocation_id");

        serde_json::from_value::<ProcessRequest>(encoded)
            .expect_err("a durable request without invocation correlation must be rejected");
    }

    #[test]
    fn unknown_id_is_a_sanitized_stable_policy_refusal() {
        let catalog = parsed(ConfiguredCommandCatalog::new([(
            parsed(ConfiguredCommandId::parse("known")),
            parsed(ConfiguredCommand::new(
                PathBuf::from("/trusted/bin/tool"),
                Vec::<LiteralArgument>::new(),
                parsed(RelativeWorkingDirectory::parse(".")),
                parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
                Duration::from_secs(1),
                parsed(OutputBounds::new(1, 1)),
            )),
        )]));
        let hostile = "unknown-secret-marker";
        let request = ProcessRequest::for_invocation(
            parsed(ConfiguredCommandId::parse(hostile)),
            parsed(ProcessInvocationId::parse("invocation-3")),
            AssignmentWorkflowProvenance::new(
                parsed(WorkflowId::parse("workflow-3")),
                parsed(AssignmentId::parse("assignment-3")),
                parsed(EffectId::parse("effect-3")),
            ),
        );

        let refusal = catalog
            .resolve(request.command_id())
            .expect_err("an unknown semantic ID must not resolve trusted configuration");

        assert_eq!(refusal.code(), "process_policy_unknown_configured_command");
        assert_eq!(refusal.to_string(), refusal.code());
        assert!(!refusal.to_string().contains(hostile));
        assert!(!refusal.to_string().contains("/ambient/working-directory"));
    }
}
