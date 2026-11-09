"use client";


import { ClientShareMarketEntryButton } from "./client-share-market";

import { Card, toast } from "@heroui/react";
import { Copy, Eye, Link2, Pencil, Plus, Route } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  operationalReasonLabel,
  operationalStateLabel,
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
  providerQuotaDetails,
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
import { shareEditPendingLabel } from "@/components/dashboard/share-edit/share-edit-section";
import { SubdomainCopyButton } from "@/components/dashboard/subdomain-copy-button";
import { ShareProviderStatusPanel } from "@/components/dashboard/share-provider-status-panel";
import type { ShareRouteChip } from "@/lib/model-routing";

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
  modelRoutes?: ShareRouteChip[];
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
  const healthTone = modelHealthTone(share, undefined, locale);
  const providerPanelView = {
    primaryLine: isApiProvider ? apiEndpoint : quotaStatusLine,
    identityLine: isApiProvider ? "-" : accountLine,
    modelsLine: actualModels,
    primaryTitle: isApiProvider
      ? `${t("dashboard.apiRequestUrl")}: ${apiEndpoint}`
      : providerEnabled && runtime ? providerQuotaDetails(runtime, locale) : quotaStatusLine,
    identityTitle: isApiProvider ? "-" : accountLine,
    modelsTitle: actualModels,
    panelTitle: modelHealthTitle(share, undefined, locale),
    primaryMonospace: isApiProvider,
    toneClassName: healthTone.className,
    healthLabel: "detailLabel" in healthTone ? healthTone.detailLabel : undefined,
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
      ? "border-red-200"
      : summary.state === "reconnecting"
        ? "border-sky-300"
        : summary.state === "degraded"
          ? "border-amber-300"
          : summary.state === "disabled"
            ? "border-border opacity-70"
            : "border-border";
  const statusDot =
    summary.state === "offline"
      ? "bg-red-500"
      : summary.state === "reconnecting"
        ? "bg-sky-500"
        : summary.state === "degraded"
          ? "bg-amber-500"
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
  // Touch screens get 44px targets; a mouse keeps the compact 32px row.
  const actionClass =
    "inline-flex h-8 shrink-0 items-center gap-1 rounded-lg border px-2.5 text-[11px] font-semibold transition-colors duration-200 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-60 pointer-coarse:h-11";
  const iconActionClass =
    "inline-flex size-8 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors duration-200 hover:bg-secondary hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring pointer-coarse:size-11";
  const metricLabel = "block text-[11px] text-muted-foreground";
  const metricValue = "block truncate text-[13px] font-semibold tabular-nums text-foreground";
  // Healthy states need no words; every other state says why in text, not only color.
  const statusLabel =
    summary.state === "online" || summary.state === "available"
      ? ""
      : issue || operationalStateLabel(summary.state, t);
  const statusPill =
    summary.state === "offline"
      ? "bg-red-50 text-red-700"
      : summary.state === "reconnecting"
        ? "bg-sky-50 text-sky-700"
        : summary.state === "degraded"
          ? "bg-amber-50 text-amber-800"
          : "bg-secondary text-muted-foreground";

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
  }, [focus.target?.source, focused]);

  return (
    <Card
      ref={cardRef}
      data-share-id={share.shareId}
      className={`w-full min-w-0 overflow-visible rounded-xl border bg-card p-0 shadow-[0_1px_3px_rgba(0,0,0,0.06)] transition-[border-color,box-shadow,opacity,transform] duration-200 ease-out hover:-translate-y-0.5 hover:border-primary/30 hover:shadow-[0_10px_15px_rgba(0,0,0,0.08)] motion-reduce:transform-none ${focused ? "border-primary ring-2 ring-primary/20" : related ? "border-primary/35" : stateTone} ${dimmed ? "opacity-40" : "opacity-100"}`}
      onMouseDown={(event) => {
        pointerDownRef.current = { x: event.clientX, y: event.clientY };
      }}
      onClick={(event) => {
        if (!shouldOpenShareCard(event, pointerDownRef.current)) return;
        pointerDownRef.current = null;
        openShareDrawer();
      }}
    >
      <Card.Content className="grid min-w-0 cursor-pointer select-text gap-3 p-3.5">
        <div className="grid min-w-0 gap-1.5">
          <div className="flex min-w-0 items-start justify-between gap-2">
            <div className="flex min-w-0 items-center gap-1.5">
              <span
                className={`h-2 w-2 shrink-0 rounded-full ${statusDot}`}
                aria-hidden
              />
              {/* The card opens on click; this is the same action for keyboards and screen readers. */}
              <button
                type="button"
                className="min-w-0 truncate rounded text-left text-sm font-semibold tracking-[-0.01em] text-foreground hover:text-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
                title={title}
                aria-label={t("dashboard.openShareDetails", { name: title })}
                onClick={(event) => {
                  event.stopPropagation();
                  openShareDrawer();
                }}
              >
                {title}
              </button>
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
                className={`${actionClass} border-primary/20 bg-primary/5 text-primary hover:bg-primary/10 disabled:border-border disabled:bg-secondary disabled:text-muted-foreground`}
                onClick={(event) => {
                  event.stopPropagation();
                  if (!connectDisabled) onConnect(share);
                }}
              >
                <Link2 className="h-3.5 w-3.5" aria-hidden />
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
                className={`${actionClass} max-w-[160px] border-border bg-card text-foreground hover:bg-secondary`}
                onClick={(event) => {
                  event.stopPropagation();
                  if (!editPending) onEdit(share);
                }}
              >
                {share.canManage ? (
                  <Pencil className="h-3.5 w-3.5" aria-hidden />
                ) : (
                  <Eye className="h-3.5 w-3.5" aria-hidden />
                )}
                <span className="truncate">{editLabel}</span>
              </button>
            </div>
          </div>
          {statusLabel ? (
            <span className={`inline-flex w-fit max-w-full items-center truncate rounded-full px-2 py-0.5 text-[11px] font-medium ${statusPill}`} title={statusLabel}>
              {statusLabel}
            </span>
          ) : null}
          {description ? (
            <span
              className="block truncate text-xs text-muted-foreground"
              title={description}
            >
              {description}
            </span>
          ) : null}
        </div>

        <ShareProviderStatusPanel view={providerPanelView} wrapPrimaryLine />

        {directApiUrl ? (
          <div className="grid min-w-0 gap-2 border-t border-border pt-2.5">
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="shrink-0 text-[11px] font-medium text-muted-foreground">
                {t("modelHub.directEndpoint")}
              </span>
              <code
                className="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground"
                title={directApiUrl}
              >
                {directApiUrl}
              </code>
              <button
                type="button"
                data-no-row-drawer
                className={iconActionClass}
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
                <Copy className="h-3.5 w-3.5" aria-hidden />
              </button>
            </div>
            <div className="flex min-w-0 items-center gap-1.5">
              <Route className="h-3.5 w-3.5 shrink-0 text-primary" aria-hidden />
              <ul className="flex min-w-0 flex-1 flex-wrap gap-1" aria-label={t("modelHub.routesForShare")}>
                {modelRoutes.length ? modelRoutes.map((chip) => {
                  const label = `${SHARE_APP_LABELS[chip.appType]} · ${chip.requestedModel || t("modelHub.modelPending")} · ${chip.order === 0 ? t("modelHub.candidatePrimary") : t("modelHub.candidateOrder", { order: chip.order + 1 })}`;
                  return (
                    <li
                      key={chip.clientId}
                      className={`max-w-full truncate rounded-md border px-1.5 py-0.5 text-[11px] font-medium ${chip.draft ? "border-dashed border-amber-300 bg-amber-50 text-amber-800" : "border-primary/15 bg-primary/5 text-primary"}`}
                      title={chip.draft ? `${label} · ${t("modelHub.routeUnsaved")}` : label}
                    >
                      {label}
                      {chip.draft ? <span className="font-normal"> · {t("modelHub.routeUnsaved")}</span> : null}
                    </li>
                  );
                }) : (
                  <li className="truncate text-[11px] text-muted-foreground">
                    {t("modelHub.noRoutesForShare")}
                  </li>
                )}
              </ul>
              {onAddModelRoute ? (
                <button
                  type="button"
                  data-no-row-drawer
                  className={`${actionClass} border-transparent px-2 text-primary hover:bg-primary/5`}
                  title={t("modelHub.addRouteForShare")}
                  onClick={(event) => {
                    event.stopPropagation();
                    onAddModelRoute(share.shareId);
                  }}
                >
                  <Plus className="h-3.5 w-3.5" aria-hidden />
                  {t("modelHub.mapModel")}
                </button>
              ) : null}
            </div>
          </div>
        ) : null}

        <div className="grid gap-1.5">
          <div className="flex items-baseline justify-between gap-2">
            <span className={metricLabel}>{t("dashboard.usage")}</span>
            <strong className="text-[13px] font-semibold tabular-nums text-foreground">
              {compactTokens(tokensUsed)}
              <span className="font-normal text-muted-foreground"> / {isUnlimited(tokenLimit) ? "∞" : compactTokens(tokenLimit)}</span>
            </strong>
          </div>
          {usagePercent != null ? (
            <div
              role="progressbar"
              aria-label={t("dashboard.usage")}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(usagePercent)}
              className="h-1.5 overflow-hidden rounded-full bg-secondary"
            >
              <div
                className={`h-full rounded-full ${usagePercent >= 90 ? "bg-red-500" : "bg-gradient-to-r from-accent to-accent-secondary"}`}
                style={{ width: `${usagePercent}%` }}
              />
            </div>
          ) : null}
        </div>
        <dl className="grid grid-cols-2 gap-x-3 gap-y-2 border-t border-border pt-2.5">
          <div className="min-w-0" title={parallelTitle}>
            <dt className={metricLabel}>{t("dashboard.parallel")}</dt>
            <dd className={metricValue}>
              {activeRequests}
              <span className="font-normal text-muted-foreground">
                /{isUnlimited(parallelLimit) ? "∞" : parallelLimit || 0}
              </span>
            </dd>
          </div>
          <div className="min-w-0">
            <dt className={metricLabel}>{t("dashboard.uptime24h")}</dt>
            <dd
              className={`${metricValue} ${observedMinutes > 0 ? (onlineRate < 90 ? "text-amber-700" : "text-emerald-700") : "text-muted-foreground"}`}
              title={onlineTitle}
            >
              {observedMinutes > 0 ? `${onlineRate.toFixed(1)}%` : "—"}
            </dd>
          </div>
          <div className="min-w-0">
            <dt className={metricLabel}>{t("dashboard.response")}</dt>
            <dd
              className={`${metricValue} ${latencyResponseToneClass(averageLatency)}`}
              title={formatLatencySeconds(averageLatency)}
            >
              {formatLatencySeconds(averageLatency)}
            </dd>
          </div>
          <div className="min-w-0">
            <dt className={metricLabel}>{t("dashboard.totalThroughput")}</dt>
            <dd
              className={metricValue}
              title={t("dashboard.ttftTpsHint", {
                ttftSamples: performance.ttftSampleCount,
                tpsSamples: performance.tpsSampleCount,
              })}
            >
              {formatTtftTps(performance)}
            </dd>
          </div>
        </dl>
        <div className="flex min-w-0 items-center justify-between gap-2" data-no-row-drawer>
          <span className="min-w-0 truncate text-[11px] text-muted-foreground" title={expiryHint}>
            {t("dashboard.expires")} · <span className="tabular-nums text-foreground">{expiryLabel}</span>
          </span>
          <ClientShareMarketEntryButton shareId={share.shareId} />
        </div>
      </Card.Content>
    </Card>
  );
});
