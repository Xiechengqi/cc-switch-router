"use client";

import { Card, toast } from "@heroui/react";
import { Copy, Eye, Link2, Pencil, Plus, Route } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  operationalReasonLabel,
  shareOperationalSummary,
} from "@/components/dashboard/operational-status";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import {
  averageRecentLatencyMs,
  formatLatencySeconds,
  formatTtftTps,
  latencyResponseToneClass,
  parallelOccupancyTitle,
  modelHealthTitle,
  modelHealthTone,
  providerActualModelNames,
  providerApiEndpoint,
  providerQuotaStatusLine,
  providerStatusIdentity,
  isApiProviderRuntime,
  recentSharePerformance,
  resolveShareAppRuntime,
  shareDisplayTitle,
  shareExpiryProgress,
  expiryTitle,
  type CoreShareApp,
} from "@/components/dashboard/data-tables";
import type { ShareRequestLog, ShareView } from "@/lib/types";
import {
  compactTokens,
  formatDateTime,
  preferredScrollBehavior,
} from "@/lib/utils";
import {
  ShareProviderLogo,
  shareProviderLogoEntries,
} from "@/components/dashboard/share-provider-logo";
import {
  resolveShareCoreApp,
  shareEnabledApps,
  SHARE_APP_LABELS,
} from "@/lib/share-app";
import { recordDashboardUxEvent } from "@/lib/api";
import { shareEditPendingLabel } from "@/components/dashboard/share-edit/share-edit-section";
import { SubdomainCopyButton } from "@/components/dashboard/subdomain-copy-button";
import { ShareProviderStatusPanel } from "@/components/dashboard/share-provider-status-panel";
import type { DraftModelRoute } from "@/lib/model-routing";

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
  onOpen,
  onEdit,
  onConnect,
  directApiUrl,
  modelRoutes = [],
  onAddModelRoute,
}: {
  share: ShareView;
  onOpen: (share: ShareView) => void;
  onEdit: (share: ShareView) => void;
  onConnect: (share: ShareView) => void;
  directApiUrl?: string;
  modelRoutes?: DraftModelRoute[];
  onAddModelRoute?: (shareId: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const focus = useDashboardFocus();
  const cardRef = React.useRef<HTMLDivElement | null>(null);
  const pointerDownRef = React.useRef<{ x: number; y: number } | null>(null);
  const apps = shareEnabledApps(share);
  const app = resolveShareCoreApp(share);
  const appRequests =
    apps.length === 1 && app
      ? (share.recentRequests || []).filter((request) =>
          requestBelongsToApp(request, app),
        )
      : share.recentRequests || [];
  const tokensUsed = share.tokensUsed || 0;
  const tokenLimit = share.tokenLimit;
  const parallelLimit = share.parallelLimit;
  const activeRequests =
    apps.length === 1 && app
      ? (share.activeRequestsByApp?.[app] ?? 0)
      : share.activeRequests || 0;
  const averageLatency = averageRecentLatencyMs(appRequests);
  const performance = recentSharePerformance(appRequests);
  const runtime = app ? resolveShareAppRuntime(share, app) : undefined;
  const providerLogos = shareProviderLogoEntries(share);
  const modelPolicyEntries = apps.flatMap((entryApp) => {
    const entryRuntime = resolveShareAppRuntime(share, entryApp);
    if (!entryRuntime) return [];
    const text = providerActualModelNames(
      entryRuntime,
      t("dashboard.modelPolicyPassthrough"),
    );
    return [{ app: entryApp, runtime: entryRuntime, text }];
  });
  const modelPolicySummary = modelPolicyEntries
    .map((entry) => `${SHARE_APP_LABELS[entry.app]}: ${entry.text}`)
    .join(" · ");
  const providerEnabled = app ? !!share.support?.[app] : !!runtime;
  const quotaStatusLine =
    providerEnabled && runtime ? providerQuotaStatusLine(runtime, locale) : "-";
  const accountLine =
    providerEnabled && runtime
      ? providerStatusIdentity(runtime)
      : share.providerId || t("dashboard.providerUnavailable");
  const actualModels =
    modelPolicySummary ||
    (providerEnabled && runtime
      ? providerActualModelNames(
          runtime,
          t("dashboard.modelPolicyPassthrough"),
        )
      : "-");
  const isApiProvider =
    providerEnabled && runtime ? isApiProviderRuntime(runtime) : false;
  const apiEndpoint =
    providerEnabled && runtime ? providerApiEndpoint(runtime) : "-";
  const healthTone = app
    ? modelHealthTone(share, app)
    : { className: "bg-slate-50 text-muted-foreground", label: "" };
  const providerPanelView = {
    primaryLine: isApiProvider ? apiEndpoint : quotaStatusLine,
    identityLine: isApiProvider ? "-" : accountLine,
    modelsLine: actualModels,
    primaryTitle: isApiProvider
      ? `${t("dashboard.apiRequestUrl")}: ${apiEndpoint}`
      : quotaStatusLine,
    identityTitle: isApiProvider ? "-" : accountLine,
    modelsTitle: actualModels,
    panelTitle: app ? modelHealthTitle(share, app) : undefined,
    primaryMonospace: isApiProvider,
    toneClassName: healthTone.className,
  };
  const summary = shareOperationalSummary(share);
  const issue = summary.primaryReason
    ? operationalReasonLabel(summary.primaryReason, t)
    : null;
  const title = shareDisplayTitle(share);
  const subdomain = share.subdomain || "";
  const hasSubdomain = Boolean(subdomain.trim());
  const description = share.description?.trim() || "";
  const usagePercent =
    !isUnlimited(tokenLimit) && Number(tokenLimit) > 0
      ? Math.min(100, Math.max(0, (tokensUsed / Number(tokenLimit)) * 100))
      : null;
  const onlineRate = share.onlineRate24h || 0;
  const observedMinutes = share.observedMinutes24h || 0;
  const observationCoverage = share.observationCoverage24h || 0;
  const onlineTitle = t("dashboard.uptimeObservation", {
    healthy: onlineRate.toFixed(1),
    observed: observedMinutes,
    coverage: observationCoverage.toFixed(1),
  });
  const expiryLabel = shareExpiryProgress(share, locale);
  const expiryHint = `${formatDateTime(share.createdAt)} / ${expiryTitle(share.expiresAt)}`;
  const parallelTitle = parallelOccupancyTitle(
    share,
    apps.length === 1 ? app : null,
    t,
  );
  const editPending = share.canManage && share.activeEdit?.status === "pending";
  const editRejected =
    share.canManage && share.activeEdit?.status === "rejected";
  const focused = focus.isFocused("share", share.shareId);
  const related = focus.isRelated("share", share.shareId);
  const dimmed = Boolean(focus.target) && !related;
  const stateTone =
    summary.state === "offline"
      ? "border-rose-200"
      : summary.state === "reconnecting"
        ? "border-sky-300"
        : summary.state === "degraded"
          ? "border-amber-300"
          : summary.state === "disabled"
            ? "border-slate-300 opacity-70"
            : "border-slate-200";
  const statusDot =
    summary.state === "offline"
      ? "bg-rose-500"
      : summary.state === "reconnecting"
        ? "bg-sky-500"
        : summary.state === "degraded"
          ? "bg-amber-400"
          : summary.state === "disabled"
            ? "bg-slate-400"
            : "bg-emerald-500";
  const connectDisabled = summary.state === "disabled";
  const editLabel = editPending
    ? shareEditPendingLabel(share.activeEdit!, t)
    : editRejected
      ? t("dashboard.applyFailed")
      : share.canManage
        ? t("common.edit")
        : t("common.view");
  const secondaryActionClass =
    "inline-flex h-6 max-w-[160px] items-center gap-1 rounded-md border border-slate-200 bg-white px-2 text-[10px] font-semibold text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50";

  const openShareDrawer = React.useCallback(() => {
    focus.setFocus({
      kind: "share",
      id: share.shareId,
      source: "client-board",
    });
    onOpen(share);
  }, [focus, onOpen, share]);

  React.useEffect(() => {
    if (!focused || focus.target?.source === "client-board") return;
    cardRef.current?.scrollIntoView({
      behavior: preferredScrollBehavior(),
      block: "nearest",
      inline: "center",
    });
    if (focus.target?.kind === "request")
      void recordDashboardUxEvent({
        eventType: "share_located_from_request",
        source: "activity",
        targetType: "share",
      });
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
              <span
                className={`h-2 w-2 shrink-0 rounded-full ${statusDot}`}
                title={issue || summary.state}
              />
              <strong
                className="min-w-0 truncate text-sm font-semibold text-foreground"
                title={title}
              >
                {title}
              </strong>
              {hasSubdomain ? (
                <SubdomainCopyButton subdomain={subdomain} />
              ) : null}
              {providerLogos.length > 0 ? (
                <span className="inline-flex shrink-0 items-center gap-1">
                  {providerLogos.map((entry) => (
                    <ShareProviderLogo
                      key={entry.key}
                      provider={entry.provider}
                      fallbackApp={entry.app}
                      size={16}
                    />
                  ))}
                </span>
              ) : null}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <button
                type="button"
                data-no-row-drawer
                disabled={connectDisabled}
                title={
                  connectDisabled ? issue || t("common.disabled") : undefined
                }
                className="inline-flex h-6 items-center gap-1 rounded-md border border-primary/20 bg-primary/5 px-2 text-[10px] font-semibold text-primary hover:bg-primary/10 disabled:cursor-not-allowed disabled:border-slate-200 disabled:bg-slate-50 disabled:text-slate-400"
                onClick={(event) => {
                  event.stopPropagation();
                  if (!connectDisabled) onConnect(share);
                }}
              >
                <Link2 className="h-3 w-3" />
                {t("dashboard.connect")}
              </button>
              <button
                type="button"
                data-no-row-drawer
                disabled={editPending}
                title={
                  editRejected
                    ? share.activeEdit?.errorMessage ||
                      t("dashboard.applyFailedFallback")
                    : undefined
                }
                className={secondaryActionClass}
                onClick={(event) => {
                  event.stopPropagation();
                  if (!editPending) onEdit(share);
                }}
              >
                {share.canManage ? (
                  <Pencil className="h-3 w-3" />
                ) : (
                  <Eye className="h-3 w-3" />
                )}
                <span className="truncate">{editLabel}</span>
              </button>
            </div>
          </div>
          {description ? (
            <span
              className="block truncate text-[10px] text-muted-foreground"
              title={description}
            >
              {description}
            </span>
          ) : null}
        </div>

        <ShareProviderStatusPanel view={providerPanelView} wrapPrimaryLine />

        {directApiUrl ? (
          <div className="grid min-w-0 gap-1.5 border-t border-slate-100 pt-2">
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="shrink-0 text-[10px] font-medium text-muted-foreground">
                {t("modelHub.directEndpoint")}
              </span>
              <code
                className="min-w-0 flex-1 truncate text-[10px] text-foreground"
                title={directApiUrl}
              >
                {directApiUrl}
              </code>
              <button
                type="button"
                data-no-row-drawer
                className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-slate-100 hover:text-foreground"
                title={t("common.copy")}
                aria-label={t("common.copy")}
                onClick={(event) => {
                  event.stopPropagation();
                  void navigator.clipboard.writeText(directApiUrl).then(
                    () => toast.success(t("common.copySuccess")),
                    () => toast.danger(t("common.copyFailed")),
                  );
                }}
              >
                <Copy className="h-3 w-3" />
              </button>
            </div>
            <div className="flex min-w-0 items-center gap-1.5">
              <Route className="h-3 w-3 shrink-0 text-primary" aria-hidden />
              <div className="flex min-w-0 flex-1 flex-wrap gap-1">
                {modelRoutes.length ? modelRoutes.map((route) => (
                  <span
                    key={route.clientId}
                    className="max-w-full truncate rounded-md border border-primary/15 bg-primary/5 px-1.5 py-0.5 text-[9px] font-medium text-primary"
                    title={`${SHARE_APP_LABELS[route.appType]} · ${route.requestedModel || t("modelHub.modelPending")}`}
                  >
                    {SHARE_APP_LABELS[route.appType]} · {route.requestedModel || t("modelHub.modelPending")}
                  </span>
                )) : (
                  <span className="truncate text-[10px] text-muted-foreground">
                    {t("modelHub.noRoutesForShare")}
                  </span>
                )}
              </div>
              {onAddModelRoute ? (
                <button
                  type="button"
                  data-no-row-drawer
                  className="inline-flex h-6 shrink-0 items-center gap-1 rounded-md px-1.5 text-[9px] font-semibold text-primary hover:bg-primary/5"
                  title={t("modelHub.addRouteForShare")}
                  onClick={(event) => {
                    event.stopPropagation();
                    onAddModelRoute(share.shareId);
                  }}
                >
                  <Plus className="h-3 w-3" />
                  {t("modelHub.mapModel")}
                </button>
              ) : null}
            </div>
          </div>
        ) : null}

        <div className="grid gap-2 text-[11px]">
          <div className="grid grid-cols-2 gap-2">
            <div className="min-w-0">
              <span className="block text-muted-foreground">
                {t("dashboard.usage")}
              </span>
              <strong className="tabular-nums">
                {compactTokens(tokensUsed)} /{" "}
                {isUnlimited(tokenLimit) ? "∞" : compactTokens(tokenLimit)}
              </strong>
              {usagePercent != null ? (
                <div className="mt-1 h-1 overflow-hidden rounded-full bg-slate-100">
                  <div
                    className={`h-full rounded-full ${usagePercent >= 90 ? "bg-rose-500" : "bg-primary/70"}`}
                    style={{ width: `${usagePercent}%` }}
                  />
                </div>
              ) : null}
            </div>
            <div className="min-w-0" title={parallelTitle}>
              <span className="block text-muted-foreground">
                {t("dashboard.parallel")}
              </span>
              <strong className="cursor-help tabular-nums">
                {activeRequests}
                <span className="text-muted-foreground">
                  /{isUnlimited(parallelLimit) ? "∞" : parallelLimit || 0}
                </span>
              </strong>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div className="min-w-0">
              <span className="block text-muted-foreground">
                {t("dashboard.expires")}
              </span>
              <strong className="tabular-nums" title={expiryHint}>
                {expiryLabel}
              </strong>
            </div>
            <div className="min-w-0">
              <span className="block text-muted-foreground">
                {t("dashboard.uptime24h")}
              </span>
              <strong
                className={`tabular-nums ${onlineRate < 90 ? "text-amber-700" : "text-emerald-700"}`}
                title={onlineTitle}
              >
                {onlineRate.toFixed(1)}%
              </strong>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div className="min-w-0">
              <span className="block text-muted-foreground">
                {t("dashboard.response")}
              </span>
              <strong
                className={`block truncate tabular-nums font-medium ${latencyResponseToneClass(averageLatency)}`}
                title={formatLatencySeconds(averageLatency)}
              >
                {formatLatencySeconds(averageLatency)}
              </strong>
            </div>
            <div className="min-w-0">
              <span className="block text-muted-foreground">
                {t("dashboard.totalThroughput")}
              </span>
              <strong
                className="block truncate tabular-nums font-medium text-foreground"
                title={t("dashboard.ttftTpsHint", {
                  ttftSamples: performance.ttftSampleCount,
                  tpsSamples: performance.tpsSampleCount,
                })}
              >
                {formatTtftTps(performance)}
              </strong>
            </div>
          </div>
        </div>
      </Card.Content>
    </Card>
  );
});
