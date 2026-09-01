import assert from "node:assert/strict";
import test from "node:test";

import {
  auditSourceProvenance,
  repositoryBoundaryViolations,
} from "./audit-source-provenance.mjs";

test("historical CC Switch repository references are rejected", () => {
  const historicalReference = [
    "https://github.com",
    ["farion", "1231"].join(""),
    "cc-switch",
  ].join("/");
  assert.match(
    repositoryBoundaryViolations("workflow.yml", historicalReference).join("\n"),
    /historical CC Switch repository reference/,
  );
  assert.deepEqual(
    repositoryBoundaryViolations(
      "workflow.yml",
      "repository: Xiechengqi/cc-switch-router",
    ),
    [],
  );
});

test("checked-in Router provenance has no external technical dependency", () => {
  assert.deepEqual(auditSourceProvenance(), []);
});
