"use client";

import { Card } from "@heroui/react";
import { ChevronRight, Eye, Link2, MoreHorizontal, Pencil } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  averageRecentLatencyMs,
  formatDurationShort,
  formatLatencySeconds,
  isUnlimitedExpiry,
  modelHealthTitle,
  modelHealthTone,
  parseShareTimestamp,
  providerAccountIdentity,
  providerAccountLevel,
  resolveShareAppRuntime,
  shareApiParts,
  shareAppSettings,
  shareAppTokensUsed,
  type CoreShareApp,
} from "@/components/dashboard/data-tables";
import type { ShareRequestLog, ShareView } from "@/lib/types";
import { compactTokens } from "@/lib/utils";
import { resolveShareCoreApp, SHARE_APP_LABELS } from "@/lib/share-app";

function requestBelongsToApp(request: ShareRequestLog, app: CoreShareApp) {
  const appType = (request.appType || "").trim().toLowerCase();
  if (appType) return appType === app;
  return (request.requestAgent || "").trim().toLowerCase() === app;
}

function isUnlimited(value?: number) {
  return Number(value) < 0;
}

function shareOperationalState(share: ShareView) {
  const status = String(share.shareStatus || "").trim().toLowerCase();
  if (status !== "active") return status === "paused" || status === "expired" ? status : "disabled";
  if (!share.isOnline) return "offline";
  const latestHealth = share.healthChecks?.at(-1);
  return latestHealth && !latestHealth.isHealthy ? "degraded" : "online";
}

function ShareStatus({ share }: { share: ShareView }) {
  const { t } = useLocaleText();
  const state = shareOperationalState(share);
  const label = state === "online"
    ? t("common.online")
    : state === "degraded"
      ? t("dashboard.degraded")
      : state === "offline"
        ? t("common.offline")
        : state === "paused"
          ? t("dashboard.shareStatus.paused")
          : state === "expired"
            ? t("dashboard.shareStatus.expired")
            : t("common.disabled");
  const style = state === "online"
    ? "border-emerald-200 bg-emerald-50 text-emerald-700"
    : state === "degraded"
      ? "border-amber-200 bg-amber-50 text-amber-700"
      : state === "offline"
        ? "border-rose-200 bg-rose-50 text-rose-700"
        : "border-slate-200 bg-slate-100 text-slate-600";
  return <span className={`inline-flex h-5 shrink-0 items-center gap-1 rounded-full border px-2 text-[10px] font-semibold ${style}`}><span className="h-1.5 w-1.5 rounded-full bg-current" />{label}</span>;
}

function exceptionalMessage({
  share,
  tokenLimit,
  tokensUsed,
  parallelLimit,
  activeRequests,
  averageLatency,
  expiresAt,
  locale,
  t,
}: {
  share: ShareView;
  tokenLimit?: number;
  tokensUsed: number;
  parallelLimit?: number;
  activeRequests: number;
  averageLatency: number | null;
  expiresAt?: string;
  locale: "en" | "zh-CN";
  t: ReturnType<typeof useLocaleText>["t"];
}) {
  if (share.canManage && share.activeEdit?.status === "rejected") return t("dashboard.applyFailed");
  if (share.canManage && share.activeEdit?.status === "pending") return t("dashboard.pendingApply");
  const status = String(share.shareStatus || "").trim().toLowerCase();
  if (status === "active" && !share.isOnline) return t("dashboard.routeOffline");
  if (status !== "active") return null;
  if (expiresAt && !isUnlimitedExpiry(expiresAt)) {
    const expiresMs = parseShareTimestamp(expiresAt);
    if (Number.isFinite(expiresMs) && expiresMs - Date.now() < 7 * 24 * 60 * 60 * 1000) {
      return `${t("dashboard.expires")} ${formatDurationShort(expiresAt, locale, "remaining")}`;
    }
  }
  if (!isUnlimited(tokenLimit) && Number(tokenLimit) > 0 && tokensUsed / Number(tokenLimit) >= 0.9) return t("dashboard.usageHigh");
  if (!isUnlimited(parallelLimit) && Number(parallelLimit) > 0 && activeRequests >= Number(parallelLimit)) return t("dashboard.parallelFull");
  if (averageLatency != null && averageLatency >= 2000) return `${t("dashboard.response")} ${formatLatencySeconds(averageLatency)}`;
  return null;
}

