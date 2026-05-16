"use client";

import { ExternalLink } from "lucide-react";
import { Card, Chip, Drawer, ProgressBar } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { AppLocale } from "@/lib/i18n";
import type { DashboardClient, DashboardMarket, HealthCheckEntry, MarketRequestLog, ShareAppRuntimes, ShareRequestLog, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { compactTokens, formatDateTime, formatNumber, formatRelativeTime } from "@/lib/utils";

function compareDesc(left: number, right: number) {
  if (left === right) return 0;
  return left > right ? -1 : 1;
}

function isUnlimited(value?: number) {
  return Number(value) < 0;
}

function isUnlimitedExpiry(value?: string) {
  if (!value) return false;
  const expiresAt = new Date(value).getTime();
  if (Number.isNaN(expiresAt)) return false;
  const fiftyYearsMs = 50 * 365 * 24 * 60 * 60 * 1000;
  return expiresAt - Date.now() >= fiftyYearsMs;
}

function expiryTitle(value?: string) {
  return isUnlimitedExpiry(value) ? "∞" : formatDateTime(value);
}

function formatDurationShort(value?: string, locale: AppLocale = "en", mode: "elapsed" | "remaining" = "elapsed") {
  if (!value) return "--";
  const ts = new Date(value).getTime();
  if (!Number.isFinite(ts)) return "--";
  const diff = mode === "remaining" ? ts - Date.now() : Date.now() - ts;
  const isZh = locale.startsWith("zh");
  if (mode === "remaining" && diff < 0) return isZh ? "已过期" : "expired";
  const abs = Math.max(0, Math.abs(diff));
  const units: Array<[string, string, number]> = [
    ["年", "y", 365 * 24 * 60 * 60 * 1000],
    ["天", "d", 24 * 60 * 60 * 1000],
    ["小时", "h", 60 * 60 * 1000],
    ["分钟", "m", 60 * 1000],
    ["秒", "s", 1000],
  ];
  const [zhUnit, enUnit, ms] = units.find(([, , unitMs]) => abs >= unitMs) || units[units.length - 1];
  const valueCount = Math.max(0, Math.floor(abs / ms));
  return isZh ? `${valueCount}${zhUnit}` : `${valueCount}${enUnit}`;
}

function shareExpiryProgress(share: ShareView, locale: AppLocale) {
  const age = formatDurationShort(share.createdAt, locale, "elapsed");
  const expiry = isUnlimitedExpiry(share.expiresAt) ? "∞" : formatDurationShort(share.expiresAt, locale, "remaining");
  return `${age}/${expiry}`;
}

function expirySortValue(share?: ShareView) {
  if (!share?.expiresAt) return 0;
  if (isUnlimitedExpiry(share.expiresAt)) return Number.POSITIVE_INFINITY;
  const value = new Date(share.expiresAt).getTime();
  return Number.isFinite(value) ? value : 0;
}

function shareApiUrlKey(share?: ShareView) {
  return share?.subdomain || share?.shareName || "";
}

function shareApiParts(share?: ShareView) {
  if (!share) return { apiUrl: "-", apiKey: "***" };
  const baseHost = typeof window === "undefined" ? "" : window.location.host || "";
  const apiUrl = share.subdomain && baseHost ? `${share.subdomain}.${baseHost}` : share.subdomain || baseHost || "-";
  return { apiUrl, apiKey: share.shareToken || "***" };
}

function maskSecret(value?: string) {
  if (!value) return "***";
  if (/^\*+$/.test(value)) return value;
  if (value.length === 1) return `${value}***${value}`;
  return `${value.slice(0, 1)}***${value.slice(-1)}`;
}

function formatUsdOneDecimal(value?: string | number) {
  const amount = Number(value || 0);
  return Number.isFinite(amount) ? `$${amount.toFixed(1)}` : "$0.0";
}

function totalTokens(log?: Partial<ShareRequestLog | MarketRequestLog>) {
  return Number(log?.inputTokens || 0) + Number(log?.outputTokens || 0) + Number(log?.cacheReadTokens || 0) + Number(log?.cacheCreationTokens || 0);
}

function requestModelRoute(log?: Partial<ShareRequestLog | MarketRequestLog>) {
  const record = (log || {}) as Partial<ShareRequestLog & MarketRequestLog>;
  const agent = record.requestAgent || "";
  const requested = record.requestedModel || record.requestModel || "";
  const actual = record.actualModel || record.model || "";
  return [agent, requested && actual && requested !== actual ? `${requested} -> ${actual}` : actual || requested].filter(Boolean).join(" · ") || "-";
}

function formatShareStatus(value?: string) {
  return value ? String(value).replaceAll("_", " ") : "-";
}

function formatPlatformVersion(platform?: string, version?: string) {
  const platformLabel = (platform || "-").toLowerCase();
  const versionLabel = version ? String(version).replace(/^v/i, "") : "-";
  return `${platformLabel}/${versionLabel}`;
}

function sortClients(clients: DashboardClient[]) {
  return [...clients].sort((left, right) => {
    const l = left.share;
    const r = right.share;
    return (
      compareDesc(l?.onlineMinutes24h || 0, r?.onlineMinutes24h || 0) ||
      compareDesc(isUnlimited(l?.tokenLimit) ? Infinity : l?.tokenLimit || 0, isUnlimited(r?.tokenLimit) ? Infinity : r?.tokenLimit || 0) ||
      compareDesc(expirySortValue(l), expirySortValue(r)) ||
      shareApiUrlKey(l).localeCompare(shareApiUrlKey(r), undefined, { sensitivity: "base" })
    );
  });
}

function sortMarkets(markets: DashboardMarket[]) {
  return [...markets].sort((a, b) => Number(b.online) - Number(a.online) || (a.displayName || a.id).localeCompare(b.displayName || b.id));
}

type TFn = ReturnType<typeof useLocaleText>["t"];
const drawerDialogClassName =
  "router-drawer-light light !w-[min(760px,calc(100vw-16px))] !max-w-[calc(100vw-16px)] !bg-white !text-slate-900 " +
  "[--foreground:rgb(var(--router-foreground))] [--muted:rgb(var(--router-muted-foreground))] [--overlay:#fff] [--overlay-foreground:rgb(var(--router-foreground))] " +
  "[--surface:#fff] [--surface-foreground:rgb(var(--router-foreground))] [--surface-secondary:rgb(var(--router-muted))] [--surface-secondary-foreground:rgb(var(--router-foreground))] " +
  "[--default:rgb(var(--router-muted))] [--default-foreground:rgb(var(--router-foreground))]";

function StatusBadge({ active, label }: { active: boolean; label: string }) {
  return <Chip color={active ? "success" : "default"} size="sm" variant={active ? "soft" : "tertiary"}>{label}</Chip>;
}

function ShareStatusBadge({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <StatusBadge active={false} label={t("dashboard.noShare")} />;
  const active = String(share.shareStatus || "").trim().toLowerCase() === "active";
  return <StatusBadge active={active} label={active ? t("common.online") : formatShareStatus(share.shareStatus)} />;
}

function UsageBar({ used, limit, t }: { used: number; limit: number; t: TFn }) {
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

function HealthDots({ entries = [] }: { entries?: HealthCheckEntry[] }) {
  const dots = entries.slice(-10);
  if (!dots.length) {
    return (
      <span className="inline-flex gap-1">
        {Array.from({ length: 10 }).map((_, index) => <i key={index} className="h-2 w-2 rounded-full bg-slate-300" />)}
      </span>
    );
  }
  return (
    <span className="inline-flex gap-1">
      {dots.map((entry, index) => (
        <i key={`${entry.checkedAt}-${index}`} className={entry.isHealthy ? "h-2 w-2 rounded-full bg-emerald-500" : "h-2 w-2 rounded-full bg-red-500"} title={formatDateTime(entry.checkedAt * 1000)} />
      ))}
    </span>
  );
}

function upstreamPercent(apps?: ShareAppRuntimes, key?: keyof ShareAppRuntimes) {
  const value = key ? apps?.[key]?.forSaleOfficialPricePercent : undefined;
  return Number.isInteger(value) && Number(value) > 0 ? `${value}%` : "-";
}

function isOfficialMarker(value?: string) {
  const normalized = String(value || "").trim().toLowerCase();
  return normalized === "official" || normalized === "offical";
}

function runtimeApiUrl(runtime?: ShareUpstreamProvider) {
  return runtime?.apiUrl || "";
}

function hasConcreteApiUrl(runtime?: ShareUpstreamProvider) {
  const apiUrl = runtimeApiUrl(runtime);
  return Boolean(apiUrl && !isOfficialMarker(apiUrl));
}

function isOfficialRuntime(runtime?: ShareUpstreamProvider) {
  if (!runtime) return false;
  const kind = String(runtime.kind || "").toLowerCase();
  const apiUrl = runtimeApiUrl(runtime);
  const models = Array.isArray(runtime.models) ? runtime.models : [];
  const modelsMarkedOfficial = models.length > 0 && models.every((item) => isOfficialMarker(item.actualModel));
  return (kind === "official_oauth" || isOfficialMarker(kind) || isOfficialMarker(apiUrl) || modelsMarkedOfficial) && !hasConcreteApiUrl(runtime);
}

function runtimeModelSummary(runtime?: ShareUpstreamProvider) {
  const models = Array.isArray(runtime?.models) ? runtime.models : [];
  return models
    .map((item) => `${item.slot || "model"}:${item.actualModel || ""}`)
    .filter((value) => !value.endsWith(":"))
    .join(" . ");
}

function runtimeEndpointSummary(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  const pieces = [];
  const apiUrl = runtimeApiUrl(runtime);
  if (apiUrl && !isOfficialMarker(apiUrl)) pieces.push(apiUrl);
  if (runtime.accountEmail) pieces.push(runtime.accountEmail);
  return pieces.join(" · ");
}

function officialAccountSummary(runtime?: ShareUpstreamProvider) {
  return runtime?.accountEmail || "";
}

function countdownStr(resetsAt?: string) {
  if (!resetsAt) return "";
  const diffMs = new Date(resetsAt).getTime() - Date.now();
  if (!Number.isFinite(diffMs) || diffMs <= 0) return "";
  const hours = Math.floor(diffMs / (1000 * 60 * 60));
  const minutes = Math.floor((diffMs % (1000 * 60 * 60)) / (1000 * 60));
  if (hours > 24) {
    const days = Math.floor(hours / 24);
    return `${days}d${hours % 24}h`;
  }
  if (hours > 0) return `${hours}h${minutes}m`;
  return `${minutes}m`;
}

function quotaSummary(runtime?: ShareUpstreamProvider) {
  if (!runtime || hasConcreteApiUrl(runtime)) return "";
  const quota = runtime.quota;
  if (!quota || (quota.status && quota.status !== "ok")) return "";
  let tiers = (quota.tiers || []).filter((tier) => tier.label);
  if (runtime.app === "claude") {
    const preferredLabels = new Set(["5h", "1w"]);
    const preferredTiers = tiers.filter((tier) => preferredLabels.has(String(tier.label).toLowerCase()));
    if (preferredTiers.length) tiers = preferredTiers;
  }
  return tiers
    .map((tier) => [tier.label, `${Math.round(tier.utilization || 0)}%`, countdownStr(tier.resetsAt)].filter(Boolean).join(" "))
    .join(" · ");
}

function ForSaleCell({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const value = share.forSale === "Free" ? t("dashboard.free") : share.forSale === "Yes" ? t("dashboard.yes") : t("dashboard.no");
  const marketLines = share.forSale === "Yes"
    ? share.marketAccessMode === "all" ? [t("dashboard.allMarkets")] : (share.marketLinks || []).map((market) => market.subdomain).filter(Boolean)
    : [];
  return (
    <div className="grid min-w-32 gap-1.5">
      <Chip size="sm" variant={value === "No" ? "tertiary" : "soft"}>{value}</Chip>
      {share.forSale === "Yes" ? (
        <div className="grid gap-0.5 font-mono text-[11px] text-muted-foreground">
          <div>Claude {upstreamPercent(share.appRuntimes, "claude")}</div>
          <div>Codex {upstreamPercent(share.appRuntimes, "codex")}</div>
          <div>Gemini {upstreamPercent(share.appRuntimes, "gemini")}</div>
        </div>
      ) : null}
      {marketLines.length ? <div className="grid gap-0.5 font-mono text-[11px] text-muted-foreground">{marketLines.map((line) => <div key={line}>{line}</div>)}</div> : null}
    </div>
  );
}

function SupportCell({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const rows: Array<[keyof ShareAppRuntimes, string]> = [["claude", "Claude"], ["codex", "Codex"], ["gemini", "Gemini"]];
  return (
    <div className="grid min-w-72 gap-1.5">
      {rows.map(([key, label]) => {
        const enabled = !!share.support?.[key];
        const runtime = share.appRuntimes?.[key];
        const official = enabled && isOfficialRuntime(runtime);
        const firstLine = enabled ? (official ? quotaSummary(runtime) : runtimeModelSummary(runtime) || quotaSummary(runtime)) : "";
        const secondLine = enabled ? (official ? officialAccountSummary(runtime) : runtimeEndpointSummary(runtime) || runtime?.accountEmail || "") : "";
        return (
          <div key={key} className={`grid grid-cols-[56px_1fr] gap-2 rounded-lg border px-2 py-1.5 text-[11px] ${enabled ? "border-emerald-200 bg-emerald-50 text-emerald-700" : "bg-slate-50 text-muted-foreground"}`}>
            <span className="font-mono uppercase">{label}</span>
            <span className="grid min-w-0 gap-0.5 text-right">
              <span className="whitespace-normal break-words font-semibold">{enabled ? firstLine || (official ? "Official" : t("dashboard.on")) : ""}</span>
              {enabled && secondLine ? <span className="whitespace-normal break-words text-[10px] font-medium opacity-75">{secondLine}</span> : null}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function ShareStatusCell({ client, share, t, locale }: { client: DashboardClient; share?: ShareView; t: TFn; locale: AppLocale }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const limit = isUnlimited(share.parallelLimit) ? "∞" : String(share.parallelLimit || 0);
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
  if (!share.isOnline) {
    return (
      <div className="grid min-w-52 gap-2 text-sm">
        <Chip size="sm" variant="tertiary">{t("common.offline")}</Chip>
      </div>
    );
  }
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.platform")}</span><strong>{formatPlatformVersion(client.installation.platform, client.installation.appVersion)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><div><strong>{compactTokens(share.tokensUsed)} / {isUnlimited(share.tokenLimit) ? "∞" : compactTokens(share.tokenLimit)}</strong><UsageBar used={share.tokensUsed} limit={share.tokenLimit} t={t} /></div></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.expires")}</span><strong title={`${formatDateTime(share.createdAt)} / ${expiryTitle(share.expiresAt)}`}>{shareExpiryProgress(share, locale)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{share.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${share.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(share.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={share.healthChecks} /></div>
    </div>
  );
}

export function ClientsTable({ clients }: { clients: DashboardClient[] }) {
  const [selected, setSelected] = React.useState<DashboardClient | null>(null);
  const { locale, t } = useLocaleText();
  const sorted = sortClients(clients);
  const selectedShareApi = shareApiParts(selected?.share);
  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>{t("dashboard.clients")} <span className="font-semibold text-foreground">{sorted.length}</span></div>
        <a href="https://github.com/Xiechengqi/cc-switch/releases" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-blue-400">{t("dashboard.install")}</a>
      </div>
      <Card className="overflow-hidden rounded-[20px]">
        <Card.Content className="overflow-x-auto p-0">
          <table className="w-full min-w-[1180px] border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-72 px-4 py-3">{t("dashboard.share")}</th>
                <th className="px-4 py-3">{t("dashboard.forSale")}</th>
                <th className="px-4 py-3">{t("dashboard.region")}</th>
                <th className="px-4 py-3">{t("dashboard.status")}</th>
                <th className="px-4 py-3">{t("dashboard.support")}</th>
                <th className="w-7 px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? sorted.map((client) => {
                const share = client.share;
                const api = shareApiParts(share);
                return (
                  <tr key={client.installation.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={() => setSelected(client)}>
                    <td className="w-72 break-words px-4 py-3 align-middle">
                      <div className="grid min-w-72 gap-1">
                        <strong className="break-all font-mono text-xs text-foreground">{share ? `${api.apiUrl}/${maskSecret(api.apiKey)}` : "-"}</strong>
                        <span className="break-all text-xs text-muted-foreground">{share?.ownerEmail || "-"}</span>
                        <div className="mt-1"><ShareStatusBadge share={share} t={t} /></div>
                      </div>
                    </td>
                    <td className="px-4 py-3 align-middle"><ForSaleCell share={share} t={t} /></td>
                    <td className="px-4 py-3 align-middle text-muted-foreground">
                      {client.installation.countryCode || "-"}
                    </td>
                    <td className="px-4 py-3 align-middle"><ShareStatusCell client={client} share={share} t={t} locale={locale} /></td>
                    <td className="px-4 py-3 align-middle"><SupportCell share={share} t={t} /></td>
                    <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                  </tr>
                );
              }) : (
                <tr><td colSpan={6} className="px-4 py-10 text-center text-muted-foreground">{t("dashboard.noClients")}</td></tr>
              )}
            </tbody>
          </table>
        </Card.Content>
      </Card>
      <Drawer isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Drawer.Backdrop>
          <Drawer.Content placement="right">
            <Drawer.Dialog className={drawerDialogClassName}>
              <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Drawer.Header>
                <div>
                  <Drawer.Heading className="break-all font-mono text-base">
                    {selected?.share ? `${selectedShareApi.apiUrl}/${maskSecret(selectedShareApi.apiKey)}` : selected?.installation.id}
                  </Drawer.Heading>
                  <p className="mt-1 break-all text-sm text-muted-foreground">{selected?.share?.ownerEmail || "-"}</p>
                  {selected?.share?.description ? (
                    <p className="mt-2 whitespace-pre-wrap break-words text-xs leading-5 text-muted-foreground">{selected.share.description}</p>
                  ) : null}
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selected ? (
                  <div className="grid gap-5">
                    <DrawerSection label={t("dashboard.markets")}><ShareMarkets share={selected.share} t={t} /></DrawerSection>
                    <DrawerSection label={t("dashboard.requestLogs")}><ShareRequestLogs logs={selected.share?.recentRequests || []} /></DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
        </Drawer.Backdrop>
      </Drawer>
    </section>
  );
}

function marketStatusLabel(market: DashboardMarket, t: TFn) {
  if (market.online) return t("common.online");
  return market.status === "active" ? t("common.offline") : market.status || t("common.offline");
}

function marketHealthLabel(market: DashboardMarket, t: TFn) {
  if (market.status === "disabled") return t("dashboard.disabled");
  if (market.status === "offline") return t("common.offline");
  if (!market.online) return t("dashboard.routeOffline");
  if ((market.shareCount || 0) === 0) return t("dashboard.noShares");
  if ((market.shareCount || 0) > 0 && (market.onlineShareCount || 0) === 0) return t("dashboard.noOnlineShares");
  return t("dashboard.healthy");
}

function formatMinutesShort(minutes?: number, locale: AppLocale = "en") {
  const value = Math.max(0, Number(minutes || 0));
  const isZh = locale.startsWith("zh");
  if (value >= 1440) {
    const days = Math.floor(value / 1440);
    const hours = Math.floor((value % 1440) / 60);
    return isZh ? `${days}天${hours ? `${hours}小时` : ""}` : `${days}d${hours ? `${hours}h` : ""}`;
  }
  if (value >= 60) {
    const hours = Math.floor(value / 60);
    const mins = value % 60;
    return isZh ? `${hours}小时${mins ? `${mins}分钟` : ""}` : `${hours}h${mins ? `${mins}m` : ""}`;
  }
  return isZh ? `${value}分钟` : `${value}m`;
}

function formatAgeDaysOrHours(value?: string, locale: AppLocale = "en") {
  if (!value) return "--";
  const ts = new Date(value).getTime();
  if (!Number.isFinite(ts)) return "--";
  const diff = Math.max(0, Date.now() - ts);
  const isZh = locale.startsWith("zh");
  const dayMs = 24 * 60 * 60 * 1000;
  const hourMs = 60 * 60 * 1000;
  if (diff >= dayMs) {
    const days = Math.floor(diff / dayMs);
    return isZh ? `${days}天` : `${days}d`;
  }
  const hours = Math.max(1, Math.floor(diff / hourMs));
  return isZh ? `${hours}小时` : `${hours}h`;
}

function MarketPricingCell({ market, t }: { market: DashboardMarket; t: TFn }) {
  const summary = market.pricingSummary || {};
  const entries = [["Claude", summary.claude], ["Codex", summary.codex], ["Gemini", summary.gemini], ["DeepSeek", summary.deepseek]];
  return (
    <div className="grid min-w-44 gap-2">
      {entries.map(([label, value]) => (
        <div key={label as string} className="grid grid-cols-[66px_1fr] gap-2 text-sm">
          <span className="mono-label text-muted-foreground">{label as string}</span>
          <strong>{typeof value === "number" ? `${value}%` : typeof value === "string" && value ? (value.toLowerCase() === "mixed" ? t("dashboard.mixed") : `${value}%`) : "-"}</strong>
        </div>
      ))}
    </div>
  );
}

function MarketStatusCell({ market, t, locale }: { market: DashboardMarket; t: TFn; locale: AppLocale }) {
  const limit = isUnlimited(market.parallelCapacity) ? "∞" : String(market.parallelCapacity || 0);
  const onlineValue = formatAgeDaysOrHours(market.createdAt, locale);
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.shares")}</span><strong>{market.onlineShareCount || 0} / {market.shareCount || 0}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={formatDateTime(market.createdAt)}>{onlineValue}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{market.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><strong>{compactTokens(market.usageTokens)} / {formatUsdOneDecimal(market.usageAmountUsd)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={market.healthChecks} /></div>
    </div>
  );
}

export function MarketsTable({ markets }: { markets: DashboardMarket[] }) {
  const [selected, setSelected] = React.useState<DashboardMarket | null>(null);
  const { locale, t } = useLocaleText();
  const sorted = sortMarkets(markets);
  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>{t("dashboard.markets")} <span className="font-semibold text-foreground">{sorted.length}</span></div>
        <a href="https://github.com/Xiechengqi/cc-switch-market/releases" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-blue-400">{t("dashboard.install")}</a>
      </div>
      <Card className="overflow-hidden rounded-[20px]">
        <Card.Content className="overflow-x-auto p-0">
          <table className="w-full min-w-[900px] border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-44 px-4 py-3">{t("dashboard.market")}</th>
                <th className="px-4 py-3">{t("dashboard.publicUrl")}</th>
                <th className="px-4 py-3">{t("dashboard.officialPrice")}</th>
                <th className="px-4 py-3">{t("dashboard.status")}</th>
                <th className="w-7 px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? sorted.map((market) => (
                <tr key={market.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={() => setSelected(market)}>
                  <td className="w-44 break-words px-4 py-3 align-middle">
                    <div className="font-medium">{market.displayName || market.id}</div>
                    <div className="text-xs text-muted-foreground">{market.email}</div>
                    <div className="mt-1"><StatusBadge active={market.online} label={marketStatusLabel(market, t)} /></div>
                  </td>
                  <td className="px-4 py-3 align-middle">
                    <a href={market.publicBaseUrl} target="_blank" rel="noreferrer" onClick={(event) => event.stopPropagation()} className="inline-flex items-center gap-1 font-semibold hover:text-primary">
                      {market.publicBaseUrl || "-"}
                      <ExternalLink className="h-3 w-3" />
                    </a>
                  </td>
                  <td className="px-4 py-3 align-middle"><MarketPricingCell market={market} t={t} /></td>
                  <td className="px-4 py-3 align-middle"><MarketStatusCell market={market} t={t} locale={locale} /></td>
                  <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                </tr>
              )) : (
                <tr><td colSpan={5} className="px-4 py-10 text-center text-muted-foreground">{t("dashboard.noMarkets")}</td></tr>
              )}
            </tbody>
          </table>
        </Card.Content>
      </Card>
      <Drawer isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Drawer.Backdrop>
          <Drawer.Content placement="right">
            <Drawer.Dialog className={drawerDialogClassName}>
              <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Drawer.Header>
                <div>
                  <Drawer.Heading>{selected?.displayName || selected?.id}</Drawer.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">{selected?.email}</p>
                  <p className="mt-1 break-all font-mono text-[11px] text-muted-foreground">{selected?.id}</p>
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selected ? (
                  <div className="grid gap-4">
                    <DrawerSection label={t("dashboard.linkedShares")}><MarketLinkedShares market={selected} t={t} /></DrawerSection>
                    <DrawerSection label={t("dashboard.recentRequests")}><MarketRequestLogs logs={selected.recentRequests || []} /></DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
        </Drawer.Backdrop>
      </Drawer>
    </section>
  );
}

function Info({ label, value }: { label: string; value?: React.ReactNode }) {
  return (
    <Card className="rounded-lg border bg-muted/30 p-0 shadow-none">
      <Card.Content className="p-3">
        <div className="mono-label text-muted-foreground">{label}</div>
        <div className="mt-2 break-words text-sm font-medium">{value || "--"}</div>
      </Card.Content>
    </Card>
  );
}

function DrawerSection({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <section className="grid gap-3">
      <div className="font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">{label}</div>
      {children}
    </section>
  );
}

function EmptyBlock({ children }: { children: React.ReactNode }) {
  return <div className="rounded-lg border bg-muted/20 p-4 text-sm text-muted-foreground">{children}</div>;
}

function ShareMarkets({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <EmptyBlock>{t("dashboard.noShare")}</EmptyBlock>;
  if (share.forSale === "Free") return <EmptyBlock>{t("dashboard.publicFreeShare")}</EmptyBlock>;
  if (share.forSale !== "Yes") return <EmptyBlock>{t("dashboard.notForSale")}</EmptyBlock>;
  const links = share.marketLinks || [];
  const unknown = share.unknownMarketEmails || [];
  return (
    <div className="grid gap-2">
      {share.marketAccessMode === "all" ? <EmptyBlock>{t("dashboard.authorizedAllMarkets")}</EmptyBlock> : null}
      {links.map((market) => (
        <Card key={market.id || market.email} className="rounded-lg border p-0 shadow-none">
          <Card.Content className="flex-row items-center justify-between gap-3 p-3">
            <div className="min-w-0">
              <div className="truncate font-medium">{market.displayName || market.subdomain || market.email}</div>
              <div className="truncate text-xs text-muted-foreground">{market.subdomain || "-"} · {market.email || "-"}</div>
            </div>
            <Chip color={market.online ? "success" : "default"} size="sm" variant={market.online ? "soft" : "tertiary"}>{market.online ? t("common.online") : t("common.offline")}</Chip>
          </Card.Content>
        </Card>
      ))}
      {unknown.map((email) => <EmptyBlock key={email}>{t("dashboard.unknownMarket")}: {email}</EmptyBlock>)}
      {!links.length && !unknown.length && share.marketAccessMode !== "all" ? <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock> : null}
    </div>
  );
}

function ShareRequestLogs({ logs }: { logs: ShareRequestLog[] }) {
  const { t } = useLocaleText();
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
                  <span>{log.providerName || log.providerId || "-"}</span>
                  <span>{log.requestedModel || log.requestModel || "-"}</span>
                  <span title={formatDateTime(log.createdAt * 1000)}>{formatRelativeTime(log.createdAt * 1000)}</span>
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

function TokenGrid({ log }: { log: ShareRequestLog | MarketRequestLog }) {
  const items = [
    ["Input", log.inputTokens || 0],
    ["Output", log.outputTokens || 0],
    ["Cache R", log.cacheReadTokens || 0],
    ["Cache W", log.cacheCreationTokens || 0],
    ["Total", totalTokens(log)],
  ];
  return (
    <div className="grid grid-cols-2 gap-2 sm:grid-cols-5">
      {items.map(([label, value]) => (
        <div key={label} className="rounded-md bg-muted/40 px-2 py-1.5 text-xs text-muted-foreground">
          {label}<span className="ml-2 font-mono font-semibold text-foreground">{formatNumber(Number(value))}</span>
        </div>
      ))}
    </div>
  );
}

function MarketLinkedShares({ market, t }: { market: DashboardMarket; t: TFn }) {
  const shares = market.linkedShares || [];
  if (!shares.length) return <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock>;
  return (
    <div className="grid gap-2">
      {shares.map((share) => {
        const supported = [
          ["claude", "Claude"],
          ["codex", "Codex"],
          ["gemini", "Gemini"],
        ].filter(([key]) => share.support?.[key as keyof typeof share.support]);
        return (
          <Card key={share.shareId} className="rounded-lg border p-0 shadow-none">
            <Card.Content className="flex-row items-center justify-between gap-3 p-3">
              <div className="min-w-0">
                <div className="truncate font-medium">{share.subdomain || share.shareName || "-"}</div>
                <div className="truncate text-xs text-muted-foreground">{share.ownerEmail || "-"}</div>
              </div>
              <div className="grid justify-items-end gap-1">
                <Chip color={share.online ? "success" : "default"} size="sm" variant={share.online ? "soft" : "tertiary"}>{share.online ? t("common.online") : t("common.offline")}</Chip>
                {supported.length ? <div className="flex gap-1">{supported.map(([, label]) => <Chip key={label} size="sm" variant="tertiary">{label}</Chip>)}</div> : null}
              </div>
            </Card.Content>
          </Card>
        );
      })}
    </div>
  );
}

function MarketRequestLogs({ logs }: { logs: MarketRequestLog[] }) {
  const { t } = useLocaleText();
  if (!logs.length) return <EmptyBlock>{t("dashboard.noMarketRequests")}</EmptyBlock>;
  return (
    <div className="grid gap-2">
      {logs.slice(0, 20).map((log) => (
        <Card key={log.requestId} className="rounded-lg border p-0 shadow-none">
          <Card.Content className="gap-3 p-3">
            <div className="min-w-0">
              <div className="truncate font-medium">
                {[log.userEmail || "-", log.shareSubdomain || log.shareId || "-", requestModelRoute(log), log.statusCode || log.status || "-", log.latencyMs ? `${log.latencyMs}ms` : "", `${compactTokens(totalTokens(log))} tokens`, formatUsdOneDecimal(log.usageAmountUsd)].filter(Boolean).join(" · ")}
              </div>
              <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                <span title={formatDateTime(log.createdAt)}>{formatRelativeTime(log.createdAt)}</span>
                <span>{log.requestId || "-"}</span>
              </div>
            </div>
            <TokenGrid log={log} />
          </Card.Content>
        </Card>
      ))}
    </div>
  );
}

export function PresenceFooter() {
  const { t } = useLocaleText();
  const [presence, setPresence] = React.useState<{ onlineCount: number; emailSent24h: number } | null>(null);
  React.useEffect(() => {
    const sessionId = crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`;
    async function tick() {
      const res = await fetch("/v1/dashboard/presence", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ sessionId }),
      });
      if (res.ok) setPresence(await res.json());
    }
    tick().catch(console.error);
    const id = window.setInterval(() => tick().catch(console.error), 15000);
    return () => window.clearInterval(id);
  }, []);
  return (
    <footer className="mx-auto flex w-[calc(100%-2rem)] max-w-7xl flex-wrap items-center justify-center gap-2 py-6 font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
      <span>{t("dashboard.pageOnline")} <strong className="ml-1 text-foreground">{presence?.onlineCount ?? 0}</strong></span>
      <span className="opacity-50">|</span>
      <span>{t("dashboard.emailSent24h")} <strong className="ml-1 text-foreground">{presence?.emailSent24h ?? 0}</strong></span>
      <span className="opacity-50">|</span>
      <a href="https://github.com/Xiechengqi/cc-switch-router" target="_blank" rel="noopener noreferrer" className="hover:text-primary">GitHub</a>
    </footer>
  );
}
