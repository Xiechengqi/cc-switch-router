"use client";

import { Eye, ExternalLink, Link2, Loader2, Maximize2, Pencil, Save, Crown, X } from "lucide-react";
import { Button, Card, Checkbox, Chip, Drawer, Input, ListBox, Modal, ProgressBar, Select, Tabs, TextArea } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ShareConnectDialog } from "@/components/dashboard/share-connect-dialog";
import { getMarketLinkedShares, getShareUsageByEmail, releaseMarketShareState, updateMarketDisabledShares, updateMarketMaintenance, updateShareSettings } from "@/lib/api";
import type { AppLocale } from "@/lib/i18n";
import type { DashboardClient, DashboardMarket, HealthCheckEntry, HealthTimelineBucket, MarketAppAvailabilityEntry, MarketRequestLog, MarketShare, MarketShareRuntimeState, ModelHealthSummary, ShareAccessByApp, ShareAppProvider, ShareAppProviders, ShareAppRuntimes, ShareModelHealthCheck, ShareRequestLog, ShareSettingsPatch, ShareUpstreamProvider, ShareUsageByEmailResponse, ShareView } from "@/lib/types";
import { cn, compactTokens, formatDateTime, formatNumber, formatRelativeTime } from "@/lib/utils";

function shouldOpenRowDrawer(event: React.MouseEvent<HTMLElement>) {
  const selection = window.getSelection();
  if (selection && !selection.isCollapsed && selection.toString().trim()) {
    return false;
  }

  const target = event.target as HTMLElement | null;
  if (target?.closest("a,button,input,textarea,select,[role='button'],[data-no-row-drawer]")) {
    return false;
  }

  return true;
}

const UNLIMITED_TOKEN_LIMIT = -1;
const UNLIMITED_PARALLEL_LIMIT = -1;
const MIN_PARALLEL_LIMIT = 3;
const DEFAULT_PARALLEL_LIMIT = 3;
const DEFAULT_TOKEN_LIMIT = 100000;
const PERMANENT_EXPIRES_AT_ISO = "2099-12-31T23:59:59Z";
const CORE_SHARE_APPS = [
  ["claude", "Claude"],
  ["codex", "Codex"],
  ["gemini", "Gemini"],
] as const;

function isUnlimitedTokenLimit(value?: number | null) {
  return value === UNLIMITED_TOKEN_LIMIT;
}

function isUnlimitedParallelLimit(value?: number | null) {
  return value === UNLIMITED_PARALLEL_LIMIT;
}

