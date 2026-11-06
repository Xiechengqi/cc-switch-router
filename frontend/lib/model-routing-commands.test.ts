import assert from "node:assert/strict";
import test from "node:test";
import { applyModelRouteCommand, diffModelRoutes, modelRoutesDraftFingerprint, type DraftModelRoute } from "./model-routing";
import type { UserModelRoutingShare } from "./types";
const saved: DraftModelRoute[] = [{ clientId: "r", appType: "codex", requestedModel: "model", targetShareId: "a", candidates: ["b", "c"] }];
const shares: UserModelRoutingShare[] = ["a", "b", "c"].map(shareId => ({ shareId, shareName: shareId, subdomain: shareId, apps: ["codex"], access: "owner", directApiUrl: "", freeAccess: false, isOnline: shareId !== "b" }));
const context = { eligibleShares: shares, savedRoutes: saved, baseRevision: 1 };
test("duplicate append fails atomically and move preserves explicit order", () => {
  const duplicate = applyModelRouteCommand(saved, { kind: "append_targets", clientId: "r", shareIds: ["b"] }, context);
  assert.equal(duplicate.routes, saved);
  assert.equal(duplicate.changed, false);
  const moved = applyModelRouteCommand(saved, { kind: "move_target", clientId: "r", from: 2, to: 0 }, context);
  assert.equal(moved.routes[0].targetShareId, "c");
  assert.deepEqual(moved.routes[0].candidates, ["a", "b"]);
  assert.deepEqual(moved.focus, { clientId: "r", shareId: "c", control: "target" });
});
test("restore is fenced by revision and draft fingerprint", () => {
  const draft = saved.map(route => ({ ...route, requestedModel: "changed" }));
  const change = diffModelRoutes(saved, draft)[0];
  const command = { kind: "restore_change" as const, changeId: change.changeId, baseRevision: 1, draftFingerprint: modelRoutesDraftFingerprint(draft) };
  assert.deepEqual(applyModelRouteCommand(draft, command, context).routes, saved);
  assert.equal(applyModelRouteCommand(draft, { ...command, baseRevision: 0 }, context).routes, draft);
});

test("CI registers routing regressions and Rust runtime, metrics and notification coverage", async () => {
  const { readFileSync } = await import("node:fs");
  const packageFile = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
  for (const file of ["model-routing-commands.test.ts", "model-routing-test-session.test.ts", "model-routing-session.test.ts"]) {
    assert.ok(packageFile.scripts["test:model-routing"].includes(file), file);
  }
  const workflow = readFileSync(new URL("../../.github/workflows/build-release.yml", import.meta.url), "utf8");
  for (const filter of ["model_rout", "failover_candidates", "route_breaker", "metrics::tests", "notification_kinds::tests"]) {
    assert.ok(workflow.includes(`cargo test --locked ${filter}`), filter);
  }
});

test("controller lifetime follows authenticated identity while visibility only suspends reads", async () => {
  const { readFileSync } = await import("node:fs");
  const board = readFileSync(new URL("../components/dashboard/client-board.tsx", import.meta.url), "utf8");
  const panel = readFileSync(new URL("../components/dashboard/model-hub-panel.tsx", import.meta.url), "utf8");
  assert.match(board, /hasViewerIdentity \? sessionEmail : ""/);
  assert.match(panel, /\}\), \[viewerKey\]\)/);
  assert.match(panel, /session\.suspend\(\)/);
});

test("model prefixes and temporary duplicate names remain editable; save validates duplicates", async () => {
  const { validateModelRoutes } = await import("./model-routing");
  for (const [existing, desired] of [["gpt-5", "gpt-5-codex"], ["claude-sonnet-4", "claude-sonnet-4-5"]]) {
    let routes = [{ ...saved[0], requestedModel: existing }, { ...saved[0], clientId: "new", requestedModel: "" }];
    for (const character of desired) {
      const value = routes[1].requestedModel + character;
      routes = applyModelRouteCommand(routes, { kind: "set_model", clientId: "new", value }, context).routes;
      assert.equal(routes[1].requestedModel, value);
      if (value === existing) assert.equal(validateModelRoutes(routes), "duplicate");
    }
    assert.equal(routes[1].requestedModel, desired);
    assert.equal(validateModelRoutes(routes), null);
    routes = applyModelRouteCommand(routes, { kind: "set_model", clientId: "new", value: desired + " " }, context).routes;
    assert.equal(routes[1].requestedModel, desired + " ");
  }
});

test("capacity rejects the entire batch and unavailable backups cannot be promoted", () => {
  const many = Array.from({ length: 7 }, (_, index) => ({ ...shares[0], shareId: `s${index}` }));
  const routes = [{ ...saved[0], targetShareId: "s0", candidates: ["s1", "s2", "s3", "s4"] }];
  const ctx = { ...context, eligibleShares: many, savedRoutes: routes };
  assert.equal(applyModelRouteCommand(routes, { kind: "append_targets", clientId: "r", shareIds: ["s5", "s6"] }, ctx).routes, routes);
  const unavailable = { ...context, eligibleShares: shares.filter(share => share.shareId !== "b") };
  assert.equal(applyModelRouteCommand(saved, { kind: "move_target", clientId: "r", from: 1, to: 0 }, unavailable).routes, saved);
});

