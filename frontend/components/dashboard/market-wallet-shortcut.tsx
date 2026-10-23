"use client";

/* Hallmark · component: Market wallet shortcut · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P4 H5 E4 S5 R5 V4
 */

import Link from "next/link";
import { WalletCards } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { useMarketWalletBalance } from "@/components/market/market-wallet-balance-provider";
import { formatWalletUnits, formatWalletUnitsAsCny } from "@/lib/market-wallet-money";

export function MarketWalletShortcut() {
  const { session } = useAuth();
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const { wallet, unavailable } = useMarketWalletBalance();

  if (!session?.authenticated || !session.user) return null;
  const display = unavailable
    ? t("marketWallet.shortcutUnavailable")
    : wallet
    ? displayCurrency === "CNY" && fxRateFresh
      ? formatWalletUnitsAsCny(wallet.availableUnits, fxRateMicros, locale)
      : formatWalletUnits(wallet.availableUnits, "$", locale)
    : t("common.loading");

  return (
    <aside className="flex min-h-11 min-w-0 flex-wrap items-center justify-between gap-x-4 gap-y-1 rounded-lg bg-muted/50 px-3 py-2 text-sm" aria-label={t("marketWallet.title")}>
      <span className="flex min-w-0 items-center gap-2">
        <WalletCards className="size-4 shrink-0 text-primary" aria-hidden="true" />
        <span className="text-muted-foreground">{t("marketWallet.available")}</span>
        <strong className="min-w-0 break-all tabular-nums" aria-live="polite">{display}</strong>
      </span>
      <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
      <Link
        href={DASHBOARD_ACCOUNT_WALLET_PATH}
        className="inline-flex min-h-11 shrink-0 items-center whitespace-nowrap font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:opacity-70"
      >
        {wallet?.canTopup ? t("marketWallet.topup") : t("marketWallet.viewWallet")}
      </Link>
      </div>
    </aside>
  );
}
