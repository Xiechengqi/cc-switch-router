"use client";

import { Card, Chip } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DiskUsage, HostMetricsInfo, MetricsHealth, MetricsSnapshot } from "@/lib/types";
import { compactTokens, formatBytes, fixed, formatNumber, percent } from "@/lib/utils";
import { diskPercent, type MetricTone, polyline } from "./metrics-utils";

export function KpiGrid({ children }: { children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
      {React.Children.map(children, (child, index) => (
        <div className="h-full animate-fade-in-up" style={{ animationDelay: `${index * 60}ms` }}>
          {child}
        </div>
      ))}
    </div>
  );
}

function Sparkline({ values, tone }: { values: number[]; tone: MetricTone }) {
  const clean = values.filter((v) => Number.isFinite(v));
  if (clean.length < 2) return <div className="h-6" />;
  const max = Math.max(1, ...clean);
  const stroke = tone === "critical" ? "#EF4444" : tone === "warning" ? "#F59E0B" : "#0052FF";
  return (
    <svg viewBox="0 0 120 24" preserveAspectRatio="none" className="h-6 w-full">
      <polyline
        fill="none"
        stroke={stroke}
        strokeWidth="1.5"
        strokeLinejoin="round"
        points={polyline(clean, 120, 24, max)}
      />
    </svg>
  );
}

export function MetricKpiCard({
  label,
  value,
  detail,
  icon,
  tone = "default",
  spark,
  emphasize = false,
}: {
  label: string;
  value: React.ReactNode;
  detail: React.ReactNode;
  icon: React.ReactNode;
  tone?: MetricTone;
  spark?: number[];
  emphasize?: boolean;
}) {
  const accent =
    tone === "critical"
      ? "before:bg-red-500"
      : tone === "warning"
        ? "before:bg-amber-500"
        : "before:bg-gradient-to-b before:from-[var(--accent,#0052FF)] before:to-[#4D7CFF]";
  const glow = tone === "critical" ? "after:bg-red-500/[0.06]" : "after:bg-transparent";
  return (
    <Card
      className={`group relative h-full overflow-hidden rounded-2xl pl-1 transition-all duration-200 hover:-translate-y-0.5 hover:shadow-lg before:absolute before:inset-y-3 before:left-0 before:w-1 before:rounded-full ${accent} after:pointer-events-none after:absolute after:inset-0 after:blur-2xl ${glow}`}
    >
      <Card.Content className="flex h-full flex-col justify-between gap-2 p-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-sm text-muted-foreground">{label}</p>
            <p className={`mt-1 truncate font-semibold ${emphasize ? "gradient-text text-2xl" : "text-lg"}`}>
              {value}
            </p>
            <p className="mt-1 truncate text-xs text-muted-foreground">{detail}</p>
          </div>
          <div className="rounded-lg bg-muted p-2.5 text-muted-foreground transition-transform duration-200 group-hover:scale-110 [&>svg]:h-5 [&>svg]:w-5">
            {icon}
          </div>
        </div>
        {spark && spark.length > 1 ? <Sparkline values={spark} tone={tone} /> : null}
      </Card.Content>
    </Card>
  );
}

export function ProcessPanel({ host }: { host?: MetricsSnapshot["host"] }) {
  const { t } = useLocaleText();
  const rows: Array<[string, React.ReactNode, React.ReactNode]> = [
    [
      t("metrics.process.openFds"),
      `${formatNumber(host?.process.openFds)} / ${formatNumber(host?.process.maxFds)}`,
      percent(host?.process.fdUsagePercent),
    ],
    [t("metrics.process.threads"), formatNumber(host?.process.threads), ""],
    [t("metrics.process.rss"), formatBytes(host?.process.rssBytes), ""],
    [t("metrics.process.cpu"), percent(host?.process.cpuPercent), ""],
  ];
  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{t("metrics.panel.routerProcess")}</Card.Title>
      </Card.Header>
      <Card.Content className="grid gap-3">
        {rows.map(([label, value, extra]) => (
          <div
            key={label}
            className="flex items-center justify-between rounded-lg border p-2.5 text-sm"
          >
            <span className="text-muted-foreground">{label}</span>
            <span className="font-medium">
              {value} {extra}
            </span>
          </div>
        ))}
      </Card.Content>
    </Card>
  );
}

