"use client";

import { Eye, ExternalLink, Loader2, Pencil, Save, Crown, X } from "lucide-react";
import { Button, Card, Checkbox, Chip, Drawer, Input, ListBox, Modal, ProgressBar, Select, Tabs, TextArea } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketLinkedShares, updateMarketDisabledShares, updateMarketMaintenance, updateShareSettings } from "@/lib/api";
import type { AppLocale } from "@/lib/i18n";
import type { DashboardClient, DashboardMarket, HealthCheckEntry, HealthTimelineBucket, MarketAppAvailabilityEntry, MarketRequestLog, MarketShare, ModelHealthSummary, ShareAppProvider, ShareAppProviders, ShareAppRuntimes, ShareModelHealthCheck, ShareRequestLog, ShareSettingsPatch, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { compactTokens, formatDateTime, formatNumber, formatRelativeTime } from "@/lib/utils";

function compareDesc(left: number, right: number) {
  if (left === right) return 0;
  return left > right ? -1 : 1;
}

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
  // P7 Step 2：installation 维度排序。#shares 降序优先，再按最近上线时间，
  // 让正在挂多个 share 的活跃机器排在最上。
  return [...clients].sort((left, right) => {
    return (
      (right.shareCount || 0) - (left.shareCount || 0) ||
      compareDesc(
        Date.parse(left.installation.lastSeenAt) || 0,
        Date.parse(right.installation.lastSeenAt) || 0,
      ) ||
      left.installation.id.localeCompare(right.installation.id, undefined, { sensitivity: "base" })
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
  return entries.filter((entry) => currentModels.has(modelHealthKey(entry.requestedModel)) || currentModels.has(modelHealthKey(entry.actualModel)));
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

type OAuthRuntimeKey = "kiro" | "cursor" | "antigravity" | "copilot";

const OAUTH_RUNTIME_ROWS: Array<[OAuthRuntimeKey, string]> = [
  ["kiro", "Kiro"],
  ["cursor", "Cursor"],
  ["antigravity", "Antigravity"],
  ["copilot", "Copilot"],
];

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
  let tiers = (quota.tiers || [])
    .map((tier) => ({ ...tier, label: tier.label || tier.name }))
    .filter((tier) => tier.label);
  if (runtime.app === "claude") {
    const preferredLabels = new Set(["5h", "1w"]);
    const preferredTiers = tiers.filter((tier) => preferredLabels.has(String(tier.label).toLowerCase()));
    if (preferredTiers.length) tiers = preferredTiers;
  }
  const tierText = tiers
    .map((tier) => [quotaTierLabel(tier.label, locale), `${Math.round(tier.utilization || 0)}%`, countdownStr(tier.resetsAt)].filter(Boolean).join(" "))
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

function modelHealthTone(share: ShareView, key: "claude" | "codex" | "gemini") {
  const entries = relevantModelHealthEntries(share, key);
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
    <div className="grid min-w-72 gap-1.5">
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
  const [marketAccessMode, setMarketAccessMode] = React.useState<"selected" | "all">("selected");
  const [selectedMarketEmails, setSelectedMarketEmails] = React.useState<string[]>([]);
  const [sharedWithEmails, setSharedWithEmails] = React.useState<string[]>([]);
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
  const transferableShareEmails = splitEmails((share?.sharedWithEmails || []).join("\n"));

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
    const initialMode = (share.marketAccessMode as "selected" | "all") || "selected";
    setMarketAccessMode(initialMode);
    setSelectedMarketEmails(
      initialMode === "selected"
        ? (share.marketLinks || []).map((link) => (link.email || "").toLowerCase()).filter(Boolean)
        : [],
    );
    setSharedWithEmails(splitEmails((share.sharedWithEmails || []).join("\n")));

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
  }, [share, t]);

  const handleForSaleChange = (next: "Yes" | "No" | "Free") => {
    if (next === "Free" && forSale !== "Free") {
      setConfirmFreeOpen(true);
      return;
    }
    setForSale(next);
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
    return markets
      .filter((market) => market.email && !blocked.has(market.email.toLowerCase()))
      .sort((a, b) => (a.displayName || a.email).localeCompare(b.displayName || b.email));
  }, [markets, selectedMarketEmails]);

  const descriptionLength = description.trim().length;
  const descriptionInvalid = descriptionLength > 200;

  const tokenParsed = Number.parseInt(tokenLimitInput, 10);
  const tokenInvalid = !tokenLimitUnlimited && (!Number.isFinite(tokenParsed) || tokenParsed <= 0);

  const parallelParsed = Number.parseInt(parallelLimitInput, 10);
  const parallelInvalid =
    !parallelLimitUnlimited && (!Number.isFinite(parallelParsed) || parallelParsed < MIN_PARALLEL_LIMIT);

  const expiryInvalid = !expiresPermanent && !expiresAtInput.trim();

  const pricingPayload = React.useMemo<Record<string, number>>(() => {
    const result: Record<string, number> = {};
    for (const app of PRICE_APPS) {
      if (!share?.support?.[app.key]) continue;
      const raw = priceInputs[app.key];
      if (!raw || !raw.trim()) continue;
      const value = Number.parseInt(raw, 10);
      if (Number.isFinite(value) && value >= 1 && value <= 100) result[app.key] = value;
    }
    return result;
  }, [priceInputs, share]);

  const pricingInvalid = React.useMemo(() => {
    const check = (raw: string) => {
      if (!raw || !raw.trim()) return false;
      const value = Number.parseInt(raw, 10);
      return !(Number.isFinite(value) && value >= 1 && value <= 100);
    };
    return PRICE_APPS.some((app) => check(priceInputs[app.key]));
  }, [priceInputs]);

  const formInvalid =
    descriptionInvalid || tokenInvalid || parallelInvalid || expiryInvalid || pricingInvalid;

  const save = async () => {
    if (!share || readOnly || busy || formInvalid) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const expiresIso = expiresPermanent
        ? PERMANENT_EXPIRES_AT_ISO
        : fromLocalDateTimeValue(expiresAtInput);
      const patch: ShareSettingsPatch = {
        description: description.trim() || null,
        forSale,
        marketAccessMode,
        sharedWithEmails: sharedWithEmails,
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
      const shared = sharedWithEmails;
      const nextShared = Array.from(new Set([
        ...shared.filter((email) => email !== targetEmail),
        share.ownerEmail || "",
      ].filter(Boolean))).sort();
      const res = await updateShareSettings(share.shareId, {
        ownerEmail: targetEmail,
        sharedWithEmails: nextShared,
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

                <div className="grid gap-3 sm:grid-cols-2">
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

                  <FieldGroup label={t("dashboard.field.marketAccess")} hint={forSale === "Yes" ? undefined : t("dashboard.hint.forSaleOnly")}>
                    <Select
                      key={marketSelectKey}
	                      selectedKey={null}
	                      onSelectionChange={(key) => onMarketPicked(String(key || ""))}
	                      isDisabled={readOnly || forSale !== "Yes"}
	                    >
                      <Select.Trigger>
                        <Select.Value>
                          {marketAccessMode === "all" ? t("dashboard.allMarkets") : "Add a market..."}
                        </Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                        <ListBox>
                          <ListBox.Item id="__all__">{t("dashboard.allMarkets")}</ListBox.Item>
                          {availableMarkets.map((market) => (
                            <ListBox.Item key={market.email} id={market.email}>
                              {(market.displayName || market.subdomain || market.email)}
                              <span className="ml-1 text-muted-foreground">· {market.email}</span>
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                  </FieldGroup>
                </div>

                {forSale === "Yes" && marketAccessMode === "selected" ? (
                  <FieldGroup label={t("dashboard.field.selectedMarkets")} hint={t("dashboard.hint.selectedMarkets")}>
                    {selectedMarketEmails.length ? (
                      <div className="flex flex-wrap gap-1.5">
                        {selectedMarketEmails.map((email) => {
                          const meta = markets.find((market) => (market.email || "").toLowerCase() === email);
                          const label = meta?.displayName || meta?.subdomain || email;
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

                {forSale === "Yes" && marketAccessMode === "all" ? (
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
	                  <EmailTagsField
	                    value={sharedWithEmails}
	                    placeholder="friend@example.com, teammate@example.com"
                      disabled={readOnly}
	                    onChange={setSharedWithEmails}
	                    onPromote={(email) => setTransferTargetEmail(email)}
	                    promotableEmails={transferableShareEmails}
	                    promoteLabel={t("dashboard.setAsOwner")}
	                  />
                </FieldGroup>

                <div className="grid gap-3 sm:grid-cols-2">
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
                </div>

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

                <FieldGroup
                  label={t("dashboard.field.modelPricing")}
                  hint={t("dashboard.hint.modelPricing")}
                  invalid={pricingInvalid}
                >
                  <div className="grid gap-3">
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
                  </div>
                </FieldGroup>
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

function ShareStatusCell({ client, share, t, locale }: { client: DashboardClient; share?: ShareView; t: TFn; locale: AppLocale }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const limit = isUnlimited(share.parallelLimit) ? "∞" : String(share.parallelLimit || 0);
  const averageLatency = averageRecentLatencyMs(share.recentRequests);
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
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.platform")}</span><strong className="whitespace-nowrap">{formatPlatformVersion(client.installation.platform, client.installation.appVersion)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><div><strong>{compactTokens(share.tokensUsed)} / {isUnlimited(share.tokenLimit) ? "∞" : compactTokens(share.tokenLimit)}</strong><UsageBar used={share.tokensUsed} limit={share.tokenLimit} t={t} /></div></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.expires")}</span><strong title={`${formatDateTime(share.createdAt)} / ${expiryTitle(share.expiresAt)}`}>{shareExpiryProgress(share, locale)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{share.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.response")}</span><strong>{formatLatencySeconds(averageLatency)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${share.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(share.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={share.healthChecks} /></div>
    </div>
  );
}

export function ClientsTable({ clients, shares, markets, onChanged }: { clients: DashboardClient[]; shares: ShareView[]; markets: DashboardMarket[]; onChanged?: () => Promise<void> | void }) {
  const [selected, setSelected] = React.useState<DashboardClient | null>(null);
  const [editingShare, setEditingShare] = React.useState<ShareView | null>(null);
  const { locale, t } = useLocaleText();
  const sorted = sortClients(clients);

  // shareId → ShareView，供 drawer 反查该 installation 的 share 摘要。
  const shareById = React.useMemo(() => {
    const map = new Map<string, ShareView>();
    shares.forEach((share) => map.set(share.shareId, share));
    return map;
  }, [shares]);

  const selectedShares = React.useMemo(() => {
    if (!selected) return [] as ShareView[];
    return (selected.shareIds || [])
      .map((id) => shareById.get(id))
      .filter((s): s is ShareView => !!s);
  }, [selected, shareById]);

  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>{t("dashboard.clients")} <span className="font-semibold text-foreground">{sorted.length}</span></div>
        <a href="https://github.com/Xiechengqi/cc-switch/releases" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-blue-400">{t("dashboard.install")}</a>
      </div>
      <Card className="overflow-hidden rounded-[20px]">
        <Card.Content className="overflow-x-auto p-0">
          <table className="w-full min-w-[960px] border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-80 px-4 py-3">{t("dashboard.installation")}</th>
                <th className="px-4 py-3">{t("dashboard.platform")}</th>
                <th className="px-4 py-3">{t("dashboard.region")}</th>
                <th className="px-4 py-3">{t("dashboard.lastSeen")}</th>
                <th className="px-4 py-3 text-right">{t("dashboard.shareCount")}</th>
                <th className="w-7 px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? sorted.map((client) => {
                const shareCount = client.shareCount ?? client.shareIds?.length ?? 0;
                return (
                  <tr key={client.installation.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={(event) => { if (shouldOpenRowDrawer(event)) setSelected(client); }}>
                    <td className="w-80 break-all px-4 py-3 align-middle font-mono text-xs text-foreground">
                      {client.installation.id}
                    </td>
                    <td className="px-4 py-3 align-middle text-xs text-muted-foreground">
                      {clientPlatformLabel(client)}
                    </td>
                    <td className="px-4 py-3 align-middle text-muted-foreground">
                      {client.installation.countryCode || "-"}
                    </td>
                    <td className="px-4 py-3 align-middle text-xs text-muted-foreground" title={formatDateTime(client.installation.lastSeenAt)}>
                      {formatRelativeTime(client.installation.lastSeenAt, locale)}
                    </td>
                    <td className="px-4 py-3 align-middle text-right">
                      <span className="font-semibold text-foreground">{shareCount}</span>
                    </td>
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
                    {selected?.installation.id}
                  </Drawer.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {selected ? clientPlatformLabel(selected) : ""}
                    {selected?.installation.countryCode ? ` · ${selected.installation.countryCode}` : ""}
                  </p>
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selected ? (
                  <div className="grid gap-5">
                    <DrawerSection label={t("dashboard.installation")}>
                      <div className="grid gap-1 text-xs text-muted-foreground">
                        <span>{t("dashboard.lastSeen")}: <strong className="text-foreground">{formatDateTime(selected.installation.lastSeenAt)}</strong></span>
                        <span>{t("dashboard.region")}: <strong className="text-foreground">{selected.installation.countryCode || "-"}</strong></span>
                        <span className="break-all">id: {selected.installation.id}</span>
                      </div>
                    </DrawerSection>
                    {/* P7 Step 2：抽屉里列出该 installation 的所有 share。深度详情仍走 SharesTable 自己的 drawer。 */}
                    <DrawerSection label={`${t("dashboard.shares")} (${selectedShares.length})`}>
                      {selectedShares.length ? (
                        <ul className="grid gap-2">
                          {selectedShares.map((share) => {
                            const api = shareApiParts(share);
                            return (
                              <li key={share.shareId} className="rounded-md border border-default p-2">
                                <div className="flex flex-wrap items-center justify-between gap-2">
                                  <strong className="break-all font-mono text-xs text-foreground">{api.apiUrl}</strong>
                                  <ShareStatusBadge share={share} t={t} />
                                </div>
                                <div className="mt-1 text-xs text-muted-foreground">
                                  <span className="break-all">{share.ownerEmail || "-"}</span>
                                  {share.appType ? <span className="ml-2 uppercase">{share.appType}</span> : null}
                                </div>
                                <div className="mt-1 flex items-center gap-2">
                                  <ShareEditAction share={share} onEdit={setEditingShare} t={t} />
                                </div>
                              </li>
                            );
                          })}
                        </ul>
                      ) : (
                        <span className="text-xs text-muted-foreground">{t("dashboard.noShares")}</span>
                      )}
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

  // shareId → 所属 installation 的 DashboardClient（含 region / platform）。
  const clientByShareId = React.useMemo(() => {
    const map = new Map<string, DashboardClient>();
    clients.forEach((c) => {
      (c.shareIds ?? []).forEach((id) => map.set(id, c));
    });
    return map;
  }, [clients]);

  // 排序：管理权限 > active 状态 > 活跃请求数 > shareName。
  const sorted = React.useMemo(() => {
    return [...shares].sort((left, right) => {
      const lOwned = left.canManage ? 1 : 0;
      const rOwned = right.canManage ? 1 : 0;
      if (lOwned !== rOwned) return rOwned - lOwned;
      const lActive = left.shareStatus === "active" ? 1 : 0;
      const rActive = right.shareStatus === "active" ? 1 : 0;
      if (lActive !== rActive) return rActive - lActive;
      if (left.activeRequests !== right.activeRequests)
        return (right.activeRequests || 0) - (left.activeRequests || 0);
      return shareApiUrlKey(left).localeCompare(
        shareApiUrlKey(right),
        undefined,
        { sensitivity: "base" },
      );
    });
  }, [shares]);

  const selectedApi = shareApiParts(selected ?? undefined);
  const selectedClient = selected ? clientByShareId.get(selected.shareId) : undefined;

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
          <table className="w-full min-w-[1180px] border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-80 px-4 py-3">{t("dashboard.share")}</th>
                <th className="px-4 py-3">{t("dashboard.appType")}</th>
                <th className="px-4 py-3">{t("dashboard.forSale")}</th>
                <th className="px-4 py-3">{t("dashboard.region")}</th>
                <th className="px-4 py-3">{t("dashboard.status")}</th>
                <th className="px-4 py-3">{t("dashboard.support")}</th>
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
                      <td className="w-80 break-words px-4 py-3 align-middle">
                        <div className="grid min-w-72 gap-1">
                          <strong className="break-all font-mono text-xs text-foreground">
                            {api.apiUrl}
                          </strong>
                          <span className="break-all text-xs text-muted-foreground">
                            {share.ownerEmail || "-"}
                          </span>
                          <div className="mt-1 flex flex-wrap items-center gap-2">
                            <ShareStatusBadge share={share} t={t} />
                            <ShareEditAction share={share} onEdit={setEditingShare} t={t} />
                          </div>
                        </div>
                      </td>
                      <td className="px-4 py-3 align-middle text-xs uppercase text-muted-foreground">
                        {/* P9: 多 app share — 优先列出实际绑定的 app slot；空时回退老 appType。 */}
                        {(() => {
                          const apps = share.bindings
                            ? Object.keys(share.bindings).filter(
                                (k) => share.bindings?.[k],
                              )
                            : [];
                          if (apps.length > 0) {
                            return apps.sort().join(" · ");
                          }
                          return share.appType || "-";
                        })()}
                      </td>
                      <td className="px-4 py-3 align-middle">
                        <ForSaleCell share={share} t={t} />
                      </td>
                      <td className="px-4 py-3 align-middle text-muted-foreground">
                        {client?.installation.countryCode || "-"}
                      </td>
                      <td className="px-4 py-3 align-middle">
                        {client ? (
                          <ShareStatusCell client={client} share={share} t={t} locale={locale} />
                        ) : (
                          <span className="text-muted-foreground">-</span>
                        )}
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
                  <td colSpan={7} className="px-4 py-10 text-center text-muted-foreground">
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
                    {selectedClient ? (
                      <DrawerSection label={t("dashboard.installation")}>
                        <div className="grid gap-1 text-xs text-muted-foreground">
                          <span>
                            {t("dashboard.platform")}:{" "}
                            <strong className="text-foreground">
                              {selectedClient.installation.platform}
                            </strong>
                          </span>
                          <span>
                            {t("dashboard.region")}:{" "}
                            <strong className="text-foreground">
                              {selectedClient.installation.countryCode || "-"}
                            </strong>
                          </span>
                          <span className="break-all">
                            id: {selectedClient.installation.id}
                          </span>
                        </div>
                      </DrawerSection>
                    ) : null}
                    <DrawerSection label={t("dashboard.markets")}>
                      <ShareMarkets share={selected} t={t} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.providers")}>
                      <ShareProvidersPanel share={selected} />
                    </DrawerSection>
                    {/* P9: 多 app share 的每槽 provider 绑定快照。client 端事实源；
                        ShareProvidersPanel 展示的是 runtime/health 视角，与本节互补。 */}
                    {selected.bindings && Object.keys(selected.bindings).length > 0 ? (
                      <DrawerSection label={t("dashboard.providerBindings")}>
                        <ul className="grid gap-1 text-xs">
                          {Object.entries(selected.bindings)
                            .sort(([a], [b]) => a.localeCompare(b))
                            .map(([app, providerId]) => (
                              <li
                                key={app}
                                className="flex items-center gap-2 rounded border border-default/40 px-2 py-1"
                              >
                                <span className="uppercase font-mono text-[10px] text-muted-foreground">
                                  {app}
                                </span>
                                <span className="font-mono text-foreground break-all">
                                  {providerId}
                                </span>
                              </li>
                            ))}
                        </ul>
                      </DrawerSection>
                    ) : null}
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
  if (!market.canManage) return null;
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

function MarketStatusCell({ market, t, locale }: { market: DashboardMarket; t: TFn; locale: AppLocale }) {
  const limit = isUnlimited(market.parallelCapacity) ? "∞" : String(market.parallelCapacity || 0);
  const ageValue = formatAgeDaysOrHours(market.createdAt, locale);
  const onlineValue = market.online ? `${(market.onlineRate24h || 0).toFixed(1)}% / ${ageValue}` : ageValue;
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
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
                <tr key={market.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={(event) => { if (shouldOpenRowDrawer(event)) setSelected(market); }}>
                  <td className="w-44 break-words px-4 py-3 align-middle">
                    <div className="min-w-0">
                      <div className="font-medium">{market.displayName || market.id}</div>
                      <div className="text-xs text-muted-foreground">{market.email}</div>
                      <div className="mt-1 flex flex-wrap items-center gap-2">
                        <StatusBadge active={market.online} label={marketStatusLabel(market, t)} />
                        {market.maintenanceEnabled ? <Chip color="warning" size="sm" variant="soft">{t("dashboard.maintenance")}</Chip> : null}
                        <MarketEditAction market={market} onEdit={setEditingMarket} t={t} />
                      </div>
                    </div>
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
                    <DrawerSection label="24h"><HealthTimelineStrip timeline={selected.healthTimeline} /></DrawerSection>
                    <DrawerSection label={t("dashboard.linkedShares")}><MarketLinkedShares market={selected} t={t} locale={locale} /></DrawerSection>
                    <DrawerSection label={t("dashboard.recentRequests")}><MarketRequestLogs logs={selected.recentRequests || []} /></DrawerSection>
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

function MarketEditDialog({ market, onClose, onSaved }: { market: DashboardMarket | null; onClose: () => void; onSaved: () => Promise<void> }) {
  const [shares, setShares] = React.useState<MarketShare[]>([]);
  const [disabledIds, setDisabledIds] = React.useState<Set<string>>(new Set());
  const [selectedIds, setSelectedIds] = React.useState<Set<string>>(new Set());
  const [maintenanceEnabled, setMaintenanceEnabled] = React.useState(false);
  const [maintenanceMessage, setMaintenanceMessage] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const { t } = useLocaleText();

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
    if (!market || busy) return;
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
    if (!market || busy) return;
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

  const selectedCount = selectedIds.size;
  const disabledCount = disabledIds.size;
  const disableSelected = () => save(new Set([...Array.from(disabledIds), ...Array.from(selectedIds)]));
  const enableSelected = () => {
    const next = new Set(disabledIds);
    for (const shareId of selectedIds) next.delete(shareId);
    return save(next);
  };
  return (
    <Modal isOpen={!!market} onOpenChange={(open) => !open && !busy && onClose()}>
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
                    <Checkbox isSelected={maintenanceEnabled} onChange={(value: boolean) => setMaintenanceEnabled(value)} isDisabled={busy}>
                      <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                      <Checkbox.Content><span className="text-sm font-medium text-slate-900">{t("dashboard.maintenanceMode")}</span></Checkbox.Content>
                    </Checkbox>
                    <Button size="sm" variant="outline" isDisabled={busy} onClick={saveMaintenance}>
                      {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
                      {t("dashboard.saveMaintenanceMode")}
                    </Button>
                  </div>
                  <FieldGroup label={t("dashboard.field.maintenanceMessage")}>
                    <TextArea
                      value={maintenanceMessage}
                      onChange={(event) => setMaintenanceMessage(event.target.value.slice(0, 240))}
                      placeholder={t("dashboard.maintenancePlaceholder")}
                      disabled={busy || !maintenanceEnabled}
                    />
                  </FieldGroup>
                </Card.Content>
              </Card>
              <div className="flex flex-wrap items-center gap-2">
                <Button size="sm" variant="outline" isDisabled={busy || selectedCount === 0} onClick={disableSelected}>
                  {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("dashboard.disableSelected")} ({selectedCount})
                </Button>
                <Button size="sm" variant="outline" isDisabled={busy || selectedCount === 0} onClick={enableSelected}>
                  {t("dashboard.enableSelected")} ({selectedCount})
                </Button>
                <Button size="sm" variant="outline" isDisabled={busy || disabledIds.size === shares.length} onClick={() => save(new Set(shares.map((share) => share.shareId)))}>
                  {t("dashboard.disableAll")}
                </Button>
                <Button size="sm" variant="outline" isDisabled={busy || disabledIds.size === 0} onClick={() => save(new Set())}>
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
                            <Checkbox isSelected={selected} onChange={() => setSelectedIds(nextSelected)} isDisabled={busy}>
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
              <Button variant="outline" onClick={onClose} isDisabled={busy}>{t("common.close")}</Button>
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

function ShareProvidersPanel({ share }: { share?: ShareView }) {
  const { locale, t } = useLocaleText();
  const [selectedKey, setSelectedKey] = React.useState<keyof ShareAppProviders>("claude");
  const providers = share?.appProviders;
  const currentProviders = providers?.[selectedKey] || [];

  return (
    <div className="grid gap-3">
      <Tabs selectedKey={selectedKey} onSelectionChange={(key: React.Key) => setSelectedKey(String(key) as keyof ShareAppProviders)} variant="secondary" className="text-foreground">
        <Tabs.List className="grid w-full grid-cols-3 text-foreground">
          {PROVIDER_APP_TABS.map((tab) => (
            <Tabs.Tab key={tab.key} id={tab.key} className="px-2 text-xs text-muted-foreground data-[selected=true]:text-foreground">
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
            const runtime = mergeStandaloneOAuthRuntime(providerRuntime(provider), share?.appRuntimes, provider);
            const endpoint = runtimeEndpointSummary(runtime);
            const meta = providerMetaLabel(provider);
            const accountLevel = providerAccountLevel(runtime, locale);
            const accountIdentity = providerAccountIdentity(runtime);
            const modelMap = providerModelMap(runtime);
            return (
              <div key={provider.id} className="rounded-lg border bg-background p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-semibold">{provider.name || provider.id}</div>
                    <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">{provider.id}</div>
                  </div>
                  <div className="flex shrink-0 flex-wrap justify-end gap-1">
                    {provider.isCurrent ? <Chip color="success" size="sm" variant="soft">{t("dashboard.current")}</Chip> : null}
                    {provider.isCurrent ? <Chip color={provider.enabled ? "success" : "default"} size="sm" variant="soft">{provider.enabled ? t("dashboard.on") : t("dashboard.off")}</Chip> : null}
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

function MarketLinkedShares({ market, t, locale }: { market: DashboardMarket; t: TFn; locale: AppLocale }) {
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
  return (
    <div className="grid gap-2">
      {shares.map((share) => {
        const supported = [
          ["claude", "Claude"],
          ["codex", "Codex"],
          ["gemini", "Gemini"],
        ].filter(([key]) => share.support?.[key as keyof typeof share.support]);
        const oauthSupported = OAUTH_RUNTIME_ROWS.filter(([key]) => share.appRuntimes?.[key]);
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
                {supported.length ? (
                  <div className="flex gap-1">
                    {supported.map(([key, label]) => {
                      const availability = share.appAvailability?.[key as keyof typeof share.appAvailability];
                      const unavailable = availability?.status === "unavailable";
                      return (
                        <Chip
                          key={label}
                          color={unavailable ? "danger" : "default"}
                          size="sm"
                          title={availabilityTitle(label, availability)}
                          variant={unavailable ? "soft" : "tertiary"}
                        >
                          {label}
                        </Chip>
                      );
                    })}
                  </div>
                ) : null}
                {oauthSupported.length ? (
                  <div className="flex flex-wrap justify-end gap-1">
                    {oauthSupported.map(([key, label]) => {
                      const runtime = share.appRuntimes?.[key];
                      const level = providerAccountLevel(runtime, locale);
                      return (
                        <Chip
                          key={key}
                          color="default"
                          size="sm"
                          title={[label, level, providerAccountIdentity(runtime)].filter(Boolean).join(" · ")}
                          variant="tertiary"
                        >
                          {level && level !== "-" ? `${label} ${level}` : label}
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
                {[log.userEmail || "-", log.shareSubdomain || log.shareId || "-", requestModelRoute(log), log.statusCode || log.status || "-", log.latencyMs ? `${log.latencyMs}ms` : "", `${compactTokens(totalTokens(log))} tokens`, formatUsdExactTrimmed(log.usageAmountUsd)].filter(Boolean).join(" · ")}
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
