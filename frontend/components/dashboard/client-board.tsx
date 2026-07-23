"use client";

import { Button, Card, Chip, Drawer, toast } from "@heroui/react";
import { Check, ChevronDown, Copy, ExternalLink, MessageCircle, Plus, Search, WalletCards } from "lucide-react";
import * as React from "react";
import { buildClientInstallCommand, InstallGuideDialog } from "@/components/dashboard/install-guide-dialog";
import { CreateClientDialog } from "@/components/dashboard/create-client-dialog";
import { SectionInstallButton } from "@/components/dashboard/section-install-button";
import { ShareConnectDialog } from "@/components/dashboard/share-connect-dialog";
import { ShareCard } from "@/components/dashboard/share-card";
import { ClientUpgradeButton } from "@/components/dashboard/client-upgrade-button";
import { ClientRemovalSchedule, clientOperationalSummary, OperationalDiagnosis, OperationalStatusPill, operationalReasonLabel, shareIsEnabled, shareOperationalSummary, summarizeShareAvailability, useStableOperationalRanks } from "@/components/dashboard/operational-status";
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
  subdomainTunnelUrl,
  ClientProvidersPanel,
  DrawerSection,
  drawerDialogClassName,
  EmptyBlock,
  formatAgeDaysOrHours,
  clientRunningDurationLabel,
  clientRunningDurationMs,
  clientTotalTokensLabel,
  clientTotalTokensUsed,
  ShareClientPanel,
  ShareEditDialog,
  ShareMarkets,
  ShareModelHealthChecks,
  ShareProvidersPanel,
  shareApiParts,
  sortClients,
} from "@/components/dashboard/data-tables";
import type { DashboardClient, DashboardMarket, OperationalState, ShareView } from "@/lib/types";
import { formatDateTime, formatRelativeTime } from "@/lib/utils";
import { usePersistentState } from "@/lib/use-persistent-state";
import { recordDashboardUxEvent } from "@/lib/api";
import { CompactSelect } from "@/components/common/compact-select";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { useClientChat } from "@/components/chat/client-chat";

const PAYOUT_NETWORK_LABELS: Record<string, string> = {
  "eip155:56": "BSC",
  "eip155:8453": "Base",
  "eip155:42161": "Arbitrum One",
};

