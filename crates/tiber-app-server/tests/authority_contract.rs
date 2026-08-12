#[cfg(test)]
mod tests {
    use tiber_app_server::inspect_protocol_schema;

    const CODEX_0_147_PROTOCOL_SCHEMA: &str =
        include_str!("fixtures/codex-0.147.0-authority-surface.json");

    #[test]
    #[expect(
        clippy::implicit_return,
        reason = "the Result mapping keeps the assertion panic-free and focused on public behavior"
    )]
    fn codex_0_147_exposes_the_reviewed_tiber_control_surface() {
        assert_eq!(
            inspect_protocol_schema(CODEX_0_147_PROTOCOL_SCHEMA).map(|report| (
                report.is_compatible(),
                report.controlled_operations().to_vec()
            )),
            Ok((
                true,
                vec![
                    "thread-item:commandExecution:runtime-policy-controlled",
                    "thread-item:fileChange:runtime-policy-controlled"
                ]
                .into_iter()
                .map(str::to_owned)
                .collect()
            ))
        );
    }

    #[test]
    #[expect(
        clippy::implicit_return,
        reason = "the Result mapping keeps the assertion panic-free and focused on the error code"
    )]
    fn malformed_schema_has_a_stable_typed_error() {
        assert_eq!(
            inspect_protocol_schema("not json").map_err(|error| error.code()),
            Err("app_server_schema_invalid")
        );
    }

    #[test]
    #[expect(
        clippy::implicit_return,
        reason = "the Result mapping keeps the assertion panic-free and focused on fail-closed behavior"
    )]
    fn unknown_schema_structure_fails_closed() {
        assert_eq!(
            inspect_protocol_schema("{}").map_err(|error| error.code()),
            Err("app_server_schema_contract_unrecognized")
        );
    }

    #[test]
    #[expect(
        clippy::implicit_return,
        reason = "the Result mapping keeps the assertion panic-free and focused on compatibility"
    )]
    fn an_unknown_protocol_control_surface_requires_a_reviewed_adapter() {
        let schema = CODEX_0_147_PROTOCOL_SCHEMA.replacen(
            "        \"permissions\": {},\n",
            "        \"builtInTools\": {},\n",
            1,
        );
        assert_eq!(
            inspect_protocol_schema(&schema).map_err(|error| error.code()),
            Err("app_server_schema_contract_unrecognized")
        );
    }

    #[test]
    #[expect(
        clippy::implicit_return,
        reason = "the Result mapping keeps the assertion panic-free and focused on fail-closed behavior"
    )]
    fn malformed_thread_item_shape_fails_closed() {
        let schema = CODEX_0_147_PROTOCOL_SCHEMA.replacen(
            "              \"enum\": [\"commandExecution\"]",
            "              \"enum\": []",
            1,
        );
        assert_eq!(
            inspect_protocol_schema(&schema).map_err(|error| error.code()),
            Err("app_server_schema_contract_unrecognized")
        );
    }
}
