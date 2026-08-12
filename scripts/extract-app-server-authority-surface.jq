{
  _provenance: {
    codexVersion: $codex_version,
    generatedCommand: "codex app-server generate-json-schema --experimental --out <directory>",
    schemaFile: "codex_app_server_protocol.v2.schemas.json",
    schemaSha256: $schema_sha256
  },
  definitions: {
    ThreadItem: {
      oneOf: [
        .definitions.ThreadItem.oneOf[]
        | { properties: { type: { enum: .properties.type.enum } } }
      ]
    },
    ThreadStartParams: {
      properties: (
        .definitions.ThreadStartParams.properties
        | with_entries(.value = {})
      )
    }
  },
  title
}
