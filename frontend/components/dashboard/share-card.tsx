"use client";

import { Card } from "@heroui/react";
import { Eye, ExternalLink, Link2, Pencil } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { operationalReasonLabel, shareOperationalSummary } from "@/components/dashboard/operational-status";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import {
  averageRecentLatencyMs,
  formatLatencySeconds,
  latencyResponseToneClass,
  parallelOccupancyTitle,
  modelHealthTitle,
  modelHealthTone,
  providerActualModelNames,
  providerApiEndpoint,
  providerQuotaStatusLine,
  providerStatusIdentity,
  isApiProviderRuntime,
  resolveShareAppRuntime,
  shareDisplayTitle,
  subdomainTunnelUrl,
  shareAppSettings,
  shareExpiryProgress,
  expiryTitle,
  type CoreShareApp,
} from "@/components/dashboard/data-tables";
import type { ShareRequestLog, ShareView } from "@/lib/types";
import { compactTokens, formatDateTime } from "@/lib/utils";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { resolveShareCoreApp } from "@/lib/share-app";
import { recordDashboardUxEvent } from "@/lib/api";

function requestBelongsToApp(request: ShareRequestLog, app: CoreShareApp) {
  const appType = (request.appType || "").trim().toLowerCase();
  if (appType) return appType === app;
  return (request.requestAgent || "").trim().toLowerCase() === app;
}

function isUnlimited(value?: number) {
  return Number(value) < 0;
}

function shouldOpenShareCard(
  event: React.MouseEvent<HTMLElement>,
  pointerDown: { x: number; y: number } | null,
) {
  if (pointerDown) {
    const deltaX = Math.abs(event.clientX - pointerDown.x);
    const deltaY = Math.abs(event.clientY - pointerDown.y);
    if (deltaX > 4 || deltaY > 4) return false;
  }

  const selection = window.getSelection();
  if (selection && !selection.isCollapsed && selection.toString().trim()) {
    return false;
  }

  const target = event.target as HTMLElement | null;
  if (target?.closest("button,a,[data-no-row-drawer]")) {
    return false;
  }

  return true;
}

