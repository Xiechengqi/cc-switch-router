"use client";

import * as React from "react";
import { AlertTriangle, CheckCircle2, CircleOff, Clock3, PauseCircle, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardClient, DashboardMarket, OperationalReason, OperationalState, OperationalSummary, ShareView } from "@/lib/types";
import { formatDateTime, formatRelativeTime } from "@/lib/utils";

export function shareIsEnabled(share: ShareView) {
  return String(share.shareStatus || "").trim().toLowerCase() === "active";
}

export function summarizeShareAvailability(shares: ShareView[]) {
  const enabledShares = shares.filter(shareIsEnabled);
  let availableCount = 0;
  let degradedCount = 0;
  let reconnectingCount = 0;
  let offlineCount = 0;
  let routeOnlineCount = 0;

  for (const share of enabledShares) {
    if (share.isOnline) routeOnlineCount += 1;
    const state = shareOperationalSummary(share).state;
    if (state === "online") availableCount += 1;
    else if (state === "reconnecting") reconnectingCount += 1;
    else if (state === "degraded") degradedCount += 1;
    else if (state === "offline") offlineCount += 1;
  }

  return {
    enabledCount: enabledShares.length,
    availableCount,
    degradedCount,
    reconnectingCount,
    offlineCount,
    routeOnlineCount,
    issueCount: enabledShares.length - availableCount,
  };
}

function fallbackShareSummary(share: ShareView): OperationalSummary {
  const status = String(share.shareStatus || "").trim().toLowerCase();
  if (status !== "active") {
    return {
      state: "disabled",
      primaryReason: { code: status === "expired" ? "expired" : "manually_disabled", severity: status === "expired" ? "critical" : "info", entityType: "share", entityId: share.shareId },
      additionalReasonCount: 0,
    };
  }
  if (share.routeState === "reconnecting") {
    return { state: "reconnecting", primaryReason: { code: "route_reconnecting", severity: "info", entityType: "share", entityId: share.shareId, startedAt: share.routeStateSince }, additionalReasonCount: 0 };
  }
  if (!share.isOnline) {
    return { state: "offline", primaryReason: { code: "route_offline", severity: "critical", entityType: "share", entityId: share.shareId }, additionalReasonCount: 0 };
  }
  if (share.canManage && share.activeEdit?.status === "rejected") {
    return { state: "degraded", primaryReason: { code: "edit_failed", severity: "critical", entityType: "share", entityId: share.shareId, startedAt: share.activeEdit.updatedAt }, additionalReasonCount: 0 };
  }
  if (share.canManage && share.activeEdit?.status === "pending") {
    return { state: "degraded", primaryReason: { code: "edit_pending", severity: "warning", entityType: "share", entityId: share.shareId, startedAt: share.activeEdit.updatedAt }, additionalReasonCount: 0 };
  }
  const latestHealth = share.healthChecks?.at(-1);
  if (latestHealth && !latestHealth.isHealthy) {
    return { state: "degraded", primaryReason: { code: "health_check_failed", severity: "warning", entityType: "share", entityId: share.shareId }, additionalReasonCount: 0 };
  }
  return { state: "online", additionalReasonCount: 0 };
}

export function shareOperationalSummary(share: ShareView): OperationalSummary {
  return share.operationalSummary || fallbackShareSummary(share);
}

export function clientOperationalSummary(client: DashboardClient, _shares: ShareView[] = []): OperationalSummary {
  if (client.operationalSummary) return client.operationalSummary;
  const latestHealth = client.healthChecks?.at(-1);
  if (latestHealth && !latestHealth.isHealthy) {
    return {
      state: "degraded",
      primaryReason: {
        code: "health_check_failed",
        severity: "warning",
        entityType: "client",
        entityId: client.installation.id,
      },
      additionalReasonCount: 0,
    };
  }
  if (client.clientTunnel) {
    if (client.clientTunnel.enabled && client.clientTunnel.routeState === "reconnecting") {
      return {
        state: "reconnecting",
        primaryReason: { code: "route_reconnecting", severity: "info", entityType: "client", entityId: client.installation.id, startedAt: client.clientTunnel.routeStateSince },
        additionalReasonCount: 0,
      };
    }
    if (client.clientTunnel.enabled && !client.clientTunnel.online) {
      return {
        state: "offline",
        primaryReason: { code: "route_offline", severity: "critical", entityType: "client", entityId: client.installation.id },
        additionalReasonCount: 0,
      };
    }
    return { state: "online", additionalReasonCount: 0 };
  }
  return { state: "online", additionalReasonCount: 0 };
}

