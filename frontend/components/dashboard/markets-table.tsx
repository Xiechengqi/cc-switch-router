"use client";

import { ExternalLink, Loader2, Pencil, Save, X } from "lucide-react";
import { Button, Card, Checkbox, Chip, Drawer, Modal, Tabs, TextArea } from "@heroui/react";
import * as React from "react";
import { DrawerSection, EmptyBlock, HealthTimelineStrip, Info, StatusBadge, TokenGrid } from "@/components/dashboard/drawer-panels";
import { FieldGroup } from "@/components/dashboard/share-edit-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketLinkedShares, getMarketSharePriority, getMarketShareSessionLoads, releaseMarketShareState, updateMarketDisabledShares, updateMarketMaintenance } from "@/lib/api";
import type { AppLocale } from "@/lib/i18n";
import type { DashboardMarket, MarketAppAvailabilityEntry, MarketRequestLog, MarketShare, MarketShareRuntimeState, ShareAppRuntimes, ShareUpstreamProvider } from "@/lib/types";
import { cn, compactTokens, formatDateTime, formatRelativeTime } from "@/lib/utils";
import { canShowMarketSharePriority, drawerDialogClassName, formatAgeDaysOrHours, formatOfficialPriceMultiplier, formatUsdExactTrimmed, formatUsdOneDecimal, HealthDots, isShareMarket, isUnlimited, isUsageMarket, marketKindDescription, marketKindLabel, requestModelRoute, shouldOpenRowDrawer, sortMarkets, usageBucketTotalTokens, type TFn } from "@/components/dashboard/share-dashboard-utils";

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
      {market.publicBaseUrl ? (
        <a
          href={market.publicBaseUrl}
          target="_blank"
          rel="noreferrer"
          onClick={(event) => event.stopPropagation()}
          className="inline-flex min-w-0 max-w-full items-center gap-1 break-all font-mono font-medium text-foreground underline-offset-4 hover:underline"
          title={market.publicBaseUrl}
        >
          <span className="min-w-0 break-all">{market.publicBaseUrl}</span>
          <ExternalLink className="h-3 w-3 shrink-0" />
        </a>
      ) : (
        <strong className="min-w-0 break-all font-mono font-medium">{market.id}</strong>
      )}
      <span className="break-all text-xs text-muted-foreground">{market.email}</span>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <MarketTypeChip market={market} t={t} />
        <StatusBadge active={market.online} label={marketStatusLabel(market, t)} />
        {market.maintenanceEnabled ? (
          <Chip color="warning" size="sm" variant="soft">
            {t("dashboard.maintenance")}
          </Chip>
        ) : null}
        <MarketEditAction market={market} onEdit={onEdit} t={t} />
      </div>
    </div>
  );
}

function MarketStatusCell({ market, t, locale }: { market: DashboardMarket; t: TFn; locale: AppLocale }) {
  const ageValue = formatAgeDaysOrHours(market.createdAt, locale);
  const rowClass = "grid grid-cols-[76px_minmax(0,1fr)] gap-2";
  const limit = isUnlimited(market.parallelCapacity) ? "∞" : String(market.parallelCapacity || 0);
  const onlineValue = market.online ? `${(market.onlineRate24h || 0).toFixed(1)}% / ${ageValue}` : ageValue;
  const usageValue = isShareMarket(market)
    ? compactTokens(market.usageTokens)
    : `${compactTokens(market.usageTokens)} / ${formatUsdOneDecimal(market.usageAmountUsd)}`;
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.shares")}</span><strong>{market.onlineShareCount || 0} / {market.shareCount || 0}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${market.onlineMinutes24h || 0} / 1440 min · ${formatDateTime(market.createdAt)}`}>{onlineValue}</strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{market.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className={rowClass}><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><strong>{usageValue}</strong></div>
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
