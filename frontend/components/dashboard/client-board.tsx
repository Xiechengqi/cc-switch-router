"use client";

import { Button, Card, Drawer, toast } from "@heroui/react";
import { ArrowRightLeft, ChevronDown, Copy, ListFilter, MessageCircle, Plus, ScrollText, Search, Terminal, X } from "lucide-react";
import * as React from "react";
import { CreateClientDialog } from "@/components/dashboard/create-client-dialog";
import { ClientMarketRentalBanner } from "@/components/dashboard/client-market-rental-banner";
import { ShareConnectDialog } from "@/components/dashboard/share-connect-dialog";
import { ClientOnlineHeatmap } from "@/components/dashboard/client-online-heatmap";
import { ShareModelHealthHeatmap } from "@/components/dashboard/share-model-health-heatmap";
import { ShareCard } from "@/components/dashboard/share-card";
import { ClientUpgradeButton } from "@/components/dashboard/client-upgrade-button";
import { ClientRemovalSchedule, clientOperationalSummary, OperationalDiagnosis, operationalReasonLabel, operationalStateLabel, shareIsEnabled, shareOperationalSummary, summarizeShareAvailability, useStableOperationalRanks } from "@/components/dashboard/operational-status";
import { useClientConsole } from "@/components/dashboard/client-console";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import { useDashboardViewState } from "@/components/dashboard/dashboard-view-state";
import { useOperationVerification } from "@/components/dashboard/operation-verification";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  ClientLinkedSharesPanel,
  clientOwnerEmail,
  clientPlatformLabel,
  clientTunnelDisplayUrl,
  ClientProvidersPanel,
  DrawerSection,
  drawerDialogClassName,
  EmptyBlock,
  formatAgeDaysOrHours,
  clientRunningDurationLabel,
  clientRunningDurationMs,
  clientTotalTokensLabel,
  clientTotalTokensUsed,
  ShareEditDialog,
  ShareEmailUsagePanel,
  ShareModelHealthChecks,
  ShareProviderRequestsPanel,
  ShareProvidersPanel,
  shareApiParts,
  sortClients,
} from "@/components/dashboard/data-tables";
import type { ClientMarketRental, DashboardClient, ShareView } from "@/lib/types";
import { formatDateTime, formatRelativeTime, preferredScrollBehavior } from "@/lib/utils";
import { usePersistentState } from "@/lib/use-persistent-state";
import { getMyClientMarketRentals, recordDashboardUxEvent } from "@/lib/api";
import { CompactSelect } from "@/components/common/compact-select";
import { clientWebTerminalUrl } from "@/lib/client-web-view";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { useClientChat } from "@/components/chat/client-chat";
import { useAuth } from "@/components/auth/auth-provider";
import { clientMarketMineHref } from "@/lib/dashboard-nav";
import { SubdomainCopyButton } from "@/components/dashboard/subdomain-copy-button";
import { ClientSubdomainTakeoverDialog } from "@/components/dashboard/client-subdomain-takeover-dialog";
import { ClientLogsDialog } from "@/components/dashboard/client-logs-dialog";
import {
  ModelHubPanel,
  useModelRoutingController,
} from "@/components/dashboard/model-hub-panel";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import {
  clientListTabFromQuery,
  configuredEligibleRouteShareIds,
  consumeModelRouteDeepLink,
  listViewerShares,
  MAX_USER_MODEL_ROUTES,
  modelRouteDeepLinkShareId,
  searchForClientListTab,
  type ClientListTab,
  type DraftModelRoute,
  type ModelRoutingProtocol,
} from "@/lib/model-routing";
import type { UserModelRoutingShare } from "@/lib/types";

function sortShares(shares: ShareView[]) {
  return [...shares].sort((left, right) => {
    return (
      (Date.parse(left.createdAt) || 0) - (Date.parse(right.createdAt) || 0) ||
      (left.subdomain || left.shareName || left.shareId).localeCompare(
        right.subdomain || right.shareName || right.shareId,
        undefined,
        { sensitivity: "base" },
      )
    );
  });
}

const CLIENT_EXPANDED_STORAGE_KEY = "cc_switch_router_client_expanded_v2";

function normalizeEmail(value?: string) {
  return value?.trim().toLowerCase() || "";
}

function includesQuery(values: Array<string | undefined>, query: string) {
  return values.some((value) => String(value || "").toLocaleLowerCase().includes(query));
}

function clientRegionLabel(installation: DashboardClient["installation"]) {
  return installation.countryCode || installation.region || "-";
}

function clientRegionIpTitle(installation: DashboardClient["installation"]) {
  return installation.publicIp ? `IP: ${installation.publicIp}` : undefined;
}

function shouldToggleClientHeader(
  event: React.MouseEvent<HTMLElement>,
  pointerDown: { x: number; y: number } | null,
) {
  if (pointerDown) {
    const deltaX = Math.abs(event.clientX - pointerDown.x);
    const deltaY = Math.abs(event.clientY - pointerDown.y);
    if (deltaX > 4 || deltaY > 4) {
      return false;
    }
  }

  const selection = window.getSelection();
  if (selection && !selection.isCollapsed && selection.toString().trim()) {
    return false;
  }

  const target = event.target as HTMLElement | null;
  if (target?.closest("a,button,input,textarea,select,[data-no-row-drawer]")) {
    return false;
  }

  return true;
}

function ClientConsoleIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className}>
      <rect x="1.5" y="2.5" width="13" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.25" />
      <path d="M5.5 12.5h5" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
      <path d="M8 12.5V14" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
      <path d="M4.5 6.25h7" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
      <path d="M4.5 8.75h4.5" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
    </svg>
  );
}

function ClientHeaderInlineButton({
  label,
  onClick,
  children,
  className,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
  className: string;
}) {
  return (
    <button
      type="button"
      data-no-row-drawer
      aria-label={label}
      onClick={(event) => {
        event.stopPropagation();
        onClick();
      }}
      className={className}
    >
      {children}
    </button>
  );
}

function ClientConsoleButton({ client }: { client: DashboardClient }) {
  const { t } = useLocaleText();
  const { openConsole } = useClientConsole();
  const tunnelUrl = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  if (!tunnelUrl) return null;
  const title = client.clientTunnel?.subdomain || tunnelUrl;
  return (
    <ClientHeaderInlineButton
      label={t("dashboard.clientConsole")}
      onClick={() =>
        openConsole({
          clientId: client.installation.id,
          kind: "console",
          url: tunnelUrl,
          title,
        })
      }
      className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-sky-200 bg-sky-50 px-2.5 text-[11px] font-medium text-sky-700 transition-colors hover:border-sky-300 hover:bg-sky-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
    >
      <ClientConsoleIcon className="h-3 w-3 shrink-0" />
      <span>{t("dashboard.clientConsole")}</span>
    </ClientHeaderInlineButton>
  );
}

function ClientTerminalButton({ client }: { client: DashboardClient }) {
  const { t } = useLocaleText();
  const { openConsole } = useClientConsole();
  const tunnelUrl = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  if (!tunnelUrl) return null;
  const title = client.clientTunnel?.subdomain || tunnelUrl;
  return (
    <ClientHeaderInlineButton
      label={t("dashboard.clientTerminal")}
      onClick={() =>
        openConsole({
          clientId: client.installation.id,
          kind: "terminal",
          url: clientWebTerminalUrl(tunnelUrl),
          title: t("dashboard.clientTerminalTitle", { target: title }),
        })
      }
      className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-emerald-200 bg-emerald-50 px-2.5 text-[11px] font-medium text-emerald-700 transition-colors hover:border-emerald-300 hover:bg-emerald-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
    >
      <Terminal className="h-3 w-3 shrink-0" />
      <span>{t("dashboard.clientTerminal")}</span>
    </ClientHeaderInlineButton>
  );
}

