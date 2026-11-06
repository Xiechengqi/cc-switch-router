import assert from "node:assert/strict";
import test from "node:test";
import { ModelRoutingTestSession } from "./model-routing-test-session";
import type { UserModelRoutingResponse, UserModelRoutingTestRequest, UserModelRoutingTestResponse } from "./types";

const profile: UserModelRoutingResponse = { enabled: true, revision: 3, apiBaseUrl: "https://example.test", routes: [], eligibleShares: [] };
const result: UserModelRoutingTestResponse = { success: true, appType: "codex", requestedModel: "model", curl: "", matchedWildcard: false, durationMs: 1 };

test("legacy primary sends exactly two fields and diagnostics never silently downgrade", async () => {
  const calls: UserModelRoutingTestRequest[] = [];
  const session = new ModelRoutingTestSession(async input => { calls.push(input); return result; });
  await session.run(profile, "codex", "model", "primary");
  assert.deepEqual(calls, [{ appType: "codex", requestedModel: "model" }]);
  await session.run(profile, "codex", "model", "saved_route");
  assert.equal(calls.length, 1);
  assert.equal(session.getSnapshot().error, "capability_missing");
});

test("late result is discarded and invalidation cannot enable duplicate metered calls", async () => {
  let complete!: (result: UserModelRoutingTestResponse) => void;
  let calls = 0;
  const session = new ModelRoutingTestSession(() => { calls++; return new Promise(resolve => { complete = resolve; }); });
  const running = session.run(profile, "codex", "model", "primary");
  session.invalidate();
  await session.run(profile, "codex", "another", "primary");
  assert.equal(calls, 1);
  complete(result);
  await running;
  assert.equal(session.getSnapshot().result, null);
  assert.equal(session.getSnapshot().running, false);
  assert.equal(session.getSnapshot().error, "invalidated", "a discarded in-flight call is reported, not hidden");
  assert.equal(session.getSnapshot().unknown, true);
  session.invalidate();
  assert.equal(session.getSnapshot().error, "", "invalidating an idle session clears the notice");
});

test("network failure is unknown and never retried", async () => {
  let calls = 0;
  const session = new ModelRoutingTestSession(async () => { calls++; throw new Error("network failure"); });
  await session.run(profile, "codex", "model", "primary");
  assert.equal(calls, 1);
  assert.equal(session.getSnapshot().unknown, true);
});

test("saved-route results require the negotiated revision and mode", async () => {
  const calls: UserModelRoutingTestRequest[] = [];
  const session = new ModelRoutingTestSession(async input => { calls.push(input); return result; });
  const capable = { ...profile, testCapabilities: { savedRoute: true, revisionCheck: true, totalTimeoutMs: 30000, maxAttempts: 4, responsePreviewBytes: 16384 } };
  await session.run(capable, "codex", "model", "saved_route");
  assert.deepEqual(calls[0], { appType: "codex", requestedModel: "model", mode: "saved_route", expectedRevision: 3 });
  assert.equal(session.getSnapshot().unknown, true);
  assert.equal(session.getSnapshot().result, null);
});

test("capability loss clears diagnostics without replaying or allowing an overlapping primary test", async () => {
  let complete!: (result: UserModelRoutingTestResponse) => void;
  const calls: UserModelRoutingTestRequest[] = [];
  const session = new ModelRoutingTestSession(input => { calls.push(input); return new Promise(resolve => { complete = resolve; }); });
  const capable = { ...profile, testCapabilities: { savedRoute: true, revisionCheck: true, totalTimeoutMs: 30000, maxAttempts: 4, responsePreviewBytes: 16384 } };
  const running = session.run(capable, "codex", "model", "saved_route");
  session.invalidate();
  await session.run(profile, "codex", "model", "primary");
  assert.equal(calls.length, 1);
  complete({ ...result, mode: "saved_route", revision: profile.revision });
  await running;
  assert.equal(session.getSnapshot().result, null);
  assert.equal(session.getSnapshot().running, false);
  session.invalidate();
  assert.equal(calls.length, 1, "restoring capabilities cannot send a request automatically");
  const primary = session.run(profile, "codex", "model", "primary");
  assert.deepEqual(calls[1], { appType: "codex", requestedModel: "model" });
  complete(result);
  await primary;
});

test("drawer lifecycle resets unavailable diagnostics and translates fixed states", async () => {
  const { readFileSync } = await import("node:fs");
  const drawer = readFileSync(new URL("../components/dashboard/model-hub-test-drawer.tsx", import.meta.url), "utf8");
  const fallback = drawer.match(/React\.useEffect\(\(\) => \{\s*if \(!supportsDiagnostic[\s\S]*?\}, \[supportsDiagnostic, mode, session\]\)/)?.[0];
  assert.ok(fallback);
  assert.match(fallback, /session\.invalidate\(\); setMode\("primary"\)/);
  assert.doesNotMatch(fallback, /session\.run/);
  assert.match(drawer, /\[session, app, model, profile\?\.revision, disabled, open\]/);
  assert.match(drawer, /t\(`modelHub\.attempt\.\$\{attempt\.outcome\}`\)/);
  assert.match(drawer, /t\("modelHub\.test\.snapshotMismatch"\)/);
  assert.match(drawer, /session\.run\(profile, app, model, mode\)\.then\(onComplete\)/);
});
