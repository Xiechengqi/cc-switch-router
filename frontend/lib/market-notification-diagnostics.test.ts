import { test } from "node:test";
import assert from "node:assert/strict";
import { deliveryState, deliveryLabels, providerFailureCount } from "./market-notification-diagnostics";

test("API acceptance is distinct from actual delivery, including legacy records", () => {
  for (const status of ["accepted", "delayed", "failed", "delivered"]) {
    assert.equal(deliveryState({ channel: "email", submissionStatus: "sent", providerStatus: status }), status);
  }
  assert.equal(deliveryState({ channel: "email", submissionStatus: "sent", providerStatus: null }), "untracked");
  assert.equal(deliveryState({ channel: "email", submissionStatus: "uncertain", providerStatus: null }), "uncertain");
  assert.notEqual(deliveryLabels["zh-CN"].accepted, deliveryLabels["zh-CN"].delivered);
});
test("provider failures include bounce, complaint and rejection, not delayed delivery", () => {
  assert.equal(providerFailureCount({ failed: 6, bounced: 2, complained: 1, suppressed: 1, delayed: 19, delivered: 385 }), 10);
  assert.equal(providerFailureCount({}), 0);
  assert.deepEqual(Object.keys(deliveryLabels.en).sort(), Object.keys(deliveryLabels["zh-CN"]).sort());
});