function ClientDetailsButton({ onOpen }: { onOpen: () => void }) {
  const { t } = useLocaleText();
  return (
    <ClientHeaderInlineButton
      label={t("dashboard.details")}
      onClick={onOpen}
      className="inline-flex h-6 shrink-0 items-center rounded-full border border-slate-200 bg-white px-2.5 text-[11px] font-medium text-slate-700 transition-colors hover:border-slate-300 hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
    >
      {t("dashboard.details")}
    </ClientHeaderInlineButton>
  );
}

function ClientLogsButton({ onOpen }: { onOpen: () => void }) {
  const { t } = useLocaleText();
  return (
    <ClientHeaderInlineButton
      label={t("clientLogs.button")}
      onClick={onOpen}
      className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-slate-200 bg-white px-2.5 text-[11px] font-medium text-slate-700 transition-colors hover:border-slate-300 hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
    >
      <ScrollText className="h-3 w-3" aria-hidden />
      <span>{t("clientLogs.button")}</span>
    </ClientHeaderInlineButton>
  );
}

function ClientTakeoverButton({ onOpen }: { onOpen: () => void }) {
  const { t } = useLocaleText();
  return (
    <ClientHeaderInlineButton
      label={t("dashboard.subdomainTakeover.action")}
      onClick={onOpen}
      className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-amber-200 bg-amber-50 px-2.5 text-[11px] font-medium text-amber-800 transition-colors hover:border-amber-300 hover:bg-amber-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
    >
      <ArrowRightLeft className="h-3 w-3" aria-hidden />
      <span>{t("dashboard.subdomainTakeover.action")}</span>
    </ClientHeaderInlineButton>
  );
}

function ClientChatButton({ client }: { client: DashboardClient }) {
  const { t } = useLocaleText();
  const { openChat, unreadByInstallation } = useClientChat();
  const unread = unreadByInstallation.get(client.installation.id) || 0;
  if (!client.chatAvailable) return null;
  return (
    <ClientHeaderInlineButton
      label={t("dashboard.chat")}
      onClick={() => void openChat(client.installation.id)}
      className="relative inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-blue-200 bg-blue-50 px-2.5 text-[11px] font-medium text-blue-700 transition-colors hover:border-blue-300 hover:bg-blue-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
    >
      <MessageCircle className="h-3 w-3" />
      <span>{t("dashboard.chat")}</span>
      {unread > 0 ? (
        <span className="absolute -right-1 -top-1 inline-flex min-h-4 min-w-4 items-center justify-center rounded-full bg-red-500 px-1 text-[9px] font-semibold text-white">
          {unread > 99 ? "99+" : unread}
        </span>
      ) : null}
    </ClientHeaderInlineButton>
  );
}

function ClientCollapseIndicator({ collapsed }: { collapsed: boolean }) {
  return (
    <span
      className="inline-flex shrink-0 items-center text-slate-300 transition-colors duration-200 group-hover/client-header:text-slate-500"
      aria-hidden="true"
    >
      <ChevronDown
        className={`h-[18px] w-[18px] stroke-[1.75] transition-transform duration-200 ease-out ${collapsed ? "" : "rotate-180"}`}
      />
    </span>
  );
}

function shareMatchesQuery(share: ShareView, query: string) {
  const runtimes = Object.values(share.appRuntimes || {});
  const providers = Object.values(share.appProviders || {}).flat();
  return includesQuery([
    share.shareName,
    share.shareId,
    share.subdomain,
    share.ownerEmail,
    share.appType,
    share.providerId,
    share.description,
    ...Object.keys(share.bindings || {}),
    ...Object.values(share.bindings || {}),
    ...runtimes.flatMap((runtime) => [
      runtime?.providerName,
      runtime?.providerType,
      runtime?.kind,
    ]),
    ...providers.flatMap((provider) => [
      provider?.name,
      provider?.providerType,
      provider?.kind,
    ]),
  ], query);
}

const ShareScroller = React.memo(function ShareScroller({
  shares,
  totalCount = shares.length,
  onOpenShare,
  onEditShare,
  onConnectShare,
  routingShareById,
  modelRoutesByShareId,
  onAddModelRoute,
}: {
  shares: ShareView[];
  totalCount?: number;
  onOpenShare: (share: ShareView) => void;
  onEditShare: (share: ShareView) => void;
  onConnectShare: (share: ShareView) => void;
  routingShareById?: ReadonlyMap<string, UserModelRoutingShare>;
  modelRoutesByShareId?: ReadonlyMap<string, DraftModelRoute[]>;
  onAddModelRoute?: (shareId: string) => void;
}) {
  const { t } = useLocaleText();
  if (!shares.length) return <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock>;

  const { enabledCount, availableCount, issueCount, degradedCount } = summarizeShareAvailability(shares);
  const disabledCount = shares.length - enabledCount;

  return (
    <div className="grid min-w-0 gap-3 rounded-lg bg-slate-50/80 p-3">
      <div className="flex items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
          <span className="font-semibold text-foreground">{t("dashboard.shares")}</span>
          <span>{shares.length === totalCount ? shares.length : `${shares.length}/${totalCount}`}</span>
          <span aria-hidden>·</span>
          {enabledCount > 0 ? (
            <>
              <span className="text-emerald-700">{t("dashboard.service")}: {availableCount}/{enabledCount} {t("dashboard.available")}</span>
              {issueCount > 0 ? <span className="text-rose-700">{issueCount} {t("dashboard.unavailable")}</span> : null}
              <span aria-hidden>·</span>
              <span className={degradedCount > 0 ? "text-amber-700" : undefined}>
                {t("dashboard.operationalQuality")}: {degradedCount > 0 ? t("dashboard.warningCount", { count: degradedCount }) : t("dashboard.noWarnings")}
              </span>
            </>
          ) : (
            <span>{t("dashboard.noEnabledShares")}</span>
          )}
          {disabledCount > 0 ? <span>{disabledCount} {t("common.disabled")}</span> : null}
        </div>
      </div>
        <div className="grid min-w-0 grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4" aria-label={t("dashboard.shares")}>
          {shares.map((share) => (
            <ShareCard
              key={share.shareId}
              share={share}
              onOpen={onOpenShare}
              onEdit={onEditShare}
              onConnect={onConnectShare}
              directApiUrl={routingShareById?.get(share.shareId)?.directApiUrl}
              modelRoutes={modelRoutesByShareId?.get(share.shareId)}
              onAddModelRoute={onAddModelRoute}
            />
          ))}
        </div>
    </div>
  );
});

