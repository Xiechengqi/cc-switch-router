"use client";

import * as React from "react";
import { WalletSectionLabel } from "@/components/market/wallet-primitives";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketFinanceOverview, type MarketFinanceOverview } from "@/lib/market-wallet-api";
import { formatMinorUsd, formatWalletUnits } from "@/lib/market-wallet-money";
import type { MessageKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type Stat = { key: MessageKey; value: string; alert?: boolean };

export function MarketFinanceOverviewSection({ refreshKey }: { refreshKey: number }) {
  const { locale, t } = useLocaleText();
  const [overview, setOverview] = React.useState<MarketFinanceOverview | null>(null);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    let active = true;
    getMarketFinanceOverview()
      .then((value) => { if (active) { setOverview(value); setError(""); } })
      .catch((cause) => { if (active) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, [refreshKey]);

  if (!overview) {
    return error ? <p role="alert" className="text-sm text-destructive">{t("settings.marketFinance.overview.unavailable", { message: error })}</p> : null;
  }

  const units = (raw: string) => formatWalletUnits(raw, "$", locale);
  const positive = (raw: string) => { try { return BigInt(raw) > 0n; } catch { return false; } };
  const stats: Stat[] = [
    { key: "settings.marketFinance.overview.walletPosted", value: units(overview.walletPostedUnits) },
    { key: "settings.marketFinance.overview.walletHeld", value: units(overview.walletHeldUnits) },
    { key: "settings.marketFinance.overview.unfulfilled", value: units(overview.unfulfilledUnits) },
    ...(positive(overview.buyerRefundHeldUnits) ? [{ key: "settings.marketFinance.overview.buyerRefundHeld" as MessageKey, value: units(overview.buyerRefundHeldUnits) }] : []),
    { key: "settings.marketFinance.overview.providerPending", value: units(overview.providerPendingUnits) },
    { key: "settings.marketFinance.overview.providerAvailable", value: units(overview.providerAvailableUnits) },
    { key: "settings.marketFinance.overview.providerHeld", value: units(overview.providerHeldUnits) },
    { key: "settings.marketFinance.overview.ownerLoss", value: units(overview.ownerLossUnits), alert: positive(overview.ownerLossUnits) },
    { key: "settings.marketFinance.overview.creditedTopups", value: formatMinorUsd(overview.creditedTopupMinor) },
    { key: "marketFinanceOverview.gross", value: formatMinorUsd(overview.grossTopupMinor) },
    { key: "marketFinanceOverview.maintenance", value: units(overview.maintenanceRevenueUnits ?? "0") },
    { key: "commission.pending", value: units(overview.commissionPendingUnits ?? "0") },
    { key: "commission.revenue", value: units(overview.commissionRevenueUnits ?? "0") },
    { key: "commission.reversed", value: units(overview.commissionReversedUnits ?? "0") },
    { key: "earningConversion.overview", value: units(overview.earningConvertedUnits ?? "0") },
    { key: "settings.marketFinance.overview.gatewayCost", value: formatMinorUsd(overview.estimatedGatewayCostMinor) },
    { key: "settings.marketFinance.overview.openCases", value: overview.openCases.toLocaleString(locale) },
    { key: "settings.marketFinance.overview.criticalCases", value: overview.openCriticalCases.toLocaleString(locale), alert: overview.openCriticalCases > 0 },
    { key: "settings.marketFinance.overview.heldLots", value: overview.heldEarningLots.toLocaleString(locale) },
  ];

  return (
    <section
      aria-labelledby="market-finance-overview-title"
      className="rounded-xl border border-border bg-muted/30 p-5 text-foreground sm:p-7"
    >
      <div className="grid gap-2">
        <WalletSectionLabel>{t("settings.marketFinance.overview.label")}</WalletSectionLabel>
        <h4 id="market-finance-overview-title" className="font-display text-xl font-semibold tracking-tight sm:text-2xl">
          {t("settings.marketFinance.overview.title")}
        </h4>
      </div>
      <dl className="mt-5 grid grid-cols-2 gap-x-4 gap-y-5 lg:grid-cols-4">
        {stats.map((stat) => (
          <div key={stat.key} className="min-w-0 border-l border-border pl-3">
            <dt className="text-xs text-muted-foreground">{t(stat.key)}</dt>
            <dd className={cn("mt-1 break-all text-lg font-semibold tabular-nums tracking-tight sm:text-xl", stat.alert && "text-amber-700")}>
              {stat.value}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