function PayoutProfilePanel({ client, detailed = false }: { client: DashboardClient; detailed?: boolean }) {
  const { locale, t } = useLocaleText();
  const profile = client.payoutProfile;
  const [copied, setCopied] = React.useState(false);
  if (!profile) {
    return detailed ? <EmptyBlock>{t("dashboard.payoutNotConfigured")}</EmptyBlock> : null;
  }
  const copyAddress = async () => {
    try {
      await navigator.clipboard.writeText(profile.address);
      toast.success(t("dashboard.payoutCopied"));
    } catch {
      toast.danger(t("dashboard.payoutCopyFailed"));
      return;
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };
  if (!detailed) {
    const networks = profile.networks.map((network) => PAYOUT_NETWORK_LABELS[network] || network).join(" / ");
    return (
      <button
        type="button"
        data-no-row-drawer
        onClick={(event) => { event.stopPropagation(); void copyAddress(); }}
        className="inline-flex min-w-0 max-w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-slate-100 hover:text-foreground"
        title={`${profile.token} · ${networks} · ${profile.address}`}
      >
        <WalletCards className="h-3.5 w-3.5 shrink-0" />
        <span className="shrink-0 font-medium text-foreground">{profile.token}</span>
        <span className="shrink-0">· {networks} ·</span>
        <code className="min-w-0 truncate font-mono text-[11px]">{profile.address}</code>
        {copied ? <Check className="h-3.5 w-3.5 shrink-0 text-emerald-600" /> : <Copy className="h-3.5 w-3.5 shrink-0" />}
      </button>
    );
  }
  return (
    <div className="grid min-w-0 gap-2 rounded-md border border-amber-200/80 bg-amber-50/50 p-3">
      <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs">
        <span className="inline-flex items-center gap-1 font-medium text-amber-800"><WalletCards className="h-3.5 w-3.5" />{t("dashboard.payout")}</span>
        <Chip size="sm" variant="tertiary">{profile.token}</Chip>
        {profile.networks.map((network) => <Chip key={network} size="sm" variant="soft">{PAYOUT_NETWORK_LABELS[network] || network}</Chip>)}
        <Chip size="sm" variant="tertiary">{t("dashboard.payoutSelfDeclared")}</Chip>
      </div>
      <div className="flex min-w-0 items-center gap-1.5">
        <code className={`${detailed ? "break-all" : "min-w-0 truncate"} font-mono text-xs text-foreground`} title={profile.address}>{profile.address}</code>
        <Button size="sm" variant="ghost" isIconOnly className="h-7 w-7 min-w-0 shrink-0 rounded-md p-0" aria-label={t("dashboard.copyPayoutAddress")} data-no-row-drawer onClick={(event) => { event.stopPropagation(); void copyAddress(); }}>
          {copied ? <Check className="h-3.5 w-3.5 text-emerald-600" /> : <Copy className="h-3.5 w-3.5" />}
        </Button>
      </div>
      <div className="grid gap-1 text-xs text-muted-foreground">
        <span>{t("dashboard.payoutUpdated")}: <strong className="text-foreground">{new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(profile.updatedAt))}</strong></span>
        <span className="text-amber-700">{t("dashboard.payoutUnverifiedHint")}</span>
      </div>
    </div>
  );
}

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
  if (client.installation.provisionSource === "router_market") return null;
  const tunnelUrl = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  if (!tunnelUrl) return null;
  const title = client.clientTunnel?.subdomain || tunnelUrl;
  return (
    <ClientHeaderInlineButton
      label={t("dashboard.clientConsole")}
      onClick={() =>
        openConsole({
          clientId: client.installation.id,
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
  return includesQuery([
    share.shareName,
    share.shareId,
    share.subdomain,
    share.ownerEmail,
    share.appType,
    share.providerId,
    ...Object.keys(share.bindings || {}),
    ...Object.values(share.bindings || {}),
  ], query);
}

const ShareScroller = React.memo(function ShareScroller({
  shares,
  totalCount = shares.length,
  referenceTunnelUrl,
  onOpenShare,
  onEditShare,
  onConnectShare,
}: {
  shares: ShareView[];
  totalCount?: number;
  referenceTunnelUrl?: string;
  onOpenShare: (share: ShareView) => void;
  onEditShare: (share: ShareView) => void;
  onConnectShare: (share: ShareView) => void;
}) {
  const { t } = useLocaleText();
  if (!shares.length) return <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock>;

  const { enabledCount, availableCount, reconnectingCount, degradedCount, offlineCount } = summarizeShareAvailability(shares);
  const disabledCount = shares.length - enabledCount;

  return (
    <div className="grid min-w-0 gap-3 rounded-lg bg-slate-50/80 p-3">
      <div className="flex items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
          <span className="font-semibold text-foreground">{t("dashboard.shares")}</span>
          <span>{shares.length === totalCount ? shares.length : `${shares.length}/${totalCount}`}</span>
          <span aria-hidden>·</span>
          <span className="text-emerald-700">{availableCount} {t("dashboard.available")}</span>
          {reconnectingCount > 0 ? <span className="text-sky-700">{reconnectingCount} {t("dashboard.reconnecting")}</span> : null}
          {degradedCount > 0 ? <span className="text-amber-700">{degradedCount} {t("dashboard.degraded")}</span> : null}
          {offlineCount > 0 ? <span className="text-rose-700">{offlineCount} {t("common.offline")}</span> : null}
          {disabledCount > 0 ? <span>{disabledCount} {t("common.disabled")}</span> : null}
        </div>
      </div>
        <div className="grid min-w-0 grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4" aria-label={t("dashboard.shares")}>
          {shares.map((share) => (
            <ShareCard key={share.shareId} share={share} referenceTunnelUrl={referenceTunnelUrl} onOpen={onOpenShare} onEdit={onEditShare} onConnect={onConnectShare} />
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
  collapsed,
  onToggleCollapsed,
}: {
  client: DashboardClient;
  shares: ShareView[];
  summaryShares?: ShareView[];
  onOpenClient: (client: DashboardClient) => void;
  onOpenShare: (share: ShareView) => void;
  onEditShare: (share: ShareView) => void;
  onConnectShare: (share: ShareView) => void;
  collapsed: boolean;
  onToggleCollapsed: () => void;
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
  const { enabledCount: enabledShareCount, availableCount, issueCount, routeOnlineCount } = shareAvailability;
  const enabledSharesTotal = enabledShareCount || allShares.length;
  const sharesMetricTitle = enabledShareCount
    ? t("dashboard.sharesAvailableDetail", {
        available: availableCount,
        total: enabledShareCount,
        routeOnline: routeOnlineCount,
      })
    : undefined;
  const identity = client.clientTunnel?.subdomain || client.installation.id;
  const identityUrl = client.clientTunnel?.subdomain ? tunnelUrl : "";
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
    <Card id={`dashboard-client-${client.installation.id}`} className={`overflow-hidden rounded-lg border border-l-[3px] bg-white p-0 shadow-sm transition-[border-color,box-shadow] ${borderTone}`}>
      <Card.Content className="grid gap-3 p-3.5">
        <div
          className="group/client-header grid min-h-16 cursor-pointer select-text grid-cols-[minmax(300px,1.3fr)_minmax(760px,1.15fr)_auto] items-center gap-6 rounded-md px-1.5 py-1 outline-none transition-colors hover:bg-primary/[0.03] focus-visible:ring-2 focus-visible:ring-primary/30"
          aria-expanded={!collapsed}
          onMouseDown={handleHeaderPointerDown}
          onClick={handleHeaderClick}
          onDoubleClick={handleHeaderDoubleClick}
        >
          <div className="grid min-w-0 gap-1.5">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <span className={`h-2 w-2 shrink-0 rounded-full ${state === "offline" ? "bg-rose-500" : state === "reconnecting" ? "bg-sky-500" : state === "degraded" ? "bg-amber-400" : "bg-emerald-500"}`} />
              {identityUrl ? (
                <a
                  href={identityUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  data-no-row-drawer
                  className="inline-flex min-w-0 max-w-full items-center gap-1 truncate text-sm font-semibold text-foreground underline-offset-4 hover:underline"
                  title={identityUrl}
                  onClick={(event) => event.stopPropagation()}
                >
                  <span className="truncate">{identity}</span>
                  <ExternalLink className="h-3 w-3 shrink-0 text-muted-foreground" aria-hidden />
                </a>
              ) : (
                <strong className="truncate text-sm font-semibold text-foreground" title={identity}>{identity}</strong>
              )}
              <span className="inline-flex shrink-0 flex-nowrap items-center gap-2">
                <OperationalStatusPill summary={summary} />
                {tunnelUrl ? <ClientConsoleButton client={client} /> : null}
                <ClientUpgradeButton client={client} />
                <ClientDetailsButton onOpen={openClientDrawer} />
                <ClientChatButton client={client} />
              </span>
              {summary.primaryReason ? <span className={`truncate text-[11px] font-medium ${state === "offline" ? "text-rose-700" : state === "reconnecting" ? "text-sky-700" : "text-amber-700"}`} title={operationalReasonLabel(summary.primaryReason, t)}>{operationalReasonLabel(summary.primaryReason, t)}</span> : null}
              {showRemoval ? <ClientRemovalSchedule removalAt={client.removalAt} className="text-[11px]" /> : null}
            </div>
            <div className="flex min-w-0 items-center text-xs text-muted-foreground">
              <span className="truncate" title={owner}>{owner}</span>
            </div>
          </div>

          <div className={`grid min-w-0 gap-3 ${showRemoval ? "grid-cols-8" : "grid-cols-7"}`}>
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
                total: clientTotalTokensUsed(allShares).toLocaleString(),
              })}
              preserveValue
            />
            <Metric label={t("dashboard.version")} value={versionLabel} title={client.installation.appVersion || versionLabel} preserveValue />
            <Metric label={t("dashboard.uptime24h")} value={`${onlineRate.toFixed(1)}%`} title={onlineTitle} tone={onlineRate < 90 ? "warning" : "success"} />
            <Metric label={t("dashboard.shares")} value={`${availableCount}/${enabledSharesTotal} ${t("dashboard.available")}`} title={sharesMetricTitle} tone={issueCount ? "danger" : "default"} />
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

        {!collapsed ? <ShareScroller shares={shares} totalCount={allShares.length} referenceTunnelUrl={client.clientTunnel?.tunnelUrl} onOpenShare={onOpenShare} onEditShare={onEditShare} onConnectShare={onConnectShare} /> : null}
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
  markets,
  onChanged,
}: {
  clients: DashboardClient[];
  shares: ShareView[];
  markets: DashboardMarket[];
  onChanged?: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const focus = useDashboardFocus();
  const { issuesOnly, setIssuesOnly, regionFilters, setRegionFilters, clearRegionFilters } = useDashboardViewState();
  const { trackOperation } = useOperationVerification();
  const [selectedClientId, setSelectedClientId] = React.useState("");
  const [selectedShareId, setSelectedShareId] = React.useState("");
  const [editingShare, setEditingShare] = React.useState<ShareView | null>(null);
  const [connectShare, setConnectShare] = React.useState<ShareView | null>(null);
  const [installOpen, setInstallOpen] = React.useState(false);
  const [createClientOpen, setCreateClientOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const [statusFilter, setStatusFilter] = usePersistentState<"all" | Extract<OperationalState, "online" | "reconnecting" | "degraded" | "offline">>("cc_switch_router_client_status_v1", "all");
  const [sortOrder, setSortOrder] = usePersistentState("cc_switch_router_client_sort_v1", "tokens");
  const [expandedClientIds, setExpandedClientIds] = usePersistentState<string[] | null>(
    CLIENT_EXPANDED_STORAGE_KEY,
    null,
  );
  const lastLocatedFocusRef = React.useRef("");

  React.useEffect(() => {
    if (sortOrder === "registered") setSortOrder("running");
  }, [setSortOrder, sortOrder]);

  React.useEffect(() => {
    if (issuesOnly) setStatusFilter("all");
  }, [issuesOnly, setStatusFilter]);

  const sortedClients = React.useMemo(() => sortClients(clients), [clients]);
  const defaultExpandedClientId = sortedClients.reduce<DashboardClient | undefined>((best, client) => {
    return !best || (client.shareIds || []).length > (best.shareIds || []).length ? client : best;
  }, undefined)?.installation.id;
  const expandedClientIdSet = React.useMemo(
    () => new Set(expandedClientIds ?? (defaultExpandedClientId ? [defaultExpandedClientId] : [])),
    [defaultExpandedClientId, expandedClientIds],
  );
  const shareById = React.useMemo(() => new Map(shares.map((share) => [share.shareId, share])), [shares]);
  const clientById = React.useMemo(() => new Map(clients.map((client) => [client.installation.id, client])), [clients]);
  const clientByShareId = React.useMemo(() => {
    const map = new Map<string, DashboardClient>();
    clients.forEach((client) => (client.shareIds || []).forEach((shareId) => map.set(shareId, client)));
    return map;
  }, [clients]);
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
        client.payoutProfile?.address,
        client.payoutProfile?.token,
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
      online: states.filter((state) => state === "online").length,
      reconnecting: states.filter((state) => state === "reconnecting").length,
      degraded: states.filter((state) => state === "degraded").length,
      offline: states.filter((state) => state === "offline").length,
      issues: states.filter((state) => state !== "online").length,
    };
  }, [sharesForClient, sortedClients]);

  const visibleOrphanShares = React.useMemo(() => {
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
      document.getElementById(`dashboard-client-${clientId}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
  }, [focus.relatedClientIds, focus.target]);

  React.useEffect(() => {
    if (focus.drawerTarget?.kind === "client" && clientById.has(focus.drawerTarget.id)) setSelectedClientId(focus.drawerTarget.id);
    if (focus.drawerTarget?.kind === "share" && shareById.has(focus.drawerTarget.id)) setSelectedShareId(focus.drawerTarget.id);
  }, [clientById, focus.drawerTarget, shareById]);

  const selectedClient = selectedClientId ? clientById.get(selectedClientId) || null : null;
  const selectedShare = selectedShareId ? shareById.get(selectedShareId) || null : null;
  const connectShareId = connectShare?.shareId || "";
  const currentConnectShare = connectShareId ? shareById.get(connectShareId) || null : null;
  const selectedClientUrl = clientTunnelDisplayUrl(selectedClient?.clientTunnel?.tunnelUrl);
  const selectedApi = shareApiParts(selectedShare ?? undefined);
  const clientInstallCommand = React.useMemo(
    () =>
      buildClientInstallCommand({
        ownerEmailPlaceholder: t("dashboard.installClientCommandOwnerPlaceholder"),
      }),
    [installOpen, t],
  );

  React.useEffect(() => {
    if (connectShareId && !shareById.has(connectShareId)) setConnectShare(null);
  }, [connectShareId, shareById]);

  return (
    <section className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex min-w-0 flex-wrap items-center gap-3">
          <div className="inline-flex max-w-full overflow-x-auto rounded-lg bg-slate-100 p-1 text-[11px]">
            {([[
              "all", t("dashboard.all"), sortedClients.length,
            ], ["online", t("common.online"), clientSummary.online], ["reconnecting", t("dashboard.reconnecting"), clientSummary.reconnecting], ["degraded", t("dashboard.degraded"), clientSummary.degraded], ["offline", t("common.offline"), clientSummary.offline]] as const).map(([value, label, count]) => (
              <button key={value} type="button" onClick={() => { setStatusFilter(value); if (value === "online") setIssuesOnly(false); }} className={`rounded-md px-2.5 py-1.5 transition-colors ${statusFilter === value ? "bg-white font-medium text-foreground shadow-sm" : value === "offline" ? "text-rose-700" : value === "reconnecting" ? "text-sky-700" : value === "degraded" ? "text-amber-700" : "text-muted-foreground"}`}>{label} · {count}</button>
            ))}
          </div>
          <SectionInstallButton label={t("dashboard.installClient")} onClick={() => setInstallOpen(true)} />
          <Button variant="primary" size="sm" className="h-7 px-3 text-xs" onClick={() => setCreateClientOpen(true)}>
            <Plus className="h-3.5 w-3.5" />
            {t("createClient.newClient")}
          </Button>
        </div>
        <div className="flex w-full min-w-0 items-center gap-2 sm:w-auto">
          <label className="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-md border bg-white px-3 text-sm focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/10 sm:min-w-64">
            <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
            <input value={query} onChange={(event) => setQuery(event.target.value)} className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground" placeholder={t("dashboard.searchClients")} aria-label={t("dashboard.searchClients")} />
          </label>
          {regions.length > 1 ? (
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
              className="w-44"
            />
          ) : null}
          <CompactSelect value={sortOrder === "registered" ? "running" : sortOrder} onChange={(value) => { setSortOrder(value); void recordDashboardUxEvent({ eventType: "filter_applied", source: "client-board", targetType: "client" }); }} options={[{ value: "issues", label: t("dashboard.sortIssues") }, { value: "name", label: t("dashboard.sortName") }, { value: "recent", label: t("dashboard.sortRecent") }, { value: "running", label: t("dashboard.sortRunning") }, { value: "tokens", label: t("dashboard.sortTokens") }, { value: "shares", label: t("dashboard.sortShares") }]} ariaLabel={t("dashboard.sortBy")} className="w-44" />
        </div>
      </div>

      <div className="grid gap-4">
        {clientRows.length ? clientRows.map(({ client, shares: visibleShares, allShares }) => (
          <ClientCard key={client.installation.id} client={client} shares={visibleShares} summaryShares={allShares} onOpenClient={openClient} onOpenShare={openShare} onEditShare={openEditShare} onConnectShare={openConnectShare} collapsed={!query && !expandedClientIdSet.has(client.installation.id)} onToggleCollapsed={() => toggleClientExpanded(client.installation.id)} />
        )) : (
          <EmptyBlock>
            <div className="grid justify-items-center gap-2">
              <span>{sortedClients.length ? t("dashboard.noFilterResults") : t("dashboard.noClients")}</span>
              {sortedClients.length ? <button type="button" className="text-xs font-medium text-primary hover:underline" onClick={() => { setQuery(""); setStatusFilter("all"); clearRegionFilters(); setIssuesOnly(false); }}>{t("dashboard.clearFilters")}</button> : null}
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
                <div>
                  <Drawer.Heading className="break-all font-mono text-base">{selectedClientUrl || "-"}</Drawer.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">{clientOwnerEmail(selectedClient)}</p>
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selectedClient ? (
                  <div className="grid gap-5">
                    <OperationalDiagnosis summary={clientOperationalSummary(selectedClient, sharesForClient(selectedClient))} kind="client" removalAt={selectedClient.removalAt} />
                    <DrawerSection label={t("dashboard.client")}>
                      <div className="grid gap-1 text-xs text-muted-foreground">
                        <span>URL: <strong className="break-all text-foreground">{selectedClientUrl || "-"}</strong></span>
                        <span>{t("dashboard.owner")}: <strong className="text-foreground">{clientOwnerEmail(selectedClient)}</strong></span>
                        <span>{t("dashboard.region")}: <strong className="text-foreground" title={clientRegionIpTitle(selectedClient.installation)}>{clientRegionLabel(selectedClient.installation)}</strong></span>
                        <span>{t("dashboard.version")}: <strong className="font-mono text-foreground">{clientPlatformLabel(selectedClient)}</strong></span>
                        <span>{t("dashboard.online")}: <strong className="text-foreground">{(selectedClient.onlineRate24h || 0).toFixed(1)}% / {formatAgeDaysOrHours(selectedClient.installation.createdAt, locale)}</strong></span>
                        {selectedClient.removalAt ? (
                          <span>
                            {t("dashboard.removalAt")}:{" "}
                            <strong className="text-rose-700" title={formatDateTime(selectedClient.removalAt)}>
                              {formatRelativeTime(selectedClient.removalAt, locale)}
                            </strong>
                            <span className="text-muted-foreground"> · {formatDateTime(selectedClient.removalAt)}</span>
                          </span>
                        ) : null}
                      </div>
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.payout")}>
                      <PayoutProfilePanel client={selectedClient} detailed />
                    </DrawerSection>
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
                    <OperationalDiagnosis summary={shareOperationalSummary(selectedShare)} kind="share" />
                    <DrawerSection label={t("dashboard.client")}>
                      <ShareClientPanel client={clientByShareId.get(selectedShare.shareId)} currentShare={selectedShare} shares={sharesForClient(clientByShareId.get(selectedShare.shareId))} onEdit={openEditShare} t={t} locale={locale} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.markets")}>
                      <ShareMarkets share={selectedShare} t={t} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.providers")}>
                      <ShareProvidersPanel share={selectedShare} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.modelHealthChecks")}>
                      <ShareModelHealthChecks checks={selectedShare.recentModelHealthChecks || []} />
                    </DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
      </Drawer.Backdrop>

      <ShareEditDialog share={editingShare} markets={markets} onClose={closeEditShare} onSaved={handleSaved} />
      <ShareConnectDialog share={currentConnectShare} open={!!currentConnectShare} onOpenChange={closeConnectDialog} />
      <InstallGuideDialog
        open={installOpen}
        onOpenChange={setInstallOpen}
        titleKey="dashboard.installClientTitle"
        descriptionKey="dashboard.installClientDescription"
        commandLabelKey="dashboard.installClientCommandLabel"
        command={clientInstallCommand}
      />
      <CreateClientDialog open={createClientOpen} onOpenChange={setCreateClientOpen} />
    </section>
  );
}
