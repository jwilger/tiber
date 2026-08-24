import { describe, expect, it } from "vitest";

import {
  foldTaskEvents,
  parseTaskEvent,
} from "../../src/core/tasks/task-board.js";
import { workflowGuidance } from "../../src/extension/workflow-orchestration.js";

const created = parseTaskEvent({
  schemaVersion: 1,
  eventId: "11111111-1111-4111-8111-111111111111",
  kind: "task-created",
  occurredAt: "2026-01-01T00:00:00.000Z",
  task: {
    id: "22222222-2222-4222-8222-222222222222",
    title: "Deliver a normal conversational workflow",
    description: "",
  },
});
if (!created.ok) throw new Error("invalid fixture");

describe("automatic Pi-native workflow orchestration", () => {
  it("gives Pi signed workflow state and directs inferred requests without manual commands", () => {
    const guidance = workflowGuidance(foldTaskEvents([created.value]));
    expect(guidance).toContain(
      "22222222-2222-4222-8222-222222222222 | Backlog | unspecified | unclaimed",
    );
    expect(guidance).toContain("Use tiber_workflow_request");
    expect(guidance).toContain("do not ask the user to type /tiber commands");
  });
});
