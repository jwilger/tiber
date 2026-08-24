import {
  parseTaskSpecification,
  type TaskSpecification,
} from "../../src/core/tasks/readiness.js";

export const validTaskSpecificationDocument = {
  outcome: "A shared task can enter Ready only after independent review",
  scenarios: [
    {
      name: "clean review",
      given: ["a complete canonical specification"],
      when: ["a fresh reviewer finds no issues"],
      then: ["the task enters Ready"],
    },
  ],
  acceptanceCriteria: ["Ready is shared"],
  exclusions: ["No automatic priority changes"],
  dependencies: [],
  testMappings: ["tests/acceptance/readiness.test.ts"],
  architectureImplications:
    "The review is advisory input to deterministic authority.",
} as const;

export function requireTaskSpecification(value: unknown): TaskSpecification {
  const parsed = parseTaskSpecification(value);
  if (!parsed.ok) throw new Error("invalid task specification fixture");
  return parsed.value;
}
