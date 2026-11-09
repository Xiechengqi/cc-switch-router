import assert from "node:assert/strict";
import test from "node:test";
import {
  applyTrafficSnapshot,
  EMPTY_TRAFFIC,
  isModelRouteTrafficSnapshot,
  protocolTrafficSummary,
  pruneTraffic,
  routeShareTraffic,
  trafficRuleKind,
  TRAFFIC_BUCKETS,
  TRAFFIC_CAPACITY,
  TRAFFIC_WINDOW_MS,
} from "./model-routing-traffic";
import type { ModelRouteTrafficEvent, ModelRouteTrafficSnapshot } from "./types";

const now = Date.parse("2026-11-06T12:00:00Z");
function event(sequence: number, patch: Partial<ModelRouteTrafficEvent> = {}): ModelRouteTrafficEvent {
  return {
    sequence: String(sequence), appType: "codex", requestedModel: "gpt-test", routeId: "r", revision: 1,
    matchedWildcard: false, startedAt: new Date(now - 30_000).toISOString(), phase: "complete", statusCode: 200,
    targetShareId: "a", position: 0, continuedSearch: false, durationMs: 1_000,
    attempts: [{ targetShareId: "a", position: 0, outcome: "response", statusCode: 200, durationMs: 900 }],
    ...patch,
  };
}
function snapshot(events: ModelRouteTrafficEvent[], patch: Partial<ModelRouteTrafficSnapshot> = {}): ModelRouteTrafficSnapshot {
  return { instanceId: "i1", cursor: "10", reset: false, windowSeconds: 900, perUserCapacity: 200, events, ...patch };
}

test("only real JSON snapshots are accepted; an older Router's web app is not", () => {
  assert.equal(isModelRouteTrafficSnapshot(snapshot([])), true);
  assert.equal(isModelRouteTrafficSnapshot({}), false);
  assert.equal(isModelRouteTrafficSnapshot(null), false);
  assert.equal(isModelRouteTrafficSnapshot({ ...snapshot([]), instanceId: "" }), false);
});

test("snapshots merge by sequence, replace on reset or a new process, and sort numerically", () => {
  let state = applyTrafficSnapshot(EMPTY_TRAFFIC, snapshot([event(9), event(10, { phase: "streaming" })], { reset: true }), now);
  assert.deepEqual(state.events.map(item => item.sequence), ["10", "9"]);
  state = applyTrafficSnapshot(state, snapshot([event(10, { phase: "complete" }), event(100)], { cursor: "12" }), now);
  assert.deepEqual(state.events.map(item => item.sequence), ["100", "10", "9"], "100 sorts above 10 and 9");
  assert.equal(state.events[1].phase, "complete", "a phase change replaces the earlier copy");
  assert.equal(state.cursor, "12");
  assert.deepEqual(applyTrafficSnapshot(state, snapshot([event(1)], { reset: true }), now).events.map(item => item.sequence), ["1"]);
  assert.deepEqual(applyTrafficSnapshot(state, snapshot([event(2)], { instanceId: "restarted" }), now).events.map(item => item.sequence), ["2"]);
});

test("pruning keeps in-progress events, drops ended ones past the window and caps the count", () => {
  const old = new Date(now - TRAFFIC_WINDOW_MS - 60_000).toISOString();
  const kept = pruneTraffic([event(1, { startedAt: old }), event(2, { startedAt: old, phase: "streaming" }), event(3)], now);
  assert.deepEqual(kept.map(item => item.sequence), ["3", "2"]);
  const many = Array.from({ length: TRAFFIC_CAPACITY + 5 }, (_, index) => event(index + 1));
  const capped = pruneTraffic(many, now);
  assert.equal(capped.length, TRAFFIC_CAPACITY);
  assert.equal(capped[0].sequence, String(TRAFFIC_CAPACITY + 5));
});

test("protocol summary counts only facts the Router recorded", () => {
  const summary = protocolTrafficSummary([
    event(1), event(2, { statusCode: 429, continuedSearch: true }), event(3, { phase: "streaming" }),
    event(4, { phase: "interrupted", statusCode: 200 }), event(5, { appType: "claude" }),
  ], "codex");
  assert.deepEqual(summary, { total: 4, ended: 3, succeeded: 1, continued: 1, inflight: 1 },
    "an interrupted 200 is not counted as a 2xx completion");
});

test("rule kinds come from the event, never from current configuration", () => {
  assert.equal(trafficRuleKind(event(1)), "exact");
  assert.equal(trafficRuleKind(event(1, { matchedWildcard: true })), "default");
  assert.equal(trafficRuleKind(event(1, { routeId: undefined, errorCode: "user_model_route_not_configured" })), "unmatched");
});

test("queue counters attribute forwards, failures and landings per Share and minute", () => {
  const failover = event(2, {
    startedAt: new Date(now - 3 * 60_000).toISOString(), targetShareId: "b", position: 1, continuedSearch: true,
    attempts: [
      { targetShareId: "a", position: 0, outcome: "response", statusCode: 429, durationMs: 10 },
      { targetShareId: "b", position: 1, outcome: "response", statusCode: 200, durationMs: 20 },
    ],
  });
  const skipped = event(3, {
    targetShareId: "b", position: 1,
    attempts: [
      { targetShareId: "a", position: 0, outcome: "breaker_open", durationMs: 0 },
      { targetShareId: "b", position: 1, outcome: "response", statusCode: 200, durationMs: 20 },
    ],
  });
  const stats = routeShareTraffic([event(1), failover, skipped, event(4, { routeId: "other" })], "r", now);
  assert.deepEqual({ ...stats.get("a")!, buckets: undefined }, { forwarded: 2, failed: 1, landedAfterFailure: 0, buckets: undefined });
  assert.deepEqual({ ...stats.get("b")!, buckets: undefined }, { forwarded: 2, failed: 0, landedAfterFailure: 1, buckets: undefined },
    "a breaker skip is not a forwarded failure, so that landing does not count as after-failure");
  assert.equal(stats.get("a")!.buckets.length, TRAFFIC_BUCKETS);
  assert.equal(stats.get("a")!.buckets[TRAFFIC_BUCKETS - 1], 1, "the current minute holds the 30-second-old request");
  assert.equal(stats.get("a")!.buckets[TRAFFIC_BUCKETS - 4], 1, "the three-minute-old request lands three buckets back");
});
