import { describe, expect, it } from "vitest";

import {
  parseDeliveryCommitBody,
  parseDeliveryCommitRevision,
  parseDeliveryCommitSubject,
  parseDeliveryDestinationRef,
  parseDeliveryTreeDigest,
} from "../../src/core/delivery/git-delivery-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("Git delivery semantic boundary", () => {
  it.each([
    [parseDeliveryCommitRevision, "a".repeat(40)],
    [parseDeliveryTreeDigest, "b".repeat(40)],
    [parseDeliveryDestinationRef, "refs/heads/feature/example"],
    [parseDeliveryDestinationRef, "refs/heads/feature/a.b"],
    [parseDeliveryCommitSubject, "feat(delivery): add exact receipts"],
    [parseDeliveryCommitBody, "Explain why this delivery is correct."],
    [parseDeliveryCommitBody, "x".repeat(12)],
  ])("parses a purpose-specific delivery value", (parse, input) => {
    expect(parse(input)).toMatchObject({ ok: true, value: input });
  });

  it.each([
    [parseDeliveryCommitRevision, "bad", "commitRevision"],
    [parseDeliveryCommitRevision, `x${"a".repeat(40)}`, "commitRevision"],
    [parseDeliveryCommitRevision, `${"a".repeat(40)}x`, "commitRevision"],
    [parseDeliveryCommitRevision, { length: 40 }, "commitRevision"],
    [
      parseDeliveryCommitRevision,
      { toString: (): string => "a".repeat(40) },
      "commitRevision",
    ],
    [parseDeliveryTreeDigest, `x${"b".repeat(40)}`, "treeDigest"],
    [parseDeliveryTreeDigest, `${"b".repeat(40)}x`, "treeDigest"],
    [parseDeliveryTreeDigest, "bad", "treeDigest"],
    [parseDeliveryDestinationRef, "main", "destinationRef"],
    [parseDeliveryDestinationRef, "xrefs/heads/main", "destinationRef"],
    [parseDeliveryDestinationRef, "refs/heads/main x", "destinationRef"],
    [parseDeliveryDestinationRef, "refs/heads/a..b", "destinationRef"],
    [parseDeliveryDestinationRef, "refs/heads/a//b", "destinationRef"],
    [parseDeliveryDestinationRef, "refs/heads/main/", "destinationRef"],
    [parseDeliveryDestinationRef, "refs/heads/main.", "destinationRef"],
    [parseDeliveryDestinationRef, "refs/heads/.hidden", "destinationRef"],
    [parseDeliveryDestinationRef, "refs/heads/a/.hidden", "destinationRef"],
    [
      parseDeliveryDestinationRef,
      { toString: (): string => "refs/heads/feature/example" },
      "destinationRef",
    ],
    [parseDeliveryDestinationRef, "refs/heads/main.lock", "destinationRef"],
    [parseDeliveryCommitSubject, "not conventional", "commitSubject"],
    [parseDeliveryCommitSubject, "xfeat: valid subject", "commitSubject"],
    [parseDeliveryCommitSubject, "feat: valid subject\nextra", "commitSubject"],
    [parseDeliveryCommitBody, "short", "commitBody"],
    [parseDeliveryCommitBody, "x".repeat(11), "commitBody"],
    [parseDeliveryCommitBody, " leading body rationale", "commitBody"],
    [parseDeliveryCommitBody, "trailing body rationale ", "commitBody"],
    [parseDeliveryCommitBody, { length: 20 }, "commitBody"],
  ])("rejects malformed delivery values", (parse, input, field) => {
    expect(parse(input)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_DELIVERY_VALUE_INVALID", field),
    });
  });
});
