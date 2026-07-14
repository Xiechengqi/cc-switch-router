"use client";

import { Alert } from "@heroui/react";
import { MarketsTable } from "@/components/dashboard/markets-table";
import { useDashboardData } from "@/components/dashboard/dashboard-data";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardMarket } from "@/lib/types";
import { formatNumber } from "@/lib/utils";

function MarketsSummaryStrip({ markets }: { markets: DashboardMarket[] }) {
  const { t } = useLocaleText();
  const onlineCount = markets.filter((market) => market.online).length;
  const activeRequests = markets.reduce((sum, market) => sum + (market.activeRequests || 0), 0);

  return (
    <div
      className="flex flex-wrap items-center gap-2 rounded-xl border border-slate-200/80 bg-white px-4 py-3 text-xs text-muted-foreground shadow-sm"
      aria-label={t("dashboard.marketsOverview")}
    >
      <span>
        <strong className="text-foreground">{formatNumber(markets.length)}</strong> {t("dashboard.markets")}
      </span>
      <span className="opacity-40">·</span>
      <span>
        <strong className="text-emerald-700">{formatNumber(onlineCount)}</strong> {t("common.online")}
      </span>
      <span className="opacity-40">·</span>
      <span title={t("nav.inFlightTitle")}>
        <strong className="text-foreground">{formatNumber(activeRequests)}</strong> {t("nav.inFlight")}
      </span>
    </div>
  );
}

export function MarketsPage() {
  const { data, error, refresh } = useDashboardData();

  return (
    <main className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-6">
      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      <MarketsSummaryStrip markets={data?.markets || []} />
      <MarketsTable markets={data?.markets || []} onChanged={refresh} />
    </main>
  );
}
