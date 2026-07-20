"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import type { DashboardClient, DashboardMarket, HealthCheckEntry, MarketRequestLog, ModelHealthSummary, ShareAppProvider, ShareAppRuntimes, ShareRequestLog, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { compactTokens, formatDateTime } from "@/lib/utils";

export function modelHealthCheckedAt(entry: Pick<ModelHealthSummary, "checkedAt" | "lastCheckedAt">) {
  return Number(entry.checkedAt ?? entry.lastCheckedAt ?? 0);
}

let rowPointerDown: { x: number; y: number } | null = null;

export function onRowPointerDown(event: React.MouseEvent<HTMLElement> | React.PointerEvent<HTMLElement>) {
  rowPointerDown = { x: event.clientX, y: event.clientY };
}

export function shouldOpenRowDrawer(event: React.MouseEvent<HTMLElement>) {
  if (rowPointerDown) {
    const deltaX = Math.abs(event.clientX - rowPointerDown.x);
    const deltaY = Math.abs(event.clientY - rowPointerDown.y);
    rowPointerDown = null;
    if (deltaX > 4 || deltaY > 4) {
      return false;
    }
  }

  const selection = window.getSelection();
  if (selection && !selection.isCollapsed && selection.toString().trim()) {
    return false;
  }

  const target = event.target as HTMLElement | null;
  const interactive = target?.closest(
    "a,button,input,textarea,select,[role='button'],[data-no-row-drawer]",
  );
  if (interactive && interactive !== event.currentTarget) {
    return false;
  }

  return true;
}

export const UNLIMITED_TOKEN_LIMIT = -1;
export const UNLIMITED_PARALLEL_LIMIT = -1;
export const DEFAULT_PARALLEL_LIMIT = 3;
export const DEFAULT_TOKEN_LIMIT = 100000;
export const PERMANENT_EXPIRES_AT_ISO = "2099-12-31T23:59:59Z";
export const CORE_SHARE_APPS = [
  ["claude", "Claude"],
  ["codex", "Codex"],
  ["gemini", "Gemini"],
] as const;

export function isUnlimitedTokenLimit(value?: number | null) {
  return value === UNLIMITED_TOKEN_LIMIT;
}

export function isUnlimitedParallelLimit(value?: number | null) {
  return value === UNLIMITED_PARALLEL_LIMIT;
}

export function isPermanentExpiryDate(value?: string | null) {
  if (!value) return false;
  const date = new Date(value);
  return !Number.isNaN(date.getTime()) && date.getUTCFullYear() >= 2099;
}

export function isUnlimited(value?: number) {
  return Number(value) < 0;
}

export function parseShareTimestamp(value?: string) {
  if (!value) return Number.NaN;
  const trimmed = value.trim();
  if (!trimmed) return Number.NaN;
  const parsed = new Date(trimmed).getTime();
  if (Number.isFinite(parsed)) return parsed;
  const numeric = Number(trimmed);
  if (!Number.isFinite(numeric)) return Number.NaN;
  if (numeric > 0 && numeric < 10_000_000_000) return numeric * 1000;
  return numeric;
}

export function isUnlimitedExpiry(value?: string) {
  if (!value) return false;
  const expiresAt = parseShareTimestamp(value);
  if (!Number.isFinite(expiresAt)) return false;
  const fiftyYearsMs = 50 * 365 * 24 * 60 * 60 * 1000;
  return expiresAt - Date.now() >= fiftyYearsMs;
}

export function expiryTitle(value?: string) {
  return isUnlimitedExpiry(value) ? "∞" : formatDateTime(value);
}