export function marketOperationalSummary(market: DashboardMarket): OperationalSummary {
  if (market.operationalSummary) return market.operationalSummary;
  const status = String(market.status || "").trim().toLowerCase();
  if (status === "disabled") return { state: "disabled", primaryReason: { code: "manually_disabled", severity: "info", entityType: "market", entityId: market.id }, additionalReasonCount: 0 };
  if (market.maintenanceEnabled) return { state: "maintenance", primaryReason: { code: "maintenance_enabled", severity: "info", entityType: "market", entityId: market.id }, additionalReasonCount: 0 };
  if (market.routeState === "reconnecting") return { state: "reconnecting", primaryReason: { code: "route_reconnecting", severity: "info", entityType: "market", entityId: market.id, startedAt: market.routeStateSince }, additionalReasonCount: 0 };
  if (!market.online || status === "offline") return { state: "offline", primaryReason: { code: "route_offline", severity: "critical", entityType: "market", entityId: market.id, startedAt: market.offlineSince }, additionalReasonCount: 0 };
  if (market.onlineShareCount === 0) return { state: "degraded", primaryReason: { code: "no_online_shares", severity: "critical", entityType: "market", entityId: market.id, currentValue: "0", threshold: String(Math.max(1, market.shareCount)) }, additionalReasonCount: 0 };
  if (market.parallelCapacity > 0 && market.activeRequests / market.parallelCapacity >= 0.9) {
    return { state: "degraded", primaryReason: { code: market.activeRequests >= market.parallelCapacity ? "parallel_capacity_full" : "parallel_capacity_warning", severity: market.activeRequests >= market.parallelCapacity ? "critical" : "warning", entityType: "market", entityId: market.id, currentValue: String(market.activeRequests), threshold: String(market.parallelCapacity) }, additionalReasonCount: 0 };
  }
  return { state: "available", additionalReasonCount: 0 };
}

export function operationalStateRank(state: OperationalState) {
  return state === "offline" ? 0 : state === "degraded" ? 1 : state === "reconnecting" ? 2 : state === "maintenance" ? 3 : state === "online" || state === "available" ? 4 : 5;
}

export function useStableOperationalRanks(entries: Array<{ id: string; state: OperationalState }>, settleMs = 15_000) {
  const memory = React.useRef(new Map<string, { observed: OperationalState; observedAt: number; committed: OperationalState }>());
  const now = Date.now();
  const activeIds = new Set(entries.map((entry) => entry.id));
  for (const id of memory.current.keys()) {
    if (!activeIds.has(id)) memory.current.delete(id);
  }
  const ranks = new Map<string, number>();
  for (const entry of entries) {
    const existing = memory.current.get(entry.id);
    if (!existing) {
      memory.current.set(entry.id, { observed: entry.state, observedAt: now, committed: entry.state });
      ranks.set(entry.id, operationalStateRank(entry.state));
      continue;
    }
    if (existing.observed !== entry.state) {
      existing.observed = entry.state;
      existing.observedAt = now;
    } else if (existing.committed !== entry.state && now - existing.observedAt >= settleMs) {
      existing.committed = entry.state;
    }
    ranks.set(entry.id, operationalStateRank(existing.committed));
  }
  return ranks;
}

export function operationalStateLabel(state: OperationalState, t: ReturnType<typeof useLocaleText>["t"]) {
  if (state === "online") return t("common.online");
  if (state === "available") return t("dashboard.available");
  if (state === "reconnecting") return t("dashboard.reconnecting");
  if (state === "degraded") return t("dashboard.degraded");
  if (state === "offline") return t("common.offline");
  if (state === "maintenance") return t("dashboard.maintenance");
  return t("dashboard.disabled");
}

export function operationalReasonLabel(reason: OperationalReason | undefined, t: ReturnType<typeof useLocaleText>["t"]) {
  if (!reason) return t("dashboard.healthy");
  const current = reason.currentValue || "-";
  const threshold = reason.threshold || "-";
  switch (reason.code) {
    case "route_reconnecting": return t("dashboard.reason.routeReconnecting");
    case "route_offline": return t("dashboard.reason.routeOffline");
    case "health_check_failed": return t("dashboard.reason.healthCheckFailed");
    case "no_online_shares": return t("dashboard.reason.noOnlineShares");
    case "partial_share_outage": return t("dashboard.reason.partialShareOutage", { current, total: threshold });
    case "parallel_capacity_full": return t("dashboard.reason.parallelFull", { current, total: threshold });
    case "parallel_capacity_warning": return t("dashboard.reason.parallelWarning", { current, total: threshold });
    case "usage_limit_warning": return t("dashboard.reason.usageWarning", { current, total: threshold });
    case "expired": return t("dashboard.reason.expired");
    case "expires_soon": return t("dashboard.reason.expiresSoon");
    case "provider_unavailable": return t("dashboard.reason.providerUnavailable");
    case "medium_latency": return t("dashboard.reason.mediumLatency", { value: formatOperationalLatencyValue(current) });
    case "high_latency": return t("dashboard.reason.highLatency", { value: formatOperationalLatencyValue(current) });
    case "edit_pending": return t("dashboard.reason.editPending");
    case "edit_failed": return t("dashboard.reason.editFailed");
    case "maintenance_enabled": return t("dashboard.reason.maintenance");
    case "manually_disabled": return t("dashboard.reason.disabled");
    default: return String(reason.code).replaceAll("_", " ");
  }
}

