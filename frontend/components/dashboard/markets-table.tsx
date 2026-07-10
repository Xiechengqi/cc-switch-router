"use client";

import { ChevronRight, ExternalLink, Loader2, Pencil, Save, Search, SlidersHorizontal, X } from "lucide-react";
import { Button, Card, Checkbox, Chip, Drawer, Modal, Tabs, TextArea } from "@heroui/react";
import * as React from "react";
import { DrawerSection, EmptyBlock, HealthTimelineStrip, Info, TokenGrid } from "@/components/dashboard/drawer-panels";
import { FieldGroup } from "@/components/dashboard/share-edit-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketLinkedShares, getMarketSharePriority, getMarketShareSessionLoads, releaseMarketShareState, updateMarketDisabledShares, updateMarketMaintenance } from "@/lib/api";
import type { DashboardMarket, MarketAppAvailabilityEntry, MarketRequestLog, MarketShare, MarketShareRuntimeState, ShareAppRuntimes, ShareUpstreamProvider } from "@/lib/types";
import { cn, compactTokens, formatDateTime, formatRelativeTime } from "@/lib/utils";
import { canShowMarketSharePriority, drawerDialogClassName, formatOfficialPriceMultiplier, formatUsdExactTrimmed, formatUsdOneDecimal, isShareMarket, isUnlimited, isUsageMarket, marketKindDescription, marketKindLabel, requestModelRoute, shouldOpenRowDrawer, sortMarkets, usageBucketTotalTokens, type TFn } from "@/components/dashboard/share-dashboard-utils";

function marketHealthLabel(market: DashboardMarket, t: TFn) {
  const status = String(market.status || "").trim().toLowerCase();
  if (status === "disabled") return t("dashboard.disabled");
  if (market.maintenanceEnabled) return t("dashboard.maintenance");
  if (status === "offline") return t("common.offline");
  if (!market.online) return t("dashboard.routeOffline");
  if ((market.shareCount || 0) === 0) return t("dashboard.noShares");
  if ((market.shareCount || 0) > 0 && (market.onlineShareCount || 0) === 0) return t("dashboard.noOnlineShares");
  const capacity = marketCapacityPercent(market);
  if (capacity != null && capacity >= 90) return t("dashboard.capacityWarning", { percent: capacity.toFixed(0) });
  const latestHealth = market.healthChecks?.at(-1);
  if (latestHealth && !latestHealth.isHealthy) return t("dashboard.healthCheckFailed");
  return t("dashboard.healthy");
}

type MarketOperationalState = "available" | "degraded" | "offline" | "maintenance" | "disabled";

function marketCapacityPercent(market: DashboardMarket) {
  if (isUnlimited(market.parallelCapacity) || market.parallelCapacity <= 0) return null;
  return Math.max(0, Math.min(100, ((market.activeRequests || 0) / market.parallelCapacity) * 100));
}

function marketOperationalState(market: DashboardMarket): MarketOperationalState {
  const status = String(market.status || "").trim().toLowerCase();
  if (status === "disabled") return "disabled";
  if (market.maintenanceEnabled) return "maintenance";
  if (!market.online || status === "offline") return "offline";
  const latestHealth = market.healthChecks?.at(-1);
  const capacity = marketCapacityPercent(market);
  if ((market.shareCount || 0) === 0 || ((market.shareCount || 0) > 0 && (market.onlineShareCount || 0) === 0) || (capacity != null && capacity >= 90) || (latestHealth && !latestHealth.isHealthy)) return "degraded";
  return "available";
}

function marketStateRank(state: MarketOperationalState) {
  return state === "offline" ? 0 : state === "degraded" ? 1 : state === "maintenance" ? 2 : state === "available" ? 3 : 4;
}

