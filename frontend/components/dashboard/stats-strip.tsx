"use client";

import { Card } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { clientOperationalSummary, shareOperationalSummary } from "@/components/dashboard/operational-status";
import type { DashboardResponse } from "@/lib/types";
import { formatNumber } from "@/lib/utils";

export function StatsStrip({ data }: { data: DashboardResponse | null }) {
  const { t } = useLocaleText();
  const countries = new Set((data?.map?.clients || []).map((point) => point.countryCode).filter(Boolean)).size;
  const shares = data?.shares || [];
  const clients = data?.clients || [];
  const clientIssues = clients.filter((client) => {
    const ids = new Set(client.shareIds || []);
    const state = clientOperationalSummary(client, shares.filter((share) => ids.has(share.shareId))).state;
    return state === "degraded" || state === "offline";
  }).length;
  const onlineShares = shares.filter((share) => shareOperationalSummary(share).state === "online").length;
  const items = [
    { label: t("stats.clients"), value: data?.stats?.clients || 0, detail: clientIssues ? t("dashboard.kpiIssues", { count: clientIssues }) : t("dashboard.kpiHealthy"), tone: clientIssues ? "text-rose-700" : "text-emerald-700" },
    { label: t("stats.countries"), value: countries, detail: t("dashboard.kpiRoutingNow"), tone: "text-muted-foreground" },
    { label: t("stats.activeShares"), value: data?.stats?.activeShares || 0, detail: t("dashboard.kpiOnline", { count: onlineShares }), tone: "text-emerald-700" },
    { label: t("stats.inFlight"), value: data?.stats?.totalActiveRequests || 0, detail: "", tone: "text-primary" },
  ];
  return (
    <section className="grid grid-cols-4 gap-3" aria-label={t("dashboard.overview")}>
      {items.map((item, index) => (
        <Card key={item.label} className="rounded-xl border bg-white p-0 shadow-sm">
          <Card.Content className="grid gap-2 p-4">
            <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.14em] text-muted-foreground">{item.label}</span>
            <div className="flex items-baseline gap-2">
              <strong className={`text-2xl font-bold tabular-nums ${index === 3 ? "text-primary" : "text-foreground"}`}>{data ? formatNumber(item.value) : "—"}</strong>
              {item.detail ? <span className={`text-[11px] font-medium ${item.tone}`}>{item.detail}</span> : <span className="live-pulse h-1.5 w-1.5 rounded-full bg-primary" />}
            </div>
          </Card.Content>
        </Card>
      ))}
    </section>
  );
}