export function DiskUsageList({ disks }: { disks: DiskUsage[] }) {
  const { t } = useLocaleText();
  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{t("metrics.panel.diskUsage")}</Card.Title>
      </Card.Header>
      <Card.Content className="grid gap-3">
        {disks.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t("metrics.panel.noDiskData")}</p>
        ) : (
          disks.map((disk) => {
            const pct = diskPercent(disk) || 0;
            const barColor = pct >= 90 ? "bg-red-500" : pct >= 80 ? "bg-amber-500" : "bg-emerald-500";
            return (
              <div key={`${disk.label}-${disk.mountPoint}`} className="rounded-lg border p-3">
                <div className="flex items-center justify-between text-sm">
                  <span className="font-medium">{disk.label}</span>
                  <span>{percent(pct)}</span>
                </div>
                <p className="mt-1 text-xs text-muted-foreground">
                  {disk.mountPoint} · {formatBytes(disk.usedBytes)} / {formatBytes(disk.totalBytes)}
                </p>
                <div className="mt-2 h-2 rounded-full bg-muted">
                  <div
                    className={`h-2 rounded-full ${barColor}`}
                    style={{ width: `${Math.min(100, pct)}%` }}
                  />
                </div>
              </div>
            );
          })
        )}
      </Card.Content>
    </Card>
  );
}

export function HostInfoPanel({
  info,
  host,
}: {
  info: HostMetricsInfo | null;
  host?: MetricsSnapshot["host"];
}) {
  const { t } = useLocaleText();
  const swapText =
    host?.swapTotalBytes && host.swapTotalBytes > 0
      ? `${formatBytes(host?.swapUsedBytes)} / ${formatBytes(host?.swapTotalBytes)}`
      : t("metrics.hostinfo.noSwap");
  const rows: Array<[string, React.ReactNode]> = [
    [t("metrics.hostinfo.hostname"), info?.hostname || "-"],
    [t("metrics.hostinfo.os"), `${info?.osName || "-"} ${info?.osVersion || ""}`],
    [t("metrics.hostinfo.kernel"), info?.kernelVersion || "-"],
    [t("metrics.hostinfo.cpu"), info?.cpuBrand || "-"],
    [
      t("metrics.hostinfo.load"),
      `${fixed(host?.load1)} · ${fixed(host?.load5)} · ${fixed(host?.load15)}`,
    ],
    [t("metrics.hostinfo.swap"), swapText],
    [
      t("metrics.hostinfo.tcp"),
      t("metrics.hostinfo.tcpValue", {
        established: formatNumber(host?.network.tcpEstablished),
        timeWait: formatNumber(host?.network.tcpTimeWait),
      }),
    ],
  ];
  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{t("metrics.panel.hostInfo")}</Card.Title>
      </Card.Header>
      <Card.Content className="grid gap-3">
        {rows.map(([k, v]) => (
          <div key={k} className="rounded-lg bg-muted p-2.5">
            <p className="text-xs text-muted-foreground">{k}</p>
            <p className="mt-1 text-sm font-medium">{v}</p>
          </div>
        ))}
      </Card.Content>
    </Card>
  );
}

export function StatusChip({ status }: { status: MetricsHealth | string }) {
  const color = status === "critical" ? "danger" : status === "warning" ? "warning" : "success";
  const dot = status === "critical" ? "bg-red-500" : status === "warning" ? "bg-amber-500" : "bg-emerald-500";
  const pulse = status === "critical" || status === "warning";
  return (
    <Chip color={color} size="sm" variant="soft">
      <span className="mr-1.5 inline-flex h-2 w-2">
        {pulse ? (
          <span className={`absolute inline-flex h-2 w-2 animate-ping rounded-full ${dot} opacity-70`} />
        ) : null}
        <span className={`relative inline-flex h-2 w-2 rounded-full ${dot}`} />
      </span>
      {status}
    </Chip>
  );
}

/**
 * Uses current snapshot values, not historical totals, so it stays truthful
 * without a separate aggregate query.
 */
export function LiveSummaryStrip({ snapshot }: { snapshot: MetricsSnapshot | null }) {
  const { t } = useLocaleText();
  const llm = snapshot?.llm;
  const items: Array<[string, React.ReactNode, string]> = [
    [t("metrics.kpi.rpm"), fixed(llm?.rpm), t("metrics.detail.requestsPerMin")],
    [t("metrics.kpi.tpm"), compactTokens(llm?.tpm), t("metrics.live.tokensPerMin")],
    [
      t("metrics.kpi.errorRate"),
      percent((llm?.errorRate || 0) * 100),
      t("metrics.live.last5m"),
    ],
    [
      t("metrics.live.shares"),
      formatNumber(llm?.activeShares),
      t("metrics.live.activeModels", { count: formatNumber(llm?.activeModels) }),
    ],
  ];
  return (
    <div className="rounded-lg border bg-card px-6 py-5 text-foreground shadow-sm">
      <div className="grid grid-cols-2 gap-6 md:grid-cols-4">
        {items.map(([label, value, hint]) => (
          <div key={label}>
            <p className="font-mono text-[10px] uppercase tracking-[0.15em] text-muted-foreground">{label}</p>
            <p className="mt-1 font-display text-3xl leading-none">{value}</p>
            <p className="mt-1 text-xs text-muted-foreground">{hint}</p>
          </div>
        ))}
      </div>
    </div>
  );
}
