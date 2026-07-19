"use client";

import { Eye, ExternalLink, Link2, Maximize2, Pencil } from "lucide-react";
import { Button, Card, Chip, Modal, ProgressBar, Tabs } from "@heroui/react";
import * as React from "react";
import { ShareClientTag } from "@/components/dashboard/share-client-tag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getShareImageGenerationRequestLogs, getShareUsageByEmail } from "@/lib/api";
import type { AppLocale } from "@/lib/i18n";
import type { DashboardClient, ImageGenerationRequestLog, MarketRequestLog, ShareAppProvider, ShareAppProviders, ShareAppRuntimes, ShareMarketListingStatus, ShareModelHealthCheck, ShareRequestLog, ShareUpstreamProvider, ShareUsageByEmailResponse, ShareView } from "@/lib/types";
import { compactTokens, formatDateTime, formatNumber, formatRelativeTime } from "@/lib/utils";
import { resolveShareCoreApp, SHARE_APP_LABELS } from "@/lib/share-app";
import { averageRecentLatencyMs, boundProviderIdForApp, cacheHitRate, clientPlatformLabel, clientTunnelDisplayUrl, configuredUpstreamPercent, CORE_SHARE_APPS, expiryTitle, formatAgeDaysOrHours, formatImageLogSizeMb, formatImageLogSpendSeconds, formatImageLogTimestamp, formatLatencySeconds, formatMinutesShort, formatPercent, formatShareStatus, HealthDots, isUnlimited, mergeStandaloneOAuthRuntime, modelHealthTitle, modelHealthTone, providerAccountIdentity, providerAccountLevel, providerModelMap, requestBelongsToApp, requestModelRoute, resolveShareAppRuntime, runtimeEndpointSummary, shareApiParts, shareAppExists, shareAppProviderRuntime, shareAppSettings, shareAppTokensUsed, shareExpiryProgress, tokenCount, usageBucketTotalTokens, type CoreShareApp, type TFn } from "@/components/dashboard/share-dashboard-utils";

export function StatusBadge({ active, label }: { active: boolean; label: string }) {
  return <Chip color={active ? "success" : "default"} size="sm" variant={active ? "soft" : "tertiary"}>{label}</Chip>;
}