export function formatDurationShort(value?: string, locale: AppLocale = "en", mode: "elapsed" | "remaining" = "elapsed") {
  if (!value) return "--";
  const ts = parseShareTimestamp(value);
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

export function shareExpiryProgress(share: ShareView, locale: AppLocale) {
  const age = formatDurationShort(share.createdAt, locale, "elapsed");
  const expiry = isUnlimitedExpiry(share.expiresAt) ? "∞" : formatDurationShort(share.expiresAt, locale, "remaining");
  return `${age}/${expiry}`;
}

export function averageRecentLatencyMs(logs?: ShareRequestLog[], limit = 10) {
  const samples = [...(logs || [])]
    .sort((left, right) => Number(right.createdAt || 0) - Number(left.createdAt || 0))
    .slice(0, limit)
    .map((log) => Number(log.latencyMs || 0))
    .filter((latency) => Number.isFinite(latency) && latency > 0);
  if (!samples.length) return null;
  return samples.reduce((sum, latency) => sum + latency, 0) / samples.length;
}

export function formatLatencySeconds(latencyMs: number | null) {
  if (latencyMs == null || !Number.isFinite(latencyMs) || latencyMs <= 0) return "-";
  const seconds = latencyMs / 1000;
  return `${seconds < 1 ? seconds.toFixed(2) : seconds.toFixed(1)}s`;
}

export function latencyResponseToneClass(latencyMs: number | null | undefined) {
  if (latencyMs == null || !Number.isFinite(latencyMs) || latencyMs <= 0) {
    return "text-foreground";
  }
  const seconds = latencyMs / 1000;
  if (seconds < 15) return "text-emerald-700";
  if (seconds < 30) return "text-amber-700";
  return "text-rose-700";
}

export function parallelOccupancyByUser(
  share: Pick<ShareView, "activeRequestsByUser">,
  app?: CoreShareApp | null,
) {
  const byApp = share.activeRequestsByUser || {};
  if (app) return byApp[app] || {};
  const merged = new Map<string, number>();
  for (const users of Object.values(byApp)) {
    for (const [email, count] of Object.entries(users || {})) {
      if (!count) continue;
      merged.set(email, (merged.get(email) || 0) + count);
    }
  }
  return Object.fromEntries(merged);
}

export function parallelOccupancyTitle(
  share: Pick<ShareView, "activeRequests" | "activeRequestsByApp" | "activeRequestsByUser">,
  app: CoreShareApp | null | undefined,
  t: (key: MessageKey, values?: Record<string, string | number>) => string,
) {
  const byUser = parallelOccupancyByUser(share, app);
  const entries = Object.entries(byUser)
    .filter(([, count]) => count > 0)
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]));
  const active = app ? share.activeRequestsByApp?.[app] ?? 0 : share.activeRequests ?? 0;
  if (!entries.length) {
    if (active > 0) return t("dashboard.parallelOccupancyUnknown");
    return t("dashboard.parallelOccupancyEmpty");
  }
  const lines = entries.map(([email, count]) => `${email}: ${count}`).join("\n");
  const accounted = entries.reduce((sum, [, count]) => sum + count, 0);
  const remainder = active - accounted;
  if (remainder > 0) {
    return `${lines}\n${t("dashboard.parallelOccupancyUnattributed", { count: remainder })}`;
  }
  return lines;
}