function isPermanentExpiryDate(value?: string | null) {
  if (!value) return false;
  const date = new Date(value);
  return !Number.isNaN(date.getTime()) && date.getUTCFullYear() >= 2099;
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

function averageRecentLatencyMs(logs?: ShareRequestLog[], limit = 10) {
  const samples = [...(logs || [])]
    .sort((left, right) => Number(right.createdAt || 0) - Number(left.createdAt || 0))
    .slice(0, limit)
    .map((log) => Number(log.latencyMs || 0))
    .filter((latency) => Number.isFinite(latency) && latency > 0);
  if (!samples.length) return null;
  return samples.reduce((sum, latency) => sum + latency, 0) / samples.length;
}

function formatLatencySeconds(latencyMs: number | null) {
  if (latencyMs == null || !Number.isFinite(latencyMs) || latencyMs <= 0) return "-";
  const seconds = latencyMs / 1000;
  return `${seconds < 1 ? seconds.toFixed(2) : seconds.toFixed(1)}s`;
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
  if (!share) return { apiUrl: "-" };
  const baseHost = typeof window === "undefined" ? "" : window.location.host || "";
  const apiUrl = share.subdomain && baseHost ? `${share.subdomain}.${baseHost}` : share.subdomain || baseHost || "-";
  return { apiUrl };
}

function clientTunnelDisplayUrl(value?: string | null) {
  const trimmed = String(value || "").trim();
  if (!trimmed) return "";
  if (/^http:\/\//i.test(trimmed)) return `https://${trimmed.slice("http://".length)}`;
  if (/^https:\/\//i.test(trimmed)) return trimmed;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)) return trimmed;
  return `https://${trimmed}`;
}

function formatUsdOneDecimal(value?: string | number) {
  const amount = Number(value || 0);
  return Number.isFinite(amount) ? `$${amount.toFixed(1)}` : "$0.0";
}

function formatUsdExactTrimmed(value?: string | number) {
  if (value == null || value === "") return "";
  const raw = String(value).trim();
  const amount = Number(raw);
  if (!Number.isFinite(amount)) return "";
  if (amount === 0) return "$0";
  const unsigned = raw.replace(/^\+/, "");
  const normalized = unsigned.includes("e") || unsigned.includes("E")
    ? amount.toFixed(12)
    : unsigned;
  return `$${normalized.replace(/(\.\d*?[1-9])0+$/, "$1").replace(/\.0+$/, "")}`;
}

function tokenCount(value?: string | number | null) {
  const count = Number(value || 0);
  return Number.isFinite(count) && count > 0 ? count : 0;
}

function usageBucketTotalTokens(log?: Partial<ShareRequestLog | MarketRequestLog>) {
  return tokenCount(log?.inputTokens) + tokenCount(log?.outputTokens) + tokenCount(log?.cacheReadTokens) + tokenCount(log?.cacheCreationTokens);
}

function cacheHitRate(log?: Partial<ShareRequestLog | MarketRequestLog>) {
  const input = tokenCount(log?.inputTokens);
  const cacheRead = tokenCount(log?.cacheReadTokens);
  const denominator = input + cacheRead;
  return denominator > 0 ? cacheRead / denominator : 0;
}

function formatPercent(value: number) {
  const percent = Math.max(0, Math.min(1, Number(value) || 0)) * 100;
  return `${percent.toFixed(percent >= 10 ? 0 : 1).replace(/\.0$/, "")}%`;
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
  const commitMatch = versionLabel.match(/^commit\s+([0-9a-f]{7,40})$/i);
  if (commitMatch) {
    return `${platformLabel}/${commitMatch[1].slice(0, 7)}`;
  }
  return `${platformLabel}/${versionLabel}`;
}

function clientPlatformLabel(client: DashboardClient) {
  return formatPlatformVersion(client.installation.platform, client.installation.appVersion);
}

function sortClients(clients: DashboardClient[]) {
  // 按注册时间升序（先注册的在前）。lastSeenAt / shareCount 都是高频变化字段，
  // 用作排序键会让行频繁上下跳动，体验很差。createdAt 一旦写入就稳定。
  return [...clients].sort((left, right) => {
    return (
      (Date.parse(left.installation.createdAt) || 0) -
        (Date.parse(right.installation.createdAt) || 0) ||
      left.installation.id.localeCompare(right.installation.id, undefined, { sensitivity: "base" })
    );
  });
}

function sortMarkets(markets: DashboardMarket[]) {
  // 同上：按注册时间升序，避免 online 抖动改变行序。
  return [...markets].sort(
    (a, b) =>
      (Date.parse(a.createdAt) || 0) - (Date.parse(b.createdAt) || 0) ||
      (a.displayName || a.id).localeCompare(b.displayName || b.id),
  );
}

function isShareMarket(market: DashboardMarket) {
  return market.marketKind === "share";
}

function isUsageMarket(market: DashboardMarket) {
  return !isShareMarket(market);
}

function marketKindLabel(market: DashboardMarket, t: TFn) {
  return isShareMarket(market) ? t("dashboard.shareMarket") : t("dashboard.tokenMarket");
}

function marketKindDescription(market: DashboardMarket, t: TFn) {
  return isShareMarket(market)
    ? t("dashboard.shareMarketTooltip")
    : t("dashboard.tokenMarketTooltip");
}

function canShowMarketSharePriority(market: DashboardMarket) {
  return isUsageMarket(market) && Boolean(market.canManage);
}

function marketLabel(market: DashboardMarket) {
  return market.displayName || market.subdomain || market.email;
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
  const status = String(share.shareStatus || "").trim().toLowerCase();
  if (status === "active") return <Chip color="success" size="sm" variant="soft">{t("dashboard.shareStatus.active")}</Chip>;
  if (status === "paused") return <Chip color="warning" size="sm" variant="soft">{t("dashboard.shareStatus.paused")}</Chip>;
  if (status === "expired") return <Chip color="default" size="sm" variant="tertiary">{t("dashboard.shareStatus.expired")}</Chip>;
  return <StatusBadge active={false} label={formatShareStatus(share.shareStatus)} />;
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

function healthTimelineTone(status?: string) {
  switch (status) {
    case "healthy":
      return "border-emerald-600 bg-emerald-500";
    case "degraded":
      return "border-lime-500 bg-lime-300";
    case "unhealthy":
      return "border-amber-500 bg-amber-400";
    case "offline":
      return "border-rose-600 bg-rose-500";
    default:
      return "border-slate-300 bg-slate-200 dark:border-slate-700 dark:bg-slate-800";
  }
}

function healthTimelineLabel(status?: string, locale: AppLocale = "en") {
  const zh = locale.startsWith("zh");
  switch (status) {
    case "healthy":
      return zh ? "健康" : "Healthy";
    case "degraded":
      return zh ? "轻微降级" : "Degraded";
    case "unhealthy":
      return zh ? "不稳定" : "Unhealthy";
    case "offline":
      return zh ? "离线" : "Offline";
    default:
      return zh ? "未知" : "Unknown";
  }
}

function HealthTimelineStrip({ timeline = [] }: { timeline?: HealthTimelineBucket[] }) {
  const { locale, t } = useLocaleText();
  const buckets = timeline.length
    ? timeline.slice(-48)
    : Array.from({ length: 48 }, (_, index) => ({
        startAt: "",
        endAt: "",
        status: "unknown",
        score: 0,
        onlineMinutes: 0,
        observedMinutes: 0,
        requestCount: 0,
        failureCount: 0,
      }));
  const latest = [...buckets].reverse().find((bucket) => bucket.status !== "unknown");
  const latestLabel = healthTimelineLabel(latest?.status, locale);
  const latestScore = latest ? `${Math.round(latest.score || 0)}%` : "--";
  return (
    <div className="grid gap-3">
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="font-semibold text-foreground">{t("dashboard.health")} · 24h</span>
        <span className="font-mono text-[11px] text-muted-foreground">{latestLabel} {latestScore}</span>
      </div>
      <div className="grid grid-cols-[repeat(24,minmax(0,1fr))] gap-1 max-sm:grid-cols-[repeat(12,minmax(0,1fr))]">
        {buckets.map((bucket, index) => {
          const title = [
            bucket.startAt && bucket.endAt ? `${formatDateTime(bucket.startAt)} - ${formatDateTime(bucket.endAt)}` : "",
            healthTimelineLabel(bucket.status, locale),
            `${Math.round(bucket.score || 0)}%`,
            `${bucket.onlineMinutes || 0}/30m`,
            bucket.requestCount ? `${bucket.requestCount} req · ${bucket.failureCount || 0} failed` : "",
          ].filter(Boolean).join(" · ");
          return (
            <span
              key={`${bucket.startAt || "unknown"}-${index}`}
              title={title}
              className={`aspect-square min-h-2 rounded-[3px] border ${healthTimelineTone(bucket.status)}`}
            />
          );
        })}
      </div>
      <div className="flex items-center justify-between gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-muted-foreground">
        <span>-24h</span>
        <span>{formatMinutesShort(30, locale)}</span>
        <span>now</span>
      </div>
    </div>
  );
}

function upstreamPercent(apps?: ShareAppRuntimes, key?: keyof ShareAppRuntimes) {
  const value = key ? apps?.[key]?.forSaleOfficialPricePercent : undefined;
  return Number.isInteger(value) && Number(value) > 0 ? `${value}%` : "-";
}

function configuredUpstreamPercent(apps?: ShareAppRuntimes, key?: keyof ShareAppRuntimes) {
  const value = key ? apps?.[key]?.forSaleOfficialPricePercent : undefined;
  return Number.isInteger(value) && Number(value) > 0 ? `${value}%` : null;
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

function runtimeLooksOAuth(runtime?: ShareUpstreamProvider) {
  const text = [
    runtime?.app,
    runtime?.kind,
    runtime?.providerName,
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  return text.includes("oauth") || Boolean(oauthRuntimeKeyFromProvider(runtime));
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
    .join(" · ");
}

function modelHealthKey(value?: string) {
  return String(value || "").trim().toLowerCase();
}

function runtimeModelKeys(runtime?: ShareUpstreamProvider) {
  const models = Array.isArray(runtime?.models) ? runtime.models : [];
  return new Set(models.map((item) => modelHealthKey(item.actualModel)).filter(Boolean));
}

function relevantModelHealthEntries(share: ShareView, key: "claude" | "codex" | "gemini") {
  const entries = share.modelHealth?.[key] || [];
  const currentModels = runtimeModelKeys(share.appRuntimes?.[key]);
  if (currentModels.size === 0) return entries;
  return entries.filter((entry) => isAppLevelQuotaBlockedModelHealth(entry, key) || currentModels.has(modelHealthKey(entry.requestedModel)) || currentModels.has(modelHealthKey(entry.actualModel)));
}

function modelHealthFailureReason(entries: ModelHealthSummary[]) {
  const failed = entries
    .filter((entry) => entry.status === "failed" || (entry.recentResults || []).includes("failed"))
    .sort((left, right) => Number(right.lastCheckedAt || 0) - Number(left.lastCheckedAt || 0))[0];
  const code = Number(failed?.statusCode || 0);
  const message = String(failed?.errorMessage || "").toLowerCase();
  if (code === 429 || message.includes("429") || message.includes("rate limit") || message.includes("rate limited") || message.includes("usage limit")) return "Rate Limited";
  if (code === 401 || message.includes("auth rejected (401)") || message.includes("token_invalidated") || message.includes("token invalidated") || message.includes("invalidated")) return "Banned";
  if (code === 403 || message.includes("403") || message.includes("forbidden")) return "Forbidden";
  if (code === 402 || message.includes("insufficient") || message.includes("quota")) return "No Credits";
  if (message.includes("timeout") || message.includes("timed out")) return "Timeout";
  if (message.includes("oauth") || message.includes("refresh token")) return "Auth Failed";
  if (code >= 500 || message.includes("server error")) return "Provider Error";
  return "Failed";
}

function isQuotaBlockedModelHealth(entry: ModelHealthSummary) {
  const message = String(entry.errorMessage || "").toLowerCase();
  return entry.status === "quota_blocked"
    || message.includes("quota exhausted")
    || message.includes("quota_exhausted")
    || message.includes("usage limit")
    || message.includes("usage_limit")
    || message.includes("weekly limit")
    || message.includes("monthly limit");
}

function isAppLevelQuotaBlockedModelHealth(entry: ModelHealthSummary, key: "claude" | "codex" | "gemini") {
  return modelHealthKey(entry.requestedModel) === key
    && modelHealthKey(entry.actualModel) === key
    && isQuotaBlockedModelHealth(entry);
}

function runtimeEndpointSummary(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  const apiUrl = runtimeApiUrl(runtime);
  return apiUrl && !isOfficialMarker(apiUrl) ? apiUrl : "";
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

function quotaTierLabel(label?: string, locale: AppLocale = "en") {
  const normalized = String(label || "").trim().toLowerCase();
  if (normalized === "premium") return locale.startsWith("zh") ? "高级请求" : "Premium request";
  return label || "";
}

function formatQuotaAmount(value?: number, locale: AppLocale = "en") {
  if (typeof value !== "number" || !Number.isFinite(value)) return "";
  return new Intl.NumberFormat(locale, {
    maximumFractionDigits: value % 1 === 0 ? 0 : 2,
    useGrouping: false,
  }).format(value);
}

type OAuthRuntimeKey = "kiro" | "cursor" | "antigravity" | "copilot";

function oauthRuntimeKeyFromProvider(value?: Partial<ShareUpstreamProvider & ShareAppProvider>): OAuthRuntimeKey | undefined {
  const text = [
    value?.app,
    value?.kind,
    value?.providerName,
    value?.providerType,
    value?.name,
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  if (text.includes("kiro")) return "kiro";
  if (text.includes("cursor")) return "cursor";
  if (text.includes("antigravity")) return "antigravity";
  if (text.includes("copilot") || text.includes("github_copilot")) return "copilot";
  return undefined;
}

function mergeStandaloneOAuthRuntime(
  runtime?: ShareUpstreamProvider,
  appRuntimes?: ShareAppRuntimes,
  provider?: Partial<ShareUpstreamProvider & ShareAppProvider>,
) {
  const key = oauthRuntimeKeyFromProvider(provider || runtime);
  const standalone = key ? appRuntimes?.[key] : undefined;
  if (!runtime) return standalone;
  if (!standalone) return runtime;
  return {
    ...runtime,
    accountEmail: runtime.accountEmail || standalone.accountEmail,
    quota: runtime.quota || standalone.quota,
    models: runtime.models?.length ? runtime.models : standalone.models,
  };
}

function quotaSummary(runtime?: ShareUpstreamProvider, locale: AppLocale = "en") {
  if (!runtime || (hasConcreteApiUrl(runtime) && !runtimeLooksOAuth(runtime))) return "";
  const quota = runtime.quota;
  const status = String(quota?.status || "").toLowerCase();
  if (!quota || (status && !["ok", "success", "valid"].includes(status))) return "";
  const isKiro = oauthRuntimeKeyFromProvider(runtime) === "kiro";
  let tiers = (quota.tiers || [])
    .map((tier) => ({ ...tier, label: tier.label || tier.name }))
    .filter((tier) => tier.label);
  if (runtime.app === "claude") {
    const preferredLabels = new Set(["5h", "1w"]);
    const preferredTiers = tiers.filter((tier) => preferredLabels.has(String(tier.label).toLowerCase()));
    if (preferredTiers.length) tiers = preferredTiers;
  }
  const tierText = tiers
    .map((tier) => {
      const used = formatQuotaAmount(tier.used, locale);
      const limit = formatQuotaAmount(tier.limit, locale);
      const usage = isKiro && used && limit ? `${used}/${limit}` : quotaTierLabel(tier.label, locale);
      return [usage, `${Math.round(tier.utilization || 0)}%`, countdownStr(tier.resetsAt)].filter(Boolean).join(" ");
    })
    .join(" · ");
  return [quota.plan || quota.credentialMessage, tierText].filter(Boolean).join(" · ");
}

function providerAccountLevel(runtime?: ShareUpstreamProvider, locale: AppLocale = "en") {
  return quotaSummary(runtime, locale) || runtime?.providerName || runtime?.kind || "-";
}

function providerAccountIdentity(runtime?: ShareUpstreamProvider) {
  return runtime?.accountEmail || "-";
}

function providerModelMap(runtime?: ShareUpstreamProvider) {
  return runtimeModelSummary(runtime) || "-";
}

function ForSaleCell({ share, t }: { share?: ShareView; t: TFn }) {
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

function modelHealthTone(share: ShareView, key: "claude" | "codex" | "gemini") {
  const entries = relevantModelHealthEntries(share, key);
  if (entries.some((entry) => isAppLevelQuotaBlockedModelHealth(entry, key))) {
    return {
      className: "border-red-200 bg-red-50 text-red-700",
      label: modelHealthFailureReason(entries),
    };
  }
  const results = entries.flatMap((entry) => (entry.recentResults || []).slice(0, 3));
  if (!results.length) {
    return {
      className: "border-emerald-200 bg-emerald-50 text-emerald-700",
      label: "healthy",
    };
  }
  const failures = results.filter((result) => result === "failed").length;
  const allModelsFailed = entries.length > 0 && entries.every((entry) => {
    const recent = (entry.recentResults || []).slice(0, 3);
    return recent.length >= 3 && recent.every((result) => result === "failed");
  });
  if (allModelsFailed) {
    return {
      className: "border-red-200 bg-red-50 text-red-700",
      label: modelHealthFailureReason(entries),
    };
  }
  if (failures > 0) {
    return {
      className: "border-amber-200 bg-amber-50 text-amber-700",
      label: "degraded",
    };
  }
  return {
    className: "border-emerald-200 bg-emerald-50 text-emerald-700",
    label: "healthy",
  };
}

function modelHealthTitle(share: ShareView, key: "claude" | "codex" | "gemini") {
  const entries = relevantModelHealthEntries(share, key);
  if (!entries.length) return "No failures recorded yet";
  return entries
    .map((entry) => {
      const recent = (entry.recentResults || []).join(" / ") || entry.status;
      const checked = entry.lastCheckedAt ? formatDateTime(entry.lastCheckedAt * 1000) : "-";
      const model = entry.requestedModel || entry.actualModel || "-";
      return `${model}: ${recent} · ${checked}${entry.errorMessage ? ` · ${entry.errorMessage}` : ""}`;
    })
    .join("\n");
}

function SupportCell({ share, t, locale }: { share?: ShareView; t: TFn; locale: AppLocale }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  type CoreAppKey = "claude" | "codex" | "gemini";
  const rows: Array<[CoreAppKey, string]> = [["claude", "Claude"], ["codex", "Codex"], ["gemini", "Gemini"]];
  return (
    <div className="grid min-w-0 gap-1.5">
      {rows.map(([key, label]) => {
        const enabled = !!share.support?.[key];
        const runtime = mergeStandaloneOAuthRuntime(share.appRuntimes?.[key], share.appRuntimes);
        const tone = enabled ? modelHealthTone(share, key) : { className: "bg-slate-50 text-muted-foreground", label: "" };
        return (
          <div key={key} title={enabled ? modelHealthTitle(share, key) : undefined} className={`grid grid-cols-[56px_1fr] gap-2 rounded-lg border px-2 py-1.5 text-[11px] ${tone.className}`}>
            <span className="font-mono uppercase">{label}</span>
            <span className="grid min-w-0 gap-0.5 text-right">
              <span className="whitespace-normal break-words font-semibold">{enabled ? providerAccountLevel(runtime, locale) : ""}</span>
              <span className="whitespace-normal break-words text-[10px] font-medium opacity-75">{enabled ? providerAccountIdentity(runtime) : ""}</span>
              <span className="whitespace-normal break-words text-[10px] font-medium opacity-75">{enabled ? providerModelMap(runtime) : ""}</span>
              <span className="text-[10px] font-semibold opacity-70">{enabled ? tone.label : ""}</span>
            </span>
          </div>
        );
      })}
    </div>
  );
}

function ShareEditAction({ share, onEdit, t }: { share?: ShareView; onEdit: (share: ShareView) => void; t: TFn }) {
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

function ShareConnectChip({
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

function splitEmails(value: string) {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);
}

function EmailTagsField({
  value,
  onChange,
  disabled,
  placeholder,
  onPromote,
  promotableEmails,
  promoteLabel,
}: {
  value: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
  placeholder?: string;
  onPromote?: (email: string) => void;
  promotableEmails?: string[];
  promoteLabel?: string;
}) {
  const [draft, setDraft] = React.useState("");
  const promotableSet = React.useMemo(
    () => new Set(promotableEmails ?? []),
    [promotableEmails],
  );
  const commit = (raw: string) => {
    const parts = splitEmails(raw);
    setDraft("");
    if (!parts.length) return;
    const next = [...value];
    for (const part of parts) {
      if (!next.includes(part)) next.push(part);
    }
    if (next.length !== value.length) onChange(next);
  };
  const removeAt = (idx: number) => onChange(value.filter((_, i) => i !== idx));
  return (
    <div
      className={`flex min-h-10 w-full flex-wrap items-center gap-1.5 rounded-lg border border-slate-200 bg-white px-2 py-1.5 text-sm transition-colors focus-within:border-primary/50 ${disabled ? "cursor-not-allowed opacity-60" : ""}`}
    >
      {value.map((email, idx) => {
        const canPromote =
          !disabled && Boolean(onPromote) && promotableSet.has(email);
        return (
          <span
            key={email}
            className="inline-flex max-w-full items-center gap-1.5 rounded-full border border-primary/20 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary"
          >
            <span className="min-w-0 truncate">{email}</span>
            {canPromote ? (
              <button
                type="button"
                aria-label={`${promoteLabel ?? "Set as owner"}: ${email}`}
                title={promoteLabel ?? "Set as owner"}
                className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-amber-100/70 text-amber-700 transition-colors hover:bg-amber-200/80"
                onClick={() => onPromote?.(email)}
              >
                <Crown className="h-3 w-3" />
              </button>
            ) : null}
            {disabled ? null : (
              <button
                type="button"
                aria-label={`remove ${email}`}
                className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-primary/15 transition-colors hover:bg-primary/25"
                onClick={() => removeAt(idx)}
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </span>
        );
      })}
      <input
        value={draft}
        disabled={disabled}
        className="h-7 min-w-[10rem] flex-1 bg-transparent text-slate-900 placeholder:text-muted-foreground focus:outline-none disabled:cursor-not-allowed"
        placeholder={value.length ? "" : placeholder}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === ",") {
            event.preventDefault();
            commit(draft);
          } else if (event.key === "Backspace" && draft === "" && value.length) {
            event.preventDefault();
            removeAt(value.length - 1);
          }
        }}
        onBlur={() => commit(draft)}
        onPaste={(event) => {
          const text = event.clipboardData.getData("text");
          if (/[\s,;]/.test(text)) {
            event.preventDefault();
            commit(text);
          }
        }}
      />
    </div>
  );
}

function toLocalDateTimeValue(value?: string) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (num: number) => String(num).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function fromLocalDateTimeValue(value: string) {
  if (!value.trim()) return undefined;
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toISOString() : value;
}

function providerHint(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  if (isOfficialRuntime(runtime)) return "Official";
  return runtime.accountEmail || runtime.apiUrl || runtime.kind || "";
}

type PriceApp = "claude" | "codex" | "gemini";
const PRICE_APPS: Array<{ key: PriceApp; label: string }> = [
  { key: "claude", label: "Claude" },
  { key: "codex", label: "Codex" },
  { key: "gemini", label: "Gemini" },
];

function shareAccessApps(share: ShareView | null): PriceApp[] {
  if (!share) return ["claude", "codex", "gemini"];
  const bound = PRICE_APPS.map((app) => app.key).filter((app) => Boolean(share.bindings?.[app]));
  return bound.length ? bound : ["claude", "codex", "gemini"];
}

function effectiveShareAccessByApp(share: ShareView): ShareAccessByApp {
  if (share.accessByApp && Object.keys(share.accessByApp).length > 0) return share.accessByApp;
  const result: ShareAccessByApp = {};
  for (const app of shareAccessApps(share)) {
    result[app] = {
      sharedWithEmails: share.sharedWithEmails ?? [],
      marketAccessMode: share.marketAccessMode === "all" ? "all" : "selected",
    };
  }
  return result;
}

function normalizedUniqueEmails(values: string[]) {
  return Array.from(new Set(values.map((value) => value.trim().toLowerCase()).filter(Boolean))).sort();
}

function ShareEditDialog({
  share,
  markets,
  onClose,
  onSaved,
}: {
  share: ShareView | null;
  markets: DashboardMarket[];
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [description, setDescription] = React.useState("");
  const [forSale, setForSale] = React.useState<"Yes" | "No" | "Free">("No");
  const [saleMarketKind, setSaleMarketKind] = React.useState<"token" | "share">("token");
  const [marketAccessMode, setMarketAccessMode] = React.useState<"selected" | "all">("selected");
  const [selectedMarketEmails, setSelectedMarketEmails] = React.useState<string[]>([]);
  const [selectedShareMarketEmail, setSelectedShareMarketEmail] = React.useState("");
  const [shareToEmailsByApp, setShareToEmailsByApp] = React.useState<Record<PriceApp, string[]>>({ claude: [], codex: [], gemini: [] });
  const [tokenLimitInput, setTokenLimitInput] = React.useState(String(DEFAULT_TOKEN_LIMIT));
  const [tokenLimitUnlimited, setTokenLimitUnlimited] = React.useState(false);
  const [lastFiniteTokenLimit, setLastFiniteTokenLimit] = React.useState(DEFAULT_TOKEN_LIMIT);
  const [parallelLimitInput, setParallelLimitInput] = React.useState(String(DEFAULT_PARALLEL_LIMIT));
  const [parallelLimitUnlimited, setParallelLimitUnlimited] = React.useState(false);
  const [lastFiniteParallelLimit, setLastFiniteParallelLimit] = React.useState(DEFAULT_PARALLEL_LIMIT);
  const [expiresAtInput, setExpiresAtInput] = React.useState("");
  const [expiresPermanent, setExpiresPermanent] = React.useState(false);
  const [priceInputs, setPriceInputs] = React.useState<Record<PriceApp, string>>({ claude: "", codex: "", gemini: "" });
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [confirmFreeOpen, setConfirmFreeOpen] = React.useState(false);
  const [transferTargetEmail, setTransferTargetEmail] = React.useState("");
  const [marketSelectKey, setMarketSelectKey] = React.useState(0);
  const { t } = useLocaleText();
  const readOnly = !!share && !share.canManage;
  const activeShareApps = React.useMemo(() => shareAccessApps(share), [share]);
  const tokenMarkets = React.useMemo(() => markets.filter((market) => !isShareMarket(market)), [markets]);
  const shareMarkets = React.useMemo(() => markets.filter(isShareMarket), [markets]);
  const publicMarketEmails = React.useMemo(
    () => new Set(markets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [markets],
  );
  const tokenMarketEmails = React.useMemo(
    () => new Set(tokenMarkets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [tokenMarkets],
  );
  const shareMarketEmails = React.useMemo(
    () => new Set(shareMarkets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [shareMarkets],
  );
  const transferableShareEmails = React.useMemo(
    () => normalizedUniqueEmails(Object.values(shareToEmailsByApp).flat().filter((email) => !publicMarketEmails.has(email))),
    [publicMarketEmails, shareToEmailsByApp],
  );

  React.useEffect(() => {
    if (!share) return;
    const pendingPricing =
      share.activeEdit?.status === "rejected"
        ? share.activeEdit.patch.forSaleOfficialPricePercentByApp || {}
        : {};
    const sharePricing = share.forSaleOfficialPricePercentByApp || {};
    const initialPricing: Record<PriceApp, string> = { claude: "", codex: "", gemini: "" };
    for (const app of PRICE_APPS) {
      const pending = pendingPricing[app.key];
      const fallback = sharePricing[app.key];
      const value = typeof pending === "number" ? pending : fallback;
      initialPricing[app.key] = typeof value === "number" && value > 0 ? String(value) : "";
    }

    setDescription(share.description || "");
    setForSale((share.forSale as "Yes" | "No" | "Free") || "No");
    const initialSaleMarketKind = share.saleMarketKind === "share" ? "share" : "token";
    setSaleMarketKind(initialSaleMarketKind);
    const initialMode = (share.marketAccessMode as "selected" | "all") || "selected";
    setMarketAccessMode(initialSaleMarketKind === "share" ? "selected" : initialMode);
    const marketLinks = share.marketLinks || [];
    setSelectedMarketEmails(
      initialSaleMarketKind === "token" && initialMode === "selected"
        ? marketLinks
            .map((link) => (link.email || "").toLowerCase())
            .filter((email) => email && !shareMarketEmails.has(email))
        : [],
    );
    setSelectedShareMarketEmail(
      initialSaleMarketKind === "share"
        ? marketLinks
            .map((link) => (link.email || "").toLowerCase())
            .find((email) => email && !tokenMarketEmails.has(email)) || ""
        : "",
    );
    const accessByApp = effectiveShareAccessByApp(share);
    setShareToEmailsByApp({
      claude: splitEmails((accessByApp.claude?.sharedWithEmails || []).join("\n")).filter((email) => !publicMarketEmails.has(email)),
      codex: splitEmails((accessByApp.codex?.sharedWithEmails || []).join("\n")).filter((email) => !publicMarketEmails.has(email)),
      gemini: splitEmails((accessByApp.gemini?.sharedWithEmails || []).join("\n")).filter((email) => !publicMarketEmails.has(email)),
    });

    const initialToken = share.tokenLimit ?? UNLIMITED_TOKEN_LIMIT;
    const tokenUnlimited = isUnlimitedTokenLimit(initialToken);
    setTokenLimitUnlimited(tokenUnlimited);
    setTokenLimitInput(tokenUnlimited ? String(UNLIMITED_TOKEN_LIMIT) : String(initialToken));
    if (!tokenUnlimited && initialToken > 0) setLastFiniteTokenLimit(initialToken);

    const initialParallel = share.parallelLimit ?? DEFAULT_PARALLEL_LIMIT;
    const parallelUnlimited = isUnlimitedParallelLimit(initialParallel);
    setParallelLimitUnlimited(parallelUnlimited);
    setParallelLimitInput(parallelUnlimited ? String(UNLIMITED_PARALLEL_LIMIT) : String(initialParallel));
    if (!parallelUnlimited && initialParallel >= MIN_PARALLEL_LIMIT) setLastFiniteParallelLimit(initialParallel);

    const permanent = isPermanentExpiryDate(share.expiresAt) || isUnlimitedExpiry(share.expiresAt);
    setExpiresPermanent(permanent);
    setExpiresAtInput(permanent ? "" : toLocalDateTimeValue(share.expiresAt));

    setPriceInputs(initialPricing);
    setError(share.activeEdit?.status === "rejected" ? share.activeEdit.errorMessage || t("dashboard.applyFailedFallback") : "");
    setNotice("");
    setConfirmFreeOpen(false);
    setTransferTargetEmail("");
    setMarketSelectKey((current) => current + 1);
  }, [publicMarketEmails, share, shareMarketEmails, t, tokenMarketEmails]);

  const handleForSaleChange = (next: "Yes" | "No" | "Free") => {
    if (next === "Free" && forSale !== "Free") {
      setConfirmFreeOpen(true);
      return;
    }
    setForSale(next);
  };

  const handleSaleMarketKindChange = (next: "token" | "share") => {
    setSaleMarketKind(next);
    if (next === "share") {
      setMarketAccessMode("selected");
      setSelectedMarketEmails([]);
      setSelectedShareMarketEmail((current) => current || shareMarkets[0]?.email.toLowerCase() || "");
      setPriceInputs({ claude: "", codex: "", gemini: "" });
    } else {
      setSelectedShareMarketEmail("");
    }
  };

  const handleTokenUnlimited = (checked: boolean) => {
    setTokenLimitUnlimited(checked);
    if (checked) {
      const parsed = Number.parseInt(tokenLimitInput, 10);
      if (Number.isFinite(parsed) && parsed > 0) setLastFiniteTokenLimit(parsed);
      setTokenLimitInput(String(UNLIMITED_TOKEN_LIMIT));
    } else {
      setTokenLimitInput(String(lastFiniteTokenLimit));
    }
  };

  const handleParallelUnlimited = (checked: boolean) => {
    setParallelLimitUnlimited(checked);
    if (checked) {
      const parsed = Number.parseInt(parallelLimitInput, 10);
      if (Number.isFinite(parsed) && parsed >= MIN_PARALLEL_LIMIT) setLastFiniteParallelLimit(parsed);
      setParallelLimitInput(String(UNLIMITED_PARALLEL_LIMIT));
    } else {
      setParallelLimitInput(String(lastFiniteParallelLimit));
    }
  };

  const removeMarketEmail = (email: string) => {
    setSelectedMarketEmails((current) => current.filter((value) => value !== email));
  };

  const onMarketPicked = (raw: string) => {
    if (!raw) return;
    if (raw === "__all__") {
      setMarketAccessMode("all");
      setSelectedMarketEmails([]);
      setMarketSelectKey((current) => current + 1);
      return;
    }
    const normalized = raw.toLowerCase();
    setMarketAccessMode("selected");
    setSelectedMarketEmails((current) => Array.from(new Set([...current, normalized])).sort());
    setMarketSelectKey((current) => current + 1);
  };

  const availableMarkets = React.useMemo(() => {
    const blocked = new Set(selectedMarketEmails);
    return tokenMarkets
      .filter((market) => market.email && !blocked.has(market.email.toLowerCase()))
      .sort((a, b) => (a.displayName || a.email).localeCompare(b.displayName || b.email));
  }, [selectedMarketEmails, tokenMarkets]);

  const descriptionLength = description.trim().length;
  const descriptionInvalid = descriptionLength > 200;

  const tokenParsed = Number.parseInt(tokenLimitInput, 10);
  const tokenInvalid = !tokenLimitUnlimited && (!Number.isFinite(tokenParsed) || tokenParsed <= 0);

  const parallelParsed = Number.parseInt(parallelLimitInput, 10);
  const parallelInvalid =
    !parallelLimitUnlimited && (!Number.isFinite(parallelParsed) || parallelParsed < MIN_PARALLEL_LIMIT);

  const expiryInvalid = !expiresPermanent && !expiresAtInput.trim();

  const pricingPayload = React.useMemo<Record<string, number>>(() => {
    if (saleMarketKind === "share") return {};
    const result: Record<string, number> = {};
    for (const app of PRICE_APPS) {
      if (!share?.support?.[app.key]) continue;
      const raw = priceInputs[app.key];
      if (!raw || !raw.trim()) continue;
      const value = Number.parseInt(raw, 10);
      if (Number.isFinite(value) && value >= 1 && value <= 100) result[app.key] = value;
    }
    return result;
  }, [priceInputs, saleMarketKind, share]);

  const pricingInvalid = React.useMemo(() => {
    if (saleMarketKind === "share") return false;
    const check = (raw: string) => {
      if (!raw || !raw.trim()) return false;
      const value = Number.parseInt(raw, 10);
      return !(Number.isFinite(value) && value >= 1 && value <= 100);
    };
    return PRICE_APPS.some((app) => check(priceInputs[app.key]));
  }, [priceInputs, saleMarketKind]);

  const shareMarketInvalid = forSale === "Yes" && saleMarketKind === "share" && !selectedShareMarketEmail;

  const formInvalid =
    descriptionInvalid || tokenInvalid || parallelInvalid || expiryInvalid || pricingInvalid || shareMarketInvalid;

  const save = async () => {
    if (!share || readOnly || busy || formInvalid) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const expiresIso = expiresPermanent
        ? PERMANENT_EXPIRES_AT_ISO
        : fromLocalDateTimeValue(expiresAtInput);
      const effectiveSaleMarketKind = forSale === "Yes" ? saleMarketKind : "token";
      const effectiveMarketAccessMode = effectiveSaleMarketKind === "share" ? "selected" : marketAccessMode;
      const accessByApp: ShareAccessByApp = {};
      for (const app of activeShareApps) {
        const shareToEmails = (shareToEmailsByApp[app] ?? []).filter((email) => !publicMarketEmails.has(email));
        const saleEmails =
          forSale === "Yes" && effectiveSaleMarketKind === "token" && effectiveMarketAccessMode === "selected"
            ? selectedMarketEmails
            : forSale === "Yes" && effectiveSaleMarketKind === "share" && selectedShareMarketEmail
              ? [selectedShareMarketEmail]
              : [];
        accessByApp[app] = {
          sharedWithEmails: normalizedUniqueEmails([
            ...shareToEmails,
            ...saleEmails,
          ]),
          marketAccessMode: effectiveMarketAccessMode,
        };
      }
      const sharedWithEmails = normalizedUniqueEmails(
        Object.values(accessByApp).flatMap((access) => access?.sharedWithEmails ?? []),
      );
      const patch: ShareSettingsPatch = {
        description: description.trim() || null,
        forSale,
        saleMarketKind: effectiveSaleMarketKind,
        marketAccessMode: effectiveMarketAccessMode,
        sharedWithEmails,
        accessByApp,
        tokenLimit: tokenLimitUnlimited ? UNLIMITED_TOKEN_LIMIT : tokenParsed,
        parallelLimit: parallelLimitUnlimited ? UNLIMITED_PARALLEL_LIMIT : parallelParsed,
      };
      if (expiresIso) patch.expiresAt = expiresIso;
      patch.forSaleOfficialPricePercentByApp = pricingPayload;
      const res = await updateShareSettings(share.shareId, patch);
      await onSaved();
      if (res.appliedSynchronously) {
        onClose();
      } else {
        setNotice(t("dashboard.shareEditQueued"));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const transferOwner = async () => {
    if (!share || readOnly || busy || !transferTargetEmail) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const targetEmail = transferTargetEmail.toLowerCase();
      const effectiveSaleMarketKind = forSale === "Yes" ? saleMarketKind : "token";
      const effectiveMarketAccessMode = effectiveSaleMarketKind === "share" ? "selected" : marketAccessMode;
      const accessByApp: ShareAccessByApp = {};
      for (const app of activeShareApps) {
        const shareToEmails = (shareToEmailsByApp[app] ?? []).filter((email) => !publicMarketEmails.has(email));
        const saleEmails =
          forSale === "Yes" && effectiveSaleMarketKind === "token" && effectiveMarketAccessMode === "selected"
            ? selectedMarketEmails
            : forSale === "Yes" && effectiveSaleMarketKind === "share" && selectedShareMarketEmail
              ? [selectedShareMarketEmail]
              : [];
        accessByApp[app] = {
          sharedWithEmails: normalizedUniqueEmails([
            ...shareToEmails.filter((email) => email !== targetEmail),
            share.ownerEmail || "",
            ...saleEmails,
          ]),
          marketAccessMode: effectiveMarketAccessMode,
        };
      }
      const nextShared = normalizedUniqueEmails(
        Object.values(accessByApp).flatMap((access) => access?.sharedWithEmails ?? []),
      );
      const res = await updateShareSettings(share.shareId, {
        ownerEmail: targetEmail,
        sharedWithEmails: nextShared,
        accessByApp,
        saleMarketKind: effectiveSaleMarketKind,
        marketAccessMode: effectiveMarketAccessMode,
      });
      await onSaved();
      setTransferTargetEmail("");
      if (res.appliedSynchronously) {
        onClose();
      } else {
        setNotice(t("dashboard.shareEditQueued"));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <Modal isOpen={!!share} onOpenChange={(open) => !open && !busy && onClose()}>
        <Modal.Backdrop>
          <Modal.Container>
            <Modal.Dialog className="share-edit-surface light w-[min(760px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
              <Modal.Header>
                <Modal.Heading>{readOnly ? t("dashboard.shareViewSettings") : t("dashboard.shareEditSettings")}</Modal.Heading>
                <p className="mt-1 break-all text-sm text-muted-foreground">{share?.subdomain || share?.shareName}</p>
                {readOnly ? (
                  <p className="mt-2 text-xs text-muted-foreground">{t("dashboard.shareReadOnlyNotice")}</p>
                ) : null}
              </Modal.Header>
              <Modal.Body className="grid max-h-[72vh] gap-4 overflow-y-auto">
                {error ? (
                  <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div>
                ) : null}
                {notice ? (
                  <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">{notice}</div>
                ) : null}

                <FieldGroup
                  label={t("dashboard.field.description")}
                  hint={<span>{t("dashboard.hint.maxChars")}<span className="ml-2 font-mono">{descriptionLength}/200</span></span>}
                  invalid={descriptionInvalid}
                >
	                  <TextArea
	                    value={description}
	                    maxLength={200}
                      disabled={readOnly}
	                    onChange={(event) => setDescription(event.target.value)}
	                  />
                </FieldGroup>

                <div className="grid gap-3 sm:grid-cols-3">
                  <FieldGroup label={t("dashboard.field.forSale")}>
	                    <Select
	                      selectedKey={forSale}
	                      onSelectionChange={(key) => handleForSaleChange(String(key || "No") as "Yes" | "No" | "Free")}
                      isDisabled={readOnly}
	                    >
                      <Select.Trigger>
                        <Select.Value>{forSale}</Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                        <ListBox>
                          {["No", "Yes", "Free"].map((item) => (
                            <ListBox.Item key={item} id={item}>{item}</ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                  </FieldGroup>

                  {forSale === "Yes" ? (
                    <FieldGroup label={t("dashboard.field.marketType")}>
                      <Select
                        selectedKey={saleMarketKind}
                        onSelectionChange={(key) => handleSaleMarketKindChange(String(key || "token") as "token" | "share")}
                        isDisabled={readOnly}
                      >
                        <Select.Trigger>
                          <Select.Value>{saleMarketKind === "share" ? t("dashboard.shareMarket") : t("dashboard.tokenMarket")}</Select.Value>
                          <Select.Indicator />
                        </Select.Trigger>
                        <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                          <ListBox>
                            <ListBox.Item id="token">{t("dashboard.tokenMarket")}</ListBox.Item>
                            <ListBox.Item id="share">{t("dashboard.shareMarket")}</ListBox.Item>
                          </ListBox>
                        </Select.Popover>
                      </Select>
                    </FieldGroup>
                  ) : null}

                  <FieldGroup label={t("dashboard.field.marketAccess")} hint={forSale === "Yes" ? undefined : t("dashboard.hint.forSaleOnly")}>
                    <Select
                      key={marketSelectKey}
	                      selectedKey={null}
	                      onSelectionChange={(key) => onMarketPicked(String(key || ""))}
	                      isDisabled={readOnly || forSale !== "Yes" || saleMarketKind === "share"}
	                    >
                      <Select.Trigger>
                        <Select.Value>
                          {saleMarketKind === "share"
                            ? t("dashboard.selectedShareMarketOnly")
                            : marketAccessMode === "all" ? t("dashboard.allMarkets") : t("dashboard.addMarket")}
                        </Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                        <ListBox>
                          <ListBox.Item id="__all__">{t("dashboard.allMarkets")}</ListBox.Item>
                          {availableMarkets.map((market) => (
                            <ListBox.Item key={market.email} id={market.email}>
                              {marketLabel(market)}
                              <span className="ml-1 text-muted-foreground">· {market.email}</span>
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                  </FieldGroup>
                </div>

                {forSale === "Yes" && saleMarketKind === "share" ? (
                  <FieldGroup label={t("dashboard.shareMarket")} hint={t("dashboard.hint.shareMarketSingle")} invalid={shareMarketInvalid}>
                    <Select
                      selectedKey={selectedShareMarketEmail || null}
                      onSelectionChange={(key) => setSelectedShareMarketEmail(String(key || "").toLowerCase())}
                      isDisabled={readOnly}
                    >
                      <Select.Trigger>
                        <Select.Value>
                          {selectedShareMarketEmail
                            ? shareMarkets.find((market) => market.email.toLowerCase() === selectedShareMarketEmail)?.displayName ||
                              shareMarkets.find((market) => market.email.toLowerCase() === selectedShareMarketEmail)?.subdomain ||
                              selectedShareMarketEmail
                            : t("dashboard.selectShareMarket")}
                        </Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                        <ListBox>
                          {shareMarkets.map((market) => (
                            <ListBox.Item key={market.email} id={market.email.toLowerCase()}>
                              {marketLabel(market)}
                              <span className="ml-1 text-muted-foreground">· {market.email}</span>
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                    {shareMarketInvalid ? <span className="text-xs text-red-600">{t("dashboard.fieldInvalid")}</span> : null}
                  </FieldGroup>
                ) : null}

                {forSale === "Yes" && saleMarketKind === "token" ? (
                <div className="grid gap-1.5 text-sm">
                  <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                    <span className="mono-label text-muted-foreground">{t("dashboard.field.modelPricing")}</span>
                    <span className="text-xs text-muted-foreground">{t("dashboard.hint.modelPricing")}</span>
                  </div>
                  <div className="grid gap-3 sm:grid-cols-3">
                    {PRICE_APPS.map((app) => {
                      const supported = !!share?.support?.[app.key];
                      const hint = providerHint(share?.appRuntimes?.[app.key]);
                      return (
                        <div key={app.key} className="grid gap-1">
                          <span className="mono-label text-muted-foreground">{app.label}</span>
                          <Input
                            type="number"
                            min={1}
                            max={100}
                            step={1}
                            value={priceInputs[app.key]}
                            disabled={readOnly || !supported}
                            placeholder={supported ? t("common.unset") : t("dashboard.noCurrentNode")}
                            onChange={(event) =>
                              setPriceInputs((current) => ({ ...current, [app.key]: event.target.value }))
                            }
                          />
                          <span className="truncate text-[11px] text-muted-foreground">{hint || "-"}</span>
                        </div>
                      );
                    })}
                  </div>
                  {pricingInvalid ? (
                    <span className="text-xs text-red-600">{t("dashboard.fieldInvalid")}</span>
                  ) : null}
                </div>
                ) : null}

                {forSale === "Yes" && saleMarketKind === "token" && marketAccessMode === "selected" ? (
                  <FieldGroup label={t("dashboard.field.selectedMarkets")} hint={t("dashboard.hint.selectedMarkets")}>
                    {selectedMarketEmails.length ? (
                      <div className="flex flex-wrap gap-1.5">
                        {selectedMarketEmails.map((email) => {
                          const meta = tokenMarkets.find((market) => (market.email || "").toLowerCase() === email);
                          const label = meta ? marketLabel(meta) : email;
                          return (
                            <span
                              key={email}
                              className="inline-flex items-center gap-1.5 rounded-full border border-primary/20 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary"
                            >
                              {label}
	                              {readOnly ? null : (
	                                <button
	                                  type="button"
	                                  aria-label={`remove ${email}`}
	                                  className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-primary/15 transition-colors hover:bg-primary/25"
	                                  onClick={() => removeMarketEmail(email)}
	                                >
	                                  <X className="h-3 w-3" />
	                                </button>
	                              )}
                            </span>
                          );
                        })}
                      </div>
                    ) : (
                      <div className="rounded-lg border border-dashed border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
                        {t("dashboard.noAuthorizedMarkets")}
                      </div>
                    )}
                  </FieldGroup>
                ) : null}

                {forSale === "Yes" && saleMarketKind === "token" && marketAccessMode === "all" ? (
                  <div className="rounded-lg border border-primary/20 bg-primary/5 px-3 py-2 text-xs text-primary">
                    {t("dashboard.allMarketsSelected")}
	                    <button
	                      type="button"
	                      className="ml-3 text-[11px] underline decoration-dotted underline-offset-2 hover:text-primary/80"
                      disabled={readOnly}
	                      onClick={() => {
                        setMarketAccessMode("selected");
                        setSelectedMarketEmails([]);
                      }}
                    >
                      {t("dashboard.switchToSelected")}
                    </button>
                  </div>
                ) : null}

	                <FieldGroup label={t("dashboard.field.sharedWith")} hint={readOnly ? t("dashboard.hint.sharedWithReadOnly") : t("dashboard.hint.sharedWith")}>
                    <div className="grid gap-3">
                      {activeShareApps.map((app) => {
                        const label = PRICE_APPS.find((item) => item.key === app)?.label ?? app;
                        return (
                          <div key={app} className="grid gap-1.5">
                            <span className="mono-label text-muted-foreground">{label}</span>
                            <EmailTagsField
                              value={shareToEmailsByApp[app] ?? []}
                              placeholder="friend@example.com, teammate@example.com"
                              disabled={readOnly}
                              onChange={(emails) =>
                                setShareToEmailsByApp((current) => ({ ...current, [app]: emails }))
                              }
                              onPromote={(email) => setTransferTargetEmail(email)}
                              promotableEmails={transferableShareEmails}
                              promoteLabel={t("dashboard.setAsOwner")}
                            />
                          </div>
                        );
                      })}
                    </div>
                </FieldGroup>

                <div className="grid gap-3 md:grid-cols-3">
                  <FieldGroup label={t("dashboard.field.tokenLimit")} invalid={tokenInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="number"
                        min={1}
                        step={1}
	                        value={tokenLimitInput}
	                        disabled={readOnly || tokenLimitUnlimited}
                        onChange={(event) => {
                          setTokenLimitInput(event.target.value);
                          const parsed = Number.parseInt(event.target.value, 10);
                          if (Number.isFinite(parsed) && parsed > 0) setLastFiniteTokenLimit(parsed);
                        }}
                      />
                      <Checkbox
	                        isSelected={tokenLimitUnlimited}
	                        onChange={(value: boolean) => handleTokenUnlimited(value)}
                          isDisabled={readOnly}
	                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">{t("common.unlimited")}</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>

                  <FieldGroup label={t("dashboard.field.parallelLimit")} hint={t("dashboard.hint.minValue", { value: MIN_PARALLEL_LIMIT })} invalid={parallelInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="number"
                        min={MIN_PARALLEL_LIMIT}
                        step={1}
	                        value={parallelLimitInput}
	                        disabled={readOnly || parallelLimitUnlimited}
                        onChange={(event) => {
                          setParallelLimitInput(event.target.value);
                          const parsed = Number.parseInt(event.target.value, 10);
                          if (Number.isFinite(parsed) && parsed >= MIN_PARALLEL_LIMIT) {
                            setLastFiniteParallelLimit(parsed);
                          }
                        }}
                      />
                      <Checkbox
	                        isSelected={parallelLimitUnlimited}
	                        onChange={(value: boolean) => handleParallelUnlimited(value)}
                          isDisabled={readOnly}
	                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">{t("common.unlimited")}</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>

                  <FieldGroup label={t("dashboard.field.expiresAt")} invalid={expiryInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="datetime-local"
	                        value={expiresAtInput}
	                        disabled={readOnly || expiresPermanent}
	                        onChange={(event) => setExpiresAtInput(event.target.value)}
                      />
                      <Checkbox
	                        isSelected={expiresPermanent}
	                        onChange={(value: boolean) => setExpiresPermanent(value)}
                          isDisabled={readOnly}
	                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">{t("dashboard.permanent")}</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>
                </div>
              </Modal.Body>
              <Modal.Footer>
	                <Button variant="outline" onClick={onClose} isDisabled={busy}>{readOnly ? t("common.close") : t("common.cancel")}</Button>
	                {readOnly ? null : (
	                  <Button variant="primary" onClick={save} isDisabled={busy || formInvalid}>
	                    {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
	                    {t("common.save")}
	                  </Button>
	                )}
              </Modal.Footer>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>

      <ConfirmAlertDialog
        open={confirmFreeOpen}
        title={t("dashboard.confirmFreeTitle")}
        description={t("dashboard.confirmFreeDesc")}
        confirmLabel={t("dashboard.confirmFree")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        onConfirm={() => {
          setForSale("Free");
          setConfirmFreeOpen(false);
        }}
        onOpenChange={(open) => !open && setConfirmFreeOpen(false)}
      />
      <ConfirmAlertDialog
        open={Boolean(transferTargetEmail)}
        title={t("dashboard.transferOwnerTitle")}
        description={t("dashboard.transferOwnerDesc", { target: transferTargetEmail || "-", owner: share?.ownerEmail || "-" })}
        confirmLabel={t("dashboard.transferOwnerConfirm")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        onConfirm={transferOwner}
        onOpenChange={(open) => !open && setTransferTargetEmail("")}
      />
    </>
  );
}

function FieldGroup({
  label,
  hint,
  invalid,
  children,
}: {
  label: string;
  hint?: React.ReactNode;
  invalid?: boolean;
  children: React.ReactNode;
}) {
  const { t } = useLocaleText();
  return (
    <div className="grid gap-1.5 text-sm">
      <span className="mono-label text-muted-foreground">{label}</span>
      {children}
      {hint || invalid ? (
        <span className={`text-xs ${invalid ? "text-red-600" : "text-muted-foreground"}`}>
          {invalid ? t("dashboard.fieldInvalid") : null}
          {hint && !invalid ? hint : null}
        </span>
      ) : null}
    </div>
  );
}

function ShareStatusCell({ share, t, locale }: { share?: ShareView; t: TFn; locale: AppLocale }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const limit = isUnlimited(share.parallelLimit) ? "∞" : String(share.parallelLimit || 0);
  const averageLatency = averageRecentLatencyMs(share.recentRequests);
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
  const shareMarketListingUrl = shareStatusShareMarketUrl(share);
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
      <div>
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
      </div>
    </div>
  );
  if (!share.isOnline) {
    return (
      <div className="grid min-w-0 gap-2 text-sm">
        {saleRow}
        <Chip size="sm" variant="tertiary">{t("common.offline")}</Chip>
      </div>
    );
  }
  return (
    <div className="grid min-w-0 gap-2 text-sm">
      {saleRow}
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><div><strong>{compactTokens(share.tokensUsed)} / {isUnlimited(share.tokenLimit) ? "∞" : compactTokens(share.tokenLimit)}</strong><UsageBar used={share.tokensUsed} limit={share.tokenLimit} t={t} /></div></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.expires")}</span><strong title={`${formatDateTime(share.createdAt)} / ${expiryTitle(share.expiresAt)}`}>{shareExpiryProgress(share, locale)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{share.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.response")}</span><strong>{formatLatencySeconds(averageLatency)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${share.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(share.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={share.healthChecks} /></div>
    </div>
  );
}

function shareStatusShareMarketUrl(share: ShareView) {
  if (share.forSale !== "Yes" || share.saleMarketKind !== "share") return null;
  const market = (share.marketLinks || []).find(
    (item) => item.marketKind === "share" && item.publicBaseUrl,
  );
  if (!market?.publicBaseUrl) return null;
  const base = market.publicBaseUrl.replace(/\/+$/, "");
  const routerId = share.routerId || "main";
  return `${base}/listing/share?router_id=${encodeURIComponent(routerId)}&share_id=${encodeURIComponent(share.shareId)}`;
}

function clientOwnerEmail(client?: DashboardClient | null) {
  return client?.clientTunnel?.ownerEmail || client?.installation.ownerEmail || "-";
}

function clientRegionLabel(client?: DashboardClient | null) {
  return client?.installation.countryCode || client?.installation.region || "-";
}

function clientDisplayLabel(client?: DashboardClient | null) {
  return clientTunnelDisplayUrl(client?.clientTunnel?.tunnelUrl) || client?.installation.id || "-";
}

function shareSupportLabel(share: ShareView) {
  return CORE_SHARE_APPS
    .filter(([key]) => !!share.support?.[key])
    .map(([, label]) => label)
    .join(" / ");
}

function shareSaleLabel(share: ShareView, t: TFn) {
  if (share.forSale === "Free") return t("dashboard.free");
  if (share.forSale === "Yes") return t("dashboard.forSale");
  return t("dashboard.no");
}

function ClientIdentityCell({ client, shareCount, t }: { client: DashboardClient; shareCount: number; t: TFn }) {
  const url = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  const ownerEmail = clientOwnerEmail(client);
  return (
    <div className="grid min-w-72 gap-1">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {url ? (
          <a
            href={url}
            target="_blank"
            rel="noopener noreferrer"
            data-no-row-drawer
            className="inline-flex min-w-0 max-w-full items-center gap-1 font-mono text-xs font-semibold text-foreground underline-offset-4 hover:underline"
            title={url}
          >
            <span className="truncate">{url}</span>
            <ExternalLink className="h-3 w-3 shrink-0 text-muted-foreground" />
          </a>
        ) : (
          <strong className="font-mono text-xs text-muted-foreground">-</strong>
        )}
        <Chip size="sm" variant="tertiary">{t("dashboard.sharesCount", { count: shareCount })}</Chip>
      </div>
      <span className="truncate text-xs text-muted-foreground" title={ownerEmail}>
        {ownerEmail}
      </span>
    </div>
  );
}

function ClientStatusCell({ client, t, locale }: { client: DashboardClient; t: TFn; locale: AppLocale }) {
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
  const region = client.installation.countryCode || client.installation.region || "-";
  const onlineMinutes = client.onlineMinutes24h || 0;
  const onlineRate = client.onlineRate24h || 0;
  const sinceRegistered = formatAgeDaysOrHours(client.installation.createdAt, locale);
  const onlineTitle = `${onlineRate.toFixed(1)}% online in last 24h (${onlineMinutes} / 1440 min) · registered ${client.installation.createdAt || "--"}`;
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className={rowClass}>
        <span className="mono-label text-muted-foreground">{t("dashboard.region")}</span>
        <strong className="whitespace-nowrap">{region}</strong>
      </div>
      <div className={rowClass}>
        <span className="mono-label text-muted-foreground">{locale.startsWith("zh") ? "版本" : "Version"}</span>
        <strong className="truncate" title={clientPlatformLabel(client)}>
          {clientPlatformLabel(client)}
        </strong>
      </div>
      <div className={rowClass}>
        <span className="mono-label text-muted-foreground">{t("dashboard.online")}</span>
        <strong title={onlineTitle}>
          {onlineRate.toFixed(1)}% / {sinceRegistered}
        </strong>
      </div>
      <div className={rowClass}>
        <span className="mono-label text-muted-foreground">{t("dashboard.health")}</span>
        <HealthDots entries={client.healthChecks || []} />
      </div>
    </div>
  );
}

function ClientReference({
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
    <div className="grid min-w-0 gap-1 rounded-md border border-default/40 bg-muted/20 px-2 py-1.5 text-xs">
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
      <span className="truncate text-muted-foreground" title={clientOwnerEmail(client)}>{clientOwnerEmail(client)}</span>
    </div>
  );
}

function ShareSummaryItem({
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

export function ClientsTable({ clients, shares, markets, onChanged }: { clients: DashboardClient[]; shares: ShareView[]; markets: DashboardMarket[]; onChanged?: () => Promise<void> | void }) {
  const [selected, setSelected] = React.useState<DashboardClient | null>(null);
  const [editingShare, setEditingShare] = React.useState<ShareView | null>(null);
  const { locale, t } = useLocaleText();
  const sorted = sortClients(clients);
  const selectedClientUrl = clientTunnelDisplayUrl(selected?.clientTunnel?.tunnelUrl);

  // shareId → ShareView，让"分享"列内联渲染该 installation 的 share 摘要。
  const shareById = React.useMemo(() => {
    const map = new Map<string, ShareView>();
    shares.forEach((share) => map.set(share.shareId, share));
    return map;
  }, [shares]);

  const sharesForClient = React.useCallback(
    (client: DashboardClient) =>
      (client.shareIds || [])
        .map((id) => shareById.get(id))
        .filter((s): s is ShareView => !!s),
    [shareById],
  );

  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>{t("dashboard.clients")} <span className="font-semibold text-foreground">{sorted.length}</span></div>
        <a href="https://github.com/Xiechengqi/cc-switch/releases" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-blue-400">{t("dashboard.install")}</a>
      </div>
      <Card className="overflow-hidden rounded-[20px]">
        <Card.Content className="overflow-x-auto p-0">
          <table className="w-full min-w-[960px] table-fixed border-collapse text-sm">
            <colgroup>
              <col className="w-[32%]" />
              <col className="w-[32%]" />
              <col className="w-[32%]" />
              <col className="w-[4%]" />
            </colgroup>
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="px-4 py-3">Client</th>
                <th className="px-4 py-3">{t("dashboard.status")}</th>
                <th className="px-4 py-3">{t("dashboard.shares")}</th>
                <th className="px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? sorted.map((client) => {
                const clientShares = sharesForClient(client);
                const visibleShares = clientShares.slice(0, 4);
                const hiddenShareCount = Math.max(0, clientShares.length - visibleShares.length);
                return (
                  <tr key={client.installation.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={(event) => { if (shouldOpenRowDrawer(event)) setSelected(client); }}>
                    <td className="px-4 py-3 align-top">
                      <ClientIdentityCell client={client} shareCount={clientShares.length} t={t} />
                    </td>
                    <td className="px-4 py-3 align-top">
                      <ClientStatusCell client={client} t={t} locale={locale} />
                    </td>
                    {/* P13：share 数据直接展开到行内。空列表显式提示，避免误以为 client 无 share。 */}
                    <td className="px-4 py-3 align-top">
                      {clientShares.length ? (
                        <ul className="grid gap-1.5">
                          {visibleShares.map((share) => (
                            <ShareSummaryItem key={share.shareId} share={share} onEdit={setEditingShare} t={t} compact />
                          ))}
                          {hiddenShareCount ? (
                            <li className="px-2 py-1 text-[11px] text-muted-foreground">
                              {t("dashboard.moreShares", { count: hiddenShareCount })}
                            </li>
                          ) : null}
                        </ul>
                      ) : (
                        <span className="text-xs text-muted-foreground">{t("dashboard.noShares")}</span>
                      )}
                    </td>
                    <td className="px-4 py-3 align-top text-lg text-muted-foreground">›</td>
                  </tr>
                );
              }) : (
                <tr><td colSpan={4} className="px-4 py-10 text-center text-muted-foreground">{t("dashboard.noClients")}</td></tr>
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
                    {selectedClientUrl || "-"}
                  </Drawer.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {selected?.clientTunnel?.ownerEmail || selected?.installation.ownerEmail || "-"}
                  </p>
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selected ? (
                  <div className="grid gap-5">
                    <DrawerSection label="24h">
                      <HealthTimelineStrip timeline={selected.healthTimeline || []} />
                    </DrawerSection>
                    <DrawerSection label="Client">
                      <div className="grid gap-1 text-xs text-muted-foreground">
                        <span>URL: <strong className="break-all text-foreground">{selectedClientUrl || "-"}</strong></span>
                        <span>Owner: <strong className="text-foreground">{selected.clientTunnel?.ownerEmail || selected.installation.ownerEmail || "-"}</strong></span>
                        <span>{t("dashboard.region")}: <strong className="text-foreground">{selected.installation.countryCode || "-"}</strong></span>
                        <span>{locale.startsWith("zh") ? "版本" : "Version"}: <strong className="text-foreground">{clientPlatformLabel(selected)}</strong></span>
                        <span>{t("dashboard.online")}: <strong className="text-foreground">{(selected.onlineRate24h || 0).toFixed(1)}% / {formatAgeDaysOrHours(selected.installation.createdAt, locale)}</strong></span>
                      </div>
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.linkedShares")}>
                      <ClientLinkedSharesPanel shares={sharesForClient(selected)} onEdit={setEditingShare} t={t} />
                    </DrawerSection>
                    {/* P14：完整的 provider 列表上移到 client 抽屉；share 抽屉只看 share 自己绑的那部分。 */}
                    <DrawerSection label={t("dashboard.providers")}>
                      <ClientProvidersPanel shares={sharesForClient(selected)} />
                    </DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
        </Drawer.Backdrop>
      </Drawer>
      <ShareEditDialog share={editingShare} markets={markets} onClose={() => setEditingShare(null)} onSaved={async () => { await onChanged?.(); }} />
    </section>
  );
}

/**
 * P7：share 维度表。每行对应一个 share；installation 维度信息（region / platform）
 * 通过 clients[].shareIds 反查得到。ClientsTable 退化为"机器维度"，share 详情统一在这里看。
 */
export function SharesTable({
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
  const [selected, setSelected] = React.useState<ShareView | null>(null);
  const [editingShare, setEditingShare] = React.useState<ShareView | null>(null);
  const [connectShare, setConnectShare] = React.useState<ShareView | null>(null);
  // 让 ShareConnectDialog 的 props 在每次 dashboard 5s 轮询期间保持引用稳定，
  // 配合 React.memo 阻断不必要的 Modal 重渲染；否则用户在弹窗里点复制时会看到
  // 轮询正好触发的卡片闪一下，误以为是复制按钮引发的。
  const closeConnectDialog = React.useCallback((next: boolean) => {
    if (!next) setConnectShare(null);
  }, []);

  // shareId → 所属 installation 的 DashboardClient（含 region / platform）。
  const clientByShareId = React.useMemo(() => {
    const map = new Map<string, DashboardClient>();
    clients.forEach((c) => {
      (c.shareIds ?? []).forEach((id) => map.set(id, c));
    });
    return map;
  }, [clients]);

  const shareById = React.useMemo(() => {
    const map = new Map<string, ShareView>();
    shares.forEach((share) => map.set(share.shareId, share));
    return map;
  }, [shares]);

  const sharesForClient = React.useCallback(
    (client?: DashboardClient) =>
      (client?.shareIds || [])
        .map((id) => shareById.get(id))
        .filter((s): s is ShareView => !!s),
    [shareById],
  );

  // 排序：按 createdAt 升序（先注册的 share 在前）。canManage / shareStatus /
  // activeRequests 都是会动态翻转的字段，作为主排序键会导致行经常上下跳动。
  const sorted = React.useMemo(() => {
    return [...shares].sort((left, right) => {
      return (
        (Date.parse(left.createdAt) || 0) - (Date.parse(right.createdAt) || 0) ||
        shareApiUrlKey(left).localeCompare(shareApiUrlKey(right), undefined, {
          sensitivity: "base",
        })
      );
    });
  }, [shares]);

  const selectedApi = shareApiParts(selected ?? undefined);

  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>
          {t("dashboard.shares")}{" "}
          <span className="font-semibold text-foreground">{sorted.length}</span>
        </div>
      </div>
      <Card className="overflow-hidden rounded-[20px]">
        <Card.Content className="overflow-x-auto p-0">
          <table className="w-full min-w-[720px] table-fixed border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-1/3 px-4 py-3">{t("dashboard.share")}</th>
                {/* P16：FOR SALE 列并入 STATUS 列首行（只保留 Yes/No/Free 摘要）。 */}
                <th className="w-1/3 px-4 py-3">{t("dashboard.status")}</th>
                <th className="w-1/3 px-4 py-3">{t("dashboard.support")}</th>
                <th className="w-7 px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? (
                sorted.map((share) => {
                  const api = shareApiParts(share);
                  const client = clientByShareId.get(share.shareId);
                  return (
                    <tr
                      key={share.shareId}
                      className="cursor-pointer border-b last:border-0 hover:bg-primary/5"
                      onClick={(event) => {
                        if (shouldOpenRowDrawer(event)) setSelected(share);
                      }}
                    >
                      <td className="break-words px-4 py-3 align-middle">
                        <div className="grid min-w-0 gap-1">
                          <strong className="break-all font-mono text-xs text-foreground">
                            {api.apiUrl}
                          </strong>
                          <span className="break-all text-xs text-muted-foreground">
                            {share.ownerEmail || "-"}
                          </span>
                          <ClientReference client={client} t={t} locale={locale} shareCount={client ? sharesForClient(client).length : 0} />
                          <div className="mt-1 flex flex-wrap items-center gap-2">
                            <ShareConnectChip share={share} onOpen={setConnectShare} t={t} />
                            <ShareStatusBadge share={share} t={t} />
                            <ShareEditAction share={share} onEdit={setEditingShare} t={t} />
                          </div>
                        </div>
                      </td>
                      <td className="px-4 py-3 align-middle">
                        <ShareStatusCell share={share} t={t} locale={locale} />
                      </td>
                      <td className="px-4 py-3 align-middle">
                        <SupportCell share={share} t={t} locale={locale} />
                      </td>
                      <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                    </tr>
                  );
                })
              ) : (
                <tr>
                  <td colSpan={5} className="px-4 py-10 text-center text-muted-foreground">
                    {t("dashboard.noShares")}
                  </td>
                </tr>
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
                    {selectedApi.apiUrl}
                  </Drawer.Heading>
                  <p className="mt-1 break-all text-sm text-muted-foreground">
                    {selected?.ownerEmail || "-"}
                  </p>
                  {selected?.description ? (
                    <p className="mt-2 whitespace-pre-wrap break-words text-xs leading-5 text-muted-foreground">
                      {selected.description}
                    </p>
                  ) : null}
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selected ? (
                  <div className="grid gap-5">
                    <DrawerSection label="24h">
                      <HealthTimelineStrip timeline={selected.healthTimeline} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.client")}>
                      <ShareClientPanel
                        client={clientByShareId.get(selected.shareId)}
                        currentShare={selected}
                        shares={sharesForClient(clientByShareId.get(selected.shareId))}
                        onEdit={setEditingShare}
                        t={t}
                        locale={locale}
                      />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.markets")}>
                      <ShareMarkets share={selected} t={t} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.providers")}>
                      <ShareProvidersPanel share={selected} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.requestLogs")}>
                      <ShareRequestLogs logs={selected.recentRequests || []} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.modelHealthChecks")}>
                      <ShareModelHealthChecks
                        checks={selected.recentModelHealthChecks || []}
                      />
                    </DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
        </Drawer.Backdrop>
      </Drawer>
      <ShareEditDialog
        share={editingShare}
        markets={markets}
        onClose={() => setEditingShare(null)}
        onSaved={async () => {
          await onChanged?.();
        }}
      />
      <ShareConnectDialog
        share={connectShare}
        open={!!connectShare}
        onOpenChange={closeConnectDialog}
      />
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

function MarketEditAction({ market, onEdit, t }: { market: DashboardMarket; onEdit: (market: DashboardMarket) => void; t: TFn }) {
  if (!market.canManage || isShareMarket(market)) return null;
  return (
    <button
      type="button"
      onClick={(event) => {
        event.stopPropagation();
        onEdit(market);
      }}
      className="inline-flex h-[22px] items-center gap-1 rounded-full border border-primary/20 bg-primary/10 px-2.5 text-[11px] font-medium text-primary transition-colors hover:border-primary/30 hover:bg-primary/15"
    >
      <Pencil className="h-3 w-3" />
      {t("common.edit")}
    </button>
  );
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

function MarketTypeChip({ market, t }: { market: DashboardMarket; t: TFn }) {
  return (
    <Chip
      size="sm"
      variant="soft"
      title={marketKindDescription(market, t)}
    >
      {marketKindLabel(market, t)}
    </Chip>
  );
}

function MarketIdentityCell({
  market,
  onEdit,
  t,
}: {
  market: DashboardMarket;
  onEdit: (market: DashboardMarket) => void;
  t: TFn;
}) {
  return (
    <div className="grid min-w-72 gap-1.5">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <strong className="min-w-0 truncate font-medium" title={market.displayName || market.id}>
          {market.displayName || market.id}
        </strong>
        <MarketTypeChip market={market} t={t} />
        <StatusBadge active={market.online} label={marketStatusLabel(market, t)} />
        {market.maintenanceEnabled ? (
          <Chip color="warning" size="sm" variant="soft">
            {t("dashboard.maintenance")}
          </Chip>
        ) : null}
        <MarketEditAction market={market} onEdit={onEdit} t={t} />
      </div>
      <span className="break-all text-xs text-muted-foreground">{market.email}</span>
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
        {market.publicBaseUrl ? (
          <a
            href={market.publicBaseUrl}
            target="_blank"
            rel="noreferrer"
            onClick={(event) => event.stopPropagation()}
            className="inline-flex min-w-0 max-w-full items-center gap-1 font-mono text-foreground underline-offset-4 hover:underline"
            title={market.publicBaseUrl}
          >
            <span className="truncate">{market.publicBaseUrl}</span>
            <ExternalLink className="h-3 w-3 shrink-0" />
          </a>
        ) : (
          <span className="font-mono">-</span>
        )}
        {market.subdomain ? (
          <span className="font-mono" title={market.subdomain}>
            {market.subdomain}
          </span>
        ) : null}
      </div>
    </div>
  );
}

function MarketStatusCell({ market, t, locale }: { market: DashboardMarket; t: TFn; locale: AppLocale }) {
  const ageValue = formatAgeDaysOrHours(market.createdAt, locale);
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
  if (isShareMarket(market)) {
    return (
      <div className="grid min-w-52 gap-2 text-sm">
        <div className={rowClass}>
          <span className="mono-label text-muted-foreground">{t("dashboard.online")}</span>
          <strong>{market.online ? t("common.online") : t("common.offline")}</strong>
        </div>
        <div className={rowClass}>
          <span className="mono-label text-muted-foreground">{t("dashboard.lastSeen")}</span>
          <strong title={formatDateTime(market.lastSeenAt)}>
            {formatRelativeTime(market.lastSeenAt, locale)}
          </strong>
        </div>
        {!market.online && market.offlineSince ? (
          <div className={rowClass}>
            <span className="mono-label text-muted-foreground">{t("dashboard.offlineSince")}</span>
            <strong title={formatDateTime(market.offlineSince)}>
              {formatRelativeTime(market.offlineSince, locale)}
            </strong>
          </div>
        ) : null}
        <div className={rowClass}>
          <span className="mono-label text-muted-foreground">{t("dashboard.shares")}</span>
          <strong>{market.onlineShareCount || 0} / {market.shareCount || 0}</strong>
        </div>
      </div>
    );
  }
  const limit = isUnlimited(market.parallelCapacity) ? "∞" : String(market.parallelCapacity || 0);
  const onlineValue = market.online ? `${(market.onlineRate24h || 0).toFixed(1)}% / ${ageValue}` : ageValue;
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.shares")}</span><strong>{market.onlineShareCount || 0} / {market.shareCount || 0}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${market.onlineMinutes24h || 0} / 1440 min · ${formatDateTime(market.createdAt)}`}>{onlineValue}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{market.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><strong>{compactTokens(market.usageTokens)} / {formatUsdOneDecimal(market.usageAmountUsd)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={market.healthChecks} /></div>
    </div>
  );
}

function MarketBasicInfoPanel({ market, t, locale }: { market: DashboardMarket; t: TFn; locale: AppLocale }) {
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      <Info label={t("dashboard.marketType")} value={<MarketTypeChip market={market} t={t} />} />
      <Info label={t("dashboard.status")} value={marketStatusLabel(market, t)} />
      <Info label={t("dashboard.publicUrl")} value={market.publicBaseUrl || "-"} />
      <Info label={t("dashboard.subdomain")} value={market.subdomain || "-"} />
      <Info label={t("dashboard.lastSeen")} value={formatRelativeTime(market.lastSeenAt, locale)} />
      {!market.online && market.offlineSince ? (
        <Info label={t("dashboard.offlineSince")} value={formatRelativeTime(market.offlineSince, locale)} />
      ) : null}
    </div>
  );
}

export function MarketsTable({ markets, onChanged }: { markets: DashboardMarket[]; onChanged?: () => Promise<void> }) {
  const [selected, setSelected] = React.useState<DashboardMarket | null>(null);
  const [editingMarket, setEditingMarket] = React.useState<DashboardMarket | null>(null);
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
          <table className="w-full min-w-[760px] border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-[48%] px-4 py-3">{t("dashboard.market")}</th>
                <th className="px-4 py-3">{t("dashboard.status")}</th>
                <th className="w-7 px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? sorted.map((market) => (
                <tr key={market.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={(event) => { if (shouldOpenRowDrawer(event)) setSelected(market); }}>
                  <td className="break-words px-4 py-3 align-middle">
                    <MarketIdentityCell market={market} onEdit={setEditingMarket} t={t} />
                  </td>
                  <td className="px-4 py-3 align-middle"><MarketStatusCell market={market} t={t} locale={locale} /></td>
                  <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                </tr>
              )) : (
                <tr><td colSpan={3} className="px-4 py-10 text-center text-muted-foreground">{t("dashboard.noMarkets")}</td></tr>
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
                    <DrawerSection label={t("dashboard.details")}>
                      <MarketBasicInfoPanel market={selected} t={t} locale={locale} />
                    </DrawerSection>
                    {isUsageMarket(selected) ? (
                      <>
                        <DrawerSection label={t("dashboard.officialPrice")}>
                          <MarketPricingCell market={selected} t={t} />
                        </DrawerSection>
                        <DrawerSection label="24h"><HealthTimelineStrip timeline={selected.healthTimeline} /></DrawerSection>
                      </>
                    ) : null}
                    <DrawerSection label={canShowMarketSharePriority(selected) ? t("dashboard.sharePriority") : t("dashboard.linkedShares")}>
                      {canShowMarketSharePriority(selected) ? <MarketSharePriorityPanel market={selected} t={t} /> : <MarketLinkedShares market={selected} t={t} />}
                    </DrawerSection>
                    {isUsageMarket(selected) ? (
                      <DrawerSection label={t("dashboard.recentRequests")}><MarketRequestLogs logs={selected.recentRequests || []} /></DrawerSection>
                    ) : null}
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
        </Drawer.Backdrop>
      </Drawer>
      <MarketEditDialog market={editingMarket} onClose={() => setEditingMarket(null)} onSaved={async () => { await onChanged?.(); }} />
    </section>
  );
}

function runtimePriceLabel(share: MarketShare, key: keyof ShareAppRuntimes) {
  const value = share.appRuntimes?.[key]?.forSaleOfficialPricePercent;
  return typeof value === "number" ? `${value}%` : "-";
}

const MARKET_SHARE_APPS = [
  ["claude", "Claude"],
  ["codex", "Codex"],
  ["gemini", "Gemini"],
] as const;

type MarketShareAppKey = (typeof MARKET_SHARE_APPS)[number][0];

function marketShareAppKey(value?: string | null): MarketShareAppKey | null {
  const normalized = (value || "").trim().toLowerCase();
  return MARKET_SHARE_APPS.some(([key]) => key === normalized) ? (normalized as MarketShareAppKey) : null;
}

function marketRuntimeStateTitle(state: MarketShareRuntimeState) {
  const parts = [
    `${state.scope}/${state.kind}`,
    state.appType,
    state.modelName || state.modelId,
    state.reasonKind,
    state.reason,
    typeof state.failureCount === "number" ? `failures=${state.failureCount}` : undefined,
    state.expiresAt ? `expires ${formatDateTime(state.expiresAt)}` : undefined,
    `updated ${formatDateTime(state.updatedAt)}`,
  ].filter(Boolean);
  return parts.join(" · ");
}

function isMarketBlockedState(state: MarketShareRuntimeState) {
  return state.kind === "model_block" || state.kind === "capability_block";
}

function isMarketReleasableState(state: MarketShareRuntimeState) {
  return state.kind === "cooldown" || isMarketBlockedState(state);
}

function marketStateKindLabel(state: MarketShareRuntimeState, t: TFn) {
  if (state.kind === "cooldown") return t("dashboard.cooldown");
  if (state.kind === "model_block") return t("dashboard.modelBlock");
  if (state.kind === "capability_block") return t("dashboard.capabilityBlock");
  return state.kind.replaceAll("_", " ");
}

function marketStateTargetLabel(state: MarketShareRuntimeState) {
  return [state.appType, state.modelName || state.modelId].filter(Boolean).join(" / ") || "-";
}

function marketBlockedStatesByApp(states?: MarketShareRuntimeState[]) {
  const result = new Map<MarketShareAppKey, MarketShareRuntimeState[]>();
  for (const state of states || []) {
    if (!isMarketBlockedState(state)) continue;
    const app = marketShareAppKey(state.appType);
    if (!app) continue;
    result.set(app, [...(result.get(app) || []), state]);
  }
  return result;
}

function MarketEditDialog({ market, onClose, onSaved }: { market: DashboardMarket | null; onClose: () => void; onSaved: () => Promise<void> }) {
  const [shares, setShares] = React.useState<MarketShare[]>([]);
  const [disabledIds, setDisabledIds] = React.useState<Set<string>>(new Set());
  const [selectedIds, setSelectedIds] = React.useState<Set<string>>(new Set());
  const [maintenanceEnabled, setMaintenanceEnabled] = React.useState(false);
  const [maintenanceMessage, setMaintenanceMessage] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [releasingKey, setReleasingKey] = React.useState<string | null>(null);
  const [error, setError] = React.useState("");
  const { t } = useLocaleText();
  const working = busy || !!releasingKey;

  const load = React.useCallback(async () => {
    if (!market) return;
    setError("");
    setMaintenanceEnabled(!!market.maintenanceEnabled);
    setMaintenanceMessage(market.maintenanceMessage || "");
    try {
      const nextShares = await getMarketLinkedShares(market.email);
      setShares(nextShares);
      setDisabledIds(new Set(nextShares.filter((share) => share.disabledByMarket).map((share) => share.shareId)));
      setSelectedIds(new Set());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [market]);

  React.useEffect(() => {
    load().catch(console.error);
  }, [load]);

  async function save(nextIds: Set<string>) {
    if (!market || working) return;
    setBusy(true);
    setError("");
    try {
      await updateMarketDisabledShares(market.email, Array.from(nextIds));
      setDisabledIds(new Set(nextIds));
      setSelectedIds(new Set());
      await load();
      await onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function saveMaintenance() {
    if (!market || working) return;
    setBusy(true);
    setError("");
    try {
      const response = await updateMarketMaintenance(market.email, {
        maintenanceEnabled,
        maintenanceMessage: maintenanceEnabled ? maintenanceMessage : null,
      });
      setMaintenanceEnabled(response.maintenanceEnabled);
      setMaintenanceMessage(response.maintenanceMessage || "");
      await onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  const releasableStates = shares.flatMap((share) =>
    (share.marketStates || [])
      .filter(isMarketReleasableState)
      .map((state) => ({ share, state })),
  );

  async function releaseState(share: MarketShare, state: MarketShareRuntimeState, key: string) {
    if (!market || working) return;
    setReleasingKey(key);
    setError("");
    try {
      await releaseMarketShareState(market.email, {
        routerId: state.routerId || share.routerId || "main",
        shareId: state.shareId || share.shareId,
        kind: state.kind,
        appType: state.appType,
        modelId: state.modelId,
      });
      await load();
      await onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setReleasingKey(null);
    }
  }

  async function releaseAllStates() {
    if (!market || working || releasableStates.length === 0) return;
    setReleasingKey("__all__");
    setError("");
    try {
      for (const { share, state } of releasableStates) {
        await releaseMarketShareState(market.email, {
          routerId: state.routerId || share.routerId || "main",
          shareId: state.shareId || share.shareId,
          kind: state.kind,
          appType: state.appType,
          modelId: state.modelId,
        });
      }
      await load();
      await onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setReleasingKey(null);
    }
  }

  const selectedCount = selectedIds.size;
  const disabledCount = disabledIds.size;
  const disableSelected = () => save(new Set([...Array.from(disabledIds), ...Array.from(selectedIds)]));
  const enableSelected = () => {
    const next = new Set(disabledIds);
    for (const shareId of selectedIds) next.delete(shareId);
    return save(next);
  };
  return (
    <Modal isOpen={!!market} onOpenChange={(open) => !open && !working && onClose()}>
      <Modal.Backdrop>
        <Modal.Container>
          <Modal.Dialog className="share-edit-surface light w-[min(1080px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("dashboard.editMarketShares")}</Modal.Heading>
              <p className="mt-1 break-all text-sm text-muted-foreground">{market?.displayName || market?.email} · {market?.subdomain}</p>
            </Modal.Header>
            <Modal.Body className="grid max-h-[72vh] gap-4 overflow-y-auto">
              {error ? <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div> : null}
              <div className="grid gap-3 sm:grid-cols-4">
                <Info label={t("dashboard.market")} value={market?.email} />
                <Info label={t("dashboard.publicUrl")} value={market?.publicBaseUrl} />
                <Info label={t("dashboard.shares")} value={`${shares.filter((share) => share.online).length} / ${shares.length}`} />
                <Info label={t("dashboard.disabled")} value={disabledCount} />
              </div>
              <Card className="rounded-lg border bg-amber-50/60 p-0 shadow-none">
                <Card.Content className="grid gap-3 p-3">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <Checkbox isSelected={maintenanceEnabled} onChange={(value: boolean) => setMaintenanceEnabled(value)} isDisabled={working}>
                      <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                      <Checkbox.Content><span className="text-sm font-medium text-slate-900">{t("dashboard.maintenanceMode")}</span></Checkbox.Content>
                    </Checkbox>
                    <Button size="sm" variant="outline" isDisabled={working} onClick={saveMaintenance}>
                      {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
                      {t("dashboard.saveMaintenanceMode")}
                    </Button>
                  </div>
                  <FieldGroup label={t("dashboard.field.maintenanceMessage")}>
                    <TextArea
                      value={maintenanceMessage}
                      onChange={(event) => setMaintenanceMessage(event.target.value.slice(0, 240))}
                      placeholder={t("dashboard.maintenancePlaceholder")}
                      disabled={working || !maintenanceEnabled}
                    />
                  </FieldGroup>
                </Card.Content>
              </Card>
              <Card className="rounded-lg border bg-white p-0 shadow-none">
                <Card.Content className="grid gap-3 p-3">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <div>
                      <div className="text-sm font-medium text-slate-900">{t("dashboard.blockList")}</div>
                      <div className="mt-1 text-xs text-muted-foreground">{t("dashboard.blockedStatesCount", { count: releasableStates.length })}</div>
                    </div>
                    <Button size="sm" variant="outline" isDisabled={working || releasableStates.length === 0} onClick={releaseAllStates}>
                      {releasingKey === "__all__" ? <Loader2 className="h-4 w-4 animate-spin" /> : <X className="h-4 w-4" />}
                      {t("dashboard.releaseAll")}
                    </Button>
                  </div>
                  <div className="overflow-x-auto rounded-lg border">
                    <table className="w-full min-w-[980px] border-collapse text-sm">
                      <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
                        <tr>
                          <th className="px-3 py-2">{t("dashboard.share")}</th>
                          <th className="px-3 py-2">{t("dashboard.type")}</th>
                          <th className="px-3 py-2">{t("dashboard.target")}</th>
                          <th className="px-3 py-2">{t("dashboard.reason")}</th>
                          <th className="px-3 py-2">{t("dashboard.expires")}</th>
                          <th className="px-3 py-2">{t("dashboard.updated")}</th>
                          <th className="w-28 px-3 py-2"></th>
                        </tr>
                      </thead>
                      <tbody>
                        {releasableStates.map(({ share, state }, index) => {
                          const key = `${state.routerId || share.routerId || "main"}:${state.shareId || share.shareId}:${state.kind}:${state.appType || ""}:${state.modelId || ""}:${index}`;
                          return (
                            <tr key={key} className="border-t">
                              <td className="px-3 py-2 align-middle">
                                <div className="font-medium">{share.subdomain || share.shareName || "-"}</div>
                                <div className="font-mono text-[11px] text-muted-foreground">{state.shareId || share.shareId}</div>
                              </td>
                              <td className="px-3 py-2 align-middle">
                                <Chip color={state.kind === "cooldown" ? "warning" : "danger"} size="sm" variant="soft">
                                  {marketStateKindLabel(state, t)}
                                </Chip>
                              </td>
                              <td className="px-3 py-2 align-middle font-mono text-xs">{marketStateTargetLabel(state)}</td>
                              <td className="max-w-[260px] px-3 py-2 align-middle">
                                <div className="truncate" title={marketRuntimeStateTitle(state)}>
                                  {[state.reasonKind, state.reason, typeof state.failureCount === "number" ? `${state.failureCount}x` : undefined].filter(Boolean).join(" · ") || "-"}
                                </div>
                              </td>
                              <td className="px-3 py-2 align-middle">{state.expiresAt ? formatDateTime(state.expiresAt) : "-"}</td>
                              <td className="px-3 py-2 align-middle">{formatDateTime(state.updatedAt)}</td>
                              <td className="px-3 py-2 text-right align-middle">
                                <Button size="sm" variant="outline" isDisabled={working} onClick={() => releaseState(share, state, key)}>
                                  {releasingKey === key ? <Loader2 className="h-4 w-4 animate-spin" /> : <X className="h-4 w-4" />}
                                  {t("dashboard.release")}
                                </Button>
                              </td>
                            </tr>
                          );
                        })}
                        {!releasableStates.length ? <tr><td colSpan={7} className="px-3 py-8 text-center text-muted-foreground">{t("dashboard.noBlockedStates")}</td></tr> : null}
                      </tbody>
                    </table>
                  </div>
                </Card.Content>
              </Card>
              <div className="flex flex-wrap items-center gap-2">
                <Button size="sm" variant="outline" isDisabled={working || selectedCount === 0} onClick={disableSelected}>
                  {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("dashboard.disableSelected")} ({selectedCount})
                </Button>
                <Button size="sm" variant="outline" isDisabled={working || selectedCount === 0} onClick={enableSelected}>
                  {t("dashboard.enableSelected")} ({selectedCount})
                </Button>
                <Button size="sm" variant="outline" isDisabled={working || disabledIds.size === shares.length} onClick={() => save(new Set(shares.map((share) => share.shareId)))}>
                  {t("dashboard.disableAll")}
                </Button>
                <Button size="sm" variant="outline" isDisabled={working || disabledIds.size === 0} onClick={() => save(new Set())}>
                  {t("dashboard.enableAll")}
                </Button>
              </div>
              <div className="overflow-x-auto rounded-lg border">
                <table className="w-full min-w-[980px] border-collapse text-sm">
                  <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
                    <tr>
                      <th className="w-16 px-3 py-2">{t("dashboard.disabled")}</th>
                      <th className="px-3 py-2">Share</th>
                      <th className="px-3 py-2">Owner</th>
                      <th className="px-3 py-2">Agents</th>
                      <th className="px-3 py-2">Price</th>
                      <th className="px-3 py-2">Status</th>
                      <th className="px-3 py-2">Parallel</th>
                    </tr>
                  </thead>
                  <tbody>
                    {shares.map((share) => {
                      const selected = selectedIds.has(share.shareId);
                      const disabled = disabledIds.has(share.shareId);
                      const nextSelected = new Set(selectedIds);
                      if (selected) nextSelected.delete(share.shareId); else nextSelected.add(share.shareId);
                      const supported = [
                        ["claude", "Claude"],
                        ["codex", "Codex"],
                        ["gemini", "Gemini"],
                      ].filter(([key]) => share.support?.[key as keyof typeof share.support]);
                      return (
                        <tr key={share.shareId} className="border-t">
                          <td className="px-3 py-2 align-middle">
                            <Checkbox isSelected={selected} onChange={() => setSelectedIds(nextSelected)} isDisabled={working}>
                              <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                            </Checkbox>
                          </td>
                          <td className="px-3 py-2 align-middle">
                            <div className="font-medium">{share.subdomain || share.shareName || "-"}</div>
                            <div className="font-mono text-[11px] text-muted-foreground">{share.shareId}</div>
                          </td>
                          <td className="px-3 py-2 align-middle">{share.ownerEmail || share.installationOwnerEmail || "-"}</td>
                          <td className="px-3 py-2 align-middle">
                            <div className="flex flex-wrap gap-1">{supported.map(([, label]) => <Chip key={label} size="sm" variant="tertiary">{label}</Chip>)}</div>
                          </td>
                          <td className="px-3 py-2 align-middle font-mono text-xs">
                            Claude {runtimePriceLabel(share, "claude")} · Codex {runtimePriceLabel(share, "codex")} · Gemini {runtimePriceLabel(share, "gemini")}
                          </td>
                          <td className="px-3 py-2 align-middle">
                            <div className="flex flex-wrap gap-1">
                              <Chip color={share.online ? "success" : "default"} size="sm" variant={share.online ? "soft" : "tertiary"}>{share.online ? t("common.online") : t("common.offline")}</Chip>
                              {disabled ? <Chip color="warning" size="sm" variant="soft">{t("dashboard.disabled")}</Chip> : null}
                            </div>
                          </td>
                          <td className="px-3 py-2 align-middle">{share.activeRequests || 0}/{isUnlimited(share.parallelLimit) ? "∞" : share.parallelLimit}</td>
                        </tr>
                      );
                    })}
                    {!shares.length ? <tr><td colSpan={7} className="px-3 py-10 text-center text-muted-foreground">{t("dashboard.noLinkedShares")}</td></tr> : null}
                  </tbody>
                </table>
              </div>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="outline" onClick={onClose} isDisabled={working}>{t("common.close")}</Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
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

function ClientLinkedSharesPanel({ shares, onEdit, t }: { shares: ShareView[]; onEdit: (share: ShareView) => void; t: TFn }) {
  if (!shares.length) return <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock>;
  return (
    <ul className="grid gap-2">
      {shares.map((share) => (
        <ShareSummaryItem key={share.shareId} share={share} onEdit={onEdit} t={t} />
      ))}
    </ul>
  );
}

function ShareClientPanel({
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

function ShareMarkets({ share, t }: { share?: ShareView; t: TFn }) {
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

const PROVIDER_APP_TABS: Array<{ key: keyof ShareAppProviders; label: string }> = [
  { key: "claude", label: "Claude" },
  { key: "codex", label: "Codex" },
  { key: "gemini", label: "Gemini" },
];

function providerRuntime(provider: ShareAppProvider): ShareUpstreamProvider {
  return {
    providerName: provider.name,
    kind: provider.kind,
    app: provider.app,
    accountEmail: provider.accountEmail,
    forSaleOfficialPricePercent: provider.forSaleOfficialPricePercent,
    apiUrl: provider.apiUrl,
    quota: provider.quota,
    models: provider.models,
  };
}

function providerMetaLabel(provider: ShareAppProvider) {
  return [provider.kind, provider.providerType].filter(Boolean).join(" · ");
}

function ProviderCard({
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

function ShareProvidersPanel({ share }: { share?: ShareView }) {
  const { locale, t } = useLocaleText();
  const [selectedKey, setSelectedKey] = React.useState<keyof ShareAppProviders>("claude");
  const providers = share?.appProviders;
  const runtimes = share?.appRuntimes;
  React.useEffect(() => {
    const firstBound = PROVIDER_APP_TABS.find((tab) => boundProviderIdForApp(share, tab.key));
    if (firstBound) setSelectedKey(firstBound.key);
  }, [share?.shareId]);
  const boundProviderId = boundProviderIdForApp(share, selectedKey);
  const currentProviders = (providers?.[selectedKey] || []).filter((provider) => provider.id === boundProviderId);

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
              <ProviderCard key={provider.id} provider={provider} runtime={runtime} t={t} locale={locale} showCurrentBadge />
            );
          })}
        </div>
      )}
      {share ? <ShareEmailUsagePanel share={share} app={selectedKey} /> : null}
    </div>
  );
}

type ShareUsagePeriod = "24h" | "1w" | "30d";
type ShareUsageViewMode = "table" | "trend";
const SHARE_USAGE_PERIODS: readonly ShareUsagePeriod[] = ["24h", "1w", "30d"];

function ShareEmailUsagePanel({
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

function ShareUsageTable({ usage, t }: { usage: ShareUsageByEmailResponse; t: TFn }) {
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

function ShareUsageTrend({ usage, t }: { usage: ShareUsageByEmailResponse; t: TFn }) {
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

function ShareUsageTrendChart({
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

function boundProviderIdForApp(share: ShareView | undefined, app: keyof ShareAppProviders) {
  return share?.bindings?.[app] || (share?.appType === app ? share.providerId : undefined);
}

/**
 * Client 侧边栏专用：跨该 installation 名下所有 share，列出全量 provider（按 app 分 tab）。
 * provider 列表是 installation 级数据（每个 share 拷贝同一份），按 (app, providerId) 去重；
 * "current" 角标在此场景没有单一答案（多个 share 各绑各的），所以隐藏。
 */
function ClientProvidersPanel({ shares }: { shares: ShareView[] }) {
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

function ShareRequestLogs({ logs }: { logs: ShareRequestLog[] }) {
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

function ShareModelHealthChecks({ checks }: { checks: ShareModelHealthCheck[] }) {
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

function TokenGrid({ log }: { log: ShareRequestLog | MarketRequestLog }) {
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

function MarketLinkedShares({ market, t }: { market: DashboardMarket; t: TFn }) {
  const shares = market.linkedShares || [];
  if (!shares.length) return <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock>;
  const availabilityTitle = (app: string, availability?: MarketAppAvailabilityEntry) => {
    if (!availability) return app;
    const parts = [
      `${app}: ${String(availability.status || "unknown")}`,
      "market request history, not client health",
      availability.reason,
      availability.requestedModel,
    ].filter(Boolean);
    return parts.join(" · ");
  };
  const appTitle = (label: string, availability: MarketAppAvailabilityEntry | undefined, blockedStates: MarketShareRuntimeState[]) => {
    const lines = [availabilityTitle(label, availability)];
    blockedStates.forEach((state) => lines.push(marketRuntimeStateTitle(state)));
    return lines.join("\n");
  };
  return (
    <div className="grid gap-2">
      {shares.map((share) => {
        const blockedByApp = marketBlockedStatesByApp(share.marketStates);
        const visibleApps = MARKET_SHARE_APPS.filter(([key]) => share.support?.[key as keyof typeof share.support] || blockedByApp.has(key));
        return (
          <Card key={share.shareId} className={`rounded-lg border p-0 shadow-none ${share.disabledByMarket ? "border-amber-200 bg-amber-50/40" : ""}`}>
            <Card.Content className="flex-row items-center justify-between gap-3 p-3">
              <div className="min-w-0">
                <div className="truncate font-medium">{share.subdomain || share.shareName || "-"}</div>
                <div className="truncate text-xs text-muted-foreground">{share.ownerEmail || "-"}</div>
              </div>
              <div className="grid justify-items-end gap-1">
                <Chip color={share.online ? "success" : "default"} size="sm" variant={share.online ? "soft" : "tertiary"}>{share.online ? t("common.online") : t("common.offline")}</Chip>
                {share.disabledByMarket ? <Chip color="warning" size="sm" variant="soft">{t("dashboard.disabled")}</Chip> : null}
                {visibleApps.length ? (
                  <div className="flex gap-1">
                    {visibleApps.map(([key, label]) => {
                      const availability = share.appAvailability?.[key as keyof typeof share.appAvailability];
                      const blockedStates = blockedByApp.get(key) || [];
                      const blocked = blockedStates.length > 0;
                      const unavailable = availability?.status === "unavailable";
                      // P15：把 "degraded" 也单独着色（黄）。后端在 share 命中 429 /
                      // upstream error 等场景会把 appAvailability.status 设成 degraded
                      // 但又没到 unavailable 的程度；以前前端只看 "unavailable" 一档，
                      // 整段 chip 还是灰色，运维看不出 share 是限流降级中。
                      const degraded =
                        !blocked && !unavailable && availability?.status === "degraded";
                      const chipColor: "danger" | "warning" | "default" =
                        blocked || unavailable ? "danger" : degraded ? "warning" : "default";
                      const chipVariant: "soft" | "tertiary" =
                        blocked || unavailable || degraded ? "soft" : "tertiary";
                      return (
                        <Chip
                          key={label}
                          color={chipColor}
                          size="sm"
                          title={appTitle(label, availability, blockedStates)}
                          variant={chipVariant}
                        >
                          {label}
                        </Chip>
                      );
                    })}
                  </div>
                ) : null}
              </div>
            </Card.Content>
          </Card>
        );
      })}
    </div>
  );
}

type MarketSharePriorityItem = {
  share: MarketShare;
  score: number;
  schedulable: boolean;
  degraded: boolean;
  reasons: string[];
  signalTitle: string;
};

function MarketSharePriorityPanel({ market, t }: { market: DashboardMarket; t: TFn }) {
  const [activeApp, setActiveApp] = React.useState<MarketShareAppKey>("claude");
  const [shares, setShares] = React.useState<MarketShare[] | null>(null);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    let cancelled = false;
    setShares(null);
    setError("");
    getMarketLinkedShares(market.email)
      .then((nextShares) => {
        if (!cancelled) setShares(nextShares);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [market.email]);

  const ranked = React.useMemo(
    () => rankMarketSharesForApp(shares || [], activeApp, t),
    [shares, activeApp, t],
  );

  return (
    <div className="grid gap-3">
      <div className="text-xs leading-5 text-muted-foreground">{t("dashboard.sharePriorityHint")}</div>
      <div className="inline-flex w-fit gap-1 rounded-xl bg-muted p-1">
        {MARKET_SHARE_APPS.map(([key, label]) => {
          const active = activeApp === key;
          return (
            <button
              key={key}
              type="button"
              onClick={() => setActiveApp(key)}
              className={cn(
                "rounded-md px-3 py-1.5 text-sm font-semibold transition",
                active ? "bg-white text-foreground shadow-sm" : "text-muted-foreground hover:bg-white/70 hover:text-foreground",
              )}
            >
              {label}
            </button>
          );
        })}
      </div>
      {error ? <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{t("dashboard.sharePriorityLoadFailed")}: {error}</div> : null}
      {!shares && !error ? (
        <div className="flex items-center gap-2 rounded-lg border bg-muted/30 px-3 py-4 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("dashboard.sharePriorityLoading")}
        </div>
      ) : null}
      {shares && ranked.length === 0 ? <EmptyBlock>{t("dashboard.sharePriorityUnavailable")}</EmptyBlock> : null}
      {shares && ranked.length ? (
        <div className="grid gap-2">
          {ranked.map((item, index) => (
            <MarketSharePriorityCard key={item.share.shareId} item={item} rank={index + 1} t={t} />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function MarketSharePriorityCard({ item, rank, t }: { item: MarketSharePriorityItem; rank: number; t: TFn }) {
  const share = item.share;
  const statusColor = item.schedulable ? (item.degraded ? "warning" : "success") : "default";
  const statusLabel = item.schedulable
    ? item.degraded
      ? t("dashboard.sharePriorityDegraded")
      : t("dashboard.sharePrioritySchedulable")
    : item.reasons[0] || t("dashboard.sharePriorityUnavailableState");
  return (
    <Card className={cn("rounded-lg border p-0 shadow-none", !item.schedulable ? "bg-muted/30 opacity-80" : item.degraded ? "border-amber-200 bg-amber-50/40" : "")}>
      <Card.Content className="grid gap-3 p-3">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <Chip color={item.schedulable ? "success" : "default"} size="sm" variant={item.schedulable ? "soft" : "tertiary"}>
                {t("dashboard.sharePriorityRank", { rank })}
              </Chip>
              <div className="truncate font-medium">{share.subdomain || share.shareName || "-"}</div>
            </div>
            <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">{share.shareId}</div>
            <div className="mt-1 truncate text-xs text-muted-foreground">{share.ownerEmail || share.installationOwnerEmail || "-"}</div>
          </div>
          <div className="grid shrink-0 justify-items-end gap-1">
            <Chip color={statusColor} size="sm" variant={item.schedulable ? "soft" : "tertiary"}>{statusLabel}</Chip>
            <div className="font-mono text-[11px] text-muted-foreground">
              {t("dashboard.sharePriorityScore")} {item.score.toFixed(3)}
            </div>
          </div>
        </div>
        {item.reasons.length > 1 || (!item.schedulable && item.reasons.length) ? (
          <div className="flex flex-wrap gap-1">
            {item.reasons.map((reason) => <Chip key={reason} size="sm" variant="tertiary">{reason}</Chip>)}
          </div>
        ) : null}
        <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
          <span title={item.signalTitle}>{t("dashboard.sharePrioritySignals")}: {item.signalTitle}</span>
          <span className="font-mono">{share.activeRequests || 0}/{isUnlimited(share.parallelLimit) ? "∞" : share.parallelLimit}</span>
        </div>
      </Card.Content>
    </Card>
  );
}

function rankMarketSharesForApp(shares: MarketShare[], app: MarketShareAppKey, t: TFn): MarketSharePriorityItem[] {
  return shares
    .filter((share) => isShareRelevantForApp(share, app))
    .map((share) => marketSharePriorityItem(share, app, t))
    .sort((left, right) => {
      return (
        Number(right.schedulable) - Number(left.schedulable) ||
        Number(left.degraded) - Number(right.degraded) ||
        right.score - left.score ||
        (left.share.activeRequests || 0) - (right.share.activeRequests || 0) ||
        (left.share.subdomain || left.share.shareName || left.share.shareId).localeCompare(
          right.share.subdomain || right.share.shareName || right.share.shareId,
          undefined,
          { sensitivity: "base" },
        )
      );
    });
}

function isShareRelevantForApp(share: MarketShare, app: MarketShareAppKey) {
  return Boolean(
    share.support?.[app] ||
      share.appRuntimes?.[app] ||
      share.appAvailability?.[app] ||
      marketBlockedStatesByApp(share.marketStates).has(app),
  );
}

function marketSharePriorityItem(share: MarketShare, app: MarketShareAppKey, t: TFn): MarketSharePriorityItem {
  const supported = Boolean(share.support?.[app] || share.appRuntimes?.[app]);
  const blockedStates = marketBlockedStatesByApp(share.marketStates).get(app) || [];
  const cooldownStates = (share.marketStates || []).filter((state) => {
    if (state.kind !== "cooldown") return false;
    const stateApp = marketShareAppKey(state.appType);
    return !stateApp || stateApp === app;
  });
  const availability = share.appAvailability?.[app];
  const parallelFull = !isUnlimited(share.parallelLimit) && Number(share.parallelLimit || 0) > 0 && Number(share.activeRequests || 0) >= Number(share.parallelLimit || 0);
  const reasons = [
    !supported ? t("dashboard.sharePriorityUnsupported") : undefined,
    !share.online ? t("dashboard.sharePriorityOffline") : undefined,
    share.disabledByMarket ? t("dashboard.sharePriorityDisabled") : undefined,
    parallelFull ? t("dashboard.sharePriorityParallelFull") : undefined,
    cooldownStates.length ? t("dashboard.sharePriorityCooldown") : undefined,
    blockedStates.length ? t("dashboard.sharePriorityBlocked") : undefined,
    availability?.status === "unavailable" ? t("dashboard.sharePriorityUnavailableState") : undefined,
    availability?.status === "degraded" ? t("dashboard.sharePriorityDegraded") : undefined,
  ].filter(Boolean) as string[];
  const schedulable =
    supported &&
    Boolean(share.online) &&
    !share.disabledByMarket &&
    !parallelFull &&
    cooldownStates.length === 0 &&
    blockedStates.length === 0 &&
    availability?.status !== "unavailable";
  const score = defaultMarketSharePriorityScore(share);
  const signalTitle = marketShareSignalTitle(share, t);
  return {
    share,
    score,
    schedulable,
    degraded: availability?.status === "degraded",
    reasons,
    signalTitle,
  };
}

function defaultMarketSharePriorityScore(share: MarketShare) {
  const stability = signalValue(share.signals?.stability, 1);
  const quota = signalValue(share.signals?.quotaHealth, 0.5);
  const headroom = effectiveShareHeadroom(share);
  const owner = signalValue(share.signals?.ownerPenalty, 1);
  return (0.35 * stability + 0.30 * quota + 0.25 * headroom + 0.10) * owner;
}

function signalValue(value: unknown, fallback: number) {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function effectiveShareHeadroom(share: MarketShare) {
  if (isUnlimited(share.parallelLimit)) return 1;
  const limit = Number(share.parallelLimit || 0);
  if (limit <= 0) return 0;
  return Math.max(0, Math.min(1, 1 - Number(share.activeRequests || 0) / limit));
}

function marketShareSignalTitle(share: MarketShare, t: TFn) {
  const stability = signalValue(share.signals?.stability, 1);
  const quota = signalValue(share.signals?.quotaHealth, 0.5);
  const headroom = effectiveShareHeadroom(share);
  const owner = signalValue(share.signals?.ownerPenalty, 1);
  return t("dashboard.sharePrioritySignalsTitle", {
    stability: stability.toFixed(2),
    quota: quota.toFixed(2),
    headroom: headroom.toFixed(2),
    owner: owner.toFixed(2),
  });
}

function MarketRequestLogs({ logs }: { logs: MarketRequestLog[] }) {
  const { locale, t } = useLocaleText();
  if (!logs.length) return <EmptyBlock>{t("dashboard.noMarketRequests")}</EmptyBlock>;
  return (
    <div className="grid gap-2">
      {logs.slice(0, 20).map((log) => (
        <Card key={log.requestId} className="rounded-lg border p-0 shadow-none">
          <Card.Content className="gap-3 p-3">
            <div className="min-w-0">
              <div className="truncate font-medium">
                {[log.userEmail || "-", log.shareSubdomain || log.shareId || "-", requestModelRoute(log), log.statusCode || log.status || "-", log.latencyMs ? `${log.latencyMs}ms` : "", `${compactTokens(usageBucketTotalTokens(log))} tokens`, formatUsdExactTrimmed(log.usageAmountUsd)].filter(Boolean).join(" · ")}
              </div>
              <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                <span title={formatDateTime(log.createdAt)}>{formatRelativeTime(log.createdAt, locale)}</span>
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
