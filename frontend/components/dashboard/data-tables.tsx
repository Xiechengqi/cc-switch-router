"use client";

import { ExternalLink } from "lucide-react";
import * as React from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { DashboardClient, DashboardMarket } from "@/lib/types";
import { compactTokens, formatDateTime, formatNumber, formatRelativeTime } from "@/lib/utils";

function StatusBadge({ active, label }: { active: boolean; label: string }) {
  return <Badge variant={active ? "success" : "outline"}>{label}</Badge>;
}

export function ClientsTable({ clients }: { clients: DashboardClient[] }) {
  const [selected, setSelected] = React.useState<DashboardClient | null>(null);
  const sorted = [...clients].sort((a, b) => new Date(b.installation.lastSeenAt).getTime() - new Date(a.installation.lastSeenAt).getTime());
  return (
    <Card className="rounded-lg">
      <CardHeader>
        <CardTitle>Clients</CardTitle>
        <CardDescription>Registered installations and their active shares.</CardDescription>
      </CardHeader>
      <CardContent className="overflow-x-auto">
        <table className="w-full min-w-[760px] text-sm">
          <thead className="text-left text-xs uppercase text-muted-foreground">
            <tr className="border-b">
              <th className="py-2 pr-4">Share</th>
              <th className="py-2 pr-4">Endpoint</th>
              <th className="py-2 pr-4">Platform</th>
              <th className="py-2 pr-4">Country</th>
              <th className="py-2 pr-4">Usage</th>
              <th className="py-2 pr-4">Status</th>
            </tr>
          </thead>
          <tbody>
            {sorted.length ? (
              sorted.map((client) => {
                const share = client.share;
                return (
                  <tr key={client.installation.id} className="cursor-pointer border-b last:border-0 hover:bg-muted/45" onClick={() => setSelected(client)}>
                    <td className="py-3 pr-4 font-medium">{share?.shareName || "No share"}</td>
                    <td className="py-3 pr-4 text-muted-foreground">{share?.subdomain || "-"}</td>
                    <td className="py-3 pr-4 text-muted-foreground">{client.installation.platform}</td>
                    <td className="py-3 pr-4 text-muted-foreground">{client.installation.countryCode || "-"}</td>
                    <td className="py-3 pr-4 text-muted-foreground">
                      {share ? `${compactTokens(share.tokensUsed)} / ${share.tokenLimit < 0 ? "unlimited" : compactTokens(share.tokenLimit)}` : "-"}
                    </td>
                    <td className="py-3 pr-4">
                      <StatusBadge active={!!share?.isOnline} label={share?.shareStatus || "idle"} />
                    </td>
                  </tr>
                );
              })
            ) : (
              <tr>
                <td colSpan={6} className="py-10 text-center text-muted-foreground">No clients yet</td>
              </tr>
            )}
          </tbody>
        </table>
      </CardContent>
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
    </Card>
  );
}

export function MarketsTable({ markets }: { markets: DashboardMarket[] }) {
  const [selected, setSelected] = React.useState<DashboardMarket | null>(null);
  const sorted = [...markets].sort((a, b) => Number(b.online) - Number(a.online) || a.displayName.localeCompare(b.displayName));
  return (
    <Card className="rounded-lg">
      <CardHeader>
        <CardTitle>Markets</CardTitle>
        <CardDescription>Public market registrations and linked share health.</CardDescription>
      </CardHeader>
      <CardContent className="overflow-x-auto">
        <table className="w-full min-w-[760px] text-sm">
          <thead className="text-left text-xs uppercase text-muted-foreground">
            <tr className="border-b">
              <th className="py-2 pr-4">Market</th>
              <th className="py-2 pr-4">Public URL</th>
              <th className="py-2 pr-4">Shares</th>
              <th className="py-2 pr-4">Usage</th>
              <th className="py-2 pr-4">Health</th>
            </tr>
          </thead>
          <tbody>
            {sorted.length ? (
              sorted.map((market) => (
                <tr key={market.id} className="cursor-pointer border-b last:border-0 hover:bg-muted/45" onClick={() => setSelected(market)}>
                  <td className="py-3 pr-4">
                    <div className="font-medium">{market.displayName || market.id}</div>
                    <div className="text-xs text-muted-foreground">{market.email}</div>
                  </td>
                  <td className="py-3 pr-4 text-muted-foreground">
                    <a href={market.publicBaseUrl} target="_blank" rel="noreferrer" onClick={(event) => event.stopPropagation()} className="inline-flex items-center gap-1 hover:text-primary">
                      {market.publicBaseUrl || "-"}
                      <ExternalLink className="h-3 w-3" />
                    </a>
                  </td>
                  <td className="py-3 pr-4 text-muted-foreground">{market.onlineShareCount} / {market.shareCount}</td>
                  <td className="py-3 pr-4 text-muted-foreground">{compactTokens(market.usageTokens)} / {market.usageAmountUsd || "$0.00"}</td>
                  <td className="py-3 pr-4">
                    <StatusBadge active={market.online} label={market.online ? "online" : market.status} />
                  </td>
                </tr>
              ))
            ) : (
              <tr>
                <td colSpan={5} className="py-10 text-center text-muted-foreground">No markets configured</td>
              </tr>
            )}
          </tbody>
        </table>
      </CardContent>
      <Dialog open={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{selected?.displayName || selected?.id}</DialogTitle>
            <DialogDescription>{selected?.email}</DialogDescription>
          </DialogHeader>
          {selected ? (
            <div className="grid gap-4">
              <div className="grid gap-4 sm:grid-cols-3">
                <Info label="Status" value={selected.online ? "online" : selected.status} />
                <Info label="Parallel" value={`${selected.activeRequests} / ${selected.parallelCapacity}`} />
                <Info label="Online 24h" value={`${Math.round(selected.onlineRate24h * 100)}%`} />
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
    </Card>
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
    <footer className="mx-auto flex w-[calc(100%-2rem)] max-w-7xl flex-wrap items-center justify-between gap-2 py-6 text-xs text-muted-foreground">
      <span>Page online {presence?.onlineCount ?? 0}</span>
      <span>Email sent 24h {presence?.emailSent24h ?? 0}</span>
      <span>Switch Router</span>
    </footer>
  );
}
