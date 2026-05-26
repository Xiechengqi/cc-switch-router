"use client";

import { Card, Chip } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DiskUsage, HostMetricsInfo, MetricsHealth, MetricsSnapshot } from "@/lib/types";
import { formatBytes, formatNumber, percent } from "@/lib/utils";
import { diskPercent, type MetricTone } from "./metrics-utils";

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

export function MetricKpiCard({
  label,
  value,
  detail,
  icon,
  tone = "default",
}: {
  label: string;
  value: React.ReactNode;
  detail: React.ReactNode;
  icon: React.ReactNode;
  tone?: MetricTone;
}) {
  const toneClass =
    tone === "critical"
      ? "border-red-300 bg-red-50/60"
      : tone === "warning"
        ? "border-amber-300 bg-amber-50/60"
        : "";
  return (
    <Card
      className={`group h-full rounded-xl transition-all duration-200 hover:-translate-y-0.5 hover:shadow-lg ${toneClass}`}
    >
      <Card.Content className="flex h-full items-center justify-between gap-3 p-4">
        <div className="min-w-0">
          <p className="text-sm text-muted-foreground">{label}</p>
          <p className="mt-1 truncate text-lg font-semibold">{value}</p>
          <p className="mt-1 truncate text-xs text-muted-foreground">{detail}</p>
        </div>
        <div className="rounded-lg bg-muted p-2.5 text-muted-foreground transition-transform duration-200 group-hover:scale-110 [&>svg]:h-5 [&>svg]:w-5">
          {icon}
        </div>
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
  const rows: Array<[string, React.ReactNode]> = [
    [t("metrics.hostinfo.hostname"), info?.hostname || "-"],
    [t("metrics.hostinfo.os"), `${info?.osName || "-"} ${info?.osVersion || ""}`],
    [t("metrics.hostinfo.kernel"), info?.kernelVersion || "-"],
    [t("metrics.hostinfo.cpu"), info?.cpuBrand || "-"],
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
  return (
    <Chip color={color} size="sm" variant="soft">
      {status}
    </Chip>
  );
}
