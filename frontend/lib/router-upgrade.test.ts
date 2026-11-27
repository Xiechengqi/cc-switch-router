import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { isRouterUpgradeTerminal, mergeRouterUpgradeLogs, mergeRouterUpgradeTask, routerUpgradeDismissedKey } from "./router-upgrade";
import type { RouterUpgradeLog, RouterUpgradeTask } from "./types";

function task(id: string, revision = 1, startedAt = "2026-11-27T00:00:00Z"): RouterUpgradeTask {
  return { schemaVersion: 1, taskId: id, revision, status: "running", phase: "downloading", canCancel: true,
    actor: "admin@example.test", sourceInstanceId: "old", target: null, targetSha256: null,
    binaryReplaced: false, restartConfirmed: false, startedAt, updatedAt: startedAt, finishedAt: null,
    downloadedBytes: 0, totalBytes: null, progress: null, error: null, persistenceError: false,
    lastSeq: 0, logsTruncated: false, logs: [] };
}

function log(taskId: string, seq: number): RouterUpgradeLog {
  return { taskId, seq, step: 2, totalSteps: 7, level: "info", message: String(seq), progress: null, at: "2026-11-27T00:00:00Z" };
}

test("replayed logs are deduplicated, ordered, scoped and bounded", () => {
  const lines = mergeRouterUpgradeLogs([log("a", 2)], [log("a", 1), log("a", 2), log("b", 3)], "a");
  assert.deepEqual(lines.map((line) => line.seq), [1, 2]);
  const bounded = mergeRouterUpgradeLogs([], Array.from({ length: 1010 }, (_, index) => log("a", index + 1)), "a");
  assert.equal(bounded.length, 1000);
  assert.equal(bounded[0].seq, 11);
});

test("an older status response cannot regress a completed SSE state", () => {
  const completed = { ...task("a", 5), status: "cancelled" as const, canCancel: false, logs: [log("a", 2)] };
  const latePoll = { ...task("a", 3), logs: [log("a", 1)] };
  const merged = mergeRouterUpgradeTask(completed, latePoll);
  assert.equal(merged.status, "cancelled");
  assert.equal(merged.canCancel, false);
  assert.deepEqual(merged.logs.map((line) => line.seq), [1, 2]);
});

test("a late previous-task response cannot replace the next upgrade", () => {
  const newer = task("new", 1, "2026-11-27T00:01:00Z");
  assert.equal(mergeRouterUpgradeTask(newer, task("old", 99)).taskId, "new");
  assert.equal(mergeRouterUpgradeTask(task("old"), newer).taskId, "new");
});

test("only final results can be dismissed and dismissal scopes are per account", () => {
  for (const status of ["running", "restarting", "cancelling"] as const) assert.equal(isRouterUpgradeTerminal(status), false);
  for (const status of ["success", "failed", "cancelled", "interrupted"] as const) assert.equal(isRouterUpgradeTerminal(status), true);
  assert.notEqual(routerUpgradeDismissedKey("admin-a"), routerUpgradeDismissedKey("admin-b"));
});

const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");

test("a token rotation without a login epoch restarts the subscription and keeps the task", () => {
  const hook = read("../components/settings/use-router-upgrade.ts");
  assert.match(hook, /auth\.loginEpoch \? "" : `:\$\{auth\.accessToken \|\| ""\}:\$\{auth\.refreshToken \|\| ""\}`/);
  assert.match(hook, /let latest: RouterUpgradeTask \| null = carried\.current\.owner === owner \? carried\.current\.task : null;/);
});

test("a rejected start or cancel is an action error, not a status outage", () => {
  const hook = read("../components/settings/use-router-upgrade.ts");
  assert.doesNotMatch(hook, /actionError: [^}]*unavailable: true/);
});

test("status blips never trap the restart and rollback confirm dialog", () => {
  const panel = read("../components/settings/version-panel.tsx");
  assert.match(panel, /busy=\{!!busy\}/);
  assert.doesNotMatch(panel, /busy=\{serviceLocked\}/);
});

test("the progress panel uses the dashboard's h-9 controls", () => {
  const panel = read("../components/settings/upgrade-progress-panel.tsx");
  assert.doesNotMatch(panel, /min-h-11/);
});