export const ShareCard = React.memo(function ShareCard({
  share,
  onOpen,
  onEdit,
  onConnect,
}: {
  share: ShareView;
  onOpen: (share: ShareView) => void;
  onEdit: (share: ShareView) => void;
  onConnect: (share: ShareView) => void;
}) {
  const { locale, t } = useLocaleText();
  const app = resolveShareCoreApp(share);
  const api = shareApiParts(share);
  const settings = app ? shareAppSettings(share, app) : null;
  const appRequests = app ? (share.recentRequests || []).filter((request) => requestBelongsToApp(request, app)) : share.recentRequests || [];
  const tokensUsed = app ? shareAppTokensUsed(share, app) : share.tokensUsed || 0;
  const tokenLimit = settings?.tokenLimit ?? share.tokenLimit;
  const parallelLimit = settings?.parallelLimit ?? share.parallelLimit;
  const activeRequests = app ? share.activeRequestsByApp?.[app] ?? 0 : share.activeRequests || 0;
  const averageLatency = averageRecentLatencyMs(appRequests);
  const expiresAt = settings?.expiresAt || share.expiresAt;
  const runtime = app ? resolveShareAppRuntime(share, app) : undefined;
  const accountLevel = runtime ? providerAccountLevel(runtime, locale) : "";
  const accountIdentity = runtime ? providerAccountIdentity(runtime) : share.providerId || "";
  const healthTone = app ? modelHealthTone(share, app) : { className: "bg-slate-50 text-muted-foreground", label: "" };
  const marketCount = share.marketAccessMode === "all" ? null : (share.marketLinks || []).length;
  const issue = exceptionalMessage({ share, tokenLimit, tokensUsed, parallelLimit, activeRequests, averageLatency, expiresAt, locale, t });
  const title = share.shareName || share.subdomain || share.shareId;
  const usagePercent = !isUnlimited(tokenLimit) && Number(tokenLimit) > 0 ? Math.min(100, Math.max(0, (tokensUsed / Number(tokenLimit)) * 100)) : null;
  const editPending = share.canManage && share.activeEdit?.status === "pending";
  const editRejected = share.canManage && share.activeEdit?.status === "rejected";

  return (
    <Card
      className="group w-64 shrink-0 snap-start overflow-visible rounded-lg border border-default/60 bg-white p-0 shadow-sm transition-colors hover:border-primary/30"
      onClick={(event) => {
        const target = event.target as HTMLElement;
        if (!target.closest("button,a,summary,details")) onOpen(share);
      }}
      onKeyDown={(event) => {
        if ((event.key === "Enter" || event.key === " ") && event.target === event.currentTarget) {
          event.preventDefault();
          onOpen(share);
        }
      }}
      role="button"
      tabIndex={0}
      aria-label={`${t("dashboard.details")}: ${title}`}
    >
      <Card.Content className="grid h-[178px] min-w-0 grid-rows-[auto_auto_1fr_auto] gap-2.5 p-3">
        <div className="flex min-w-0 items-start justify-between gap-2">
          <div className="min-w-0">
            <strong className="block truncate text-sm font-semibold text-foreground" title={title}>{title}</strong>
            <span className="block truncate font-mono text-[10px] text-muted-foreground" title={api.apiUrl}>{api.apiUrl}</span>
          </div>
          <ShareStatus share={share} />
        </div>

        <div className={`grid min-w-0 gap-0.5 rounded-md border px-2 py-1.5 text-[11px] ${healthTone.className}`} title={app ? modelHealthTitle(share, app) : undefined}>
          <div className="flex min-w-0 items-center justify-between gap-2">
            <span className="font-semibold">{app ? SHARE_APP_LABELS[app] : share.appType || t("dashboard.appType")}</span>
            {healthTone.label ? <span className="truncate text-[10px] opacity-75">{healthTone.label}</span> : null}
          </div>
          <span className="truncate opacity-80" title={[accountLevel, accountIdentity].filter(Boolean).join(" · ")}>{[accountLevel, accountIdentity].filter((value) => value && value !== "-").join(" · ") || t("dashboard.providerUnavailable")}</span>
        </div>

        <div className="grid grid-cols-2 gap-3 text-[11px]">
          <div className="min-w-0">
            <span className="block text-muted-foreground">{t("dashboard.usage")}</span>
            <strong className="tabular-nums">{compactTokens(tokensUsed)} / {isUnlimited(tokenLimit) ? "∞" : compactTokens(tokenLimit)}</strong>
            {usagePercent != null ? <div className="mt-1 h-1 overflow-hidden rounded-full bg-slate-100"><div className={`h-full rounded-full ${usagePercent >= 90 ? "bg-rose-500" : "bg-primary/70"}`} style={{ width: `${usagePercent}%` }} /></div> : null}
          </div>
          <div>
            <span className="block text-muted-foreground">{t("dashboard.parallel")}</span>
            <strong className="tabular-nums">{activeRequests}<span className="text-muted-foreground">/{isUnlimited(parallelLimit) ? "∞" : parallelLimit || 0}</span></strong>
            <span className="mt-1 block truncate text-[10px] text-muted-foreground">{share.forSale === "No" ? t("dashboard.notListed") : marketCount == null ? t("dashboard.allMarkets") : t("dashboard.marketsCount", { count: marketCount })}</span>
          </div>
        </div>

        <div className="flex min-w-0 items-center justify-between gap-2 border-t pt-2">
          <span className={`min-w-0 truncate text-[10px] ${issue ? "font-medium text-amber-700" : "text-muted-foreground"}`} title={issue || undefined}>
            {issue || `${t("dashboard.response")} ${formatLatencySeconds(averageLatency)}`}
          </span>
          <div className="flex shrink-0 items-center gap-1">
            <button type="button" data-no-row-drawer className="inline-flex h-6 items-center gap-1 rounded-md border border-emerald-200 bg-emerald-50 px-2 text-[10px] font-semibold text-emerald-700 hover:bg-emerald-100" onClick={(event) => { event.stopPropagation(); onConnect(share); }}>
              <Link2 className="h-3 w-3" />{t("dashboard.connect")}
            </button>
            <details className="relative" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
              <summary className="flex h-6 w-6 cursor-pointer list-none items-center justify-center rounded-md text-muted-foreground hover:bg-slate-100 hover:text-foreground" aria-label={t("dashboard.moreActions")}>
                <MoreHorizontal className="h-4 w-4" />
              </summary>
              <div className="absolute bottom-7 right-0 z-30 grid w-32 overflow-hidden rounded-md border bg-white p-1 text-xs shadow-lg">
                <button type="button" disabled={editPending} title={editRejected ? share.activeEdit?.errorMessage || t("dashboard.applyFailedFallback") : undefined} className="flex items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-slate-100 disabled:cursor-not-allowed disabled:opacity-50" onClick={() => { if (!editPending) onEdit(share); }}>
                  {share.canManage ? <Pencil className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}{editPending ? t("dashboard.pendingApply") : editRejected ? t("dashboard.applyFailed") : share.canManage ? t("common.edit") : t("common.view")}
                </button>
                <button type="button" className="flex items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-slate-100" onClick={() => onOpen(share)}>
                  <ChevronRight className="h-3.5 w-3.5" />{t("dashboard.details")}
                </button>
              </div>
            </details>
            <ChevronRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:text-foreground" />
          </div>
        </div>
      </Card.Content>
    </Card>
  );
});
