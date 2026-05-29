import type { DiskUsage, MetricEvent, MetricsSnapshot } from "@/lib/types";

export type MetricTone = "default" | "warning" | "critical";

export type ChartSeries = {
  label: string;
  color: string;
  values: number[];
};

/**
 * Derives the base "is the pipeline healthy" state for charts from the
 * snapshot. The chart itself decides no-data vs no-traffic from the series;
 * this only covers states that the series alone cannot express: loading,
 * collection disabled, and a collector that appears to have stopped.
 */
export function pipelineState(
  snapshot: MetricsSnapshot | null,
  loading: boolean,
): "loading" | "disabled" | "stale" | "ready" {
  if (loading && !snapshot) return "loading";
  if (snapshot && snapshot.enabled === false) return "disabled";
  if (snapshot?.lastPersistedAt) {
    const ageSecs = Date.now() / 1000 - snapshot.lastPersistedAt;
    const interval = Math.max(snapshot.sampleIntervalSecs || 5, 1);
    if (ageSecs > Math.max(interval * 6, 60)) return "stale";
  }
  return "ready";
}

export function memoryPercent(host?: MetricsSnapshot["host"]) {
  if (!host?.memoryTotalBytes || !host.memoryUsedBytes) return undefined;
  return (host.memoryUsedBytes * 100) / host.memoryTotalBytes;
}

export function diskPercent(disk?: DiskUsage) {
  if (!disk?.totalBytes) return undefined;
  return (disk.usedBytes * 100) / disk.totalBytes;
}

export function toneFor(value: unknown, warning: number, critical: number): MetricTone {
  const n = Number(value);
  if (!Number.isFinite(n)) return "default";
  if (n >= critical) return "critical";
  if (n >= warning) return "warning";
  return "default";
}

export function polyline(values: number[], width: number, height: number, max: number) {
  return values
    .map((value, index) => {
      const x = (index / Math.max(values.length - 1, 1)) * width;
      const y = height - (Math.max(0, value) / max) * height;
      return `${x},${Math.max(0, Math.min(height, y))}`;
    })
    .join(" ");
}

/**
 * Converts a monotonic counter series (e.g. `*_total`) into per-bucket deltas
 * so charts show the rate of new events rather than an ever-rising staircase.
 * Counter resets (process restart) clamp to 0 instead of going negative.
 */
export function deltaSeries(values: number[] | undefined): number[] {
  if (!values || values.length === 0) return [];
  const out: number[] = [0];
  for (let i = 1; i < values.length; i += 1) {
    const diff = values[i] - values[i - 1];
    out.push(diff > 0 ? diff : 0);
  }
  return out;
}

// The router computes live threshold alerts into `snapshot.alerts` but does not
// persist them to the metric_events log, so merge both so the alerts panel stays
// consistent with the health status chip.
export function mergeMetricEvents(
  live: MetricEvent[] | undefined,
  persisted: MetricEvent[],
): MetricEvent[] {
  const seen = new Set<string>();
  const out: MetricEvent[] = [];
  for (const event of [...(live || []), ...persisted]) {
    const key = `${event.timestamp}-${event.kind}-${event.severity}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(event);
  }
  return out.sort((a, b) => b.timestamp - a.timestamp);
}