function MarketStatusPill({ state, t }: { state: MarketOperationalState; t: TFn }) {
  const style = state === "available"
    ? "border-emerald-200 bg-emerald-50 text-emerald-700"
    : state === "degraded"
      ? "border-amber-200 bg-amber-50 text-amber-700"
      : state === "offline"
        ? "border-rose-200 bg-rose-50 text-rose-700"
        : state === "maintenance"
          ? "border-blue-200 bg-blue-50 text-blue-700"
          : "border-slate-200 bg-slate-100 text-slate-600";
  const label = state === "available"
    ? t("dashboard.available")
    : state === "degraded"
      ? t("dashboard.degraded")
      : state === "offline"
        ? t("common.offline")
        : state === "maintenance"
          ? t("dashboard.maintenance")
          : t("dashboard.disabled");
  return <span className={`inline-flex h-6 items-center gap-1.5 rounded-full border px-2.5 text-[11px] font-semibold ${style}`}><span className="h-1.5 w-1.5 rounded-full bg-current" />{label}</span>;
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
    <div className="overflow-hidden rounded-lg border border-default-200">
      <table className="w-full table-fixed text-left text-xs">
        <tbody>
          <tr>
            {entries.map(([label, value]) => (
              <td key={label as string} className="border-r border-default-200 px-2.5 py-2 font-mono text-foreground last:border-r-0">
                {formatOfficialPriceMultiplier(value, label as string, t)}
              </td>
            ))}
          </tr>
        </tbody>
      </table>
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

function MarketIdentityCell({ market, t }: { market: DashboardMarket; t: TFn }) {
  return (
    <div className="grid min-w-0 gap-1">
      <strong className="truncate text-sm font-semibold text-foreground" title={market.displayName || market.subdomain || market.id}>{market.displayName || market.subdomain || market.id}</strong>
      {market.publicBaseUrl ? (
        <a
          href={market.publicBaseUrl}
          target="_blank"
          rel="noreferrer"
          onClick={(event) => event.stopPropagation()}
          className="inline-flex min-w-0 max-w-full items-center gap-1 truncate font-mono text-[10px] text-muted-foreground underline-offset-4 hover:underline"
          title={market.publicBaseUrl}
        >
          <span className="min-w-0 truncate">{market.publicBaseUrl}</span>
          <ExternalLink className="h-3 w-3 shrink-0" />
        </a>
      ) : (
        <span className="min-w-0 truncate font-mono text-[10px] text-muted-foreground">{market.id}</span>
      )}
      <div className="flex min-w-0 items-center gap-1.5">
        <MarketTypeChip market={market} t={t} />
        <span className="min-w-0 truncate text-[10px] text-muted-foreground" title={market.email}>{market.email}</span>
      </div>
    </div>
  );
}

export function MarketsTable({ markets, onChanged }: { markets: DashboardMarket[]; onChanged?: () => Promise<void> }) {
  const [selected, setSelected] = React.useState<DashboardMarket | null>(null);
  const [editingMarket, setEditingMarket] = React.useState<DashboardMarket | null>(null);
  const [query, setQuery] = React.useState("");
  const [statusFilter, setStatusFilter] = React.useState<"all" | MarketOperationalState>("all");
  const [sortOrder, setSortOrder] = React.useState("issues");
  const [onlyIssues, setOnlyIssues] = React.useState(false);
  const { locale, t } = useLocaleText();
  const stableMarkets = React.useMemo(() => sortMarkets(markets), [markets]);
  const summary = React.useMemo(() => {
    const states = stableMarkets.map(marketOperationalState);
    return {
      available: states.filter((state) => state === "available").length,
      issues: states.filter((state) => state === "degraded" || state === "offline" || state === "maintenance").length,
      disabled: states.filter((state) => state === "disabled").length,
    };
  }, [stableMarkets]);
  const rows = React.useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    const stableOrder = new Map(stableMarkets.map((market, index) => [market.id, index]));
    const next = stableMarkets.map((market) => ({ market, state: marketOperationalState(market) })).filter(({ market, state }) => {
      if (normalizedQuery && ![
        market.id,
        market.displayName,
        market.email,
        market.subdomain,
        market.publicBaseUrl,
        market.marketKind,
      ].some((value) => String(value || "").toLocaleLowerCase().includes(normalizedQuery))) return false;
      if (statusFilter !== "all" && state !== statusFilter) return false;
      if (onlyIssues && state !== "degraded" && state !== "offline" && state !== "maintenance") return false;
      return true;
    });
    next.sort((left, right) => {
      if (sortOrder === "name") return (left.market.displayName || left.market.subdomain || left.market.id).localeCompare(right.market.displayName || right.market.subdomain || right.market.id, undefined, { sensitivity: "base" });
      if (sortOrder === "capacity") return (marketCapacityPercent(right.market) ?? -1) - (marketCapacityPercent(left.market) ?? -1);
      if (sortOrder === "activity") return (right.market.activeRequests || 0) - (left.market.activeRequests || 0);
      if (sortOrder === "shares") return (right.market.shareCount || 0) - (left.market.shareCount || 0);
      if (sortOrder === "updated") return (Date.parse(right.market.lastSeenAt) || 0) - (Date.parse(left.market.lastSeenAt) || 0);
      return marketStateRank(left.state) - marketStateRank(right.state) || (stableOrder.get(left.market.id) || 0) - (stableOrder.get(right.market.id) || 0);
    });
    return next;
  }, [onlyIssues, query, sortOrder, stableMarkets, statusFilter]);

  return (
    <section className="grid gap-3">
      <div className="grid gap-3 rounded-lg border bg-white p-3 shadow-sm">
        <div className="flex items-center justify-between gap-4">
          <div className="flex items-baseline gap-2">
            <h2 className="text-sm font-semibold text-foreground">{t("dashboard.markets")}</h2>
            <span className="text-xs text-muted-foreground">{stableMarkets.length}</span>
            <span className="text-xs text-emerald-700">{summary.available} {t("dashboard.available")}</span>
            {summary.issues ? <span className="text-xs font-medium text-rose-700">{summary.issues} {t("dashboard.issues")}</span> : null}
            {summary.disabled ? <span className="text-xs text-muted-foreground">{summary.disabled} {t("common.disabled")}</span> : null}
          </div>
          <a href="https://github.com/Xiechengqi/cc-switch-market/releases" target="_blank" rel="noopener noreferrer" className="font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground transition-colors hover:text-blue-500">{t("dashboard.install")}</a>
        </div>
        <div className="flex items-center gap-2">
          <label className="flex h-9 min-w-64 flex-1 items-center gap-2 rounded-md border bg-white px-3 text-sm focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/10">
            <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
            <input value={query} onChange={(event) => setQuery(event.target.value)} className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground" placeholder={t("dashboard.searchMarkets")} aria-label={t("dashboard.searchMarkets")} />
          </label>
          <select value={statusFilter} onChange={(event) => setStatusFilter(event.target.value as "all" | MarketOperationalState)} className="h-9 rounded-md border bg-white px-3 text-xs text-foreground outline-none focus:border-primary/50" aria-label={t("dashboard.filterStatus")}>
            <option value="all">{t("dashboard.allStatuses")}</option>
            <option value="available">{t("dashboard.available")}</option>
            <option value="degraded">{t("dashboard.degraded")}</option>
            <option value="offline">{t("common.offline")}</option>
            <option value="maintenance">{t("dashboard.maintenance")}</option>
            <option value="disabled">{t("dashboard.disabled")}</option>
          </select>
          <button type="button" onClick={() => setOnlyIssues((value) => !value)} aria-pressed={onlyIssues} className={`inline-flex h-9 items-center gap-1.5 rounded-md border px-3 text-xs font-medium transition-colors ${onlyIssues ? "border-amber-300 bg-amber-50 text-amber-800" : "bg-white text-muted-foreground hover:text-foreground"}`}>
            <SlidersHorizontal className="h-3.5 w-3.5" />{t("dashboard.onlyIssues")}
          </button>
          <select value={sortOrder} onChange={(event) => setSortOrder(event.target.value)} className="h-9 rounded-md border bg-white px-3 text-xs text-foreground outline-none focus:border-primary/50" aria-label={t("dashboard.sortBy")}>
            <option value="issues">{t("dashboard.sortIssues")}</option>
            <option value="name">{t("dashboard.sortName")}</option>
            <option value="capacity">{t("dashboard.sortCapacity")}</option>
            <option value="activity">{t("dashboard.sortActivity")}</option>
            <option value="shares">{t("dashboard.sortShares")}</option>
            <option value="updated">{t("dashboard.sortUpdated")}</option>
          </select>
        </div>
      </div>
      <Card className="overflow-hidden rounded-lg border bg-white shadow-sm">
        <Card.Content className="overflow-x-auto p-0">
          <table className="w-full min-w-[1080px] table-fixed border-collapse text-sm">
            <thead className="bg-slate-50 text-left text-[11px] font-semibold text-muted-foreground">
              <tr>
                <th className="w-[22%] px-3 py-2.5">{t("dashboard.market")}</th>
                <th className="w-[11%] px-3 py-2.5">{t("dashboard.status")}</th>
                <th className="w-[13%] px-3 py-2.5">{t("dashboard.capacity")}</th>
                <th className="w-[14%] px-3 py-2.5">{t("dashboard.activity")}</th>
                <th className="w-[10%] px-3 py-2.5">{t("dashboard.shares")}</th>
                <th className="w-[14%] px-3 py-2.5">{t("dashboard.health")}</th>
                <th className="w-[10%] px-3 py-2.5">{t("dashboard.updated")}</th>
                <th className="w-[6%] px-3 py-2.5 text-right">{t("dashboard.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.length ? rows.map(({ market, state }) => {
                const capacityPercent = marketCapacityPercent(market);
                const capacityLimit = isUnlimited(market.parallelCapacity) ? "∞" : market.parallelCapacity > 0 ? String(market.parallelCapacity) : "-";
                const usageValue = isShareMarket(market) ? compactTokens(market.usageTokens) : `${compactTokens(market.usageTokens)} · ${formatUsdOneDecimal(market.usageAmountUsd)}`;
                const rowTone = state === "offline" ? "border-l-rose-500" : state === "degraded" ? "border-l-amber-400" : state === "maintenance" ? "border-l-blue-400" : state === "disabled" ? "border-l-slate-300 opacity-70" : "border-l-transparent";
                return (
                  <tr key={market.id} tabIndex={0} className={`cursor-pointer border-b border-l-[3px] outline-none last:border-b-0 hover:bg-primary/[0.03] focus-visible:bg-primary/[0.05] focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/30 ${rowTone}`} onClick={(event) => { if (shouldOpenRowDrawer(event)) setSelected(market); }} onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); setSelected(market); } }}>
                    <td className="px-3 py-2.5 align-middle"><MarketIdentityCell market={market} t={t} /></td>
                    <td className="px-3 py-2.5 align-middle"><MarketStatusPill state={state} t={t} /></td>
                    <td className="px-3 py-2.5 align-middle">
                      <div className="grid gap-1">
                        <strong className="text-xs tabular-nums">{market.activeRequests || 0}<span className="font-normal text-muted-foreground">/{capacityLimit}</span></strong>
                        <div className="h-1.5 overflow-hidden rounded-full bg-slate-100" title={capacityPercent == null ? t("dashboard.capacityUnknown") : `${capacityPercent.toFixed(0)}%`}>
                          {capacityPercent != null ? <div className={`h-full rounded-full ${capacityPercent >= 90 ? "bg-rose-500" : capacityPercent >= 70 ? "bg-amber-400" : "bg-primary/70"}`} style={{ width: `${capacityPercent}%` }} /> : null}
                        </div>
                      </div>
                    </td>
                    <td className="px-3 py-2.5 align-middle">
                      <strong className="block text-xs tabular-nums">{market.activeRequests || 0} {t("dashboard.active")}</strong>
                      <span className="block truncate text-[10px] text-muted-foreground" title={usageValue}>{usageValue}</span>
                    </td>
                    <td className="px-3 py-2.5 align-middle">
                      <strong className="text-xs tabular-nums">{market.onlineShareCount || 0}<span className="font-normal text-muted-foreground">/{market.shareCount || 0}</span></strong>
                      <span className="block text-[10px] text-muted-foreground">{t("common.online")}</span>
                    </td>
                    <td className="px-3 py-2.5 align-middle">
                      <span className={`block truncate text-xs font-medium ${state === "offline" ? "text-rose-700" : state === "degraded" ? "text-amber-700" : "text-foreground"}`} title={marketHealthLabel(market, t)}>{marketHealthLabel(market, t)}</span>
                      <span className="block text-[10px] text-muted-foreground">{(market.onlineRate24h || 0).toFixed(1)}% · 24h</span>
                    </td>
                    <td className="px-3 py-2.5 align-middle">
                      <span className="block text-xs" title={formatDateTime(market.lastSeenAt)}>{formatRelativeTime(market.lastSeenAt, locale)}</span>
                    </td>
                    <td className="px-3 py-2.5 align-middle">
                      <div className="flex items-center justify-end gap-1">
                        <MarketEditAction market={market} onEdit={setEditingMarket} t={t} />
                        <ChevronRight className="h-4 w-4 text-muted-foreground" />
                      </div>
                    </td>
                  </tr>
                );
              }) : (
                <tr><td colSpan={8} className="px-4 py-10 text-center text-muted-foreground">
                  <div className="grid justify-items-center gap-2">
                    <span>{stableMarkets.length ? t("dashboard.noFilterResults") : t("dashboard.noMarkets")}</span>
                    {stableMarkets.length ? <button type="button" className="text-xs font-medium text-primary hover:underline" onClick={() => { setQuery(""); setStatusFilter("all"); setOnlyIssues(false); }}>{t("dashboard.clearFilters")}</button> : null}
                  </div>
                </td></tr>
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
                  <Drawer.Heading className="break-all font-mono text-base">{selected?.publicBaseUrl || selected?.id}</Drawer.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">{selected?.email}</p>
                  <p className="mt-1 break-all font-mono text-[11px] text-muted-foreground">{selected?.id}</p>
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selected ? (
                  <div className="grid gap-4">
                    {isUsageMarket(selected) ? (
                      <>
                        <HealthTimelineStrip timeline={selected.healthTimeline} />
                        <DrawerSection label={t("dashboard.officialPrice")}>
                          <MarketPricingCell market={selected} t={t} />
                        </DrawerSection>
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

function marketStateRowKey(share: MarketShare, state: MarketShareRuntimeState) {
  return [
    state.routerId || share.routerId || "main",
    state.shareId || share.shareId,
    state.scope,
    state.kind,
    state.appType || "",
    state.modelId || "",
    state.modelName || "",
    state.updatedAt || "",
  ].join(":");
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

  const blockedStateRows = shares.flatMap((share) =>
    (share.marketStates || [])
      .filter(isMarketReleasableState)
      .map((state) => ({ share, state, key: marketStateRowKey(share, state) })),
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
    if (!market || working || blockedStateRows.length === 0) return;
    setReleasingKey("__all__");
    setError("");
    try {
      for (const { share, state } of blockedStateRows) {
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
              <p className="mt-1 break-all text-sm text-muted-foreground">{market?.publicBaseUrl || market?.email}</p>
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
                      <div className="mt-1 text-xs text-muted-foreground">{t("dashboard.blockedStatesCount", { count: blockedStateRows.length })}</div>
                    </div>
                    <Button size="sm" variant="outline" isDisabled={working || blockedStateRows.length === 0} onClick={releaseAllStates}>
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
                        {blockedStateRows.map(({ share, state, key }) => {
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
                        {!blockedStateRows.length ? <tr><td colSpan={7} className="px-3 py-8 text-center text-muted-foreground">{t("dashboard.noBlockedStates")}</td></tr> : null}
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
    Promise.all([
      getMarketSharePriority(market.email, activeApp),
      getMarketShareSessionLoads(market.publicBaseUrl, activeApp).catch(() => []),
    ])
      .then(([nextShares, sessionLoads]) => {
        if (cancelled) return;
        const loadByShare = new Map(sessionLoads.map((load) => [`${load.routerId}:${load.shareId}`, load.sessionLoad]));
        setShares(nextShares.map((share) => ({
          ...share,
          sessionLoad: loadByShare.get(`${share.routerId}:${share.shareId}`) ?? 0,
        })));
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [market.email, market.publicBaseUrl, activeApp]);

  const ranked = React.useMemo(
    () => rankMarketSharesForApp(shares || [], activeApp, t),
    [shares, activeApp, t],
  );

  return (
    <div className="grid gap-3">
      <div className="text-xs leading-5 text-muted-foreground">{t("dashboard.sharePriorityHint")}</div>
      <Tabs selectedKey={activeApp} onSelectionChange={(key: React.Key) => setActiveApp(String(key) as MarketShareAppKey)} variant="secondary" className="text-foreground">
        <Tabs.List className="grid w-full grid-cols-3 text-foreground">
          {MARKET_SHARE_APPS.map(([key, label]) => (
            <Tabs.Tab
              key={key}
              id={key}
              className="rounded-md border border-transparent px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors data-[selected=true]:border-primary/30 data-[selected=true]:bg-primary/10 data-[selected=true]:text-primary"
            >
              {label}
            </Tabs.Tab>
          ))}
        </Tabs.List>
      </Tabs>
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
          <span className="font-mono">
            {t("dashboard.sharePrioritySessions")} {share.sessionLoad ?? 0} · {share.activeRequests || 0}/{isUnlimited(share.parallelLimit) ? "∞" : share.parallelLimit}
          </span>
        </div>
      </Card.Content>
    </Card>
  );
}

function rankMarketSharesForApp(shares: MarketShare[], app: MarketShareAppKey, t: TFn): MarketSharePriorityItem[] {
  return shares
    .filter((share) => isShareRelevantForApp(share, app))
    .map((share) => marketSharePriorityItem(share, app, t))
    .sort((a, b) => {
      if (a.schedulable !== b.schedulable) return a.schedulable ? -1 : 1;
      if (a.degraded !== b.degraded) return a.degraded ? 1 : -1;
      const sessionDelta = Number(a.share.sessionLoad ?? 0) - Number(b.share.sessionLoad ?? 0);
      if (sessionDelta !== 0) return sessionDelta;
      const activeDelta = Number(a.share.activeRequests || 0) - Number(b.share.activeRequests || 0);
      if (activeDelta !== 0) return activeDelta;
      return b.score - a.score;
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

function runtimeDispatchReady(runtime?: ShareUpstreamProvider | null) {
  if (!runtime) return false;
  if (runtime.kind === "official_oauth") return true;
  return Array.isArray(runtime.models) && runtime.models.some((model) => model.slot && model.actualModel);
}

function marketSharePriorityItem(share: MarketShare, app: MarketShareAppKey, t: TFn): MarketSharePriorityItem {
  const supported = Boolean(share.support?.[app]) || runtimeDispatchReady(share.appRuntimes?.[app]);
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
