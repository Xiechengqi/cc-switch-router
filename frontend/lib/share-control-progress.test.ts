import assert from "node:assert/strict";
import test from "node:test";
import { messages } from "./i18n";
import { controlProgressMessageKey, controlRetryTime } from "./share-control-progress";

test("reclaim progress uses distinct bilingual messages and rejects unknown stages", () => {
  const stages = ["awaiting_revocation", "awaiting_final_usage", "awaiting_settlement", "retry_scheduled", "manual_intervention"];
  const keys = stages.map(controlProgressMessageKey);
  assert.equal(new Set(keys).size, stages.length);
  for (const key of keys) {
    assert.ok(key);
    assert.ok(messages.en[key]);
    assert.ok(messages["zh-CN"][key]);
  }
  for (const stage of ["", "unknown", "toString", "__proto__"]) {
    assert.equal(controlProgressMessageKey(stage), null);
  }
});

test("retry time is localized and missing or malformed times never leak raw data", () => {
  for (const locale of ["en", "zh-CN"] as const) {
    assert.ok(controlRetryTime("2026-11-02T15:47:07Z", locale));
    assert.equal(controlRetryTime(undefined, locale), null);
    assert.equal(controlRetryTime("not-a-clock", locale), null);
    assert.equal(controlRetryTime("", locale), null);
  }
});