export function formatImageLogTimestamp(value?: number | null) {
  if (!value) return "-";
  const date = new Date(value * 1000);
  if (!Number.isFinite(date.getTime())) return "-";
  const pad = (next: number) => String(next).padStart(2, "0");
  return `${pad(date.getMonth() + 1)}${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export function formatImageLogSpendSeconds(latencyMs?: number | null) {
  if (latencyMs == null || !Number.isFinite(latencyMs) || latencyMs <= 0) return "-";
  const seconds = latencyMs / 1000;
  return `${seconds < 10 ? seconds.toFixed(2) : seconds < 100 ? seconds.toFixed(1) : Math.round(seconds)}s`;
}

export function formatImageLogSizeMb(bytes?: number | null) {
  if (bytes == null || !Number.isFinite(bytes) || bytes <= 0) return "-";
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

export function expirySortValue(share?: ShareView) {
  if (!share?.expiresAt) return 0;
  if (isUnlimitedExpiry(share.expiresAt)) return Number.POSITIVE_INFINITY;
  const value = new Date(share.expiresAt).getTime();
  return Number.isFinite(value) ? value : 0;
}

export function shareApiUrlKey(share?: ShareView) {
  return share?.subdomain || share?.shareName || "";
}

export function shareDisplayTitle(share?: Pick<ShareView, "subdomain" | "shareId">) {
  return share?.subdomain || share?.shareId || "-";
}

export function tunnelDomainHost(referenceTunnelUrl?: string | null) {
  const normalized = clientTunnelDisplayUrl(referenceTunnelUrl);
  if (normalized) {
    try {
      const { hostname, port } = new URL(normalized);
      const dot = hostname.indexOf(".");
      if (dot > 0) {
        return port ? `${hostname.slice(dot + 1)}:${port}` : hostname.slice(dot + 1);
      }
    } catch {
      // fall through to window host
    }
  }
  if (typeof window !== "undefined" && window.location.host) {
    return window.location.host;
  }
  return "";
}

export function subdomainTunnelUrl(subdomain?: string | null, referenceTunnelUrl?: string | null) {
  const sub = String(subdomain || "").trim();
  if (!sub) return "";
  const host = tunnelDomainHost(referenceTunnelUrl);
  if (!host) return "";
  return clientTunnelDisplayUrl(`${sub}.${host}`);
}

export function shareApiParts(share?: ShareView, referenceTunnelUrl?: string | null) {
  if (!share) return { apiUrl: "-" };
  const apiUrl = subdomainTunnelUrl(share.subdomain, referenceTunnelUrl) || share.subdomain || "-";
  return { apiUrl };
}

export function clientTunnelDisplayUrl(value?: string | null) {
  const trimmed = String(value || "").trim();
  if (!trimmed) return "";
  if (/^http:\/\//i.test(trimmed)) return `https://${trimmed.slice("http://".length)}`;
  if (/^https:\/\//i.test(trimmed)) return trimmed;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)) return trimmed;
  return `https://${trimmed}`;
}

export function formatUsdOneDecimal(value?: string | number) {
  const amount = Number(value || 0);
  return Number.isFinite(amount) ? `$${amount.toFixed(1)}` : "$0.0";
}

export function formatUsdExactTrimmed(value?: string | number) {
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

export function tokenCount(value?: string | number | null) {
  const count = Number(value || 0);
  return Number.isFinite(count) && count > 0 ? count : 0;
}

export function usageBucketTotalTokens(log?: Partial<ShareRequestLog | MarketRequestLog>) {
  return tokenCount(log?.inputTokens) + tokenCount(log?.outputTokens) + tokenCount(log?.cacheReadTokens) + tokenCount(log?.cacheCreationTokens);
}

export function cacheHitRate(log?: Partial<ShareRequestLog | MarketRequestLog>) {
  const input = tokenCount(log?.inputTokens);
  const cacheRead = tokenCount(log?.cacheReadTokens);
  const denominator = input + cacheRead;
  return denominator > 0 ? cacheRead / denominator : 0;
}

export function formatPercent(value: number) {
  const percent = Math.max(0, Math.min(1, Number(value) || 0)) * 100;
  return `${percent.toFixed(percent >= 10 ? 0 : 1).replace(/\.0$/, "")}%`;
}

export function formatOfficialPriceMultiplier(value: string | number | null | undefined, label: string, t: TFn) {
  if (typeof value === "string" && value.trim().toLowerCase() === "mixed") {
    return `${t("dashboard.mixed")} x ${label}`;
  }
  const percent = Number(value);
  if (!Number.isFinite(percent) || percent <= 0) return `- x ${label}`;
  const multiplier = percent / 100;
  const text = multiplier >= 1
    ? multiplier.toFixed(2)
    : multiplier.toFixed(3);
  return `${text.replace(/(\.\d*?[1-9])0+$/, "$1").replace(/\.0+$/, "")} x ${label}`;
}

export function requestModelRoute(log?: Partial<ShareRequestLog | MarketRequestLog>) {
  const record = (log || {}) as Partial<ShareRequestLog & MarketRequestLog>;
  const agent = record.requestAgent || "";
  const requested = record.requestedModel || record.requestModel || "";
  const actual = record.actualModel || record.model || "";
  return [agent, requested && actual && requested !== actual ? `${requested} -> ${actual}` : actual || requested].filter(Boolean).join(" · ") || "-";
}

export function formatShareStatus(value?: string) {
  return value ? String(value).replaceAll("_", " ") : "-";
}

export function formatClientAppVersion(version?: string) {
  const raw = String(version || "").trim();
  if (!raw) return "-";
  const normalized = raw.replace(/^v/i, "");
  const commitMatch = normalized.match(/^(?:commit\s+)?([0-9a-f]{7,40})$/i);
  if (commitMatch) {
    return commitMatch[1].toLowerCase().slice(0, 7);
  }
  const embeddedCommit = normalized.match(/\(([0-9a-f]{7,40})\)/i);
  if (embeddedCommit) {
    return embeddedCommit[1].toLowerCase().slice(0, 7);
  }
  return normalized;
}

export function clientPlatformLabel(client: DashboardClient) {
  return formatClientAppVersion(client.installation.appVersion);
}

export function sortClients(clients: DashboardClient[]) {
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

export function sortMarkets(markets: DashboardMarket[]) {
  // 同上：按注册时间升序，避免 online 抖动改变行序。
  return [...markets].sort(
    (a, b) =>
      (Date.parse(a.createdAt) || 0) - (Date.parse(b.createdAt) || 0) ||
      (a.publicBaseUrl || a.email || a.id).localeCompare(b.publicBaseUrl || b.email || b.id),
  );
}

export function isShareMarket(market: DashboardMarket) {
  return market.marketKind === "share";
}

export function isUsageMarket(market: DashboardMarket) {
  return !isShareMarket(market);
}

export function marketKindLabel(market: DashboardMarket, t: TFn) {
  return isShareMarket(market) ? t("dashboard.shareMarket") : t("dashboard.tokenMarket");
}

export function marketKindDescription(market: DashboardMarket, t: TFn) {
  return isShareMarket(market)
    ? t("dashboard.shareMarketTooltip")
    : t("dashboard.tokenMarketTooltip");
}

export function canShowMarketSharePriority(market: DashboardMarket) {
  return isUsageMarket(market);
}

export function marketLabel(market: Pick<DashboardMarket, "publicBaseUrl" | "email" | "subdomain">) {
  return market.publicBaseUrl || market.email || market.subdomain;
}

export type TFn = ReturnType<typeof useLocaleText>["t"];
export const drawerDialogClassName =
  "router-drawer-light light !w-[min(760px,calc(100vw-16px))] !max-w-[calc(100vw-16px)] !bg-white !text-slate-900 " +
  "[--foreground:rgb(var(--router-foreground))] [--muted:rgb(var(--router-muted-foreground))] [--overlay:#fff] [--overlay-foreground:rgb(var(--router-foreground))] " +
  "[--surface:#fff] [--surface-foreground:rgb(var(--router-foreground))] [--surface-secondary:rgb(var(--router-muted))] [--surface-secondary-foreground:rgb(var(--router-foreground))] " +
  "[--default:rgb(var(--router-muted))] [--default-foreground:rgb(var(--router-foreground))]";

export function HealthDots({ entries = [] }: { entries?: HealthCheckEntry[] }) {
  const dots = entries.slice(-10);
  if (!dots.length) {
    return React.createElement(
      "span",
      { className: "inline-flex gap-1" },
      Array.from({ length: 10 }).map((_, index) => React.createElement("i", { key: index, className: "h-2 w-2 rounded-full bg-slate-300" })),
    );
  }
  return React.createElement(
    "span",
    { className: "inline-flex gap-1" },
    dots.map((entry, index) =>
      React.createElement("i", {
        key: `${entry.checkedAt}-${index}`,
        className: entry.isHealthy ? "h-2 w-2 rounded-full bg-emerald-500" : "h-2 w-2 rounded-full bg-red-500",
        title: formatDateTime(entry.checkedAt * 1000),
      }),
    ),
  );
}

export function upstreamPercent(apps?: ShareAppRuntimes, key?: keyof ShareAppRuntimes) {
  const value = key ? apps?.[key]?.forSaleOfficialPricePercent : undefined;
  return Number.isInteger(value) && Number(value) > 0 ? `${value}%` : "-";
}

export function configuredUpstreamPercent(apps?: ShareAppRuntimes, key?: keyof ShareAppRuntimes) {
  const value = key ? apps?.[key]?.forSaleOfficialPricePercent : undefined;
  return Number.isInteger(value) && Number(value) > 0 ? `${value}%` : null;
}

export function isOfficialMarker(value?: string) {
  const normalized = String(value || "").trim().toLowerCase();
  return normalized === "official" || normalized === "offical";
}

const API_KEY_PROVIDER_TYPES = new Set([
  "nvidia",
  "deepseek_api",
  "openrouter",
  "ollama_cloud",
]);

const API_KEY_PROVIDER_DEFAULT_URLS: Record<string, string> = {
  nvidia: "https://integrate.api.nvidia.com/v1",
  deepseek_api: "https://api.deepseek.com",
  openrouter: "https://openrouter.ai/api",
  ollama_cloud: "https://ollama.com",
};

const MODEL_MAPPING_METADATA = new Set([
  "single",
  "mode",
  "type",
  "default",
  "available",
  "model",
]);

function normalizedProviderType(runtime?: ShareUpstreamProvider) {
  return String(runtime?.providerType || runtime?.kind || "").trim().toLowerCase();
}

function normalizedProviderName(runtime?: ShareUpstreamProvider) {
  return String(runtime?.providerName || "").trim().toLowerCase();
}

function resolvedApiKeyProviderType(runtime?: ShareUpstreamProvider) {
  const providerType = normalizedProviderType(runtime);
  if (API_KEY_PROVIDER_TYPES.has(providerType)) return providerType;
  const providerName = normalizedProviderName(runtime);
  if (providerName.includes("nvidia")) return "nvidia";
  if (providerName.includes("deepseek") && providerName.includes("api")) return "deepseek_api";
  if (providerName.includes("openrouter")) return "openrouter";
  if (providerName.includes("ollama")) return "ollama_cloud";
  const apiUrl = String(runtime?.apiUrl || "").trim().toLowerCase();
  if (apiUrl.includes("integrate.api.nvidia.com")) return "nvidia";
  if (apiUrl.includes("api.deepseek.com")) return "deepseek_api";
  if (apiUrl.includes("openrouter.ai")) return "openrouter";
  if (apiUrl.includes("ollama.com")) return "ollama_cloud";
  return "";
}

export function runtimeApiUrl(runtime?: ShareUpstreamProvider) {
  const direct = String(runtime?.apiUrl || "").trim();
  if (direct && !isOfficialMarker(direct)) return direct;
  const providerType = resolvedApiKeyProviderType(runtime);
  return API_KEY_PROVIDER_DEFAULT_URLS[providerType] || "";
}

export function hasConcreteApiUrl(runtime?: ShareUpstreamProvider) {
  const apiUrl = runtimeApiUrl(runtime);
  return Boolean(apiUrl && !isOfficialMarker(apiUrl));
}

export function runtimeLooksOAuth(runtime?: ShareUpstreamProvider) {
  const text = [
    runtime?.app,
    runtime?.kind,
    runtime?.providerName,
    runtime?.providerType,
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  return text.includes("oauth") || text.includes("ollama_cloud") || Boolean(oauthRuntimeKeyFromProvider(runtime));
}

export function isOllamaCloudRuntime(runtime?: ShareUpstreamProvider) {
  const text = [
    runtime?.providerType,
    runtime?.providerName,
    runtime?.kind,
    runtime?.app,
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  return text.includes("ollama_cloud") || text.includes("ollama cloud") || text.includes("ollama");
}

export function isOfficialRuntime(runtime?: ShareUpstreamProvider) {
  if (!runtime) return false;
  const kind = String(runtime.kind || "").toLowerCase();
  const apiUrl = runtimeApiUrl(runtime);
  const models = Array.isArray(runtime.models) ? runtime.models : [];
  const modelsMarkedOfficial = models.length > 0 && models.every((item) => isOfficialMarker(item.actualModel));
  return (kind === "official_oauth" || isOfficialMarker(kind) || isOfficialMarker(apiUrl) || modelsMarkedOfficial) && !hasConcreteApiUrl(runtime);
}

export function runtimeModelSummary(runtime?: ShareUpstreamProvider) {
  const models = Array.isArray(runtime?.models) ? runtime.models : [];
  return models
    .map((item) => `${item.slot || "model"}:${item.actualModel || ""}`)
    .filter((value) => !value.endsWith(":"))
    .join(" · ");
}

export function modelHealthKey(value?: string) {
  return String(value || "").trim().toLowerCase();
}

export function runtimeModelKeys(runtime?: ShareUpstreamProvider) {
  const models = Array.isArray(runtime?.models) ? runtime.models : [];
  return new Set(models.map((item) => modelHealthKey(item.actualModel)).filter(Boolean));
}

export function relevantModelHealthEntries(share: ShareView, key: "claude" | "codex" | "gemini") {
  const entries = share.modelHealth?.[key] || [];
  const currentModels = runtimeModelKeys(share.appRuntimes?.[key]);
  if (currentModels.size === 0) return entries;
  return entries.filter((entry) => isAppLevelQuotaBlockedModelHealth(entry, key) || currentModels.has(modelHealthKey(entry.requestedModel)) || currentModels.has(modelHealthKey(entry.actualModel)));
}

export function modelHealthFailureReason(entries: ModelHealthSummary[]) {
  const failed = entries
    .filter((entry) => entry.status === "failed" || (entry.recentResults || []).includes("failed"))
    .sort((left, right) => modelHealthCheckedAt(right) - modelHealthCheckedAt(left))[0];
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

export function isQuotaBlockedModelHealth(entry: ModelHealthSummary) {
  const message = String(entry.errorMessage || "").toLowerCase();
  return entry.status === "quota_blocked"
    || message.includes("quota exhausted")
    || message.includes("quota_exhausted")
    || message.includes("usage limit")
    || message.includes("usage_limit")
    || message.includes("weekly limit")
    || message.includes("monthly limit");
}

export function isAppLevelQuotaBlockedModelHealth(entry: ModelHealthSummary, key: "claude" | "codex" | "gemini") {
  return modelHealthKey(entry.requestedModel) === key
    && modelHealthKey(entry.actualModel) === key
    && isQuotaBlockedModelHealth(entry);
}

export function runtimeEndpointSummary(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  const apiUrl = runtimeApiUrl(runtime);
  return apiUrl && !isOfficialMarker(apiUrl) ? apiUrl : "";
}

export function isCursorApiKeyRuntime(runtime?: ShareUpstreamProvider) {
  return String(runtime?.providerType || "").trim().toLowerCase() === "cursor_apikey";
}

export function isApiProviderRuntime(runtime?: ShareUpstreamProvider) {
  if (!runtime || isCursorApiKeyRuntime(runtime) || runtimeLooksOAuth(runtime)) {
    return false;
  }
  if (resolvedApiKeyProviderType(runtime)) {
    return true;
  }
  return hasConcreteApiUrl(runtime);
}

export function providerApiEndpoint(runtime?: ShareUpstreamProvider) {
  return runtimeEndpointSummary(runtime) || "-";
}

export function countdownStr(resetsAt?: string) {
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

export function formatExpireDistance(expiresAt?: string) {
  if (!expiresAt) return "";
  const diffMs = new Date(expiresAt).getTime() - Date.now();
  if (!Number.isFinite(diffMs)) return "";
  if (diffMs <= 0) return "expired";
  const minutes = Math.max(1, Math.floor(diffMs / (1000 * 60)));
  if (minutes < 60) return `expire in ${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `expire in ${hours}h`;
  return `expire in ${Math.floor(hours / 24)}d`;
}

export function quotaTierLabel(label?: string, locale: AppLocale = "en") {
  const normalized = String(label || "").trim().toLowerCase();
  if (normalized === "cursor credits" || normalized === "cursor included usage") return "Usage";
  if (normalized === "premium") return locale.startsWith("zh") ? "高级请求" : "Premium request";
  return label || "";
}

export function normalizeCompactTierLabel(label?: string, locale: AppLocale = "en") {
  const normalized = String(label || "").trim().toLowerCase();
  if (normalized === "1w" || normalized === "weekly_limit") return "7d";
  if (normalized === "five_hour") return "5h";
  if (normalized === "seven_day") return "7d";
  if (normalized === "30_day" || normalized === "monthly") return "30d";
  if (normalized === "seven_day_opus" || normalized === "seven_day_omelette") return "7d Opus";
  if (normalized === "seven_day_sonnet") return "7d Sonnet";
  const mapped = quotaTierLabel(label, locale);
  return mapped || String(label || "").trim();
}

export function formatCompactQuotaTier(
  tier: { label?: string; name?: string; utilization?: number; resetsAt?: string; used?: number; limit?: number; unit?: string },
  locale: AppLocale = "en",
) {
  const label = normalizeCompactTierLabel(tier.label || tier.name, locale);
  const utilization =
    typeof tier.utilization === "number" && Number.isFinite(tier.utilization)
      ? `${utilizationPercentForDisplay(tier.utilization)}%`
      : "";
  const countdown = countdownStr(tier.resetsAt);
  const amount = formatQuotaUsageAmount(tier, locale);
  return [label, amount, utilization, countdown].filter(Boolean).join(" ");
}

export function quotaPlanLabel(runtime: ShareUpstreamProvider, plan?: string) {
  const normalized = String(plan || "").trim();
  if (!normalized) return "";
  if (isOllamaCloudRuntime(runtime) && !normalized.toLowerCase().includes("ollama")) {
    return `ollama ${normalized}`;
  }
  return normalized;
}

export function formatQuotaAmount(value?: number, locale: AppLocale = "en") {
  if (typeof value !== "number" || !Number.isFinite(value)) return "";
  return new Intl.NumberFormat(locale, {
    maximumFractionDigits: value % 1 === 0 ? 0 : 2,
    useGrouping: false,
  }).format(value);
}

export function formatQuotaUsageAmount(
  tier: { used?: number; limit?: number; unit?: string },
  locale: AppLocale = "en",
) {
  const used = formatQuotaAmount(tier.used, locale);
  const limit = formatQuotaAmount(tier.limit, locale);
  if (!used || !limit) return "";
  const unit = String(tier.unit || "").trim();
  if (unit.toUpperCase() === "USD") return `$${used}/$${limit}`;
  return [`${used}/${limit}`, unit].filter(Boolean).join(" ");
}

type OAuthRuntimeKey = "kiro" | "cursor" | "antigravity" | "copilot";

export function oauthRuntimeKeyFromProvider(value?: Partial<ShareUpstreamProvider & ShareAppProvider>): OAuthRuntimeKey | undefined {
  if (String(value?.providerType || "").trim().toLowerCase() === "cursor_apikey") {
    return undefined;
  }
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

export function mergeStandaloneOAuthRuntime(
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

export function shareAppProviderRuntime(provider: ShareAppProvider): ShareUpstreamProvider {
  return {
    providerName: provider.name,
    kind: provider.kind,
    app: provider.app,
    providerType: provider.providerType,
    accountEmail: provider.accountEmail,
    forSaleOfficialPricePercent: provider.forSaleOfficialPricePercent,
    apiUrl: provider.apiUrl,
    quota: provider.quota,
    models: provider.models,
  };
}

export function boundProviderIdForApp(share: ShareView | undefined, app: CoreShareApp) {
  return share?.bindings?.[app] || (share?.appType === app ? share.providerId : undefined);
}

export function resolveShareAppRuntime(share: ShareView, app: CoreShareApp) {
  const runtimes = share.appRuntimes;
  const boundProviderId = boundProviderIdForApp(share, app);
  const providers = share.appProviders?.[app] || [];
  const provider =
    providers.find((item) => item.id === boundProviderId) ||
    providers.find((item) => item.isCurrent) ||
    providers[0];
  if (provider) {
    return mergeStandaloneOAuthRuntime(shareAppProviderRuntime(provider), runtimes, provider);
  }
  const slotRuntime = runtimes?.[app];
  if (slotRuntime) {
    return mergeStandaloneOAuthRuntime(slotRuntime, runtimes);
  }
  return slotRuntime;
}

export function utilizationPercentForDisplay(value: number) {
  if (!Number.isFinite(value)) return 0;
  if (value >= 0 && value <= 1) return Math.round(value * 100);
  return Math.round(value);
}

export function quotaSummary(runtime?: ShareUpstreamProvider, locale: AppLocale = "en") {
  if (!runtime || (hasConcreteApiUrl(runtime) && !runtimeLooksOAuth(runtime))) return "";
  const quota = runtime.quota;
  const status = String(quota?.status || "").toLowerCase();
  if (!quota || (status && !["ok", "success", "valid"].includes(status))) return "";
  let tiers = (quota.tiers || [])
    .map((tier) => ({ ...tier, label: tier.label || tier.name }))
    .filter((tier) => tier.label);
  if (runtime.app === "claude") {
    const preferredLabels = new Set(["5h", "1w", "7d"]);
    const preferredTiers = tiers.filter((tier) => preferredLabels.has(String(tier.label).toLowerCase()));
    if (preferredTiers.length) tiers = preferredTiers;
  }
  const tierText = tiers
    .map((tier) => formatCompactQuotaTier(tier, locale))
    .filter(Boolean)
    .join(" · ");
  const expireText = providerSubscriptionExpiry(runtime, locale);
  return [quotaPlanLabel(runtime, quota.plan || quota.credentialMessage), expireText, tierText]
    .filter(Boolean)
    .join(" · ");
}

export function providerSubscriptionExpiry(
  runtime?: ShareUpstreamProvider,
  locale: AppLocale = "en",
): string | null {
  const subscriptionEnd = runtime?.quota?.subscriptionPeriodEnd;
  if (!subscriptionEnd) return null;
  if (isUnlimitedExpiry(subscriptionEnd)) return "∞";
  const remaining =
    countdownStr(subscriptionEnd) ||
    formatDurationShort(subscriptionEnd, locale, "remaining");
  if (!remaining || remaining === "--") return null;
  if (remaining === "expired" || remaining === "已过期") return remaining;
  return `expire in ${remaining}`;
}

export function providerAccountLevel(runtime?: ShareUpstreamProvider, locale: AppLocale = "en") {
  return quotaSummary(runtime, locale) || runtime?.providerName || runtime?.kind || "-";
}

export function providerAccountIdentity(runtime?: ShareUpstreamProvider) {
  if (isApiProviderRuntime(runtime)) {
    return "-";
  }
  if (isCursorApiKeyRuntime(runtime)) {
    const name = String(runtime?.providerName || "").trim();
    if (name) return name;
    const message = String(runtime?.quota?.credentialMessage || "").trim();
    if (message) return message;
    return "Cursor API Key";
  }
  const account = String(runtime?.accountEmail || "").trim();
  if (!account || account.startsWith("cursor_apikey_")) return "-";
  return account;
}

export function providerStatusIdentity(runtime?: ShareUpstreamProvider) {
  const identity = providerAccountIdentity(runtime);
  if (identity && identity !== "-") return identity;
  const name = String(runtime?.providerName || "").trim();
  return name || "-";
}

export function providerModelMap(runtime?: ShareUpstreamProvider) {
  return runtimeModelSummary(runtime) || "-";
}

function preferredQuotaTiers(runtime?: ShareUpstreamProvider) {
  const quota = runtime?.quota;
  if (!quota) return [];
  const status = String(quota.status || "").toLowerCase();
  if (status && !["ok", "success", "valid"].includes(status)) return [];
  let tiers = (quota.tiers || [])
    .map((tier) => ({ ...tier, label: tier.label || tier.name }))
    .filter((tier) => tier.label);
  if (runtime?.app === "claude") {
    const preferredLabels = new Set(["5h", "1w", "7d"]);
    const preferredTiers = tiers.filter((tier) => preferredLabels.has(String(tier.label).toLowerCase()));
    if (preferredTiers.length) tiers = preferredTiers;
  }
  return tiers;
}

export function providerQuotaStatusLine(runtime?: ShareUpstreamProvider, locale: AppLocale = "en") {
  if (!runtime) return "-";
  if (hasConcreteApiUrl(runtime) && !runtimeLooksOAuth(runtime)) {
    return runtime.providerName || runtime.kind || "-";
  }
  const summary = quotaSummary(runtime, locale);
  return summary || providerAccountTierLabel(runtime);
}

export function providerAccountTierLabel(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "-";
  if (isCursorApiKeyRuntime(runtime)) {
    const quota = runtime.quota;
    if (quota) {
      const status = String(quota.status || "").toLowerCase();
      if (status && !["ok", "success", "valid"].includes(status)) {
        const message = String(quota.credentialMessage || "").trim();
        if (message) return message;
      }
      const plan = String(quota.plan || quota.credentialMessage || "").trim();
      if (plan) return plan;
    }
    return runtime.providerName || "Cursor API Key";
  }
  if (hasConcreteApiUrl(runtime) && !runtimeLooksOAuth(runtime)) {
    return runtime.providerName || runtime.kind || "-";
  }
  const plan = String(runtime.quota?.plan || runtime.quota?.credentialMessage || "").trim();
  if (plan) {
    return isOllamaCloudRuntime(runtime) && !plan.toLowerCase().includes("ollama")
      ? `ollama ${plan}`
      : plan;
  }
  return runtime.providerName || runtime.kind || "-";
}

export function providerQuotaExpiry(runtime?: ShareUpstreamProvider, locale: AppLocale = "en") {
  if (!runtime?.quota) return "-";
  const tiers = preferredQuotaTiers(runtime);
  const resets = tiers
    .map((tier) => tier.resetsAt)
    .filter(Boolean)
    .sort()[0];
  if (resets) {
    return countdownStr(resets) || formatDurationShort(resets, locale, "remaining") || "-";
  }
  const subscriptionEnd = runtime.quota.subscriptionPeriodEnd;
  if (!subscriptionEnd) return "-";
  if (isUnlimitedExpiry(subscriptionEnd)) return "∞";
  const remaining = countdownStr(subscriptionEnd) || formatDurationShort(subscriptionEnd, locale, "remaining");
  return remaining === "--" ? "-" : remaining;
}

export function providerUsageData(runtime?: ShareUpstreamProvider, locale: AppLocale = "en") {
  const line = providerQuotaStatusLine(runtime, locale);
  return line === "-" ? "-" : line;
}

export function providerActualModelNames(runtime?: ShareUpstreamProvider) {
  const models = Array.isArray(runtime?.models) ? runtime.models : [];
  const names = models
    .map((item) => String(item.actualModel || "").trim())
    .filter((name) => name && !isOfficialMarker(name) && !MODEL_MAPPING_METADATA.has(name.toLowerCase()));
  const deduped = [...new Set(names)];
  if (deduped.length === 1) {
    return deduped[0];
  }
  if (deduped.length > 1) {
    const withoutMetadata = deduped.filter((name) => !MODEL_MAPPING_METADATA.has(name.toLowerCase()));
    if (withoutMetadata.length === 1) {
      return withoutMetadata[0];
    }
    return withoutMetadata.join(" · ");
  }
  return "-";
}

export function modelHealthTone(share: ShareView, key: "claude" | "codex" | "gemini") {
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

export function modelHealthTitle(share: ShareView, key: "claude" | "codex" | "gemini") {
  const entries = relevantModelHealthEntries(share, key);
  if (!entries.length) return "No failures recorded yet";
  return entries
    .map((entry) => {
      const recent = (entry.recentResults || []).join(" / ") || entry.status;
      const checked = modelHealthCheckedAt(entry) ? formatDateTime(modelHealthCheckedAt(entry) * 1000) : "-";
      const model = entry.requestedModel || entry.actualModel || "-";
      return `${model}: ${recent} · ${checked}${entry.errorMessage ? ` · ${entry.errorMessage}` : ""}`;
    })
    .join("\n");
}

export type CoreShareApp = "claude" | "codex" | "gemini";

export function shareAppSettings(share: ShareView, app: CoreShareApp) {
  const access = share.accessByApp?.[app];
  return {
    forSale: share.appSettings?.[app]?.forSale ?? share.forSale,
    saleMarketKind: share.appSettings?.[app]?.saleMarketKind ?? share.saleMarketKind ?? "token",
    marketAccessMode: share.appSettings?.[app]?.marketAccessMode ?? access?.marketAccessMode ?? share.marketAccessMode,
    sharedWithEmails: share.appSettings?.[app]?.sharedWithEmails ?? access?.sharedWithEmails ?? share.sharedWithEmails ?? [],
    tokenLimit: share.appSettings?.[app]?.tokenLimit ?? share.tokenLimit,
    parallelLimit: share.appSettings?.[app]?.parallelLimit ?? share.parallelLimit,
    expiresAt: share.appSettings?.[app]?.expiresAt || share.expiresAt,
  };
}

export function shareAppExists(share: ShareView, app: CoreShareApp) {
  return Boolean(
    share.bindings?.[app] ||
      share.support?.[app] ||
      share.appSettings?.[app] ||
      share.accessByApp?.[app] ||
      share.appRuntimes?.[app] ||
      share.modelHealth?.[app]?.length ||
      share.appType === app,
  );
}

export function requestBelongsToApp(request: ShareRequestLog, app: CoreShareApp) {
  const appType = (request.appType || "").trim().toLowerCase();
  if (appType) return appType === app;
  const agent = (request.requestAgent || "").trim().toLowerCase();
  return agent === app;
}

export function formatMinutesShort(minutes?: number, locale: AppLocale = "en") {
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

export function formatAgeDaysOrHours(value?: string, locale: AppLocale = "en") {
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

export function clientRunningDurationMs(client: DashboardClient, now = Date.now()) {
  const ts = Date.parse(client.installation.createdAt);
  if (!Number.isFinite(ts)) return 0;
  return Math.max(0, now - ts);
}

export function clientRunningDurationLabel(client: DashboardClient, locale: AppLocale = "en") {
  return formatAgeDaysOrHours(client.installation.createdAt, locale);
}

export function clientTotalTokensUsed(shares: ShareView[]) {
  return shares.reduce((sum, share) => sum + (Number(share.tokensUsed) || 0), 0);
}

export function clientTotalTokensLabel(shares: ShareView[]) {
  return compactTokens(clientTotalTokensUsed(shares));
}
