"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardResponse } from "@/lib/types";
import { formatNumber } from "@/lib/utils";

function countDistinctCountries(data: DashboardResponse | null) {
  const set = new Set<string>();
  if (data?.map?.server?.countryCode) set.add(data.map.server.countryCode);
  for (const client of data?.map?.clients || []) {
    if (client.countryCode) set.add(client.countryCode);
  }
  return set.size;
}

export function StatsStrip({ data }: { data: DashboardResponse | null }) {
  const { t } = useLocaleText();

  return (
    <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground" aria-label={t("dashboard.overview")}>
      <span title={t("nav.clientsTitle")}>
        <strong className="text-foreground">{formatNumber(data?.stats?.clients || 0)}</strong> {t("nav.clients")}
      </span>
      <span className="opacity-40">·</span>
      <span title={t("nav.countriesTitle")}>
        <strong className="text-foreground">{formatNumber(countDistinctCountries(data))}</strong> {t("nav.countries")}
      </span>
      <span className="opacity-40">·</span>
      <span title={t("nav.activeSharesTitle")}>
        <strong className="text-foreground">{formatNumber(data?.stats?.activeShares || 0)}</strong> {t("nav.activeShares")}
      </span>
      <span className="opacity-40">·</span>
      <span title={t("nav.inFlightTitle")}>
        <strong className="text-foreground">{formatNumber(data?.stats?.totalActiveRequests || 0)}</strong> {t("nav.inFlight")}
      </span>
    </div>
  );
}
