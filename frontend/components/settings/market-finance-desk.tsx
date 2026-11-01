"use client";

import * as React from "react";
import { RefreshCw } from "lucide-react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { MarketFinanceCaseDesk } from "@/components/settings/market-finance-case-desk";
import { MarketFinanceDisputesPanel } from "@/components/settings/market-finance-disputes-panel";
import { MarketFinanceEarningHolds } from "@/components/settings/market-finance-earning-holds";
import { MarketFinanceOverviewSection } from "@/components/settings/market-finance-overview";
import { MarketFinanceTopupRecoveryPanel } from "@/components/settings/market-finance-topup-recovery-panel";
import { MarketFinanceUserLookup } from "@/components/settings/market-finance-user-lookup";

/**
 * Day-to-day wallet work: balances, open cases, held earnings, top-up
 * recovery and disputes. Wallet policy and payment channels stay in
 * Settings → Market & Billing → Wallet & payments.
 */
export function MarketFinanceDesk() {
  const { t } = useLocaleText();
  const [refreshKey, setRefreshKey] = React.useState(0);
  const refresh = React.useCallback(async () => setRefreshKey((value) => value + 1), []);

  return (
    <section className="grid min-w-0 gap-6" aria-labelledby="market-finance-desk-title">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 id="market-finance-desk-title" className="text-lg font-semibold">{t("operations.finance.title")}</h2>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">{t("operations.finance.description")}</p>
        </div>
        <Button variant="ghost" size="sm" onPress={() => void refresh()}>
          <RefreshCw className="size-4" /> {t("settings.marketFinance.refresh")}
        </Button>
      </div>
      <MarketFinanceOverviewSection refreshKey={refreshKey} />
      <MarketFinanceCaseDesk refreshKey={refreshKey} onChanged={refresh} />
      <MarketFinanceUserLookup />
      <MarketFinanceEarningHolds refreshKey={refreshKey} onChanged={refresh} />
      <MarketFinanceTopupRecoveryPanel onRecovered={refresh} />
      <MarketFinanceDisputesPanel />
    </section>
  );
}
