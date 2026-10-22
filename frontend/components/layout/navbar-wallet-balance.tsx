"use client";

import Link from "next/link";
import { WalletCards } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useMarketWalletBalance } from "@/components/market/market-wallet-balance-provider";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { formatWalletUnits, formatWalletUnitsAsCny } from "@/lib/market-wallet-money";

export function NavbarWalletBalance() {
  const { session } = useAuth();
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const { wallet, unavailable } = useMarketWalletBalance();
  if (!session?.authenticated || !session.user) return null;

  const loading = !wallet && !unavailable;
  const amount = wallet && !unavailable
    ? displayCurrency === "CNY" && fxRateFresh
      ? formatWalletUnitsAsCny(wallet.availableUnits, fxRateMicros, locale)
      : formatWalletUnits(wallet.availableUnits, "$", locale)
    : loading ? "…" : "—";
  const label = unavailable ? t("marketWallet.shortcutUnavailable")
    : loading ? t("common.loading") : amount;
  const description = `${t("marketWallet.navBalance")}: ${label}`;

  return (
    <Link
      href={DASHBOARD_ACCOUNT_WALLET_PATH}
      aria-label={description}
      aria-busy={loading}
      title={description}
      data-wallet-balance
      data-state={unavailable ? "error" : loading ? "loading" : "ready"}
      className="inline-flex h-11 min-w-0 max-w-24 items-center gap-1.5 whitespace-nowrap rounded-md px-1.5 text-xs font-medium text-foreground transition-colors hover:bg-secondary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:bg-secondary sm:max-w-52 sm:px-2 sm:text-sm"
    >
      <WalletCards className="hidden size-4 shrink-0 text-muted-foreground sm:block" aria-hidden="true" />
      <span className="hidden text-muted-foreground 2xl:inline">{t("marketWallet.navBalance")}</span>
      <span className="min-w-0 truncate tabular-nums" aria-live="polite" aria-atomic="true">{amount}</span>
    </Link>
  );
}
