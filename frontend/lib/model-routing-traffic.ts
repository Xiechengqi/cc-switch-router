// Plan P5-1: pure helpers for the Router's bounded record of unified-API route
// executions. Everything shown is a fact the Router observed; nothing here
// predicts where a future request will land.
import type { ModelRouteTrafficEvent, ModelRouteTrafficSnapshot, ModelRoutingApp } from "@/lib/types";

export const TRAFFIC_WINDOW_MS = 15 * 60_000;
export const TRAFFIC_CAPACITY = 200;
export const TRAFFIC_BUCKETS = 15;

export type ModelRouteTrafficState = {
  instanceId: string;
  cursor: string;
  /** Newest first. */
  events: ModelRouteTrafficEvent[];
};

export const EMPTY_TRAFFIC: ModelRouteTrafficState = { instanceId: "", cursor: "", events: [] };

/** An older Router serves its web app for unknown paths; only real JSON counts. */
export function isModelRouteTrafficSnapshot(value: unknown): value is ModelRouteTrafficSnapshot {
  const candidate = value as Partial<ModelRouteTrafficSnapshot> | null;
  return !!candidate && typeof candidate.instanceId === "string" && !!candidate.instanceId
    && typeof candidate.cursor === "string" && typeof candidate.reset === "boolean" && Array.isArray(candidate.events);
}

/** Decimal sequence strings compare by length first, so no BigInt is needed. */
function compareSequenceDesc(a: ModelRouteTrafficEvent, b: ModelRouteTrafficEvent) {
  return b.sequence.length - a.sequence.length || (b.sequence > a.sequence ? 1 : b.sequence < a.sequence ? -1 : 0);
}

export function trafficEnded(event: ModelRouteTrafficEvent) {
  return event.phase === "complete" || event.phase === "interrupted";
}

function startedMs(event: ModelRouteTrafficEvent) {
  const value = Date.parse(event.startedAt);
  return Number.isFinite(value) ? value : 0;
}

export function pruneTraffic(events: ModelRouteTrafficEvent[], nowMs: number) {
  return events
    .filter(event => !trafficEnded(event) || startedMs(event) + (event.durationMs || 0) > nowMs - TRAFFIC_WINDOW_MS)
    .sort(compareSequenceDesc)
    .slice(0, TRAFFIC_CAPACITY);
}

export function applyTrafficSnapshot(state: ModelRouteTrafficState, snapshot: ModelRouteTrafficSnapshot, nowMs: number): ModelRouteTrafficState {
  const replace = snapshot.reset || snapshot.instanceId !== state.instanceId;
  const merged = new Map((replace ? [] : state.events).map(event => [event.sequence, event]));
  for (const event of snapshot.events) merged.set(event.sequence, event);
  return { instanceId: snapshot.instanceId, cursor: snapshot.cursor, events: pruneTraffic([...merged.values()], nowMs) };
}

export function trafficRuleKind(event: ModelRouteTrafficEvent): "default" | "exact" | "unmatched" {
  return !event.routeId ? "unmatched" : event.matchedWildcard ? "default" : "exact";
}

const is2xx = (status?: number) => !!status && status >= 200 && status < 300;

export type ProtocolTrafficSummary = { total: number; ended: number; succeeded: number; continued: number; inflight: number };

export function protocolTrafficSummary(events: ModelRouteTrafficEvent[], app: ModelRoutingApp): ProtocolTrafficSummary {
  const summary = { total: 0, ended: 0, succeeded: 0, continued: 0, inflight: 0 };
  for (const event of events) {
    if (event.appType !== app) continue;
    summary.total += 1;
    if (event.continuedSearch) summary.continued += 1;
    if (!trafficEnded(event)) { summary.inflight += 1; continue; }
    summary.ended += 1;
    if (event.phase === "complete" && is2xx(event.statusCode)) summary.succeeded += 1;
  }
  return summary;
}

export type ShareRouteTraffic = {
  /** Attempts that actually reached this Share and got a response. */
  forwarded: number;
  failed: number;
  /** Requests that ended on this Share after an earlier forwarded attempt. */
  landedAfterFailure: number;
  /** Forwards per minute, oldest first; the last bucket is the current minute. */
  buckets: number[];
};

export function routeShareTraffic(events: ModelRouteTrafficEvent[], routeId: string, nowMs: number) {
  const byShare = new Map<string, ShareRouteTraffic>();
  const entry = (shareId: string) => {
    let stats = byShare.get(shareId);
    if (!stats) byShare.set(shareId, stats = { forwarded: 0, failed: 0, landedAfterFailure: 0, buckets: Array(TRAFFIC_BUCKETS).fill(0) });
    return stats;
  };
  for (const event of events) {
    if (event.routeId !== routeId) continue;
    const age = Math.floor((nowMs - startedMs(event)) / 60_000);
    const bucket = TRAFFIC_BUCKETS - 1 - Math.max(0, age);
    const forwarded = event.attempts.filter(attempt => attempt.outcome === "response");
    for (const attempt of forwarded) {
      const stats = entry(attempt.targetShareId);
      stats.forwarded += 1;
      if (!is2xx(attempt.statusCode)) stats.failed += 1;
      if (bucket >= 0) stats.buckets[bucket] += 1;
    }
    const landed = event.targetShareId;
    if (landed && forwarded.length > 1 && forwarded[0].targetShareId !== landed) entry(landed).landedAfterFailure += 1;
  }
  return byShare;
}
