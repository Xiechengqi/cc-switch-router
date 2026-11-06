"use client";

import * as React from "react";
import { ApiError, getUserModelRoutingTraffic } from "@/lib/api";
import { applyTrafficSnapshot, EMPTY_TRAFFIC, isModelRouteTrafficSnapshot, pruneTraffic, type ModelRouteTrafficState } from "@/lib/model-routing-traffic";

const POLL_MS = 3_000;
const BACKOFF_MS = 15_000;

export type ModelRoutingTrafficStatus = "idle" | "ready" | "error" | "unsupported";

/** Polls only while `enabled` and the page is visible. Data belongs to one
 * viewer: switching accounts drops it and fences any in-flight response. */
export function useModelRoutingTraffic(enabled: boolean, viewerKey: string) {
  const [traffic, setTraffic] = React.useState<ModelRouteTrafficState>(EMPTY_TRAFFIC);
  const [status, setStatus] = React.useState<ModelRoutingTrafficStatus>("idle");
  const trafficRef = React.useRef(traffic);
  trafficRef.current = traffic;

  React.useEffect(() => {
    // Runs before the polling effect below, so its first read for the new
    // viewer cannot reuse the previous viewer's cursor.
    trafficRef.current = EMPTY_TRAFFIC;
    setTraffic(EMPTY_TRAFFIC);
    setStatus("idle");
  }, [viewerKey]);

  React.useEffect(() => {
    if (!enabled || status === "unsupported") return;
    let cancelled = false;
    let timer = 0;
    let controller: AbortController | null = null;
    const schedule = (delay: number) => {
      window.clearTimeout(timer);
      if (!cancelled && document.visibilityState === "visible") timer = window.setTimeout(poll, delay);
    };
    const poll = async () => {
      controller?.abort();
      controller = new AbortController();
      const { cursor, instanceId } = trafficRef.current;
      try {
        const snapshot = await getUserModelRoutingTraffic({ after: cursor || undefined, instance: instanceId || undefined }, controller.signal);
        if (cancelled) return;
        if (!isModelRouteTrafficSnapshot(snapshot)) { setStatus("unsupported"); return; }
        setTraffic(current => applyTrafficSnapshot(current, snapshot, Date.now()));
        setStatus("ready");
        schedule(POLL_MS);
      } catch (error) {
        if (cancelled || (error instanceof DOMException && error.name === "AbortError")) return;
        if (error instanceof ApiError && (error.status === 404 || error.status === 405)) { setStatus("unsupported"); return; }
        setStatus("error");
        schedule(BACKOFF_MS);
      }
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") schedule(0);
      else { window.clearTimeout(timer); controller?.abort(); }
    };
    schedule(0);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      controller?.abort();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [enabled, viewerKey, status === "unsupported"]);

  // Ended events age out of the window even when no poll brings news.
  React.useEffect(() => {
    if (!enabled) return;
    const timer = window.setInterval(() => setTraffic(current => {
      const events = pruneTraffic(current.events, Date.now());
      return events.length === current.events.length ? current : { ...current, events };
    }), 30_000);
    return () => window.clearInterval(timer);
  }, [enabled]);

  return { traffic, status };
}