test("an unavailable saved primary may move down the queue but never back up", async () => {
  const { hasNewUnavailableRouteTarget } = await import("./model-routing");
  const revoked = { ...context, eligibleShares: shares.filter(share => share.shareId !== "a") };
  const demoted = applyModelRouteCommand(saved, { kind: "move_target", clientId: "r", from: 0, to: 2 }, revoked);
  assert.equal(demoted.changed, true);
  assert.equal(demoted.error, undefined);
  assert.equal(demoted.routes[0].targetShareId, "b");
  assert.deepEqual(demoted.routes[0].candidates, ["c", "a"]);
  const profile = { enabled: true, revision: 1, apiBaseUrl: "", eligibleShares: revoked.eligibleShares,
    routes: [{ id: "r", appType: "codex" as const, requestedModel: "model", targetShareId: "a", candidates: ["b", "c"], createdAt: "", updatedAt: "" }] };
  assert.equal(hasNewUnavailableRouteTarget(demoted.routes, profile), false, "save pre-check agrees with the command");
  const promoted = applyModelRouteCommand(demoted.routes, { kind: "move_target", clientId: "r", from: 2, to: 0 }, revoked);
  assert.equal(promoted.error?.code, "target_unavailable");
});

test("clearing defaults removes the rule; clearing exact targets retains an invalid draft", () => {
  assert.equal(applyModelRouteCommand(saved, { kind: "remove_targets", clientId: "r", shareIds: ["a", "b", "c"] }, context).routes.length, 1);
  const defaults = [{ ...saved[0], requestedModel: "*" }];
  assert.equal(applyModelRouteCommand(defaults, { kind: "remove_targets", clientId: "r", shareIds: ["a", "b", "c"] }, { ...context, savedRoutes: defaults }).routes.length, 0);
});

test("Unicode limits count code points and unreliable model suggestions are empty", async () => {
  const { validateModelRoutes, modelRoutingSuggestions } = await import("./model-routing");
  assert.equal(validateModelRoutes([{ ...saved[0], requestedModel: "😀".repeat(200) }]), null);
  assert.ok(validateModelRoutes([{ ...saved[0], requestedModel: "😀".repeat(201) }]));
  assert.deepEqual(modelRoutingSuggestions({ enabled: true, revision: 1, apiBaseUrl: "", routes: [], eligibleShares: shares }, "codex", saved), []);
});

test("model editing remains possible when a saved target loses eligibility", () => {
  const result = applyModelRouteCommand(saved, { kind: "set_model", clientId: "r", value: "new-name " }, { ...context, eligibleShares: [] });
  assert.equal(result.routes[0].requestedModel, "new-name ");
  assert.equal(result.error, undefined);
});

test("removing and restoring rules provide a stable focus destination", () => {
  const routes = [...saved, { ...saved[0], clientId: "next", requestedModel: "next-model" }];
  const ctx = { ...context, savedRoutes: routes };
  const removed = applyModelRouteCommand(routes, { kind: "remove_rule", clientId: "r" }, ctx);
  assert.deepEqual(removed.focus, { clientId: "next", control: "model" });
  const change = diffModelRoutes(routes, removed.routes)[0];
  const restored = applyModelRouteCommand(removed.routes, { kind: "restore_change", changeId: change.changeId, baseRevision: 1,
    draftFingerprint: modelRoutesDraftFingerprint(removed.routes) }, ctx);
  assert.deepEqual(restored.routes, routes);
  assert.equal(restored.focus?.clientId, "r");
  assert.equal(restored.focus?.control, "model");
  const last = applyModelRouteCommand(saved, { kind: "remove_rule", clientId: "r" }, context);
  assert.equal(last.focus?.control, "add");
});

test("production transfer events use edit commands and focus targets have DOM anchors", async () => {
  const { readFileSync } = await import("node:fs");
  const picker = readFileSync(new URL("../components/dashboard/model-hub-share-picker.tsx", import.meta.url), "utf8");
  const editor = readFileSync(new URL("../components/dashboard/model-hub-rule-editor.tsx", import.meta.url), "utf8");
  const panel = readFileSync(new URL("../components/dashboard/model-hub-panel.tsx", import.meta.url), "utf8");
  assert.match(picker, /onAction\(\{ kind: direction === "in" \? "append_targets" : "remove_targets"/);
  assert.match(picker, /onAction\(\{ kind: "move_target", from, to \}\)/);
  assert.match(editor, /onAction=\{onAction\}/);
  // Transfer owns focus after moving rows, including the first default rule.
  assert.match(panel, /controller\.command\(\{\s*\.\.\.action,\s*clientId:\s*route\.clientId\s*\},\s*\{\s*focus:\s*false\s*\}\)/);
  assert.match(panel, /controller\.command\(\{\s*\.\.\.action,\s*clientId:\s*activeSlot\.passthrough\.clientId\s*\},\s*\{\s*focus:\s*false\s*\}\)/);
  assert.match(panel, /controller\.setPassthroughTargets\(activeProtocol,\s*action\.shareIds,\s*\{\s*focus:\s*false\s*\}\)/);
  assert.match(picker, /data-model-route-target=\{targetId\}/);
  assert.match(panel, /node\.dataset\.modelRouteTarget === focus\.shareId/);
  assert.match(panel, /lastFocusRequestRef\.current\.request !== forceExpanded/);
});
