"use client";

import { ExternalLink } from "lucide-react";
import * as React from "react";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
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

function StatusBadge({ active, label }: { active: boolean; label: string }) {
  return <Badge variant={active ? "success" : "outline"}>{label}</Badge>;
}

function UsageBar({ used, limit }: { used: number; limit: number }) {
  if (isUnlimited(limit)) return <span className="text-muted-foreground">unlimited</span>;
  const pct = limit > 0 ? Math.min(100, Math.max(0, (used / limit) * 100)) : 0;
  return (
    <div className="mt-1 h-1 w-32 overflow-hidden rounded bg-muted">
      <div className="h-full rounded bg-primary" style={{ width: `${pct}%` }} />
    </div>
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

function ForSaleCell({ share }: { share?: ShareView }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  const value = share.forSale === "Free" ? "Free" : share.forSale === "Yes" ? "Yes" : "No";
  const marketLines = share.marketAccessMode === "all" ? ["All markets"] : (share.marketLinks || []).map((market) => market.subdomain).filter(Boolean);
  return (
    <div className="grid min-w-32 gap-1.5">
      <Badge variant={value === "No" ? "outline" : "secondary"}>{value}</Badge>
      {value === "Yes" ? (
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

function SupportCell({ share }: { share?: ShareView }) {
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
            <span className="truncate text-right font-semibold">{enabled ? runtime?.kind || models || "on" : "off"}</span>
          </div>
        );
      })}
    </div>
  );
}

function ShareStatusCell({ share }: { share?: ShareView }) {
  if (!share) return <span className="text-muted-foreground">-</span>;
  if (!share.isOnline) return <Badge variant="outline">Offline</Badge>;
  const limit = isUnlimited(share.parallelLimit) ? "∞" : String(share.parallelLimit || 0);
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Usage</span><div><strong>{compactTokens(share.tokensUsed)} / {isUnlimited(share.tokenLimit) ? "∞" : compactTokens(share.tokenLimit)}</strong><UsageBar used={share.tokensUsed} limit={share.tokenLimit} /></div></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Expires</span><strong title={formatDateTime(share.expiresAt)}>{formatRelativeTime(share.expiresAt)}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Parallel</span><strong>{share.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Online</span><strong title={`${share.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(share.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Health</span><HealthDots entries={share.healthChecks} /></div>
    </div>
  );
}

export function ClientsTable({ clients }: { clients: DashboardClient[] }) {
  const [selected, setSelected] = React.useState<DashboardClient | null>(null);
  const sorted = sortClients(clients);
  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>CLIENTS <span className="font-semibold text-foreground">{sorted.length}</span></div>
        <a href="https://github.com/Xiechengqi/cc-switch/releases" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-blue-400">[install]</a>
      </div>
      <Card className="overflow-hidden rounded-[20px]">
        <CardContent className="overflow-x-auto p-0">
          <table className="w-full min-w-[1180px] border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-44 px-4 py-3">Share</th>
                <th className="px-4 py-3">API URL&KEY</th>
                <th className="px-4 py-3">ForSale</th>
                <th className="px-4 py-3">Region</th>
                <th className="px-4 py-3">Status</th>
                <th className="px-4 py-3">Support</th>
                <th className="w-7 px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? sorted.map((client) => {
                const share = client.share;
                const api = shareApiParts(share);
                return (
                  <tr key={client.installation.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={() => setSelected(client)}>
                    <td className="w-44 break-words px-4 py-3 align-middle font-medium text-muted-foreground">{share?.shareName || "No share"}</td>
                    <td className="px-4 py-3 align-middle">
                      <div className="grid min-w-48 gap-1.5">
                        <strong>{api.apiUrl}</strong>
                        <span className="break-all font-mono text-xs text-muted-foreground">{share?.canViewSecret ? api.apiKey : share ? "***" : "-"}</span>
                      </div>
                    </td>
                    <td className="px-4 py-3 align-middle"><ForSaleCell share={share} /></td>
                    <td className="px-4 py-3 align-middle text-muted-foreground">
                      {client.installation.countryCode || "-"}
                    </td>
                    <td className="px-4 py-3 align-middle"><ShareStatusCell share={share} /></td>
                    <td className="px-4 py-3 align-middle"><SupportCell share={share} /></td>
                    <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                  </tr>
                );
              }) : (
                <tr><td colSpan={7} className="px-4 py-10 text-center text-muted-foreground">No clients yet</td></tr>
              )}
            </tbody>
          </table>
        </CardContent>
      </Card>
      <Dialog open={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{selected?.share?.shareName || selected?.installation.id}</DialogTitle>
            <DialogDescription>{selected?.installation.id}</DialogDescription>
          </DialogHeader>
          {selected ? (
            <div className="grid gap-4 sm:grid-cols-2">
              <Info label="Platform" value={`${selected.installation.platform} ${selected.installation.appVersion}`} />
              <Info label="Last seen" value={formatDateTime(selected.installation.lastSeenAt)} />
              <Info label="Owner" value={selected.share?.ownerEmail || "-"} />
              <Info label="Active requests" value={formatNumber(selected.share?.activeRequests || 0)} />
              <Info label="Created" value={formatDateTime(selected.share?.createdAt)} />
              <Info label="Expires" value={selected.share?.expiresAt || "-"} />
            </div>
          ) : null}
        </DialogContent>
      </Dialog>
    </section>
  );
}

function marketStatusLabel(market: DashboardMarket) {
  if (market.online) return "Online";
  return market.status === "active" ? "Offline" : market.status || "Offline";
}

function MarketPricingCell({ market }: { market: DashboardMarket }) {
  const summary = market.pricingSummary || {};
  const entries = [["Claude", summary.claude], ["Codex", summary.codex], ["Gemini", summary.gemini], ["DeepSeek", summary.deepseek]];
  return (
    <div className="grid min-w-44 gap-2">
      {entries.map(([label, value]) => (
        <div key={label as string} className="grid grid-cols-[66px_1fr] gap-2 text-sm">
          <span className="mono-label text-muted-foreground">{label as string}</span>
          <strong>{typeof value === "number" ? `${value}%` : typeof value === "string" && value ? (value.toLowerCase() === "mixed" ? "mixed" : `${value}%`) : "-"}</strong>
        </div>
      ))}
    </div>
  );
}

function MarketStatusCell({ market }: { market: DashboardMarket }) {
  const limit = isUnlimited(market.parallelCapacity) ? "∞" : String(market.parallelCapacity || 0);
  return (
    <div className="grid min-w-52 gap-2 text-sm">
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Shares</span><strong>{market.onlineShareCount || 0} / {market.shareCount || 0}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Seen</span><strong>{formatRelativeTime(market.lastSeenAt)}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Parallel</span><strong>{market.activeRequests || 0}<span className="text-muted-foreground">/{limit}</span></strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Online</span><strong title={`${market.onlineMinutes24h || 0} / 1440 min with successful route probes in last 24h`}>{(market.onlineRate24h || 0).toFixed(1)}%</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Usage</span><strong>{compactTokens(market.usageTokens)} / {market.usageAmountUsd || "$0.00"}</strong></div>
      <div className="grid grid-cols-[54px_1fr] gap-2"><span className="mono-label text-muted-foreground">Health</span><HealthDots entries={market.healthChecks} /></div>
    </div>
  );
}

export function MarketsTable({ markets }: { markets: DashboardMarket[] }) {
  const [selected, setSelected] = React.useState<DashboardMarket | null>(null);
  const sorted = sortMarkets(markets);
  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>MARKETS <span className="font-semibold text-foreground">{sorted.length}</span></div>
        <a href="https://github.com/Xiechengqi/cc-switch-market/releases" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-blue-400">[install]</a>
      </div>
      <Card className="overflow-hidden rounded-[20px]">
        <CardContent className="overflow-x-auto p-0">
          <table className="w-full min-w-[900px] border-collapse text-sm">
            <thead className="bg-muted text-left font-mono text-[11px] uppercase tracking-[0.1em] text-muted-foreground">
              <tr>
                <th className="w-44 px-4 py-3">Market</th>
                <th className="px-4 py-3">Public URL</th>
                <th className="px-4 py-3">?% official price</th>
                <th className="px-4 py-3">Status</th>
                <th className="w-7 px-4 py-3" />
              </tr>
            </thead>
            <tbody>
              {sorted.length ? sorted.map((market) => (
                <tr key={market.id} className="cursor-pointer border-b last:border-0 hover:bg-primary/5" onClick={() => setSelected(market)}>
                  <td className="w-44 break-words px-4 py-3 align-middle">
                    <div className="font-medium">{market.displayName || market.id}</div>
                    <div className="text-xs text-muted-foreground">{market.email}</div>
                    <div className="mt-1"><StatusBadge active={market.online} label={marketStatusLabel(market)} /></div>
                  </td>
                  <td className="px-4 py-3 align-middle">
                    <a href={market.publicBaseUrl} target="_blank" rel="noreferrer" onClick={(event) => event.stopPropagation()} className="inline-flex items-center gap-1 font-semibold hover:text-primary">
                      {market.publicBaseUrl || "-"}
                      <ExternalLink className="h-3 w-3" />
                    </a>
                  </td>
                  <td className="px-4 py-3 align-middle"><MarketPricingCell market={market} /></td>
                  <td className="px-4 py-3 align-middle"><MarketStatusCell market={market} /></td>
                  <td className="px-4 py-3 align-middle text-lg text-muted-foreground">›</td>
                </tr>
              )) : (
                <tr><td colSpan={5} className="px-4 py-10 text-center text-muted-foreground">No markets configured</td></tr>
              )}
            </tbody>
          </table>
        </CardContent>
      </Card>
      <Dialog open={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{selected?.displayName || selected?.id}</DialogTitle>
            <DialogDescription>{selected?.email}</DialogDescription>
          </DialogHeader>
          {selected ? (
            <div className="grid gap-4">
              <div className="grid gap-4 sm:grid-cols-3">
                <Info label="Status" value={marketStatusLabel(selected)} />
                <Info label="Parallel" value={`${selected.activeRequests} / ${isUnlimited(selected.parallelCapacity) ? "∞" : selected.parallelCapacity}`} />
                <Info label="Online 24h" value={`${(selected.onlineRate24h || 0).toFixed(1)}%`} />
              </div>
              <div className="rounded-lg border">
                {(selected.linkedShares || []).slice(0, 8).map((share) => (
                  <div key={share.shareId} className="flex items-center justify-between border-b px-3 py-2 last:border-0">
                    <span className="font-medium">{share.shareName}</span>
                    <Badge variant={share.online ? "success" : "outline"}>{share.online ? "online" : "offline"}</Badge>
                  </div>
                ))}
                {!selected.linkedShares?.length ? <div className="p-4 text-sm text-muted-foreground">No linked shares</div> : null}
              </div>
            </div>
          ) : null}
        </DialogContent>
      </Dialog>
    </section>
  );
}

function Info({ label, value }: { label: string; value?: React.ReactNode }) {
  return (
    <div className="rounded-lg border bg-muted/30 p-3">
      <div className="mono-label text-muted-foreground">{label}</div>
      <div className="mt-2 break-words text-sm font-medium">{value || "--"}</div>
    </div>
  );
}

export function PresenceFooter() {
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
      <span>Page Online <strong className="ml-1 text-foreground">{presence?.onlineCount ?? 0}</strong></span>
      <span className="opacity-50">|</span>
      <span>EMAIL SENT 24H <strong className="ml-1 text-foreground">{presence?.emailSent24h ?? 0}</strong></span>
      <span className="opacity-50">|</span>
      <a href="https://github.com/Xiechengqi/cc-switch-router" target="_blank" rel="noopener noreferrer" className="hover:text-primary">GitHub</a>
    </footer>
  );
}
