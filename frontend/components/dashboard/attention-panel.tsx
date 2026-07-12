"use client";

import { AlertTriangle, ChevronRight, CircleCheck } from "lucide-react";
import * as React from "react";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import { useDashboardViewState } from "@/components/dashboard/dashboard-view-state";
import { clientOperationalSummary, marketOperationalSummary, operationalReasonLabel } from "@/components/dashboard/operational-status";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardResponse, OperationalSummary } from "@/lib/types";
import { formatRelativeTime } from "@/lib/utils";

type AttentionItem = {
  id: string;
  kind: "client" | "market";
  label: string;
  summary: OperationalSummary;
};

export function AttentionPanel({ data }: { data: DashboardResponse | null }) {
  const { locale, t } = useLocaleText();
  const focus = useDashboardFocus();
  const { issuesOnly, setIssuesOnly } = useDashboardViewState();
  const items = React.useMemo<AttentionItem[]>(() => {
    if (!data) return [];
    const shares = data.shares || [];
    const clients = data.clients.flatMap((client) => {
      const ids = new Set(client.shareIds || []);
      const summary = clientOperationalSummary(client, shares.filter((share) => ids.has(share.shareId)));
      if (summary.state !== "degraded" && summary.state !== "offline") return [];
      return [{ id: client.installation.id, kind: "client" as const, label: client.clientTunnel?.subdomain || client.installation.id, summary }];
    });
    const markets = (data.markets || []).flatMap((market) => {
      const summary = marketOperationalSummary(market);
      if (!["degraded", "offline", "maintenance"].includes(summary.state)) return [];
      return [{ id: market.id, kind: "market" as const, label: market.displayName || market.subdomain || market.id, summary }];
    });
    return [...clients, ...markets].sort((left, right) => {
      const rank = (state: string) => state === "offline" ? 0 : state === "degraded" ? 1 : 2;
      return rank(left.summary.state) - rank(right.summary.state) || left.label.localeCompare(right.label);
    });
  }, [data]);

  const locate = (item: AttentionItem) => {
    const source = item.kind === "client" ? "client-board" : "market-table";
    focus.setFocus({ kind: item.kind, id: item.id, source });
    focus.openDrawer(item.kind, item.id);
    window.requestAnimationFrame(() => document.getElementById(`dashboard-${item.kind}-${item.id}`)?.scrollIntoView({ behavior: "smooth", block: "center" }));
  };

  return (
    <section className={`overflow-hidden rounded-xl border shadow-sm ${items.length ? "border-amber-200 bg-amber-50/55" : "border-emerald-200 bg-emerald-50/45"}`} aria-label={t("dashboard.needsAttention")}>
      <div className="flex items-center justify-between gap-3 px-4 pb-1 pt-3">
        <div className={`flex items-center gap-2 font-mono text-[10px] font-semibold uppercase tracking-[0.14em] ${items.length ? "text-amber-800" : "text-emerald-800"}`}>
          {items.length ? <AlertTriangle className="h-3.5 w-3.5" /> : <CircleCheck className="h-3.5 w-3.5" />}
          {items.length ? `${t("dashboard.needsAttention")} · ${items.length}` : t("dashboard.allSystemsHealthy")}
        </div>
        <button type="button" aria-pressed={issuesOnly} disabled={!items.length} onClick={() => setIssuesOnly(!issuesOnly)} className="text-[11px] font-medium text-amber-800 hover:underline disabled:cursor-default disabled:opacity-40">
          {issuesOnly ? t("dashboard.showAll") : t("dashboard.onlyIssues")}
        </button>
      </div>
      {items.length ? (
        <div className="grid grid-cols-2 gap-1 p-2">
          {items.map((item) => {
            const critical = item.summary.state === "offline";
            const since = item.summary.primaryReason?.startedAt || item.summary.changedAt;
            return (
              <button key={`${item.kind}:${item.id}`} type="button" onClick={() => locate(item)} className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-lg px-2.5 py-2 text-left hover:bg-amber-100/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30">
                <span className={`h-2 w-2 rounded-full ${critical ? "bg-rose-500" : "bg-amber-400"}`} />
                <span className="truncate text-xs text-foreground">
                  <strong>{item.label}</strong> <span className="text-muted-foreground">{item.kind === "client" ? t("dashboard.client") : t("dashboard.market")} · </span>
                  <span className={critical ? "text-rose-700" : "text-amber-700"}>{operationalReasonLabel(item.summary.primaryReason, t)}{since ? ` · ${formatRelativeTime(since, locale)}` : ""}</span>
                </span>
                <ChevronRight className="h-3.5 w-3.5 text-amber-700" />
              </button>
            );
          })}
        </div>
      ) : <p className="px-4 pb-3 pt-1 text-xs text-emerald-800/80">{t("dashboard.noOperationalIssues")}</p>}
    </section>
  );
}