export function operationalImpactLabel(kind: "client" | "share" | "market", reason: OperationalReason | undefined, t: ReturnType<typeof useLocaleText>["t"]) {
  if (!reason) return t("dashboard.impact.none");
  if (reason.code === "route_reconnecting") return t("dashboard.impact.routeReconnecting");
  if (reason.code === "route_offline" || reason.code === "no_online_shares") return kind === "market" ? t("dashboard.impact.marketOffline") : t("dashboard.impact.routeOffline");
  if (reason.code === "parallel_capacity_full") return t("dashboard.impact.capacityFull");
  if (reason.code === "provider_unavailable") return t("dashboard.impact.providerUnavailable");
  if (reason.code === "maintenance_enabled" || reason.code === "manually_disabled") return t("dashboard.impact.disabled");
  return t("dashboard.impact.degraded");
}

function formatOperationalLatencyValue(value: string) {
  const ms = Number(value);
  if (!Number.isFinite(ms) || ms <= 0) return value;
  const seconds = ms / 1000;
  if (seconds < 10) return `${seconds.toFixed(2)}s`;
  if (seconds < 100) return `${seconds.toFixed(1)}s`;
  return `${Math.round(seconds)}s`;
}

export function ClientRemovalSchedule({
  removalAt,
  className = "",
  showLabel = true,
}: {
  removalAt?: string;
  className?: string;
  showLabel?: boolean;
}) {
  const { locale, t } = useLocaleText();
  if (!removalAt) return null;
  const ts = Date.parse(removalAt);
  if (!Number.isFinite(ts)) return null;
  return (
    <span
      className={`inline-flex min-w-0 items-center gap-1 truncate font-medium text-rose-700 ${className}`}
      title={formatDateTime(removalAt)}
    >
      {showLabel ? <span className="shrink-0">{t("dashboard.removalAt")}</span> : null}
      <time className="truncate" dateTime={removalAt}>
        {formatRelativeTime(removalAt, locale)}
      </time>
    </span>
  );
}

export function OperationalStatusPill({ summary, className = "" }: { summary: OperationalSummary; className?: string }) {
  const { t } = useLocaleText();
  const state = summary.state;
  const style = state === "online" || state === "available"
    ? "border-emerald-200 bg-emerald-50 text-emerald-700"
    : state === "reconnecting"
      ? "border-sky-200 bg-sky-50 text-sky-700"
    : state === "degraded"
      ? "border-amber-200 bg-amber-50 text-amber-700"
      : state === "offline"
        ? "border-rose-200 bg-rose-50 text-rose-700"
        : state === "maintenance"
          ? "border-blue-200 bg-blue-50 text-blue-700"
          : "border-slate-200 bg-slate-100 text-slate-600";
  const Icon = state === "online" || state === "available" ? CheckCircle2 : state === "reconnecting" ? RefreshCw : state === "degraded" ? AlertTriangle : state === "offline" ? CircleOff : state === "maintenance" ? Clock3 : PauseCircle;
  return <span className={`inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-[11px] font-semibold ${style} ${className}`}><Icon className="h-3 w-3" />{operationalStateLabel(state, t)}</span>;
}

export function OperationalDiagnosis({
  summary,
  kind,
  removalAt,
}: {
  summary: OperationalSummary;
  kind: "client" | "share" | "market";
  removalAt?: string;
}) {
  const { locale, t } = useLocaleText();
  const reason = summary.primaryReason;
  return (
    <section className="grid gap-3 rounded-lg border bg-slate-50 p-3" aria-label={t("dashboard.diagnosis")}>
      <div className="flex items-center justify-between gap-3">
        <OperationalStatusPill summary={summary} />
        {summary.changedAt ? <span className="text-[11px] text-muted-foreground">{t("dashboard.since")} {formatRelativeTime(summary.changedAt, locale)}</span> : null}
      </div>
      <div className="grid gap-1">
        <strong className="text-sm text-foreground">{operationalReasonLabel(reason, t)}</strong>
        <span className="text-xs leading-5 text-muted-foreground">{operationalImpactLabel(kind, reason, t)}</span>
        {kind === "client" && summary.state === "offline" && removalAt ? (
          <ClientRemovalSchedule removalAt={removalAt} className="text-xs" />
        ) : null}
        {summary.additionalReasonCount > 0 ? <span className="text-[11px] font-medium text-amber-700">{t("dashboard.otherIssues", { count: summary.additionalReasonCount })}</span> : null}
      </div>
    </section>
  );
}
