"use client";

import { ExternalLink, Loader2, Pencil, Save, X } from "lucide-react";
import { Button, Card, Checkbox, Chip, Drawer, Input, ListBox, Modal, ProgressBar, Select, TextArea } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketLinkedShares, updateMarketDisabledShares, updateShareSettings } from "@/lib/api";
import type { AppLocale } from "@/lib/i18n";
import type { DashboardClient, DashboardMarket, HealthCheckEntry, MarketRequestLog, MarketShare, ShareAppRuntimes, ShareRequestLog, ShareSettingsPatch, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { compactTokens, formatDateTime, formatNumber, formatRelativeTime } from "@/lib/utils";

function compareDesc(left: number, right: number) {
  if (left === right) return 0;
  return left > right ? -1 : 1;
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

function shareApiKeyLabel(share?: ShareView, apiKey?: string) {
  if (!share) return "***";
  return share.canViewSecret ? apiKey || "***" : maskSecret(apiKey);
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
  return `${platformLabel}/${versionLabel}`;
}

function sortClients(clients: DashboardClient[]) {
  return [...clients].sort((left, right) => {
    const l = left.share;
    const r = right.share;
    return (
      Number(!!r?.canManage) - Number(!!l?.canManage) ||
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

function ShareEditAction({ share, onEdit, t: _t }: { share?: ShareView; onEdit: (share: ShareView) => void; t: TFn }) {
  if (!share?.canManage) return null;
  if (share.activeEdit?.status === "pending") {
    return <Chip size="sm" color="warning" variant="soft">Pending apply</Chip>;
  }
  const handle = (event: React.MouseEvent) => {
    event.stopPropagation();
    onEdit(share);
  };
  if (share.activeEdit?.status === "rejected") {
    return (
      <button
        type="button"
        onClick={handle}
        title={share.activeEdit.errorMessage || "上一轮应用失败"}
        className="inline-flex h-[22px] items-center gap-1 rounded-full border border-red-200 bg-red-50 px-2.5 text-[11px] font-medium text-red-700 transition-colors hover:border-red-300 hover:bg-red-100"
      >
        <Pencil className="h-3 w-3" />
        应用失败
      </button>
    );
  }
  return (
    <button
      type="button"
      onClick={handle}
      className="inline-flex h-[22px] items-center gap-1 rounded-full border border-primary/20 bg-primary/10 px-2.5 text-[11px] font-medium text-primary transition-colors hover:border-primary/30 hover:bg-primary/15"
    >
      <Pencil className="h-3 w-3" />
      编辑
    </button>
  );
}

function splitEmails(value: string) {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);
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
  const [sharedWithEmails, setSharedWithEmails] = React.useState("");
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
  const [confirmFreeOpen, setConfirmFreeOpen] = React.useState(false);
  const [marketSelectKey, setMarketSelectKey] = React.useState(0);

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
    setSharedWithEmails((share.sharedWithEmails || []).join("\n"));

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
    setError(share.activeEdit?.status === "rejected" ? share.activeEdit.errorMessage || "上一轮应用失败" : "");
    setConfirmFreeOpen(false);
    setMarketSelectKey((current) => current + 1);
  }, [share]);

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
    if (!share || busy || formInvalid) return;
    setBusy(true);
    setError("");
    try {
      const expiresIso = expiresPermanent
        ? PERMANENT_EXPIRES_AT_ISO
        : fromLocalDateTimeValue(expiresAtInput);
      const patch: ShareSettingsPatch = {
        description: description.trim() || null,
        forSale,
        marketAccessMode,
        sharedWithEmails: splitEmails(sharedWithEmails),
        tokenLimit: tokenLimitUnlimited ? UNLIMITED_TOKEN_LIMIT : tokenParsed,
        parallelLimit: parallelLimitUnlimited ? UNLIMITED_PARALLEL_LIMIT : parallelParsed,
      };
      if (expiresIso) patch.expiresAt = expiresIso;
      patch.forSaleOfficialPricePercentByApp = pricingPayload;
      await updateShareSettings(share.shareId, patch);
      await onSaved();
      onClose();
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
                <Modal.Heading>编辑 share 设置</Modal.Heading>
                <p className="mt-1 break-all text-sm text-muted-foreground">{share?.subdomain || share?.shareName}</p>
              </Modal.Header>
              <Modal.Body className="grid max-h-[72vh] gap-4 overflow-y-auto">
                {error ? (
                  <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div>
                ) : null}

                <FieldGroup
                  label="Description"
                  hint={<span>最多 200 字。<span className="ml-2 font-mono">{descriptionLength}/200</span></span>}
                  invalid={descriptionInvalid}
                >
                  <TextArea
                    value={description}
                    maxLength={200}
                    onChange={(event) => setDescription(event.target.value)}
                  />
                </FieldGroup>

                <div className="grid gap-3 sm:grid-cols-2">
                  <FieldGroup label="For sale">
                    <Select
                      selectedKey={forSale}
                      onSelectionChange={(key) => handleForSaleChange(String(key || "No") as "Yes" | "No" | "Free")}
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

                  <FieldGroup label="Market access" hint={forSale === "Yes" ? undefined : "仅 ForSale = Yes 时生效"}>
                    <Select
                      key={marketSelectKey}
                      selectedKey={null}
                      onSelectionChange={(key) => onMarketPicked(String(key || ""))}
                      isDisabled={forSale !== "Yes"}
                    >
                      <Select.Trigger>
                        <Select.Value>
                          {marketAccessMode === "all" ? "All markets" : "Add a market…"}
                        </Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                        <ListBox>
                          <ListBox.Item id="__all__">All markets</ListBox.Item>
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
                  <FieldGroup label="Selected markets" hint="点 × 移除；空 = 不授权任何 market">
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
                              <button
                                type="button"
                                aria-label={`remove ${email}`}
                                className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-primary/15 transition-colors hover:bg-primary/25"
                                onClick={() => removeMarketEmail(email)}
                              >
                                <X className="h-3 w-3" />
                              </button>
                            </span>
                          );
                        })}
                      </div>
                    ) : (
                      <div className="rounded-lg border border-dashed border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
                        默认不授权任何 market
                      </div>
                    )}
                  </FieldGroup>
                ) : null}

                {forSale === "Yes" && marketAccessMode === "all" ? (
                  <div className="rounded-lg border border-primary/20 bg-primary/5 px-3 py-2 text-xs text-primary">
                    已选择「All markets」— 所有在线 market 都可访问此 share
                    <button
                      type="button"
                      className="ml-3 text-[11px] underline decoration-dotted underline-offset-2 hover:text-primary/80"
                      onClick={() => {
                        setMarketAccessMode("selected");
                        setSelectedMarketEmails([]);
                      }}
                    >
                      切回 selected
                    </button>
                  </div>
                ) : null}

                <FieldGroup label="Shared with" hint="多个邮箱用换行/逗号分隔。这些邮箱登录 dashboard 后可看到此 share 的 API Key 明文。">
                  <TextArea
                    value={sharedWithEmails}
                    placeholder="friend@example.com, teammate@example.com"
                    onChange={(event) => setSharedWithEmails(event.target.value)}
                  />
                </FieldGroup>

                <div className="grid gap-3 sm:grid-cols-2">
                  <FieldGroup label="Token limit" invalid={tokenInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="number"
                        min={1}
                        step={1}
                        value={tokenLimitInput}
                        disabled={tokenLimitUnlimited}
                        onChange={(event) => {
                          setTokenLimitInput(event.target.value);
                          const parsed = Number.parseInt(event.target.value, 10);
                          if (Number.isFinite(parsed) && parsed > 0) setLastFiniteTokenLimit(parsed);
                        }}
                      />
                      <Checkbox
                        isSelected={tokenLimitUnlimited}
                        onChange={(value: boolean) => handleTokenUnlimited(value)}
                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">无限制</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>

                  <FieldGroup label="Parallel limit" hint={`最小 ${MIN_PARALLEL_LIMIT}`} invalid={parallelInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="number"
                        min={MIN_PARALLEL_LIMIT}
                        step={1}
                        value={parallelLimitInput}
                        disabled={parallelLimitUnlimited}
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
                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">无限制</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>
                </div>

                <FieldGroup label="Expires at" invalid={expiryInvalid}>
                  <div className="grid gap-2">
                    <Input
                      type="datetime-local"
                      value={expiresAtInput}
                      disabled={expiresPermanent}
                      onChange={(event) => setExpiresAtInput(event.target.value)}
                    />
                    <Checkbox
                      isSelected={expiresPermanent}
                      onChange={(value: boolean) => setExpiresPermanent(value)}
                    >
                      <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                      <Checkbox.Content><span className="text-xs text-muted-foreground">永久（不过期）</span></Checkbox.Content>
                    </Checkbox>
                  </div>
                </FieldGroup>

                <FieldGroup
                  label="Model pricing (% of official)"
                  hint="Share 级默认定价；供应商配置了百分比时优先使用供应商值。留空则使用 market 默认定价；范围 1-100"
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
                              disabled={!supported}
                              placeholder={supported ? "未设置" : "无当前节点"}
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
                <Button variant="outline" onClick={onClose} isDisabled={busy}>取消</Button>
                <Button variant="primary" onClick={save} isDisabled={busy || formInvalid}>
                  {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
                  保存
                </Button>
              </Modal.Footer>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>

      <ConfirmAlertDialog
        open={confirmFreeOpen}
        title="确认切换为 Free"
        description="Free share 会向所有市场免费曝光，且不再产生收益。请确认切换。"
        confirmLabel="确认切换"
        cancelLabel="取消"
        tone="danger"
        onConfirm={() => {
          setForSale("Free");
          setConfirmFreeOpen(false);
        }}
        onOpenChange={(open) => !open && setConfirmFreeOpen(false)}
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
  return (
    <div className="grid gap-1.5 text-sm">
      <span className="mono-label text-muted-foreground">{label}</span>
      {children}
      {hint || invalid ? (
        <span className={`text-xs ${invalid ? "text-red-600" : "text-muted-foreground"}`}>
          {invalid ? "请检查该字段" : null}
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
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.platform")}</span><strong>{formatPlatformVersion(client.installation.platform, client.installation.appVersion)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><div><strong>{compactTokens(share.tokensUsed)} / {isUnlimited(share.tokenLimit) ? "∞" : compactTokens(share.tokenLimit)}</strong><UsageBar used={share.tokensUsed} limit={share.tokenLimit} t={t} /></div></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.expires")}</span><strong title={`${formatDateTime(share.createdAt)} / ${expiryTitle(share.expiresAt)}`}>{shareExpiryProgress(share, locale)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{share.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.response")}</span><strong>{formatLatencySeconds(averageLatency)}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${share.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(share.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={share.healthChecks} /></div>
    </div>
  );
}

export function ClientsTable({ clients, markets, onChanged }: { clients: DashboardClient[]; markets: DashboardMarket[]; onChanged?: () => Promise<void> | void }) {
  const [selected, setSelected] = React.useState<DashboardClient | null>(null);
  const [editingShare, setEditingShare] = React.useState<ShareView | null>(null);
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
	                        <strong className="break-all font-mono text-xs text-foreground">{share ? `${api.apiUrl}/${shareApiKeyLabel(share, api.apiKey)}` : "-"}</strong>
	                        <span className="break-all text-xs text-muted-foreground">{share?.ownerEmail || "-"}</span>
	                        <div className="mt-1 flex flex-wrap items-center gap-2">
	                          <ShareStatusBadge share={share} t={t} />
                          <ShareEditAction share={share} onEdit={setEditingShare} t={t} />
                        </div>
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
	                    {selected?.share ? `${selectedShareApi.apiUrl}/${shareApiKeyLabel(selected.share, selectedShareApi.apiKey)}` : selected?.installation.id}
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
      <ShareEditDialog share={editingShare} markets={markets} onClose={() => setEditingShare(null)} onSaved={async () => { await onChanged?.(); }} />
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

function MarketEditAction({ market, onEdit }: { market: DashboardMarket; onEdit: (market: DashboardMarket) => void }) {
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
      编辑
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
                <tr key={market.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={() => setSelected(market)}>
                  <td className="w-44 break-words px-4 py-3 align-middle">
                    <div className="min-w-0">
                      <div className="font-medium">{market.displayName || market.id}</div>
                      <div className="text-xs text-muted-foreground">{market.email}</div>
                      <div className="mt-1 flex flex-wrap items-center gap-2">
                        <StatusBadge active={market.online} label={marketStatusLabel(market, t)} />
                        <MarketEditAction market={market} onEdit={setEditingMarket} />
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
                    <DrawerSection label={t("dashboard.linkedShares")}><MarketLinkedShares market={selected} t={t} /></DrawerSection>
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
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const { t } = useLocaleText();

  const load = React.useCallback(async () => {
    if (!market) return;
    setError("");
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
          <Card key={share.shareId} className={`rounded-lg border p-0 shadow-none ${share.disabledByMarket ? "border-amber-200 bg-amber-50/40" : ""}`}>
            <Card.Content className="flex-row items-center justify-between gap-3 p-3">
              <div className="min-w-0">
                <div className="truncate font-medium">{share.subdomain || share.shareName || "-"}</div>
                <div className="truncate text-xs text-muted-foreground">{share.ownerEmail || "-"}</div>
              </div>
              <div className="grid justify-items-end gap-1">
                <Chip color={share.online ? "success" : "default"} size="sm" variant={share.online ? "soft" : "tertiary"}>{share.online ? t("common.online") : t("common.offline")}</Chip>
                {share.disabledByMarket ? <Chip color="warning" size="sm" variant="soft">{t("dashboard.disabled")}</Chip> : null}
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
                {[log.userEmail || "-", log.shareSubdomain || log.shareId || "-", requestModelRoute(log), log.statusCode || log.status || "-", log.latencyMs ? `${log.latencyMs}ms` : "", `${compactTokens(totalTokens(log))} tokens`, formatUsdExactTrimmed(log.usageAmountUsd)].filter(Boolean).join(" · ")}
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
