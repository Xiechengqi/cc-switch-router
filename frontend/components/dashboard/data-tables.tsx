"use client";

import { ExternalLink } from "lucide-react";
import { Card, Chip, Modal, ProgressBar } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardClient, DashboardMarket, HealthCheckEntry, ShareAppRuntimes, ShareView } from "@/lib/types";
import { compactTokens, formatDateTime, formatNumber, formatRelativeTime } from "@/lib/utils";

function compareDesc(left: number, right: number) {
  if (left === right) return 0;
  return left > right ? -1 : 1;
}

function isUnlimited(value?: number) {
  return Number(value) < 0;
}

function expirySortValue(share?: ShareView) {
  if (!share?.expiresAt) return 0;
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

function StatusBadge({ active, label }: { active: boolean; label: string }) {
  return <Chip color={active ? "success" : "default"} size="sm" variant={active ? "soft" : "tertiary"}>{label}</Chip>;
}

function UsageBar({ used, limit, t }: { used: number; limit: number; t: TFn }) {
  if (isUnlimited(limit)) return <span className="text-muted-foreground">{t("common.unlimited")}</span>;
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

function ForSaleCell({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const value = share.forSale === "Free" ? t("dashboard.free") : share.forSale === "Yes" ? t("dashboard.yes") : t("dashboard.no");
  const marketLines = share.marketAccessMode === "all" ? [t("dashboard.allMarkets")] : (share.marketLinks || []).map((market) => market.subdomain).filter(Boolean);
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
    <div className="grid min-w-44 gap-1.5">
      {rows.map(([key, label]) => {
        const enabled = !!share.support?.[key];
        const runtime = share.appRuntimes?.[key];
        const models = runtime?.models?.map((model) => model.actualModel || model.slot).filter(Boolean).join(", ");
        return (
          <div key={key} className={`grid grid-cols-[56px_1fr] gap-2 rounded-lg border px-2 py-1.5 text-[11px] ${enabled ? "border-emerald-200 bg-emerald-50 text-emerald-700" : "bg-slate-50 text-muted-foreground"}`}>
            <span className="font-mono uppercase">{label}</span>
            <span className="truncate text-right font-semibold">{enabled ? runtime?.kind || models || t("dashboard.on") : t("dashboard.off")}</span>
          </div>
        );
      })}
    </div>
  );
}

function ShareStatusCell({ share, t }: { share?: ShareView; t: TFn }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  if (!share.isOnline) return <Chip size="sm" variant="tertiary">{t("common.offline")}</Chip>;
  const limit = isUnlimited(share.parallelLimit) ? "∞" : String(share.parallelLimit || 0);
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><div><strong>{compactTokens(share.tokensUsed)} / {isUnlimited(share.tokenLimit) ? "∞" : compactTokens(share.tokenLimit)}</strong><UsageBar used={share.tokensUsed} limit={share.tokenLimit} t={t} /></div></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.expires")}</span><strong title={formatDateTime(share.expiresAt)}>{formatRelativeTime(share.expiresAt)}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{share.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${share.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(share.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={share.healthChecks} /></div>
    </div>
  );
}

export function ClientsTable({ clients }: { clients: DashboardClient[] }) {
  const [selected, setSelected] = React.useState<DashboardClient | null>(null);
  const { t } = useLocaleText();
  const sorted = sortClients(clients);
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
                <th className="w-44 px-4 py-3">{t("dashboard.share")}</th>
                <th className="px-4 py-3">{t("dashboard.apiUrlKey")}</th>
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
                    <td className="w-44 break-words px-4 py-3 align-middle font-medium text-muted-foreground">{share?.shareName || t("dashboard.noShare")}</td>
                    <td className="px-4 py-3 align-middle">
                      <div className="grid min-w-48 gap-1.5">
                        <strong>{api.apiUrl}</strong>
                        <span className="break-all font-mono text-xs text-muted-foreground">{share?.canViewSecret ? api.apiKey : share ? "***" : "-"}</span>
                      </div>
                    </td>
                    <td className="px-4 py-3 align-middle"><ForSaleCell share={share} t={t} /></td>
                    <td className="px-4 py-3 align-middle text-muted-foreground">
                      {client.installation.countryCode || "-"}
                    </td>
                    <td className="px-4 py-3 align-middle"><ShareStatusCell share={share} t={t} /></td>
                    <td className="px-4 py-3 align-middle"><SupportCell share={share} t={t} /></td>
                    <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                  </tr>
                );
              }) : (
                <tr><td colSpan={7} className="px-4 py-10 text-center text-muted-foreground">{t("dashboard.noClients")}</td></tr>
              )}
            </tbody>
          </table>
        </Card.Content>
      </Card>
      <Modal isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Modal.Backdrop>
          <Modal.Container placement="center" size="lg">
            <Modal.Dialog className="!bg-white !text-slate-900">
              <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Modal.Header>
                <div>
                  <Modal.Heading>{selected?.share?.shareName || selected?.installation.id}</Modal.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">{selected?.installation.id}</p>
                </div>
              </Modal.Header>
              <Modal.Body>
                {selected ? (
                  <div className="grid gap-4 sm:grid-cols-2">
                    <Info label={t("dashboard.platform")} value={`${selected.installation.platform} ${selected.installation.appVersion}`} />
                    <Info label={t("dashboard.lastSeen")} value={formatDateTime(selected.installation.lastSeenAt)} />
                    <Info label={t("dashboard.owner")} value={selected.share?.ownerEmail || "-"} />
                    <Info label={t("dashboard.activeRequests")} value={formatNumber(selected.share?.activeRequests || 0)} />
                    <Info label={t("dashboard.created")} value={formatDateTime(selected.share?.createdAt)} />
                    <Info label={t("dashboard.expires")} value={selected.share?.expiresAt || "-"} />
                  </div>
                ) : null}
              </Modal.Body>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>
    </section>
  );
}