function ClientCard({
  client,
  shares,
  summaryShares,
  onOpenClient,
  onOpenShare,
  onEditShare,
  onConnectShare,
  routingShareById,
  modelRoutesByShareId,
  onAddModelRoute,
  rental,
  onRentalChanged,
  collapsed,
  onToggleCollapsed,
  onOpenTakeover,
  onOpenLogs,
}: {
  client: DashboardClient;
  shares: ShareView[];
  summaryShares?: ShareView[];
  onOpenClient: (client: DashboardClient) => void;
  onOpenShare: (share: ShareView) => void;
  onEditShare: (share: ShareView) => void;
  onConnectShare: (share: ShareView) => void;
  routingShareById?: ReadonlyMap<string, UserModelRoutingShare>;
  modelRoutesByShareId?: ReadonlyMap<string, DraftModelRoute[]>;
  onAddModelRoute?: (shareId: string) => void;
  rental?: ClientMarketRental;
  onRentalChanged: () => Promise<void> | void;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onOpenTakeover?: () => void;
  onOpenLogs?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const tunnelUrl = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  const owner = clientOwnerEmail(client);
  const allShares = summaryShares || shares;
  const onlineRate = client.onlineRate24h || 0;
  const onlineTitle = t("dashboard.uptimeObservation", { healthy: onlineRate.toFixed(1), observed: client.observedMinutes24h || 0, coverage: (client.observationCoverage24h || 0).toFixed(1) });
  const summary = clientOperationalSummary(client, allShares);
  const state = summary.state;
  const shareAvailability = summarizeShareAvailability(allShares);
  const { enabledCount: enabledShareCount, availableCount, issueCount, routeOnlineCount, degradedCount } = shareAvailability;
  const sharesMetricTitle = enabledShareCount
    ? t("dashboard.sharesAvailableDetail", {
        available: availableCount,
        total: enabledShareCount,
        routeOnline: routeOnlineCount,
        warnings: degradedCount,
      })
    : t("dashboard.noEnabledShares");
  const sharesMetricValue = enabledShareCount
    ? `${availableCount}/${enabledShareCount} ${t("dashboard.available")}`
    : t("dashboard.noEnabledShares");
  const sharesMetricTone = !enabledShareCount ? "default" : issueCount ? "danger" : degradedCount ? "warning" : "success";
  const subdomain = client.clientTunnel?.subdomain || "";
  const hasSubdomain = Boolean(subdomain.trim());
  const identity = hasSubdomain ? subdomain : client.installation.id;
  const versionLabel = clientPlatformLabel(client);
  const showRemoval = state === "offline" && !!client.removalAt;
  const borderTone = state === "offline" ? "border-l-rose-500" : state === "reconnecting" ? "border-l-sky-500" : state === "degraded" ? "border-l-amber-400" : "border-l-slate-200";
  const headerPointerDownRef = React.useRef<{ x: number; y: number } | null>(null);

  const openClientDrawer = React.useCallback(() => {
    onOpenClient(client);
  }, [client, onOpenClient]);

  const handleHeaderPointerDown = React.useCallback(
    (event: React.MouseEvent<HTMLElement>) => {
      headerPointerDownRef.current = { x: event.clientX, y: event.clientY };
    },
    [],
  );

  const handleHeaderClick = React.useCallback(
    (event: React.MouseEvent<HTMLElement>) => {
      const pointerDown = headerPointerDownRef.current;
      headerPointerDownRef.current = null;
      if (!shouldToggleClientHeader(event, pointerDown)) return;
      onToggleCollapsed();
    },
    [onToggleCollapsed],
  );

  const handleHeaderDoubleClick = React.useCallback(
    (event: React.MouseEvent<HTMLElement>) => {
      const pointerDown = headerPointerDownRef.current;
      headerPointerDownRef.current = null;
      if (!shouldToggleClientHeader(event, pointerDown)) return;
      openClientDrawer();
    },
    [openClientDrawer],
  );

  return (
    <Card id={`dashboard-client-${client.installation.id}`} className={`min-w-0 max-w-full overflow-hidden rounded-lg border border-l-[3px] bg-white p-0 shadow-sm transition-[border-color,box-shadow] ${borderTone}`}>
      <Card.Content className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-3 p-3.5">
        <div
          className="group/client-header grid min-h-16 cursor-pointer select-text grid-cols-[minmax(0,1fr)_auto] items-start gap-3 rounded-md px-1.5 py-1 outline-none transition-colors hover:bg-primary/[0.03] focus-visible:ring-2 focus-visible:ring-primary/30 xl:grid-cols-[minmax(280px,1.1fr)_minmax(0,2.4fr)_auto] xl:items-center xl:gap-6"
          aria-expanded={!collapsed}
          onMouseDown={handleHeaderPointerDown}
          onClick={handleHeaderClick}
          onDoubleClick={handleHeaderDoubleClick}
        >
          <div className="grid min-w-0 gap-1.5">
            <div className="flex min-w-0 items-center gap-2">
              <span
                className={`h-2 w-2 shrink-0 rounded-full ${state === "offline" ? "bg-rose-500" : state === "reconnecting" ? "bg-sky-500" : state === "degraded" ? "bg-amber-400" : "bg-emerald-500"}`}
                title={operationalStateLabel(state, t)}
                aria-label={operationalStateLabel(state, t)}
              />
              <strong className="min-w-0 truncate text-sm font-semibold text-foreground" title={identity}>{identity}</strong>
              {hasSubdomain ? <SubdomainCopyButton subdomain={subdomain} /> : null}
              {owner && owner !== "-" ? (
                <span className="min-w-0 truncate text-xs text-muted-foreground" title={owner}>
                  {owner}
                </span>
              ) : null}
              <ClientDetailsButton onOpen={openClientDrawer} />
              {summary.primaryReason ? (
                <span
                  className={`min-w-0 truncate text-[11px] font-medium ${state === "offline" ? "text-rose-700" : state === "reconnecting" ? "text-sky-700" : "text-amber-700"}`}
                  title={operationalReasonLabel(summary.primaryReason, t)}
                >
                  {operationalReasonLabel(summary.primaryReason, t)}
                </span>
              ) : null}
              {showRemoval ? <ClientRemovalSchedule removalAt={client.removalAt} className="text-[11px]" /> : null}
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-2 pl-4">
              <ClientMarketRentalBanner
                rental={rental}
                onChanged={onRentalChanged}
                readOnly
                resumeRelease={false}
                showSchedule={false}
                manageHref={rental ? clientMarketMineHref(rental.installationId) : undefined}
              />
              {tunnelUrl ? <ClientConsoleButton client={client} /> : null}
              {tunnelUrl ? <ClientTerminalButton client={client} /> : null}
              <ClientUpgradeButton client={client} />
              {onOpenTakeover ? <ClientTakeoverButton onOpen={onOpenTakeover} /> : null}
              {client.logCollectionEnabled && onOpenLogs ? <ClientLogsButton onOpen={onOpenLogs} /> : null}
              <ClientChatButton client={client} />
            </div>
          </div>

          <div className={`order-3 col-span-2 grid min-w-0 grid-cols-2 gap-3 sm:grid-cols-4 xl:order-none xl:col-span-1 ${showRemoval ? "xl:grid-cols-8" : "xl:grid-cols-7"}`}>
            <Metric
              label={t("dashboard.region")}
              value={clientRegionLabel(client.installation)}
              title={clientRegionIpTitle(client.installation)}
            />
            <Metric
              label={t("dashboard.runningDuration")}
              value={clientRunningDurationLabel(client, locale)}
              title={t("dashboard.clientRunningSince", { date: formatDateTime(client.installation.createdAt) })}
              preserveValue
            />
            <Metric
              label={t("dashboard.totalTokens")}
              value={clientTotalTokensLabel(allShares)}
              title={t("dashboard.clientTotalTokensDetail", {
                count: allShares.length,
                total: new Intl.NumberFormat(locale).format(clientTotalTokensUsed(allShares)),
              })}
              preserveValue
            />
            <Metric label={t("dashboard.version")} value={versionLabel} title={client.installation.appVersion || versionLabel} preserveValue />
            <Metric label={t("dashboard.uptime24h")} value={`${onlineRate.toFixed(1)}%`} title={onlineTitle} tone={onlineRate < 90 ? "warning" : "success"} />
            <Metric label={t("dashboard.shares")} value={sharesMetricValue} title={sharesMetricTitle} tone={sharesMetricTone} />
            <Metric label={t("dashboard.lastSeen")} value={formatRelativeTime(client.installation.lastSeenAt, locale)} tone={state === "offline" ? "danger" : "default"} />
            {showRemoval ? (
              <Metric
                label={t("dashboard.removalAt")}
                value={formatRelativeTime(client.removalAt, locale)}
                title={formatDateTime(client.removalAt)}
                tone="danger"
              />
            ) : null}
          </div>

          <div className="flex items-center justify-end self-center pl-1">
            <ClientCollapseIndicator collapsed={collapsed} />
          </div>
        </div>

        {!collapsed ? (
          <ShareScroller
            shares={shares}
            totalCount={allShares.length}
            onOpenShare={onOpenShare}
            onEditShare={onEditShare}
            onConnectShare={onConnectShare}
            routingShareById={routingShareById}
            modelRoutesByShareId={modelRoutesByShareId}
            onAddModelRoute={onAddModelRoute}
          />
        ) : null}
      </Card.Content>
    </Card>
  );
}

function Metric({ label, value, title, tone = "default", preserveValue = false }: { label: string; value: string; title?: string; tone?: "default" | "success" | "warning" | "danger"; preserveValue?: boolean }) {
  const color = tone === "success" ? "text-emerald-700" : tone === "warning" ? "text-amber-700" : tone === "danger" ? "text-rose-700" : "text-foreground";
  return (
    <div className="grid min-w-0 gap-1" title={title}>
      <span className="font-mono text-[9px] uppercase tracking-[0.12em] text-slate-400">{label}</span>
      <strong className={`text-xs font-semibold ${preserveValue ? "font-mono whitespace-nowrap tabular-nums" : "truncate"} ${color}`}>{value}</strong>
    </div>
  );
}

export function ClientBoard({
  clients,
  shares,
  onChanged,
}: {
  clients: DashboardClient[];
  shares: ShareView[];
  onChanged?: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const sessionEmail = normalizeEmail(session?.user?.email);
  const hasViewerIdentity = authed && !!sessionEmail;
  const pathname = usePathname() || "/clients/";
  const router = useRouter();
  const searchParams = useSearchParams();
  const searchString = searchParams.toString();
  const focus = useDashboardFocus();
  const { issuesOnly, setIssuesOnly, regionFilters, setRegionFilters, clearRegionFilters } = useDashboardViewState();
  const { trackOperation } = useOperationVerification();
  const [selectedClientId, setSelectedClientId] = React.useState("");
  const [selectedShareId, setSelectedShareId] = React.useState("");
  const [editingShare, setEditingShare] = React.useState<ShareView | null>(null);
  const [connectShare, setConnectShare] = React.useState<ShareView | null>(null);
  const [createClientOpen, setCreateClientOpen] = React.useState(false);
  const [takeoverTargetId, setTakeoverTargetId] = React.useState("");
  const [logClientId, setLogClientId] = React.useState("");
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const filtersRef = React.useRef<HTMLDivElement>(null);
  const [query, setQuery] = React.useState("");
  const [statusFilterRaw, setStatusFilter] = usePersistentState<ClientListTab>("cc_switch_router_client_status_v1", "all");
  const statusFilter = clientListTabFromQuery(
    statusFilterRaw,
    searchParams.get("tab"),
    hasViewerIdentity,
  );
  const modelRouting = useModelRoutingController(
    hasViewerIdentity && statusFilter === "mine",
  );
  const [sortOrder, setSortOrder] = usePersistentState("cc_switch_router_client_sort_v1", "tokens");
  const [expandedClientIds, setExpandedClientIds] = usePersistentState<string[] | null>(
    CLIENT_EXPANDED_STORAGE_KEY,
    null,
  );
  const [marketRentals, setMarketRentals] = React.useState<Map<string, ClientMarketRental>>(new Map());
  const lastLocatedFocusRef = React.useRef("");
  const consumedModelRouteDeepLinkRef = React.useRef("");

  const selectClientListTab = React.useCallback((tab: ClientListTab) => {
    if (tab !== "mine") setStatusFilter(tab);
    const nextSearch = searchForClientListTab(searchString, tab);
    const href = `${pathname}${nextSearch ? `?${nextSearch}` : ""}`;
    router.replace(href, { scroll: false });
  }, [pathname, router, searchString, setStatusFilter]);

  const loadMarketRentals = React.useCallback(async () => {
    if (!authed) {
      setMarketRentals(new Map());
      return;
    }
    try {
      const records = await getMyClientMarketRentals();
      setMarketRentals(new Map(records.map((record) => [record.installationId, record])));
    } catch {
      // Rental metadata is supplementary; authorization/API errors must not hide Clients.
    }
  }, [authed]);

  React.useEffect(() => {
    void loadMarketRentals();
    if (!authed) return;
    const timer = window.setInterval(() => void loadMarketRentals(), 20_000);
    return () => window.clearInterval(timer);
  }, [authed, loadMarketRentals]);

  const refreshRentalsAndDashboard = React.useCallback(async () => {
    await Promise.all([loadMarketRentals(), Promise.resolve(onChanged?.())]);
  }, [loadMarketRentals, onChanged]);

  React.useEffect(() => {
    if (sortOrder === "registered") setSortOrder("running");
  }, [setSortOrder, sortOrder]);

  React.useEffect(() => {
    if (searchParams.get("tab") !== "mine" && statusFilterRaw !== statusFilter) {
      setStatusFilter(statusFilter);
    }
  }, [searchParams, setStatusFilter, statusFilter, statusFilterRaw]);

  React.useEffect(() => {
    if (issuesOnly) selectClientListTab("all");
  }, [issuesOnly, selectClientListTab]);

  const sortedClients = React.useMemo(() => sortClients(clients), [clients]);
  const defaultExpandedClientId = sortedClients.reduce<DashboardClient | undefined>((best, client) => {
    return !best || (client.shareIds || []).length > (best.shareIds || []).length ? client : best;
  }, undefined)?.installation.id;
  const expandedClientIdSet = React.useMemo(
    () => new Set(expandedClientIds ?? (defaultExpandedClientId ? [defaultExpandedClientId] : [])),
    [defaultExpandedClientId, expandedClientIds],
  );
  const shareById = React.useMemo(() => new Map(shares.map((share) => [share.shareId, share])), [shares]);
  const routedEligibleShareIds = React.useMemo(
    () => configuredEligibleRouteShareIds(modelRouting.profile),
    [modelRouting.profile],
  );
  const mineShares = React.useMemo(() => {
    if (!hasViewerIdentity) return [];
    return sortShares(listViewerShares(
      shares,
      sortedClients,
      sessionEmail,
      routedEligibleShareIds,
    ));
  }, [hasViewerIdentity, routedEligibleShareIds, sessionEmail, shares, sortedClients]);
  const clientById = React.useMemo(() => new Map(clients.map((client) => [client.installation.id, client])), [clients]);
  const canViewClientLogs = React.useCallback(
    (client: DashboardClient) => Boolean(client.logCollectionEnabled),
    [],
  );
  const takeoverSourcesFor = React.useCallback(
    (target: DashboardClient) => {
      if (!sessionEmail || target.clientTunnel?.routeState !== "active" || !target.clientTunnel.enabled) return [];
      const targetOwner = normalizeEmail(target.clientTunnel.ownerEmail || target.installation.ownerEmail);
      if (targetOwner !== sessionEmail) return [];
      return clients.filter((candidate) => {
        if (candidate.installation.id === target.installation.id || !candidate.clientTunnel?.subdomain) return false;
        const owner = normalizeEmail(candidate.clientTunnel.ownerEmail || candidate.installation.ownerEmail);
        return owner === sessionEmail;
      });
    },
    [clients, sessionEmail],
  );
  const linkedShareIds = React.useMemo(() => {
    const ids = new Set<string>();
    clients.forEach((client) => (client.shareIds || []).forEach((shareId) => ids.add(shareId)));
    return ids;
  }, [clients]);

  const sharesForClient = React.useCallback(
    (client?: DashboardClient) => sortShares((client?.shareIds || []).map((id) => shareById.get(id)).filter((share): share is ShareView => !!share)),
    [shareById],
  );
  const orphanShares = React.useMemo(() => sortShares(shares.filter((share) => !linkedShareIds.has(share.shareId))), [linkedShareIds, shares]);

  const regions = React.useMemo(() => Array.from(new Set(
    clients.map((client) => client.installation.countryCode || client.installation.region || "").filter(Boolean),
  )).sort((left, right) => left.localeCompare(right)), [clients]);
  const stableStateRanks = useStableOperationalRanks(sortedClients.map((client) => ({
    id: client.installation.id,
    state: clientOperationalSummary(client, sharesForClient(client)).state,
  })));

  const clientRows = React.useMemo(() => {
    if (statusFilter === "mine") return [];
    const normalizedQuery = query.trim().toLocaleLowerCase();
    const stableOrder = new Map(sortedClients.map((client, index) => [client.installation.id, index]));
    const rows = sortedClients.map((client) => {
      const allShares = sharesForClient(client);
      const clientMatch = !normalizedQuery || includesQuery([
        client.installation.id,
        client.installation.ownerEmail,
        client.installation.countryCode,
        client.installation.region,
        client.installation.platform,
        client.installation.appVersion,
        client.clientTunnel?.subdomain,
        client.clientTunnel?.tunnelUrl,
        client.clientTunnel?.ownerEmail,
      ], normalizedQuery);
      const matchedShares = clientMatch ? allShares : allShares.filter((share) => shareMatchesQuery(share, normalizedQuery));
      return {
        client,
        shares: matchedShares,
        allShares,
        state: clientOperationalSummary(client, allShares).state,
        clientMatch,
        runningDurationMs: clientRunningDurationMs(client),
        totalTokens: clientTotalTokensUsed(allShares),
      };
    }).filter((row) => {
      if (normalizedQuery && row.shares.length === 0 && !row.clientMatch) return false;
      const region = row.client.installation.countryCode || row.client.installation.region || "";
      if (regionFilters.length > 0 && !regionFilters.includes(region)) return false;
      if (statusFilter !== "all" && row.state !== statusFilter) return false;
      if (issuesOnly && row.state === "online") return false;
      return true;
    });
    rows.sort((left, right) => {
      if (sortOrder === "name") {
        const leftName = left.client.clientTunnel?.subdomain || left.client.installation.id;
        const rightName = right.client.clientTunnel?.subdomain || right.client.installation.id;
        return leftName.localeCompare(rightName, undefined, { sensitivity: "base" });
      }
      if (sortOrder === "recent") {
        return (Date.parse(right.client.installation.lastSeenAt) || 0) - (Date.parse(left.client.installation.lastSeenAt) || 0);
      }
      if (sortOrder === "running") {
        return (
          right.runningDurationMs - left.runningDurationMs ||
          (stableOrder.get(left.client.installation.id) || 0) - (stableOrder.get(right.client.installation.id) || 0)
        );
      }
      if (sortOrder === "tokens") {
        return (
          right.totalTokens - left.totalTokens ||
          (stableOrder.get(left.client.installation.id) || 0) - (stableOrder.get(right.client.installation.id) || 0)
        );
      }
      if (sortOrder === "shares") return right.allShares.length - left.allShares.length;
      if (focus.target) return (stableOrder.get(left.client.installation.id) || 0) - (stableOrder.get(right.client.installation.id) || 0);
      return (stableStateRanks.get(left.client.installation.id) || 0) - (stableStateRanks.get(right.client.installation.id) || 0) || (stableOrder.get(left.client.installation.id) || 0) - (stableOrder.get(right.client.installation.id) || 0);
    });
    return rows;
  }, [focus.target, issuesOnly, query, regionFilters, sharesForClient, sortOrder, sortedClients, stableStateRanks, statusFilter]);

  const clientSummary = React.useMemo(() => {
    const states = sortedClients.map((client) => clientOperationalSummary(client, sharesForClient(client)).state);
    return {
      mine: mineShares.length,
      online: states.filter((state) => state === "online").length,
      reconnecting: states.filter((state) => state === "reconnecting").length,
      degraded: states.filter((state) => state === "degraded").length,
      offline: states.filter((state) => state === "offline").length,
      issues: states.filter((state) => state !== "online").length,
    };
  }, [mineShares.length, sharesForClient, sortedClients]);

  const visibleMineShares = React.useMemo(() => {
    if (statusFilter !== "mine") return [];
    const normalizedQuery = query.trim().toLocaleLowerCase();
    const hostByShareId = new Map<string, DashboardClient>();
    for (const client of sortedClients) {
      for (const shareId of client.shareIds || []) {
        if (!hostByShareId.has(shareId)) hostByShareId.set(shareId, client);
      }
    }
    const filtered = mineShares.filter((share) => {
      if (normalizedQuery && !shareMatchesQuery(share, normalizedQuery)) return false;
      if (regionFilters.length > 0) {
        const host = hostByShareId.get(share.shareId);
        const region = host?.installation.countryCode || host?.installation.region || "";
        if (!regionFilters.includes(region)) return false;
      }
      return true;
    });
    const order = new Map(mineShares.map((share, index) => [share.shareId, index]));
    return [...filtered].sort((left, right) => {
      if (sortOrder === "name") {
        const leftName = left.subdomain || left.shareName || left.shareId;
        const rightName = right.subdomain || right.shareName || right.shareId;
        return leftName.localeCompare(rightName, undefined, { sensitivity: "base" });
      }
      if (sortOrder === "recent" || sortOrder === "running") {
        return (Date.parse(right.createdAt) || 0) - (Date.parse(left.createdAt) || 0);
      }
      if (sortOrder === "tokens") {
        return (right.tokensUsed || 0) - (left.tokensUsed || 0);
      }
      return (order.get(left.shareId) || 0) - (order.get(right.shareId) || 0);
    });
  }, [mineShares, query, regionFilters, sortOrder, sortedClients, statusFilter]);

  const visibleOrphanShares = React.useMemo(() => {
    if (statusFilter === "mine") return [];
    const normalizedQuery = query.trim().toLocaleLowerCase();
    return orphanShares.filter((share) => {
      if (normalizedQuery && !shareMatchesQuery(share, normalizedQuery)) return false;
      const shareState = shareOperationalSummary(share).state;
      if (statusFilter === "online" && shareState !== "online") return false;
      if (statusFilter === "reconnecting" && shareState !== "reconnecting") return false;
      if (statusFilter === "degraded" && shareState !== "degraded") return false;
      if (statusFilter === "offline" && shareState !== "offline") return false;
      if (issuesOnly && shareState === "online") return false;
      return true;
    });
  }, [issuesOnly, orphanShares, query, statusFilter]);
  const openClient = React.useCallback((client: DashboardClient) => {
    setSelectedClientId(client.installation.id);
    focus.openDrawer("client", client.installation.id);
    void recordDashboardUxEvent({ eventType: "drawer_opened", source: "client-board", targetType: "client" });
  }, [focus]);
  const closeClientDrawer = React.useCallback((open: boolean) => { if (!open) { setSelectedClientId(""); focus.closeDrawer(); } }, [focus]);
  const openShare = React.useCallback((share: ShareView) => {
    setSelectedShareId(share.shareId);
    focus.openDrawer("share", share.shareId);
    void recordDashboardUxEvent({ eventType: "drawer_opened", source: "client-board", targetType: "share" });
  }, [focus]);
  const closeShareDrawer = React.useCallback((open: boolean) => {
    if (open) return;
    const closingShareId = selectedShareId;
    setSelectedShareId("");
    focus.closeDrawer();
    if (focus.target?.kind === "share" && focus.target.id === closingShareId) {
      focus.clearFocus();
    }
  }, [focus, selectedShareId]);
  const openEditShare = React.useCallback((share: ShareView) => setEditingShare(share), []);
  const closeEditShare = React.useCallback(() => setEditingShare(null), []);
  const openConnectShare = React.useCallback((share: ShareView) => setConnectShare(share), []);
  const closeConnectDialog = React.useCallback((open: boolean) => { if (!open) setConnectShare(null); }, []);
  const handleSaved = React.useCallback(async ({ appliedSynchronously }: { appliedSynchronously: boolean }) => {
    if (editingShare) trackOperation({ kind: "share", id: editingShare.shareId, requireHealthyRoute: true });
    await onChanged?.();
    if (!appliedSynchronously) toast.info(t("dashboard.shareEditQueued"));
  }, [editingShare, onChanged, t, trackOperation]);
  const toggleClientExpanded = React.useCallback((clientId: string) => {
    setExpandedClientIds((current) => {
      const next = new Set(current ?? (defaultExpandedClientId ? [defaultExpandedClientId] : []));
      if (next.has(clientId)) next.delete(clientId);
      else next.add(clientId);
      return Array.from(next);
    });
  }, [defaultExpandedClientId, setExpandedClientIds]);

  React.useEffect(() => {
    if (!focus.target || focus.target.source === "client-board" || focus.target.source === "map") return;
    const focusKey = `${focus.target.kind}:${focus.target.id}`;
    if (lastLocatedFocusRef.current === focusKey) return;
    lastLocatedFocusRef.current = focusKey;
    const clientId = focus.target.kind === "client"
      ? focus.target.id
      : Array.from(focus.relatedClientIds)[0];
    if (!clientId) return;
    window.requestAnimationFrame(() => {
      document.getElementById(`dashboard-client-${clientId}`)?.scrollIntoView({ behavior: preferredScrollBehavior(), block: "center" });
    });
  }, [focus.relatedClientIds, focus.target]);

  React.useEffect(() => {
    if (focus.drawerTarget?.kind === "client" && clientById.has(focus.drawerTarget.id)) setSelectedClientId(focus.drawerTarget.id);
    if (focus.drawerTarget?.kind === "share" && shareById.has(focus.drawerTarget.id)) setSelectedShareId(focus.drawerTarget.id);
  }, [clientById, focus.drawerTarget, shareById]);

  const selectedClient = selectedClientId ? clientById.get(selectedClientId) || null : null;
  const takeoverTarget = takeoverTargetId ? clientById.get(takeoverTargetId) || null : null;
  const logClientCandidate = logClientId ? clientById.get(logClientId) || null : null;
  const logClient = logClientCandidate && canViewClientLogs(logClientCandidate) ? logClientCandidate : null;
  const takeoverSources = React.useMemo(
    () => (takeoverTarget ? takeoverSourcesFor(takeoverTarget) : []),
    [takeoverSourcesFor, takeoverTarget],
  );
  const selectedShare = selectedShareId ? shareById.get(selectedShareId) || null : null;
  const editingShareId = editingShare?.shareId || "";
  const currentEditingShare = editingShareId ? shareById.get(editingShareId) || editingShare : null;
  const connectShareId = connectShare?.shareId || "";
  const currentConnectShare = connectShareId ? shareById.get(connectShareId) || null : null;
  const selectedClientUrl = clientTunnelDisplayUrl(selectedClient?.clientTunnel?.tunnelUrl);
  const selectedClientSummary = selectedClient
    ? clientOperationalSummary(selectedClient, sharesForClient(selectedClient))
    : null;
  const selectedApi = shareApiParts(selectedShare ?? undefined);

  React.useEffect(() => {
    if (logClientId && !logClient) setLogClientId("");
  }, [logClient, logClientId]);

  React.useEffect(() => {
    if (connectShareId && !shareById.has(connectShareId)) setConnectShare(null);
  }, [connectShareId, shareById]);

  React.useEffect(() => {
    if (!filtersOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Element | null;
      if (!target) return;
      if (filtersRef.current?.contains(target)) return;
      // HeroUI Select popovers portal outside the filter panel.
      if (target.closest?.('[role="listbox"], [data-slot="popover"], [data-rac]')) return;
      setFiltersOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [filtersOpen]);

  const [hubForceExpanded, setHubForceExpanded] = React.useState(false);
  const [hubFocusProtocol, setHubFocusProtocol] = React.useState<ModelRoutingProtocol | null>(null);
  const addModelRouteForShare = React.useCallback((shareId: string) => {
    if (modelRouting.routes.length >= MAX_USER_MODEL_ROUTES) {
      toast.danger(t("modelHub.validationTooMany"));
      return;
    }
    const appType = modelRouting.addRouteForShare(shareId);
    if (appType) setHubFocusProtocol(appType);
    setHubForceExpanded(true);
    window.requestAnimationFrame(() => {
      document.getElementById("model-hub")?.scrollIntoView({
        behavior: preferredScrollBehavior(),
        block: "start",
      });
    });
  }, [modelRouting.addRouteForShare, modelRouting.routes.length, t]);

  React.useEffect(() => {
    const shareId = modelRouteDeepLinkShareId(searchString);
    if (!shareId) {
      consumedModelRouteDeepLinkRef.current = "";
      return;
    }
    if (!modelRouting.profile) return;
    const actionKey = `${shareId}:${searchString}`;
    if (consumedModelRouteDeepLinkRef.current === actionKey) return;
    consumedModelRouteDeepLinkRef.current = actionKey;
    if (
      modelRouting.profile.eligibleShares.some(
        (share) => share.shareId === shareId,
      )
    ) {
      addModelRouteForShare(shareId);
    } else {
      toast.danger(t("modelHub.targetUnavailable"));
    }
    const nextSearch = consumeModelRouteDeepLink(searchString);
    router.replace(
      `${pathname}${nextSearch ? `?${nextSearch}` : ""}`,
      { scroll: false },
    );
  }, [
    addModelRouteForShare,
    modelRouting.profile,
    pathname,
    router,
    searchString,
    t,
  ]);

  const routingShareById = React.useMemo(
    () => new Map(
      (modelRouting.profile?.eligibleShares || []).map((share) => [
        share.shareId,
        share,
      ]),
    ),
    [modelRouting.profile?.eligibleShares],
  );
  const modelRoutesByShareId = React.useMemo(() => {
    const byShare = new Map<string, DraftModelRoute[]>();
    for (const route of modelRouting.routes) {
      const current = byShare.get(route.targetShareId) || [];
      byShare.set(route.targetShareId, [...current, route]);
    }
    return byShare;
  }, [modelRouting.routes]);

  const activeFilterCount = regionFilters.length;
  const clientTabs: Array<{ value: ClientListTab; label: string; count: number }> = [
    ...(hasViewerIdentity ? [{ value: "mine" as const, label: t("dashboard.mine"), count: clientSummary.mine }] : []),
    { value: "all", label: t("dashboard.all"), count: sortedClients.length },
    { value: "online", label: t("common.online"), count: clientSummary.online },
    { value: "reconnecting", label: t("dashboard.reconnecting"), count: clientSummary.reconnecting },
    { value: "degraded", label: t("dashboard.degraded"), count: clientSummary.degraded },
    { value: "offline", label: t("common.offline"), count: clientSummary.offline },
  ];
  const mineIsEmpty = statusFilter === "mine" && mineShares.length === 0;
  const mineFilterEmpty = statusFilter === "mine" && !mineIsEmpty && visibleMineShares.length === 0;

  return (
    <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4">
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-4">
        <div className="flex w-full min-w-0 flex-wrap items-center gap-3 sm:w-auto">
          <div className="inline-flex max-w-full overflow-x-auto rounded-lg bg-slate-100 p-1 text-[11px]">
            {clientTabs.map(({ value, label, count }) => (
              <button key={value} type="button" aria-pressed={statusFilter === value} onClick={() => { selectClientListTab(value); if (value === "mine" || value === "online") setIssuesOnly(false); }} className={`rounded-md px-2.5 py-1.5 transition-colors ${statusFilter === value ? "bg-white font-medium text-foreground shadow-sm" : value === "mine" ? "text-primary" : value === "offline" ? "text-rose-700" : value === "reconnecting" ? "text-sky-700" : value === "degraded" ? "text-amber-700" : "text-muted-foreground"}`}>{label} · {count}</button>
            ))}
          </div>
          {statusFilter === "mine" ? null : (
            <Button variant="outline" size="sm" className="h-7 px-3 text-xs" onClick={() => setCreateClientOpen(true)}>
              <Plus className="h-3.5 w-3.5" />
              {t("createClient.newClient")}
            </Button>
          )}
        </div>
        <div className="flex w-full min-w-0 items-center gap-2 sm:w-auto">
          <label className="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-md border bg-white px-3 text-sm focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/10 sm:min-w-64">
            <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
            <input value={query} onChange={(event) => setQuery(event.target.value)} className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground" placeholder={t(statusFilter === "mine" ? "dashboard.searchShares" : "dashboard.searchClients")} aria-label={t(statusFilter === "mine" ? "dashboard.searchShares" : "dashboard.searchClients")} />
            {query ? (
              <button
                type="button"
                className="rounded p-0.5 text-muted-foreground hover:bg-slate-100 hover:text-foreground"
                aria-label={t("common.close")}
                onClick={() => setQuery("")}
              >
                <X className="h-3.5 w-3.5" />
              </button>
            ) : null}
          </label>
          <div ref={filtersRef} className="relative shrink-0">
            <Button
              variant="outline"
              size="sm"
              className="h-9 gap-1.5 px-3 text-xs"
              aria-expanded={filtersOpen}
              aria-haspopup="dialog"
              onClick={() => setFiltersOpen((open) => !open)}
            >
              <ListFilter className="h-3.5 w-3.5" />
              {activeFilterCount
                ? t("dashboard.filterActive", { count: activeFilterCount })
                : t("dashboard.filter")}
              <ChevronDown className={`h-3.5 w-3.5 text-muted-foreground transition-transform ${filtersOpen ? "rotate-180" : ""}`} />
            </Button>
            {filtersOpen ? (
              <div
                role="dialog"
                aria-label={t("dashboard.filter")}
                className="absolute right-0 z-30 mt-2 w-[min(18rem,calc(100vw-2rem))] rounded-xl border border-border bg-card p-3 shadow-lg"
              >
                <div className="grid gap-3">
                  <label className="grid gap-1.5 text-xs text-muted-foreground">
                    {t("dashboard.region")}
                    <CompactRegionMultiSelect
                      values={regionFilters}
                      onChange={(value) => {
                        setRegionFilters(value);
                        void recordDashboardUxEvent({ eventType: "filter_applied", source: "client-board", targetType: "client" });
                      }}
                      options={regions.map((region) => ({ value: region, label: region }))}
                      allLabel={t("dashboard.allRegions")}
                      moreLabel={(count) => t("dashboard.regionsMore", { count })}
                      clearLabel={t("dashboard.clearRegionSelection")}
                      ariaLabel={t("dashboard.filterRegion")}
                      className="w-full"
                    />
                  </label>
                  <label className="grid gap-1.5 text-xs text-muted-foreground">
                    {t("dashboard.filterGeneral")}
                    <CompactSelect
                      value={sortOrder === "registered" ? "running" : sortOrder}
                      onChange={(value) => {
                        setSortOrder(value);
                        void recordDashboardUxEvent({ eventType: "filter_applied", source: "client-board", targetType: "client" });
                      }}
                      options={[
                        { value: "issues", label: t("dashboard.sortIssues") },
                        { value: "name", label: t("dashboard.sortName") },
                        { value: "recent", label: t("dashboard.sortRecent") },
                        { value: "running", label: t("dashboard.sortRunning") },
                        { value: "tokens", label: t("dashboard.sortTokens") },
                        { value: "shares", label: t("dashboard.sortShares") },
                      ]}
                      ariaLabel={t("dashboard.sortBy")}
                      className="w-full"
                    />
                  </label>
                  {activeFilterCount ? (
                    <button
                      type="button"
                      className="justify-self-start text-xs font-medium text-primary hover:underline"
                      onClick={() => {
                        clearRegionFilters();
                        void recordDashboardUxEvent({ eventType: "filter_applied", source: "client-board", targetType: "client" });
                      }}
                    >
                      {t("dashboard.clearFilters")}
                    </button>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
        </div>
      </div>

      {statusFilter === "mine" ? (
        <ModelHubPanel controller={modelRouting} forceExpanded={hubForceExpanded} focusProtocol={hubFocusProtocol} />
      ) : null}

      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4">
        {statusFilter === "mine" ? (
          visibleMineShares.length ? (
            <ShareScroller
              shares={visibleMineShares}
              totalCount={mineShares.length}
              onOpenShare={openShare}
              onEditShare={openEditShare}
              onConnectShare={openConnectShare}
              routingShareById={routingShareById}
              modelRoutesByShareId={modelRoutesByShareId}
              onAddModelRoute={addModelRouteForShare}
            />
          ) : (
            <EmptyBlock>
              <div className="grid justify-items-center gap-2">
                <span>{mineIsEmpty ? t("dashboard.noMyShares") : t("dashboard.noFilterResults")}</span>
                {mineFilterEmpty || mineIsEmpty ? (
                  <button
                    type="button"
                    className="text-xs font-medium text-primary hover:underline"
                    onClick={() => {
                      setQuery("");
                      if (mineIsEmpty) selectClientListTab("all");
                      clearRegionFilters();
                      setIssuesOnly(false);
                    }}
                  >
                    {mineIsEmpty ? t("dashboard.showAll") : t("dashboard.clearFilters")}
                  </button>
                ) : null}
              </div>
            </EmptyBlock>
          )
        ) : clientRows.length ? clientRows.map(({ client, shares: visibleShares, allShares }) => (
          <ClientCard key={client.installation.id} client={client} shares={visibleShares} summaryShares={allShares} onOpenClient={openClient} onOpenShare={openShare} onEditShare={openEditShare} onConnectShare={openConnectShare} rental={marketRentals.get(client.installation.id)} onRentalChanged={refreshRentalsAndDashboard} collapsed={!query && !expandedClientIdSet.has(client.installation.id)} onToggleCollapsed={() => toggleClientExpanded(client.installation.id)} onOpenTakeover={takeoverSourcesFor(client).length ? () => setTakeoverTargetId(client.installation.id) : undefined} onOpenLogs={canViewClientLogs(client) ? () => setLogClientId(client.installation.id) : undefined} />
        )) : (
          <EmptyBlock>
            <div className="grid justify-items-center gap-2">
              <span>{sortedClients.length ? t("dashboard.noFilterResults") : t("dashboard.noClients")}</span>
              {sortedClients.length ? <button type="button" className="text-xs font-medium text-primary hover:underline" onClick={() => { setQuery(""); selectClientListTab("all"); clearRegionFilters(); setIssuesOnly(false); }}>{t("dashboard.clearFilters")}</button> : null}
            </div>
          </EmptyBlock>
        )}
      </div>

      {visibleOrphanShares.length ? (
        <div className="grid gap-3">
          <div className="font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
            {t("dashboard.unlinkedClients")} <span className="font-semibold text-foreground">{visibleOrphanShares.length}</span>
          </div>
          <Card className="rounded-lg border bg-white p-0 shadow-sm">
            <Card.Content className="p-4">
              <ShareScroller shares={visibleOrphanShares} onOpenShare={openShare} onEditShare={openEditShare} onConnectShare={openConnectShare} />
            </Card.Content>
          </Card>
        </div>
      ) : null}

      <Drawer.Backdrop isOpen={!!selectedClient} onOpenChange={closeClientDrawer}>
          <Drawer.Content placement="right">
            <Drawer.Dialog className={drawerDialogClassName}>
              <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Drawer.Header>
                <Drawer.Heading className="pr-8 text-base">{t("dashboard.client")}</Drawer.Heading>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selectedClient ? (
                  <div className="grid gap-5">
                    {selectedClientSummary && !["online", "available"].includes(selectedClientSummary.state) ? (
                      <OperationalDiagnosis summary={selectedClientSummary} kind="client" removalAt={selectedClient.removalAt} />
                    ) : null}
                    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
                      <dt className="text-slate-500">URL</dt>
                      <dd className="flex min-w-0 items-start gap-1.5">
                        <span className="min-w-0 break-all font-mono font-medium text-slate-900">{selectedClientUrl || "-"}</span>
                        {selectedClientUrl ? (
                          <button
                            type="button"
                            aria-label={t("common.copy")}
                            title={t("common.copy")}
                            className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded text-slate-400 transition-colors hover:bg-slate-100 hover:text-slate-700"
                            onClick={() => {
                              void navigator.clipboard.writeText(selectedClientUrl).then(
                                () => toast.success(t("common.copySuccess")),
                                () => toast.danger(t("common.copyFailed")),
                              );
                            }}
                          >
                            <Copy className="h-3.5 w-3.5" aria-hidden />
                          </button>
                        ) : null}
                      </dd>
                      <dt className="text-slate-500">{t("dashboard.owner")}</dt>
                      <dd className="min-w-0 break-all font-medium text-slate-900">{clientOwnerEmail(selectedClient)}</dd>
                      <dt className="text-slate-500">{t("dashboard.region")}</dt>
                      <dd className="min-w-0 font-medium text-slate-900" title={clientRegionIpTitle(selectedClient.installation)}>{clientRegionLabel(selectedClient.installation)}</dd>
                      <dt className="text-slate-500">{t("dashboard.version")}</dt>
                      <dd className="min-w-0 font-mono font-medium text-slate-900">{clientPlatformLabel(selectedClient)}</dd>
                      <dt className="text-slate-500">{t("dashboard.online")}</dt>
                      <dd className="min-w-0 font-medium text-slate-900">{(selectedClient.onlineRate24h || 0).toFixed(1)}% / {formatAgeDaysOrHours(selectedClient.installation.createdAt, locale)}</dd>
                      {selectedClient.removalAt ? (
                        <>
                          <dt className="text-rose-700">{t("dashboard.removalAt")}</dt>
                          <dd className="min-w-0 font-medium text-rose-700" title={formatDateTime(selectedClient.removalAt)}>
                            {formatRelativeTime(selectedClient.removalAt, locale)}
                            <span className="ml-1 font-normal text-slate-500">· {formatDateTime(selectedClient.removalAt)}</span>
                          </dd>
                        </>
                      ) : null}
                    </dl>
                    <ClientMarketRentalBanner
                      rental={marketRentals.get(selectedClient.installation.id)}
                      onChanged={refreshRentalsAndDashboard}
                      readOnly
                      resumeRelease={false}
                      manageHref={clientMarketMineHref(selectedClient.installation.id)}
                    />
                    <ClientOnlineHeatmap installationId={selectedClient.installation.id} />
                    <DrawerSection label={t("dashboard.linkedShares")}>
                      <ClientLinkedSharesPanel shares={sharesForClient(selectedClient)} onEdit={openEditShare} t={t} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.providers")}>
                      <ClientProvidersPanel shares={sharesForClient(selectedClient)} />
                    </DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
      </Drawer.Backdrop>

      <Drawer.Backdrop isOpen={!!selectedShare} onOpenChange={closeShareDrawer}>
          <Drawer.Content placement="right">
            <Drawer.Dialog className={drawerDialogClassName}>
              <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Drawer.Header>
                <div>
                  <Drawer.Heading className="break-all font-mono text-base">{selectedApi.apiUrl}</Drawer.Heading>
                  {selectedShare?.description ? <p className="mt-2 whitespace-pre-wrap break-words text-xs leading-5 text-muted-foreground">{selectedShare.description}</p> : null}
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selectedShare ? (
                  <div className="grid gap-5">
                    {!["online", "available"].includes(shareOperationalSummary(selectedShare).state) ? (
                      <OperationalDiagnosis summary={shareOperationalSummary(selectedShare)} kind="share" />
                    ) : null}
                    <DrawerSection label={t("dashboard.providers")}>
                      <ShareProvidersPanel share={selectedShare} />
                    </DrawerSection>
                    <ShareModelHealthHeatmap shareId={selectedShare.shareId} />
                    {selectedShare ? <ShareEmailUsagePanel key={selectedShare.shareId} share={selectedShare} /> : null}
                    {selectedShare ? <ShareProviderRequestsPanel key={`${selectedShare.shareId}:requests`} share={selectedShare} /> : null}
                    <DrawerSection label={t("dashboard.modelHealthChecks")}>
                      <ShareModelHealthChecks checks={selectedShare.recentModelHealthChecks || []} />
                    </DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
      </Drawer.Backdrop>

      <ShareEditDialog share={currentEditingShare} onClose={closeEditShare} onSaved={handleSaved} />
      <ShareConnectDialog share={currentConnectShare} open={!!currentConnectShare} onOpenChange={closeConnectDialog} />
      <CreateClientDialog open={createClientOpen} onOpenChange={setCreateClientOpen} onCreated={() => void refreshRentalsAndDashboard()} />
      <ClientSubdomainTakeoverDialog
        target={takeoverTarget}
        sources={takeoverSources}
        open={!!takeoverTarget}
        onOpenChange={(open) => !open && setTakeoverTargetId("")}
        onCompleted={async () => {
          setTakeoverTargetId("");
          await refreshRentalsAndDashboard();
        }}
      />
      <ClientLogsDialog
        client={logClient}
        open={!!logClient?.logCollectionEnabled}
        onOpenChange={(open) => !open && setLogClientId("")}
      />
    </section>
  );
}
