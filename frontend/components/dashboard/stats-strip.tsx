"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardResponse } from "@/lib/types";
import { cn, formatNumber } from "@/lib/utils";

function countDistinctCountries(data: DashboardResponse | null) {
  if (data?.map?.countries?.length) return data.map.countries.length;
  const set = new Set<string>();
  if (data?.map?.server?.countryCode) set.add(data.map.server.countryCode);
  return set.size;
}

export function StatsStrip({ data, className }: { data: DashboardResponse | null; className?: string }) {
  const { t } = useLocaleText();

  return (
    <div
      className={cn("flex flex-wrap items-center gap-2 text-xs text-muted-foreground select-text", className)}
      aria-label={t("dashboard.overview")}
    >
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