export const ShareCard = React.memo(function ShareCard({
  share,
  referenceTunnelUrl,
  onOpen,
  onEdit,
  onConnect,
}: {
  share: ShareView;
  referenceTunnelUrl?: string;
  onOpen: (share: ShareView) => void;
  onEdit: (share: ShareView) => void;
  onConnect: (share: ShareView) => void;
}) {
  const { locale, t } = useLocaleText();
  const focus = useDashboardFocus();
  const cardRef = React.useRef<HTMLDivElement | null>(null);
  const pointerDownRef = React.useRef<{ x: number; y: number } | null>(null);
  const app = resolveShareCoreApp(share);
  const settings = app ? shareAppSettings(share, app) : null;
  const appRequests = app ? (share.recentRequests || []).filter((request) => requestBelongsToApp(request, app)) : share.recentRequests || [];
  const tokensUsed = share.tokensUsed || 0;
  const tokenLimit = settings?.tokenLimit ?? share.tokenLimit;
  const parallelLimit = settings?.parallelLimit ?? share.parallelLimit;
  const activeRequests = app ? share.activeRequestsByApp?.[app] ?? 0 : share.activeRequests || 0;
  const averageLatency = averageRecentLatencyMs(appRequests);
  const runtime = app ? resolveShareAppRuntime(share, app) : undefined;
  const providerEnabled = app ? !!share.support?.[app] : !!runtime;
  const quotaStatusLine = providerEnabled && runtime ? providerQuotaStatusLine(runtime, locale) : "-";
  const accountLine = providerEnabled && runtime
    ? providerStatusIdentity(runtime)
    : share.providerId || t("dashboard.providerUnavailable");
  const actualModels = providerEnabled && runtime ? providerActualModelNames(runtime) : "-";
  const isApiProvider = providerEnabled && runtime ? isApiProviderRuntime(runtime) : false;
  const apiEndpoint = providerEnabled && runtime ? providerApiEndpoint(runtime) : "-";
  const healthTone = app ? modelHealthTone(share, app) : { className: "bg-slate-50 text-muted-foreground", label: "" };
  const marketCount = share.marketAccessMode === "all" ? null : (share.marketLinks || []).length;
  const summary = shareOperationalSummary(share);
  const issue = summary.primaryReason ? operationalReasonLabel(summary.primaryReason, t) : null;
  const title = shareDisplayTitle(share);
  const titleUrl = subdomainTunnelUrl(share.subdomain, referenceTunnelUrl);
  const description = share.description?.trim() || "";
  const usagePercent = !isUnlimited(tokenLimit) && Number(tokenLimit) > 0 ? Math.min(100, Math.max(0, (tokensUsed / Number(tokenLimit)) * 100)) : null;
  const onlineRate = share.onlineRate24h || 0;
  const observedMinutes = share.observedMinutes24h || 0;
  const observationCoverage = share.observationCoverage24h || 0;
  const onlineTitle = t("dashboard.uptimeObservation", { healthy: onlineRate.toFixed(1), observed: observedMinutes, coverage: observationCoverage.toFixed(1) });
  const expiryLabel = shareExpiryProgress(share, locale);
  const expiryHint = `${formatDateTime(share.createdAt)} / ${expiryTitle(share.expiresAt)}`;
  const parallelTitle = parallelOccupancyTitle(share, app, t);
  const saleLabel = share.forSale === "No" ? t("dashboard.notListed") : marketCount == null ? t("dashboard.allMarkets") : t("dashboard.marketsCount", { count: marketCount });
  const editPending = share.canManage && share.activeEdit?.status === "pending";
  const editRejected = share.canManage && share.activeEdit?.status === "rejected";
  const focused = focus.isFocused("share", share.shareId);
  const related = focus.isRelated("share", share.shareId);
  const dimmed = Boolean(focus.target) && !related;
  const stateTone = summary.state === "offline" ? "border-rose-200" : summary.state === "reconnecting" ? "border-sky-300" : summary.state === "degraded" ? "border-amber-300" : summary.state === "disabled" ? "border-slate-300 opacity-70" : "border-slate-200";
  const statusDot = summary.state === "offline" ? "bg-rose-500" : summary.state === "reconnecting" ? "bg-sky-500" : summary.state === "degraded" ? "bg-amber-400" : summary.state === "disabled" ? "bg-slate-400" : "bg-emerald-500";
  const connectDisabled = summary.state === "disabled";
  const editLabel = editPending
    ? t("dashboard.pendingApply")
    : editRejected
      ? t("dashboard.applyFailed")
      : share.canManage
        ? t("common.edit")
        : t("common.view");
  const secondaryActionClass =
    "inline-flex h-6 items-center gap-1 rounded-md border border-slate-200 bg-white px-2 text-[10px] font-semibold text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50";

  const openShareDrawer = React.useCallback(() => {
    focus.setFocus({ kind: "share", id: share.shareId, source: "client-board" });
    onOpen(share);
  }, [focus, onOpen, share]);

  React.useEffect(() => {
    if (!focused || focus.target?.source === "client-board") return;
    cardRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "center" });
    if (focus.target?.kind === "request") void recordDashboardUxEvent({ eventType: "share_located_from_request", source: "activity", targetType: "share" });
  }, [focus.target?.source, focused]);

  return (
    <Card
      ref={cardRef}
      data-share-id={share.shareId}
      className={`w-full min-w-0 overflow-visible rounded-xl border bg-white p-0 shadow-sm transition-[border-color,box-shadow,opacity] hover:border-primary/35 ${focused ? "border-primary ring-2 ring-primary/20" : related ? "border-primary/35" : stateTone} ${dimmed ? "opacity-40" : "opacity-100"}`}
      onMouseDown={(event) => {
        pointerDownRef.current = { x: event.clientX, y: event.clientY };
      }}
      onClick={(event) => {
        if (!shouldOpenShareCard(event, pointerDownRef.current)) return;
        pointerDownRef.current = null;
        openShareDrawer();
      }}
    >
      <Card.Content className="grid min-h-[150px] min-w-0 cursor-pointer select-text grid-rows-[auto_auto_1fr] gap-2.5 p-3">
        <div className="grid min-w-0 gap-1">
          <div className="flex min-w-0 items-center justify-between gap-2">
            <div className="flex min-w-0 items-center gap-1.5">
              <span className={`h-2 w-2 shrink-0 rounded-full ${statusDot}`} title={issue || summary.state} />
              {titleUrl ? (
                <a
                  href={titleUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  data-no-row-drawer
                  className="inline-flex min-w-0 max-w-full items-center gap-1 truncate text-sm font-semibold text-foreground underline-offset-4 hover:underline"
                  title={titleUrl}
                  onClick={(event) => event.stopPropagation()}
                >
                  <span className="truncate">{title}</span>
                  <ExternalLink className="h-3 w-3 shrink-0 text-muted-foreground" aria-hidden />
                </a>
              ) : (
                <strong className="truncate text-sm font-semibold text-foreground" title={title}>{title}</strong>
              )}
              {app ? <ShareAppLogo app={app} size={14} /> : null}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <button type="button" data-no-row-drawer disabled={connectDisabled} title={connectDisabled ? issue || t("common.disabled") : undefined} className="inline-flex h-6 items-center gap-1 rounded-md border border-primary/20 bg-primary/5 px-2 text-[10px] font-semibold text-primary hover:bg-primary/10 disabled:cursor-not-allowed disabled:border-slate-200 disabled:bg-slate-50 disabled:text-slate-400" onClick={(event) => { event.stopPropagation(); if (!connectDisabled) onConnect(share); }}>
                <Link2 className="h-3 w-3" />{t("dashboard.connect")}
              </button>
              <button
                type="button"
                data-no-row-drawer
                disabled={editPending}
                title={editRejected ? share.activeEdit?.errorMessage || t("dashboard.applyFailedFallback") : undefined}
                className={secondaryActionClass}
                onClick={(event) => {
                  event.stopPropagation();
                  if (!editPending) onEdit(share);
                }}
              >
                {share.canManage ? <Pencil className="h-3 w-3" /> : <Eye className="h-3 w-3" />}
                {editLabel}
              </button>
            </div>
          </div>
          {description ? (
            <span className="block truncate text-[10px] text-muted-foreground" title={description}>{description}</span>
          ) : null}
        </div>

        <div className={`grid min-w-0 gap-1 rounded-md border px-2 py-1.5 text-[11px] ${healthTone.className}`} title={app ? modelHealthTitle(share, app) : undefined}>
          {isApiProvider ? (
            <>
              <span className="min-w-0 truncate font-mono text-[10px] font-semibold leading-4" title={`${t("dashboard.apiRequestUrl")}: ${apiEndpoint}`}>{apiEndpoint}</span>
              <span className="min-w-0 truncate opacity-80">-</span>
              <span className="min-w-0 truncate opacity-80" title={actualModels}>{actualModels}</span>
            </>
          ) : (
            <>
              <span className="min-w-0 truncate font-semibold leading-4" title={quotaStatusLine}>{quotaStatusLine}</span>
              <span className="min-w-0 truncate opacity-80" title={accountLine}>{accountLine}</span>
              <span className="min-w-0 truncate opacity-80" title={actualModels}>{actualModels}</span>
            </>
          )}
        </div>

        <div className="grid gap-2 text-[11px]">
          <div className="grid grid-cols-2 gap-2">
            <div className="min-w-0">
              <span className="block text-muted-foreground">{t("dashboard.usage")}</span>
              <strong className="tabular-nums">{compactTokens(tokensUsed)} / {isUnlimited(tokenLimit) ? "∞" : compactTokens(tokenLimit)}</strong>
              {usagePercent != null ? <div className="mt-1 h-1 overflow-hidden rounded-full bg-slate-100"><div className={`h-full rounded-full ${usagePercent >= 90 ? "bg-rose-500" : "bg-primary/70"}`} style={{ width: `${usagePercent}%` }} /></div> : null}
            </div>
            <div className="min-w-0" title={parallelTitle}>
              <span className="block text-muted-foreground">{t("dashboard.parallel")}</span>
              <strong className="cursor-help tabular-nums">{activeRequests}<span className="text-muted-foreground">/{isUnlimited(parallelLimit) ? "∞" : parallelLimit || 0}</span></strong>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div className="min-w-0">
              <span className="block text-muted-foreground">{t("dashboard.expires")}</span>
              <strong className="tabular-nums" title={expiryHint}>{expiryLabel}</strong>
            </div>
            <div className="min-w-0">
              <span className="block text-muted-foreground">{t("dashboard.uptime24h")}</span>
              <strong className={`tabular-nums ${onlineRate < 90 ? "text-amber-700" : "text-emerald-700"}`} title={onlineTitle}>{onlineRate.toFixed(1)}%</strong>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div className="min-w-0">
              <span className="block text-muted-foreground">{t("dashboard.response")}</span>
              <strong
                className={`block truncate tabular-nums font-medium ${latencyResponseToneClass(averageLatency)}`}
                title={formatLatencySeconds(averageLatency)}
              >
                {formatLatencySeconds(averageLatency)}
              </strong>
            </div>
            <div className="min-w-0">
              <span className="block text-muted-foreground">{t("dashboard.forSale")}</span>
              <span className="block truncate text-foreground" title={saleLabel}>{saleLabel}</span>
            </div>
          </div>
        </div>
      </Card.Content>
    </Card>
  );
});