function marketStatusLabel(market: DashboardMarket, t: TFn) {
  if (market.online) return t("common.online");
  return market.status === "active" ? t("common.offline") : market.status || t("common.offline");
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

function MarketStatusCell({ market, t }: { market: DashboardMarket; t: TFn }) {
  const limit = isUnlimited(market.parallelCapacity) ? "∞" : String(market.parallelCapacity || 0);
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.shares")}</span><strong>{market.onlineShareCount || 0} / {market.shareCount || 0}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.seen")}</span><strong>{formatRelativeTime(market.lastSeenAt)}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span><strong>{market.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.online")}</span><strong title={`${market.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(market.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span><strong>{compactTokens(market.usageTokens)} / {market.usageAmountUsd || "$0.00"}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">{t("dashboard.health")}</span><HealthDots entries={market.healthChecks} /></div>
    </div>
  );
}

export function MarketsTable({ markets }: { markets: DashboardMarket[] }) {
  const [selected, setSelected] = React.useState<DashboardMarket | null>(null);
  const { t } = useLocaleText();
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
                  <td className="px-4 py-3 align-middle"><MarketStatusCell market={market} t={t} /></td>
                  <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                </tr>
              )) : (
                <tr><td colSpan={5} className="px-4 py-10 text-center text-muted-foreground">{t("dashboard.noMarkets")}</td></tr>
              )}
            </tbody>
          </table>
        </Card.Content>
      </Card>
      <Modal isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Modal.Backdrop>
          <Modal.Container placement="center" size="lg">
            <Modal.Dialog className="!bg-white !text-slate-900">
              <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Modal.Header>
                <div>
                  <Modal.Heading>{selected?.displayName || selected?.id}</Modal.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">{selected?.email}</p>
                </div>
              </Modal.Header>
              <Modal.Body>
                {selected ? (
                  <div className="grid gap-4">
                    <div className="grid gap-4 sm:grid-cols-3">
                      <Info label={t("dashboard.status")} value={marketStatusLabel(selected, t)} />
                      <Info label={t("dashboard.parallel")} value={`${selected.activeRequests} / ${isUnlimited(selected.parallelCapacity) ? "∞" : selected.parallelCapacity}`} />
                      <Info label={t("dashboard.online24h")} value={`${(selected.onlineRate24h || 0).toFixed(1)}%`} />
                    </div>
                    <Card className="rounded-lg border p-0 shadow-none">
                      <Card.Content className="gap-0 p-0">
                      {(selected.linkedShares || []).slice(0, 8).map((share) => (
                        <div key={share.shareId} className="flex items-center justify-between border-b px-3 py-2 last:border-0">
                          <span className="font-medium">{share.shareName}</span>
                          <Chip color={share.online ? "success" : "default"} size="sm" variant={share.online ? "soft" : "tertiary"}>{share.online ? t("common.online") : t("common.offline")}</Chip>
                        </div>
                      ))}
                      {!selected.linkedShares?.length ? <div className="p-4 text-sm text-muted-foreground">{t("dashboard.noLinkedShares")}</div> : null}
                      </Card.Content>
                    </Card>
                  </div>
                ) : null}
              </Modal.Body>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>
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
