"use client";

import { Activity, Globe2, RadioTower, UsersRound } from "lucide-react";
import type { DashboardResponse } from "@/lib/types";
import { formatNumber, formatRelativeTime } from "@/lib/utils";

export function StatsStrip({ data }: { data: DashboardResponse | null }) {
  const countries = new Set((data?.map?.clients || []).map((point) => point.countryCode).filter(Boolean)).size;
  const items = [
    { label: "Clients", value: data?.stats?.clients || 0, icon: UsersRound },
    { label: "Countries", value: countries, icon: Globe2 },
    { label: "Active Shares", value: data?.stats?.activeShares || 0, icon: RadioTower },
    { label: "In-flight", value: data?.stats?.totalActiveRequests || 0, icon: Activity },
  ];
  return (
    <section className="grid gap-3 md:grid-cols-4">
      {items.map((item) => (
        <div key={item.label} className="surface-elevated rounded-lg p-4">
          <div className="flex items-center justify-between text-muted-foreground">
            <span className="mono-label">{item.label}</span>
            <item.icon className="h-4 w-4" />
          </div>
          <div className="mt-3 text-2xl font-semibold">{formatNumber(item.value)}</div>
        </div>
      ))}
      <div className="md:col-span-4 -mt-1 text-right text-xs text-muted-foreground">
        Synced {formatRelativeTime(data?.generatedAt)}
      </div>
    </section>
  );
}
