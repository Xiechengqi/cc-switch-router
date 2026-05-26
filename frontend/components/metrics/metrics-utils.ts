import type { DiskUsage, MetricEvent, MetricsSnapshot } from "@/lib/types";

export type MetricTone = "default" | "warning" | "critical";

export type ChartSeries = {
  label: string;
  color: string;
  values: number[];
};

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
