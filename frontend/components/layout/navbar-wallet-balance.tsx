"use client";

import * as React from "react";
import { usePathname } from "next/navigation";
import { WalletCards } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useMarketWalletBalance } from "@/components/market/market-wallet-balance-provider";
import { DASHBOARD_ACCOUNT_WALLET_PATH, WALLET_ACTIONS_ID } from "@/lib/dashboard-nav";
import { formatWalletUnits, formatWalletUnitsAsCny } from "@/lib/market-wallet-money";

// Loaded on first open: top-up, C2C and sending stay out of every page's first bundle.
const WalletQuickDialog = React.lazy(() =>
  import("@/components/market/wallet-quick-dialog").then((module) => ({ default: module.WalletQuickDialog })));

export function NavbarWalletBalance() {
  const { session } = useAuth();
  const pathname = usePathname();
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const { wallet, unavailable } = useMarketWalletBalance();
  const [open, setOpen] = React.useState(false);
  const [mounted, setMounted] = React.useState(false);
  const actor = session?.authenticated ? session.user?.id : undefined;
  React.useEffect(() => { setOpen(false); setMounted(false); }, [actor]);
  if (!session?.authenticated || !session.user) return null;

  const loading = !wallet && !unavailable;
  const amount = wallet && !unavailable
    ? displayCurrency === "CNY" && fxRateFresh
      ? formatWalletUnitsAsCny(wallet.availableUnits, fxRateMicros, locale)
      : formatWalletUnits(wallet.availableUnits, "$", locale)
    : loading ? "…" : "—";
  const label = unavailable ? t("marketWallet.shortcutUnavailable")
    : loading ? t("common.loading") : amount;
  const onWalletPage = pathname?.replace(/\/$/, "") === DASHBOARD_ACCOUNT_WALLET_PATH.replace(/\/$/, "");

  const activate = () => {
    // The balance page already holds the same actions; do not stack a second
    // top-up form over it.
    if (onWalletPage) {
      const section = document.getElementById(WALLET_ACTIONS_ID);
      if (section) {
        section.scrollIntoView({ block: "start" });
        section.focus({ preventScroll: true });
        return;
      }
    }
    setMounted(true);
    setOpen(true);
  };

  return (
    <>
      <button
        type="button"
        onClick={activate}
        aria-label={onWalletPage ? `${t("marketWallet.navBalance")}: ${label}` : t("walletQuick.open", { amount: label })}
        aria-haspopup={onWalletPage ? undefined : "dialog"}
        aria-busy={loading}
        title={`${t("marketWallet.navBalance")}: ${label}`}
        data-wallet-balance
        data-state={unavailable ? "error" : loading ? "loading" : "ready"}
        className="inline-flex h-11 min-w-0 max-w-24 items-center gap-1.5 whitespace-nowrap rounded-md px-1.5 text-xs font-medium text-foreground transition-colors hover:bg-secondary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:bg-secondary sm:max-w-52 sm:px-2 sm:text-sm"
      >
        <WalletCards className="hidden size-4 shrink-0 text-muted-foreground sm:block" aria-hidden="true" />
        <span className="min-w-0 truncate tabular-nums" aria-live="polite" aria-atomic="true">{amount}</span>
      </button>
      {mounted ? (
        <React.Suspense fallback={null}>
          <WalletQuickDialog key={actor} open={open} onOpenChange={setOpen} />
        </React.Suspense>
      ) : null}
    </>
  );
}