export function ShareStatusBadge({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <StatusBadge active={false} label={t("dashboard.noShare")} />;
  const status = String(share.shareStatus || "").trim().toLowerCase();
  if (status === "active") return <Chip color="success" size="sm" variant="soft">{t("dashboard.shareStatus.active")}</Chip>;
  if (status === "paused") return <Chip color="warning" size="sm" variant="soft">{t("dashboard.shareStatus.paused")}</Chip>;
  if (status === "expired") return <Chip color="default" size="sm" variant="tertiary">{t("dashboard.shareStatus.expired")}</Chip>;
  return <StatusBadge active={false} label={formatShareStatus(share.shareStatus)} />;
}

export function ShareExceptionalStatusBadge({ share, t }: { share?: ShareView; t: TFn }) {
  const status = String(share?.shareStatus || "").trim().toLowerCase();
  if (!share || status === "active") return null;
  return <ShareStatusBadge share={share} t={t} />;
}

export function UsageBar({ used, limit, t }: { used: number; limit: number; t: TFn }) {
  if (isUnlimited(limit)) return null;
  const pct = limit > 0 ? Math.min(100, Math.max(0, (used / limit) * 100)) : 0;
  return (
    <ProgressBar aria-label={t("progress.usage")} value={pct} minValue={0} maxValue={100} size="sm" className="mt-1 w-32 gap-0">
      <ProgressBar.Track className="h-1 rounded bg-muted">
        <ProgressBar.Fill className="rounded bg-primary" />
      </ProgressBar.Track>
    </ProgressBar>
  );
}

export function ForSaleCell({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const value = share.forSale === "Free" ? t("dashboard.free") : share.forSale === "Yes" ? t("dashboard.yes") : t("dashboard.no");
  const saleMarketKind = share.saleMarketKind === "share" ? "share" : "token";
  const pricingLines = share.forSale === "Yes" && saleMarketKind === "token"
    ? [
        ["Claude", configuredUpstreamPercent(share.appRuntimes, "claude")],
        ["Codex", configuredUpstreamPercent(share.appRuntimes, "codex")],
        ["Gemini", configuredUpstreamPercent(share.appRuntimes, "gemini")],
      ].filter(([, percent]) => !!percent)
    : [];
  const marketLines = share.forSale === "Yes"
    ? saleMarketKind === "share"
      ? [t("dashboard.shareMarket"), ...(share.marketLinks || []).map((market) => market.subdomain || market.email).filter(Boolean)]
      : [t("dashboard.tokenMarket"), ...(share.marketAccessMode === "all" ? [t("dashboard.allMarkets")] : (share.marketLinks || []).map((market) => market.subdomain || market.email).filter(Boolean))]
    : [];
  return (
    <div className="grid min-w-32 gap-1.5">
      <Chip size="sm" variant={value === "No" ? "tertiary" : "soft"}>{value}</Chip>
      {pricingLines.length ? (
        <div className="grid gap-0.5 font-mono text-[11px] text-muted-foreground">
          {pricingLines.map(([label, percent]) => <div key={label}>{label} {percent}</div>)}
        </div>
      ) : null}
      {marketLines.length ? <div className="grid gap-0.5 font-mono text-[11px] text-muted-foreground">{marketLines.map((line) => <div key={line}>{line}</div>)}</div> : null}
    </div>
  );
}

export function ShareAppSupportCard({
  share,
  app,
  label,
  locale,
}: {
  share: ShareView;
  app: CoreShareApp;
  label: string;
  locale: AppLocale;
}) {
  const enabled = !!share.support?.[app];
  const runtime = resolveShareAppRuntime(share, app);
  const tone = enabled ? modelHealthTone(share, app) : { className: "bg-slate-50 text-muted-foreground", label: "" };
  const accountEmail = enabled ? providerAccountIdentity(runtime) : "";
  const modelSummary = enabled ? providerModelMap(runtime) : "";
  return (
    <div
      data-no-row-drawer
      title={enabled ? modelHealthTitle(share, app) : undefined}
      className={`select-text grid min-w-0 grid-cols-[56px_minmax(0,1fr)] gap-2 overflow-hidden rounded-lg border px-2 py-1.5 text-[11px] ${tone.className}`}
    >
      <span className="min-w-0 self-start font-mono uppercase">{label}</span>
      <span className="grid min-w-0 gap-0.5 overflow-hidden text-right">
        <span className="min-w-0 whitespace-normal break-words font-semibold">
          {enabled ? providerAccountLevel(runtime, locale) : ""}
        </span>
        {accountEmail && accountEmail !== "-" ? (
          <span className="min-w-0 whitespace-normal break-all text-[10px] font-medium opacity-75">
            {accountEmail}
          </span>
        ) : null}
        {modelSummary && modelSummary !== "-" ? (
          <span className="min-w-0 whitespace-normal break-all text-[10px] font-medium opacity-75">
            {modelSummary}
          </span>
        ) : null}
        {enabled && tone.label ? (
          <span className="min-w-0 text-[10px] font-semibold opacity-70">{tone.label}</span>
        ) : null}
      </span>
    </div>
  );
}

export function ShareEditAction({ share, onEdit, t }: { share?: ShareView; onEdit: (share: ShareView) => void; t: TFn }) {
  if (!share) return null;
  if (share.canManage && share.activeEdit?.status === "pending") {
    return <Chip size="sm" color="warning" variant="soft">{t("dashboard.pendingApply")}</Chip>;
  }
  const handle = (event: React.MouseEvent) => {
    event.stopPropagation();
    onEdit(share);
  };
  if (share.canManage && share.activeEdit?.status === "rejected") {
    return (
      <button
        type="button"
        onClick={handle}
        title={share.activeEdit.errorMessage || t("dashboard.applyFailedFallback")}
        className="inline-flex h-[22px] items-center gap-1 rounded-full border border-red-200 bg-red-50 px-2.5 text-[11px] font-medium text-red-700 transition-colors hover:border-red-300 hover:bg-red-100"
      >
        <Pencil className="h-3 w-3" />
        {t("dashboard.applyFailed")}
      </button>
    );
  }
  return (
    <button
      type="button"
      onClick={handle}
      className="inline-flex h-[22px] items-center gap-1 rounded-full border border-primary/20 bg-primary/10 px-2.5 text-[11px] font-medium text-primary transition-colors hover:border-primary/30 hover:bg-primary/15"
    >
      {share.canManage ? <Pencil className="h-3 w-3" /> : <Eye className="h-3 w-3" />}
      {share.canManage ? t("common.edit") : t("common.view")}
    </button>
  );
}

export function ShareConnectChip({
  share,
  onOpen,
  t,
}: {
  share: ShareView;
  onOpen: (share: ShareView) => void;
  t: TFn;
}) {
  // data-no-row-drawer 让外层 <tr onClick> 的 shouldOpenRowDrawer 跳过，避免
  // 点击 chip 又触发 drawer。stopPropagation 已经覆盖了主要路径，data 属性是
  // 二保险（针对 selection / hover 等边角情况）。
  const handle = (event: React.MouseEvent) => {
    event.stopPropagation();
    onOpen(share);
  };
  return (
    <button
      type="button"
      onClick={handle}
      data-no-row-drawer
      className="inline-flex h-[22px] items-center gap-1 rounded-full border border-emerald-200 bg-emerald-50 px-2.5 text-[11px] font-medium text-emerald-700 transition-colors hover:border-emerald-300 hover:bg-emerald-100"
    >
      <Link2 className="h-3 w-3" />
      {t("dashboard.connect")}
    </button>
  );
}

export { ShareClientTag };

export function ShareStatusCell({ share, t, locale }: { share?: ShareView; t: TFn; locale: AppLocale }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const limit = isUnlimited(share.parallelLimit) ? "∞" : String(share.parallelLimit || 0);
  const averageLatency = averageRecentLatencyMs(share.recentRequests);
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
  const shareMarketListingUrl = shareStatusShareMarketUrl(share, share.appType as CoreShareApp);
  const saleValue =
    share.forSale === "Free"
      ? t("dashboard.free")
      : share.forSale === "Yes"
        ? share.saleMarketKind === "share"
          ? t("dashboard.shareMarket")
          : t("dashboard.tokenMarket")
        : t("dashboard.no");
  const saleVariant: "soft" | "tertiary" = share.forSale === "No" ? "tertiary" : "soft";
  const saleRow = (
    <div className={rowClass}>
      <span className="mono-label text-muted-foreground">{t("dashboard.forSale")}</span>
      <div className="flex min-w-0 flex-wrap items-center gap-1">
        {shareMarketListingUrl ? (
          <a
            href={shareMarketListingUrl}
            target="_blank"
            rel="noreferrer"
            data-no-row-drawer
            className="inline-flex items-center gap-1"
            title={shareMarketListingUrl}
          >
            <Chip size="sm" variant={saleVariant}>
              {saleValue}
              <ExternalLink className="ml-1 inline h-3 w-3" />
            </Chip>
          </a>
        ) : (
          <Chip size="sm" variant={saleVariant}>{saleValue}</Chip>
        )}
        <ShareMarketListingStatusChip share={share} app={share.appType as CoreShareApp} t={t} />
      </div>
    </div>
  );
  const routeStatus = share.routeState === "reconnecting"
    ? <Chip color="accent" size="sm" variant="soft">{t("dashboard.reconnecting")}</Chip>
    : !share.isOnline
      ? <Chip size="sm" variant="tertiary">{t("common.offline")}</Chip>
      : null;
  const onlineTitle = t("dashboard.uptimeObservation", {
    healthy: (share.onlineRate24h || 0).toFixed(1),
    observed: share.observedMinutes24h || 0,
    coverage: (share.observationCoverage24h || 0).toFixed(1),
  });
  return (
    <div className="grid min-w-0 gap-2 text-sm">
      {routeStatus}
      {saleRow}
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><div><strong>{compactTokens(share.tokensUsed)} / {isUnlimited(share.tokenLimit) ? "∞" : compactTokens(share.tokenLimit)}</strong><UsageBar used={share.tokensUsed} limit={share.tokenLimit} t={t} /></div></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.expires")}</span><strong title={`${formatDateTime(share.createdAt)} / ${expiryTitle(share.expiresAt)}`}>{shareExpiryProgress(share, locale)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{share.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.response")}</span><strong>{formatLatencySeconds(averageLatency)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={onlineTitle}>{(share.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={share.healthChecks} /></div>
    </div>
  );
}

export function ShareMarketListingStatusChip({ share, app, t }: { share: ShareView; app?: CoreShareApp; t: TFn }) {
  const listing = shareMarketListingForApp(share, app);
  if (!listing) return null;
  const status = listing.status || "unknown";
  const label = shareMarketListingStatusLabel(listing, t);
  const color =
    status === "full"
      ? "success"
      : status === "carpooling"
        ? "warning"
        : status === "unavailable"
          ? "default"
          : status === "unknown"
            ? "default"
            : "accent";
  return (
    <Chip size="sm" variant="soft" color={color as "success" | "warning" | "default" | "accent"}>
      {label}
    </Chip>
  );
}

export function shareMarketListingStatusLabel(listing: ShareMarketListingStatus, t: TFn) {
  const status = listing.status || "unknown";
  const base =
    status === "idle"
      ? t("dashboard.shareMarketListingIdle")
      : status === "carpooling"
        ? t("dashboard.shareMarketListingCarpooling")
        : status === "full"
          ? t("dashboard.shareMarketListingFull")
          : status === "unavailable"
            ? t("dashboard.shareMarketListingUnavailable")
            : t("dashboard.shareMarketListingUnknown");
  if (
    status === "carpooling" &&
    typeof listing.filledSeats === "number" &&
    typeof listing.requiredSeats === "number" &&
    listing.requiredSeats > 0
  ) {
    return `${base} ${listing.filledSeats}/${listing.requiredSeats}`;
  }
  return base;
}

export function shareMarketListingForApp(share: ShareView, app?: CoreShareApp) {
  if (!app) return undefined;
  const market = (share.marketLinks || []).find(
    (item) => item.marketKind === "share" && item.publicBaseUrl,
  );
  return market?.listingStatusByApp?.[app];
}

export function shareStatusShareMarketUrl(share: ShareView, app?: CoreShareApp) {
  const settings = app ? shareAppSettings(share, app) : undefined;
  const forSale = settings?.forSale ?? share.forSale;
  const saleMarketKind = settings?.saleMarketKind ?? share.saleMarketKind;
  if (forSale !== "Yes" || saleMarketKind !== "share") return null;
  const market = (share.marketLinks || []).find(
    (item) => item.marketKind === "share" && item.publicBaseUrl,
  );
  if (!market?.publicBaseUrl) return null;
  const listing = app ? market.listingStatusByApp?.[app] : undefined;
  if (listing?.listingUrl) return listing.listingUrl;
  const base = market.publicBaseUrl.replace(/\/+$/, "");
  const routerId = share.routerId || "main";
  const appParam = app ? `&app_type=${encodeURIComponent(app)}` : "";
  return `${base}/listing/share?router_id=${encodeURIComponent(routerId)}&share_id=${encodeURIComponent(share.shareId)}${appParam}`;
}

export function clientOwnerEmail(client?: DashboardClient | null) {
  return client?.clientTunnel?.ownerEmail || client?.installation.ownerEmail || "-";
}

export function clientRegionLabel(client?: DashboardClient | null) {
  return client?.installation.countryCode || client?.installation.region || "-";
}

export function clientDisplayLabel(client?: DashboardClient | null) {
  return clientTunnelDisplayUrl(client?.clientTunnel?.tunnelUrl) || client?.installation.id || "-";
}

export function shareSupportLabel(share: ShareView) {
  const app = resolveShareCoreApp(share);
  if (app && share.support?.[app]) return SHARE_APP_LABELS[app];
  return CORE_SHARE_APPS
    .filter(([key]) => !!share.support?.[key])
    .map(([, label]) => label)
    .join(" / ");
}

export function shareSaleLabel(share: ShareView, t: TFn) {
  if (share.forSale === "Free") return t("dashboard.free");
  if (share.forSale === "Yes") return t("dashboard.forSale");
  return t("dashboard.no");
}

export function ClientReference({
  client,
  t,
  locale: _locale,
  shareCount: _shareCount,
}: {
  client?: DashboardClient;
  t: TFn;
  locale: AppLocale;
  shareCount?: number;
}) {
  if (!client) return <span className="text-xs text-muted-foreground">{t("dashboard.noClient")}</span>;
  const label = clientDisplayLabel(client);
  const url = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  return (
    <div className="grid min-w-0 gap-2 rounded-md border border-default/40 bg-muted/20 px-2 py-1.5 text-xs">
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          {url ? (
            <a
              href={url}
              target="_blank"
              rel="noopener noreferrer"
              data-no-row-drawer
              className="inline-flex min-w-0 max-w-full items-center gap-1 font-mono font-semibold text-foreground underline-offset-4 hover:underline"
              title={url}
            >
              <span className="truncate">{label}</span>
              <ExternalLink className="h-3 w-3 shrink-0 text-muted-foreground" />
            </a>
          ) : (
            <strong className="min-w-0 truncate font-mono text-foreground" title={label}>{label}</strong>
          )}
        </div>
        <ShareClientTag client={client} t={t} />
      </div>
      <span className="truncate text-muted-foreground" title={clientOwnerEmail(client)}>{clientOwnerEmail(client)}</span>
    </div>
  );
}

export function ShareSummaryItem({
  share,
  onEdit,
  t,
  compact = false,
}: {
  share: ShareView;
  onEdit: (share: ShareView) => void;
  t: TFn;
  compact?: boolean;
}) {
  const api = shareApiParts(share);
  const support = shareSupportLabel(share);
  const owner = share.ownerEmail || "-";
  return (
    <li className="grid max-w-full gap-1 rounded-md border border-default/40 bg-white/70 px-2 py-1.5">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <strong className="min-w-0 break-all font-mono text-xs text-foreground">{api.apiUrl}</strong>
        <ShareStatusBadge share={share} t={t} />
        <ShareEditAction share={share} onEdit={onEdit} t={t} />
      </div>
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-muted-foreground">
        {!compact ? <span className="truncate" title={owner}>{owner}</span> : null}
        <span>{support || t("dashboard.noProviders")}</span>
        <span>{shareSaleLabel(share, t)}</span>
      </div>
    </li>
  );
}

export function Info({ label, value }: { label: string; value?: React.ReactNode }) {
  return (
    <Card className="rounded-lg border bg-muted/30 p-0 shadow-none">
      <Card.Content className="p-3">
        <div className="mono-label text-muted-foreground">{label}</div>
        <div className="mt-2 break-words text-sm font-medium">{value || "--"}</div>
      </Card.Content>
    </Card>
  );
}

export function DrawerSection({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <section className="grid gap-3">
      <div className="font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">{label}</div>
      {children}
    </section>
  );
}

export function EmptyBlock({ children }: { children: React.ReactNode }) {
  return <div className="rounded-lg border bg-muted/20 p-4 text-sm text-muted-foreground">{children}</div>;
}

export function ClientLinkedSharesPanel({ shares, onEdit, t }: { shares: ShareView[]; onEdit: (share: ShareView) => void; t: TFn }) {
  if (!shares.length) return <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock>;
  return (
    <ul className="grid gap-2">
      {shares.map((share) => (
        <ShareSummaryItem key={share.shareId} share={share} onEdit={onEdit} t={t} />
      ))}
    </ul>
  );
}

export function ShareClientPanel({
  client,
  currentShare,
  shares,
  onEdit,
  t,
  locale,
}: {
  client?: DashboardClient;
  currentShare: ShareView;
  shares: ShareView[];
  onEdit: (share: ShareView) => void;
  t: TFn;
  locale: AppLocale;
}) {
  if (!client) return <EmptyBlock>{t("dashboard.noClient")}</EmptyBlock>;
  const otherShares = shares.filter((share) => share.shareId !== currentShare.shareId);
  return (
    <div className="grid gap-3">
      <ClientReference client={client} t={t} locale={locale} shareCount={shares.length} />
      {otherShares.length ? (
        <div className="grid gap-2">
          <div className="mono-label text-muted-foreground">{t("dashboard.otherShares")}</div>
          <ul className="grid gap-2">
            {otherShares.map((share) => (
              <ShareSummaryItem key={share.shareId} share={share} onEdit={onEdit} t={t} compact />
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

export function ShareMarkets({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <EmptyBlock>{t("dashboard.noShare")}</EmptyBlock>;
  if (share.forSale === "Free") return <EmptyBlock>{t("dashboard.publicFreeShare")}</EmptyBlock>;
  if (share.forSale !== "Yes") return <EmptyBlock>{t("dashboard.notForSale")}</EmptyBlock>;
  const saleMarketKind = share.saleMarketKind === "share" ? "share" : "token";
  if (saleMarketKind === "token" && share.marketAccessMode === "all") return <EmptyBlock>{t("dashboard.authorizedAllMarkets")}</EmptyBlock>;
  const links = share.marketLinks || [];
  const unknown = share.unknownMarketEmails || [];
  return (
    <div className="grid gap-2">
      <Chip size="sm" variant="tertiary">{saleMarketKind === "share" ? t("dashboard.shareMarket") : t("dashboard.tokenMarket")}</Chip>
      {links.map((market) => {
        const reconnecting = market.routeState === "reconnecting";
        return (
          <Card key={market.id || market.email} className="rounded-lg border p-0 shadow-none">
            <Card.Content className="flex-row items-center justify-between gap-3 p-3">
              <div className="min-w-0">
                <div className="truncate font-medium">{market.publicBaseUrl || market.email || "-"}</div>
                <div className="truncate text-xs text-muted-foreground">{market.email || "-"}</div>
              </div>
              <Chip color={market.online ? "success" : reconnecting ? "accent" : "default"} size="sm" variant={market.online || reconnecting ? "soft" : "tertiary"}>{market.online ? t("common.online") : reconnecting ? t("dashboard.reconnecting") : t("common.offline")}</Chip>
            </Card.Content>
          </Card>
        );
      })}
      {unknown.map((email) => <EmptyBlock key={email}>{t("dashboard.unknownMarket")}: {email}</EmptyBlock>)}
      {!links.length && !unknown.length && share.marketAccessMode !== "all" ? <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock> : null}
    </div>
  );
}

const PROVIDER_APP_TABS: Array<{ key: keyof ShareAppProviders; label: string }> = [
  { key: "claude", label: "Claude" },
  { key: "codex", label: "Codex" },
  { key: "gemini", label: "Gemini" },
];

export function providerRuntime(provider: ShareAppProvider): ShareUpstreamProvider {
  return shareAppProviderRuntime(provider);
}

export function providerMetaLabel(provider: ShareAppProvider) {
  return [provider.kind, provider.providerType].filter(Boolean).join(" · ");
}

export function ProviderCard({
  provider,
  runtime,
  t,
  locale,
  showCurrentBadge,
}: {
  provider: ShareAppProvider;
  runtime: ShareUpstreamProvider | undefined;
  t: TFn;
  locale: AppLocale;
  /** false 时不显示 "current" 角标。client 侧边栏跨多 share 看时无意义。 */
  showCurrentBadge: boolean;
}) {
  const endpoint = runtimeEndpointSummary(runtime);
  const meta = providerMetaLabel(provider);
  const accountLevel = providerAccountLevel(runtime, locale);
  const accountIdentity = providerAccountIdentity(runtime);
  const modelMap = providerModelMap(runtime);
  return (
    <div className="rounded-lg border bg-background p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">{provider.name || provider.id}</div>
          <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">{provider.id}</div>
        </div>
        <div className="flex shrink-0 flex-wrap justify-end gap-1">
          {showCurrentBadge && provider.isCurrent ? <Chip color="success" size="sm" variant="soft">{t("dashboard.current")}</Chip> : null}
          {showCurrentBadge && provider.isCurrent ? <Chip color={provider.enabled ? "success" : "default"} size="sm" variant="soft">{provider.enabled ? t("dashboard.on") : t("dashboard.off")}</Chip> : null}
        </div>
      </div>
      <div className="mt-2 grid gap-1 text-xs text-muted-foreground">
        {meta ? <div className="break-words">{meta}</div> : null}
        {endpoint ? <div className="break-words">{endpoint}</div> : null}
        {provider.forSaleOfficialPricePercent ? <div>{provider.forSaleOfficialPricePercent}%</div> : null}
        <div className="break-words">{accountLevel}</div>
        <div className="break-words">{accountIdentity}</div>
        <div className="break-words">{modelMap}</div>
      </div>
    </div>
  );
}

export function ShareProvidersPanel({ share }: { share?: ShareView }) {
  const { locale, t } = useLocaleText();
  const shareApp = resolveShareCoreApp(share);
  const providers = share?.appProviders;
  const runtimes = share?.appRuntimes;
  const boundProviderId = shareApp ? boundProviderIdForApp(share, shareApp) : undefined;
  const currentProviders = shareApp
    ? (providers?.[shareApp] || []).filter((provider) => provider.id === boundProviderId)
    : [];

  if (!shareApp) {
    return <EmptyBlock>{t("dashboard.noProviders")}</EmptyBlock>;
  }

  return (
    <div className="grid gap-3">
      {!currentProviders.length ? (
        <EmptyBlock>{t("dashboard.noProviders")}</EmptyBlock>
      ) : (
        <div className="grid gap-2">
          {currentProviders.map((provider) => {
            const runtime = mergeStandaloneOAuthRuntime(providerRuntime(provider), runtimes, provider);
            return (
              <ProviderCard key={provider.id} provider={provider} runtime={runtime} t={t} locale={locale} showCurrentBadge />
            );
          })}
        </div>
      )}
      {share ? <ShareEmailUsagePanel share={share} app={shareApp} /> : null}
      {share ? <ShareProviderRequestsPanel share={share} app={shareApp} /> : null}
    </div>
  );
}

type ShareUsagePeriod = "24h" | "1w" | "30d";
type ShareUsageViewMode = "table" | "trend";
const SHARE_USAGE_PERIODS: readonly ShareUsagePeriod[] = ["24h", "1w", "30d"];

export function ShareEmailUsagePanel({
  share,
  app,
}: {
  share: ShareView;
  app: keyof ShareAppProviders;
}) {
  const { t } = useLocaleText();
  const [period, setPeriod] = React.useState<ShareUsagePeriod>("24h");
  const [mode, setMode] = React.useState<ShareUsageViewMode>("table");
  const [usage, setUsage] = React.useState<ShareUsageByEmailResponse | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");
    getShareUsageByEmail(share.shareId, app, period)
      .then((data) => {
        if (!cancelled) setUsage(data);
      })
      .catch((err) => {
        if (!cancelled) {
          setUsage(null);
          setError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [share.shareId, app, period]);

  const total = usage?.totalTokens ?? 0;
  return (
    <div className="grid gap-3 rounded-lg border bg-background p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="text-sm font-semibold">{t("dashboard.emailTokenUsage")}</div>
          <div className="text-xs text-muted-foreground">{t("dashboard.emailTokenUsageSubtitle", { app: PROVIDER_APP_TABS.find((tab) => tab.key === app)?.label ?? app, total: compactTokens(total) })}</div>
        </div>
        <div className="flex flex-wrap items-center gap-1">
          {SHARE_USAGE_PERIODS.map((item) => (
            <button
              key={item}
              type="button"
              className={`rounded-md border px-2 py-1 text-xs transition-colors ${period === item ? "border-primary/40 bg-primary/10 text-primary" : "border-border bg-muted/20 text-muted-foreground hover:bg-muted/40"}`}
              onClick={() => setPeriod(item)}
            >
              {t(`dashboard.usagePeriod.${item}`)}
            </button>
          ))}
          {(["table", "trend"] as const).map((item) => (
            <button
              key={item}
              type="button"
              className={`rounded-md border px-2 py-1 text-xs transition-colors ${mode === item ? "border-primary/40 bg-primary/10 text-primary" : "border-border bg-muted/20 text-muted-foreground hover:bg-muted/40"}`}
              onClick={() => setMode(item)}
            >
              {item === "table" ? t("dashboard.usageView.table") : t("dashboard.usageView.trend")}
            </button>
          ))}
        </div>
      </div>
      {loading ? <EmptyBlock>{t("dashboard.usageEmail.loading")}</EmptyBlock> : null}
      {error ? <EmptyBlock>{error}</EmptyBlock> : null}
      {!loading && !error && usage ? (
        usage.rows.length ? (
          mode === "table" ? <ShareUsageTable usage={usage} t={t} /> : <ShareUsageTrend usage={usage} t={t} />
        ) : (
          <EmptyBlock>{t("dashboard.usageEmail.noAclEmails")}</EmptyBlock>
        )
      ) : null}
    </div>
  );
}

export function ShareUsageTable({ usage, t }: { usage: ShareUsageByEmailResponse; t: TFn }) {
  const roleLabel = (role: string) => {
    if (role === "owner") return t("dashboard.usageEmail.role.owner");
    if (role === "shareto") return t("dashboard.usageEmail.role.shareto");
    if (role === "market") return t("dashboard.usageEmail.role.market");
    return role || "-";
  };
  return (
    <div className="overflow-hidden rounded-md border">
      <table className="w-full table-fixed border-collapse text-[11px]">
        <colgroup>
          <col className="w-[31%]" />
          <col className="w-[13%]" />
          <col className="w-[9%]" />
          <col className="w-[9%]" />
          <col className="w-[10%]" />
          <col className="w-[10%]" />
          <col className="w-[10%]" />
          <col className="w-[8%]" />
        </colgroup>
        <thead className="bg-muted/50 text-left font-mono uppercase tracking-[0.08em] text-muted-foreground">
          <tr>
            <th className="px-1.5 py-2">{t("dashboard.usageEmail.email")}</th>
            <th className="px-1.5 py-2">{t("dashboard.usageEmail.role")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.input")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.output")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.cacheRead")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.cacheWrite")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.total")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.percent")}</th>
          </tr>
        </thead>
        <tbody>
          {usage.rows.map((row) => (
            <tr key={row.email} className="border-t">
              <td className="whitespace-normal break-all px-1.5 py-2 font-medium leading-4">{row.email}</td>
              <td className="break-words px-1.5 py-2 text-muted-foreground">{roleLabel(row.role)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.inputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.outputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.cacheReadTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.cacheCreationTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono font-semibold">{compactTokens(row.totalTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{Math.round(row.percent)}%</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function ShareUsageTrend({ usage, t }: { usage: ShareUsageByEmailResponse; t: TFn }) {
  const rows = usage.rows.filter((row) => row.totalTokens > 0).slice(0, 5);
  const [expanded, setExpanded] = React.useState(false);
  if (!rows.length) return <EmptyBlock>{t("dashboard.usageEmail.noData")}</EmptyBlock>;
  const colors = ["#2563eb", "#16a34a", "#d97706", "#9333ea", "#dc2626"];
  return (
    <div className="grid gap-2">
      <div className="relative overflow-x-auto rounded-md border bg-muted/10 p-2">
        <Button
          variant="outline"
          size="sm"
          isIconOnly
          className="absolute right-2 top-2 z-10 h-7 w-7 min-w-0 rounded-md bg-background/90 p-0"
          aria-label={t("dashboard.usageEmail.expandTrend")}
          onClick={() => setExpanded(true)}
        >
          <Maximize2 className="h-3.5 w-3.5" />
        </Button>
        <ShareUsageTrendChart usage={usage} rows={rows} colors={colors} t={t} size="compact" />
      </div>
      <div className="flex flex-wrap gap-2">
        {rows.map((row, idx) => (
          <div key={row.email} className="flex max-w-full items-center gap-1.5 text-xs text-muted-foreground">
            <span className="h-2.5 w-2.5 rounded-full" style={{ backgroundColor: colors[idx % colors.length] }} />
            <span className="truncate">{row.email}</span>
            <span className="font-mono">{compactTokens(row.totalTokens)}</span>
          </div>
        ))}
      </div>
      <Modal isOpen={expanded} onOpenChange={setExpanded}>
        <Modal.Backdrop>
          <Modal.Container placement="center">
            <Modal.Dialog className="light w-[min(1120px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900 [--foreground:rgb(15,23,42)] [--muted:rgb(100,116,139)] [--overlay:#fff] [--overlay-foreground:rgb(15,23,42)] [--surface:#fff] [--surface-foreground:rgb(15,23,42)]">
              <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Modal.Header>
                <Modal.Heading>{t("dashboard.usageEmail.trendTitle")}</Modal.Heading>
              </Modal.Header>
              <Modal.Body className="grid gap-3">
                <div className="overflow-x-auto rounded-md border bg-muted/10 p-3">
                  <ShareUsageTrendChart usage={usage} rows={rows} colors={colors} t={t} size="expanded" />
                </div>
              </Modal.Body>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>
    </div>
  );
}

export function ShareUsageTrendChart({
  usage,
  rows,
  colors,
  t,
  size,
}: {
  usage: ShareUsageByEmailResponse;
  rows: ShareUsageByEmailResponse["rows"];
  colors: string[];
  t: TFn;
  size: "compact" | "expanded";
}) {
  const [hover, setHover] = React.useState<{ rowIdx: number; bucketIdx: number } | null>(null);
  const width = 620;
  const height = 220;
  const padding = { left: 34, right: 12, top: 12, bottom: 28 };
  const dates = usage.rows[0]?.daily.map((bucket) => bucket.date) ?? [];
  const bucketGranularity = usage.bucketGranularity ?? (usage.period === "24h" ? "hour" : "day");
  const maxY = Math.max(1, ...rows.flatMap((row) => row.daily.map((bucket) => bucket.totalTokens)));
  const chartWidth = width - padding.left - padding.right;
  const chartHeight = height - padding.top - padding.bottom;
  const pointPosition = (value: number, idx: number) => {
    const x = padding.left + (dates.length <= 1 ? 0 : (idx / (dates.length - 1)) * chartWidth);
    const y = padding.top + chartHeight - (value / maxY) * chartHeight;
    return { x, y };
  };
  const point = (value: number, idx: number) => {
    const { x, y } = pointPosition(value, idx);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  };
  const hoverPoint = hover ? pointPosition(rows[hover.rowIdx]?.daily[hover.bucketIdx]?.totalTokens ?? 0, hover.bucketIdx) : null;
  const hoverBucket = hover ? rows[hover.rowIdx]?.daily[hover.bucketIdx] : null;
  const tooltipWidth = 214;
  const tooltipHeight = 86;
  const tooltipX = hoverPoint ? Math.max(4, Math.min(width - tooltipWidth - 4, hoverPoint.x + 10)) : 0;
  const tooltipY = hoverPoint ? Math.max(4, Math.min(height - tooltipHeight - 4, hoverPoint.y - tooltipHeight - 8)) : 0;
  const tooltipEmail = hover ? rows[hover.rowIdx]?.email ?? "" : "";
  const shortEmail = tooltipEmail.length > 30 ? `${tooltipEmail.slice(0, 27)}...` : tooltipEmail;
  const formatBucketLabel = (bucket: string, detail = false) => {
    if (bucketGranularity === "hour") {
      const date = bucket.slice(5, 10);
      const hour = bucket.slice(11, 13);
      return detail ? `${date} ${hour}:00 UTC` : `${hour}:00`;
    }
    return detail ? bucket : bucket.slice(5);
  };
  const updateHover = (event: React.PointerEvent<SVGPolylineElement>, rowIdx: number) => {
    const svg = event.currentTarget.ownerSVGElement;
    if (!svg || !dates.length) return;
    const rect = svg.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / rect.width) * width;
    const ratio = dates.length <= 1 ? 0 : (x - padding.left) / chartWidth;
    const bucketIdx = Math.max(0, Math.min(dates.length - 1, Math.round(ratio * (dates.length - 1))));
    setHover({ rowIdx, bucketIdx });
  };
  const shouldShowDateLabel = (idx: number) => {
    if (bucketGranularity === "hour") {
      if (idx === 0 || idx === dates.length - 1) return true;
      return idx % 4 === 0;
    }
    if (dates.length <= 10) return true;
    if (idx === 0 || idx === dates.length - 1) return true;
    if (dates.length - 1 - idx < 4) return false;
    return idx % 7 === 0;
  };
  return (
        <svg viewBox={`0 0 ${width} ${height}`} className={`${size === "expanded" ? "h-[520px]" : "h-[220px]"} min-w-[620px] w-full`} role="img" aria-label={t("dashboard.usageEmail.trendAria")} onPointerLeave={() => setHover(null)}>
          <line x1={padding.left} y1={padding.top} x2={padding.left} y2={padding.top + chartHeight} stroke="currentColor" className="text-border" />
          <line x1={padding.left} y1={padding.top + chartHeight} x2={padding.left + chartWidth} y2={padding.top + chartHeight} stroke="currentColor" className="text-border" />
          <text x={padding.left - 6} y={padding.top + 8} textAnchor="end" className="fill-muted-foreground text-[10px]">{compactTokens(maxY)}</text>
          <text x={padding.left - 6} y={padding.top + chartHeight} textAnchor="end" className="fill-muted-foreground text-[10px]">0</text>
          {dates.map((date, idx) => {
            if (!shouldShowDateLabel(idx)) return null;
            const x = padding.left + (dates.length <= 1 ? 0 : (idx / (dates.length - 1)) * chartWidth);
            return (
              <text key={date} x={x} y={height - 8} textAnchor={idx === 0 ? "start" : idx === dates.length - 1 ? "end" : "middle"} className="fill-muted-foreground text-[10px]">
                {formatBucketLabel(date)}
              </text>
            );
          })}
          {rows.map((row, rowIdx) => {
            const points = row.daily.map((bucket, idx) => point(bucket.totalTokens, idx)).join(" ");
            return (
              <React.Fragment key={row.email}>
                <polyline points={points} fill="none" stroke={colors[rowIdx % colors.length]} strokeWidth="2.5" strokeLinejoin="round" strokeLinecap="round" />
                <polyline
                  points={points}
                  fill="none"
                  stroke="transparent"
                  strokeWidth="14"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  className="cursor-crosshair"
                  pointerEvents="stroke"
                  onPointerMove={(event) => updateHover(event, rowIdx)}
                  onFocus={() => setHover({ rowIdx, bucketIdx: row.daily.length - 1 })}
                  tabIndex={0}
                />
              </React.Fragment>
            );
          })}
          {hover && hoverPoint && hoverBucket ? (
            <g pointerEvents="none">
              <line x1={hoverPoint.x} y1={padding.top} x2={hoverPoint.x} y2={padding.top + chartHeight} stroke="currentColor" strokeDasharray="3 3" className="text-muted-foreground/60" />
              <circle cx={hoverPoint.x} cy={hoverPoint.y} r="4" fill={colors[hover.rowIdx % colors.length]} stroke="white" strokeWidth="1.5" />
              <rect x={tooltipX} y={tooltipY} width={tooltipWidth} height={tooltipHeight} rx="6" className="fill-background stroke-border" />
              <text x={tooltipX + 10} y={tooltipY + 18} className="fill-foreground text-[11px] font-semibold">{shortEmail}</text>
              <text x={tooltipX + 10} y={tooltipY + 34} className="fill-muted-foreground text-[10px]">{formatBucketLabel(hoverBucket.date, true)}</text>
              <text x={tooltipX + 10} y={tooltipY + 52} className="fill-foreground text-[10px]">{t("dashboard.usageEmail.total")}: {compactTokens(hoverBucket.totalTokens)}</text>
              <text x={tooltipX + 10} y={tooltipY + 68} className="fill-muted-foreground text-[10px]">
                {t("dashboard.usageEmail.input")} {compactTokens(hoverBucket.inputTokens)} · {t("dashboard.usageEmail.output")} {compactTokens(hoverBucket.outputTokens)}
              </text>
              <text x={tooltipX + 10} y={tooltipY + 80} className="fill-muted-foreground text-[10px]">
                {t("dashboard.usageEmail.cacheRead")} {compactTokens(hoverBucket.cacheReadTokens)} · {t("dashboard.usageEmail.cacheWrite")} {compactTokens(hoverBucket.cacheCreationTokens)}
              </text>
            </g>
          ) : null}
        </svg>
  );
}

/**
 * Client 侧边栏专用：跨该 installation 名下所有 share，列出全量 provider（按 app 分 tab）。
 * provider 列表是 installation 级数据（每个 share 拷贝同一份），按 (app, providerId) 去重；
 * "current" 角标在此场景没有单一答案（多个 share 各绑各的），所以隐藏。
 */
export function ClientProvidersPanel({ shares }: { shares: ShareView[] }) {
  const { locale, t } = useLocaleText();
  const [selectedKey, setSelectedKey] = React.useState<keyof ShareAppProviders>("claude");

  const merged = React.useMemo(() => {
    const out: Record<keyof ShareAppProviders, ShareAppProvider[]> = {
      claude: [],
      codex: [],
      gemini: [],
    };
    const seen: Record<keyof ShareAppProviders, Set<string>> = {
      claude: new Set(),
      codex: new Set(),
      gemini: new Set(),
    };
    shares.forEach((share) => {
      (Object.keys(out) as Array<keyof ShareAppProviders>).forEach((app) => {
        (share.appProviders?.[app] || []).forEach((p) => {
          if (seen[app].has(p.id)) return;
          seen[app].add(p.id);
          out[app].push(p);
        });
      });
    });
    return out;
  }, [shares]);

  // appRuntimes 也是 installation 级，取第一个有数据的 share 即可。
  const runtimes = shares.find((s) => s.appRuntimes)?.appRuntimes;
  const currentProviders = merged[selectedKey];

  return (
    <div className="grid gap-3">
      <Tabs selectedKey={selectedKey} onSelectionChange={(key: React.Key) => setSelectedKey(String(key) as keyof ShareAppProviders)} variant="secondary" className="text-foreground">
        <Tabs.List className="grid w-full grid-cols-3 text-foreground">
          {PROVIDER_APP_TABS.map((tab) => (
            <Tabs.Tab
              key={tab.key}
              id={tab.key}
              className="rounded-md border border-transparent px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors data-[selected=true]:border-primary/30 data-[selected=true]:bg-primary/10 data-[selected=true]:text-primary"
            >
              {tab.label}
            </Tabs.Tab>
          ))}
        </Tabs.List>
      </Tabs>
      {!currentProviders.length ? (
        <EmptyBlock>{t("dashboard.noProviders")}</EmptyBlock>
      ) : (
        <div className="grid gap-2">
          {currentProviders.map((provider) => {
            const runtime = mergeStandaloneOAuthRuntime(providerRuntime(provider), runtimes, provider);
            return (
              <ProviderCard
                key={provider.id}
                provider={provider}
                runtime={runtime}
                t={t}
                locale={locale}
                showCurrentBadge={false}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

type RequestLogTab = "text" | "image";

export function ShareProviderRequestsPanel({
  share,
  app,
}: {
  share: ShareView;
  app: keyof ShareAppProviders;
}) {
  const { t } = useLocaleText();
  const [selectedKey, setSelectedKey] = React.useState<RequestLogTab>("text");
  const textLogs = React.useMemo(
    () => (share.recentRequests || []).filter((log) => (log.appType || "").toLowerCase() === app),
    [share.recentRequests, app],
  );
  return (
    <div className="grid gap-3">
      <div className="mono-label text-muted-foreground">{t("dashboard.requestLogs")}</div>
      <Tabs selectedKey={selectedKey} onSelectionChange={(key: React.Key) => setSelectedKey(String(key) as RequestLogTab)} variant="secondary" className="text-foreground">
        <Tabs.List className="grid w-full grid-cols-2 text-foreground">
          <Tabs.Tab id="text" className="rounded-md border border-transparent px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors data-[selected=true]:border-primary/30 data-[selected=true]:bg-primary/10 data-[selected=true]:text-primary">
            {t("dashboard.textRequests")}
          </Tabs.Tab>
          <Tabs.Tab id="image" className="rounded-md border border-transparent px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors data-[selected=true]:border-primary/30 data-[selected=true]:bg-primary/10 data-[selected=true]:text-primary">
            {t("dashboard.imageJobs")}
          </Tabs.Tab>
        </Tabs.List>
      </Tabs>
      {selectedKey === "text" ? (
        <ShareRequestLogs logs={textLogs} />
      ) : app === "codex" ? (
        <ShareImageRequestLogs shareId={share.shareId} />
      ) : (
        <EmptyBlock>{t("dashboard.noImageJobs")}</EmptyBlock>
      )}
    </div>
  );
}

export function ShareImageRequestLogs({ shareId }: { shareId: string }) {
  const { t } = useLocaleText();
  const [logs, setLogs] = React.useState<ImageGenerationRequestLog[]>([]);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");
    getShareImageGenerationRequestLogs(shareId, 50)
      .then((nextLogs) => {
        if (!cancelled) setLogs(nextLogs);
      })
      .catch((err) => {
        if (!cancelled) {
          setLogs([]);
          setError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [shareId]);

  if (loading) return <EmptyBlock>{t("dashboard.usageEmail.loading")}</EmptyBlock>;
  if (error) return <EmptyBlock>{error}</EmptyBlock>;
  if (!logs.length) return <EmptyBlock>{t("dashboard.noImageJobs")}</EmptyBlock>;

  return (
    <div className="grid gap-2">
      {logs.slice(0, 20).map((log) => {
        const failed = log.status === "failed";
        return (
          <Card key={log.requestId} className="rounded-lg border p-0 shadow-none">
            <Card.Content className="gap-3 p-3">
              <div className="min-w-0">
                <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                  <span className="truncate font-medium">{log.model || "-"}</span>
                  <span className="font-mono text-[11px] text-muted-foreground">{log.requestId}</span>
                </div>
              </div>
              <div className="overflow-x-auto rounded-md border border-default-200">
                <table className="w-full min-w-[840px] table-fixed text-left text-xs">
                  <thead className="bg-muted/50 text-[11px] uppercase text-muted-foreground">
                    <tr>
                      <th className="w-[11%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.preview")}</th>
                      <th className="w-[16%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.user")}</th>
                      <th className="w-[16%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.provider")}</th>
                      <th className="w-[14%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.created")}</th>
                      <th className="w-[10%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.spend")}</th>
                      <th className="w-[12%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.type")}</th>
                      <th className="w-[9%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.size")}</th>
                      <th className="w-[6%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.country")}</th>
                      <th className="w-[6%] px-2 py-1.5 font-medium">{t("dashboard.imageLog.status")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr className="text-muted-foreground">
                      <td className="px-2 py-2">
                        {log.resultUrl ? (
                          <a href={log.resultUrl} target="_blank" rel="noopener noreferrer" className="block h-12 w-12 overflow-hidden rounded-md border border-default-200 bg-muted">
                            <img src={log.resultUrl} alt="" className="h-full w-full object-cover" loading="lazy" />
                          </a>
                        ) : "-"}
                      </td>
                      <td className="truncate px-2 py-2" title={log.createdByEmail || "-"}>{log.createdByEmail || "-"}</td>
                      <td className="truncate px-2 py-2" title={log.providerName || log.providerId || "-"}>{log.providerName || log.providerId || "-"}</td>
                      <td className="truncate px-2 py-2 font-mono" title={formatDateTime(log.createdAt * 1000)}>{formatImageLogTimestamp(log.createdAt)}</td>
                      <td className="truncate px-2 py-2 font-mono">{formatImageLogSpendSeconds(log.latencyMs)}</td>
                      <td className="truncate px-2 py-2" title={log.resultMimeType || "-"}>{log.resultMimeType || "-"}</td>
                      <td className="truncate px-2 py-2 font-mono">{formatImageLogSizeMb(log.resultSizeBytes)}</td>
                      <td className="truncate px-2 py-2">{log.userCountry || "-"}</td>
                      <td className="truncate px-2 py-2 font-mono">{typeof log.statusCode === "number" ? log.statusCode : failed ? log.status : "-"}</td>
                    </tr>
                  </tbody>
                </table>
              </div>
              {log.promptPreview ? <div className="truncate rounded-md bg-muted px-2 py-1.5 text-xs text-muted-foreground" title={log.promptPreview}>{log.promptPreview}</div> : null}
              {log.errorMessage ? <div className="truncate rounded-md bg-danger-50 px-2 py-1.5 text-xs text-danger-700" title={log.errorMessage}>{log.errorMessage}</div> : null}
            </Card.Content>
          </Card>
        );
      })}
    </div>
  );
}

export function ShareRequestLogs({ logs }: { logs: ShareRequestLog[] }) {
  const { locale, t } = useLocaleText();
  if (!logs.length) return <EmptyBlock>{t("dashboard.noRequestLogs")}</EmptyBlock>;
  return (
    <div className="grid gap-2">
      {logs.slice(0, 20).map((log) => (
        <Card key={log.requestId} className="rounded-lg border p-0 shadow-none">
          <Card.Content className="gap-3 p-3">
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <div className="truncate font-medium">{requestModelRoute(log)}</div>
                <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                  {log.isHealthCheck ? <Chip color={log.statusCode >= 200 && log.statusCode < 400 ? "success" : "danger"} size="sm" variant="soft">{t("dashboard.healthCheck")}</Chip> : null}
                  {log.userEmail ? <span>{log.userEmail}</span> : null}
                  <span>{log.providerName || log.providerId || "-"}</span>
                  <span>{log.requestedModel || log.requestModel || "-"}</span>
                  <span title={formatDateTime(log.createdAt * 1000)}>{formatRelativeTime(log.createdAt * 1000, locale)}</span>
                  {log.isStreaming ? <span>stream</span> : null}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
                <Chip color={log.statusCode >= 200 && log.statusCode < 400 ? "success" : "danger"} size="sm" variant="soft">{log.statusCode}</Chip>
                <span>{log.latencyMs}ms</span>
              </div>
            </div>
            <TokenGrid log={log} />
          </Card.Content>
        </Card>
      ))}
    </div>
  );
}

export function ShareModelHealthChecks({ checks }: { checks: ShareModelHealthCheck[] }) {
  const { locale, t } = useLocaleText();
  if (!checks.length) return <EmptyBlock>{t("dashboard.noModelHealthChecks")}</EmptyBlock>;
  return (
    <div className="grid gap-2">
      {checks.slice(0, 10).map((check) => {
        const success = check.status === "success";
        const model = check.actualModel || check.requestedModel || "-";
        return (
          <Card key={check.requestId} className="rounded-lg border p-0 shadow-none">
            <Card.Content className="gap-3 p-3">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="truncate font-medium">{check.appType} · {model}</div>
                  <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                    <span title={formatDateTime(check.checkedAt * 1000)}>{formatRelativeTime(check.checkedAt * 1000, locale)}</span>
                    <span>{check.source || "-"}</span>
                    {check.requestedModel && check.requestedModel !== model ? <span>{check.requestedModel}</span> : null}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
                  <Chip color={success ? "success" : "danger"} size="sm" variant="soft">{success ? t("dashboard.success") : t("dashboard.failed")}</Chip>
                  {typeof check.statusCode === "number" ? <Chip color={check.statusCode >= 200 && check.statusCode < 400 ? "success" : "danger"} size="sm" variant="soft">{check.statusCode}</Chip> : null}
                  <span>{check.latencyMs}ms</span>
                </div>
              </div>
              {check.errorMessage ? <div className="truncate rounded-md bg-danger-50 px-2 py-1.5 text-xs text-danger-700" title={check.errorMessage}>{check.errorMessage}</div> : null}
            </Card.Content>
          </Card>
        );
      })}
    </div>
  );
}

export function TokenGrid({ log }: { log: ShareRequestLog | MarketRequestLog }) {
  const items = [
    ["Input", tokenCount(log.inputTokens), "Fresh input tokens used for input pricing."],
    ["Output", tokenCount(log.outputTokens), "Output tokens used for output pricing."],
    ["Cache R", tokenCount(log.cacheReadTokens), "Cache read tokens used for cache-read pricing."],
    ["Cache W", tokenCount(log.cacheCreationTokens), "Cache creation tokens used for cache-write pricing."],
    ["Total", usageBucketTotalTokens(log), "Input + Output + Cache R + Cache W."],
    ["Hit", formatPercent(cacheHitRate(log)), "Cache R / (Input + Cache R)."],
  ];
  return (
    <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 xl:grid-cols-6">
      {items.map(([label, value, title]) => (
        <div key={label} className="rounded-md bg-muted/40 px-2 py-1.5 text-xs text-muted-foreground" title={String(title)}>
          {label}<span className="ml-2 font-mono font-semibold text-foreground">{typeof value === "number" ? formatNumber(value) : value}</span>
        </div>
      ))}
    </div>
  );
}
