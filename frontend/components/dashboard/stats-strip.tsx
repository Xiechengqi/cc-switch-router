"use client";

import { Activity, Globe2, RadioTower, UsersRound } from "lucide-react";
import { Card } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardResponse } from "@/lib/types";
import { formatNumber, formatRelativeTime } from "@/lib/utils";

export function StatsStrip({ data }: { data: DashboardResponse | null }) {
  const { t } = useLocaleText();
  const countries = new Set((data?.map?.clients || []).map((point) => point.countryCode).filter(Boolean)).size;
  const items = [
    { label: t("stats.clients"), value: data?.stats?.clients || 0, icon: UsersRound },
    { label: t("stats.countries"), value: countries, icon: Globe2 },
    { label: t("stats.activeShares"), value: data?.stats?.activeShares || 0, icon: RadioTower },
    { label: t("stats.inFlight"), value: data?.stats?.totalActiveRequests || 0, icon: Activity },
  ];
  return (
    <section className="grid gap-3 md:grid-cols-4">
      {items.map((item) => (
        <Card key={item.label} className="surface-elevated rounded-lg p-0">
          <Card.Content className="p-4">
            <div className="flex items-center justify-between text-muted-foreground">
              <span className="mono-label">{item.label}</span>
              <item.icon className="h-4 w-4" />
            </div>
            <div className="mt-3 text-2xl font-semibold">{formatNumber(item.value)}</div>
          </Card.Content>
        </Card>
      ))}
      <div className="md:col-span-4 -mt-1 text-right text-xs text-muted-foreground">
        {t("dashboard.synced", { time: formatRelativeTime(data?.generatedAt) })}
      </div>
    </section>
  );
}
