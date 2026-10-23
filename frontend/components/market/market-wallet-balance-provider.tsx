"use client";

import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { subscribeMarketBalanceChanged } from "@/lib/market-balance-events";
import { getMarketWallet, type MarketWallet } from "@/lib/market-wallet-api";

type WalletBalanceState = {
  wallet: MarketWallet | null;
  unavailable: boolean;
};

const EMPTY_BALANCE: WalletBalanceState = { wallet: null, unavailable: false };
const WalletBalanceContext = React.createContext<WalletBalanceState>(EMPTY_BALANCE);

/** One visible-page poll for navigation and Market shortcuts. Balances stay
 * in memory, are account-scoped, and never enter browser storage. */
export function MarketWalletBalanceProvider({ children }: { children: React.ReactNode }) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id || session.user?.email : undefined;
  const [snapshot, setSnapshot] = React.useState<WalletBalanceState & { actor: string } | null>(null);

  React.useEffect(() => {
    setSnapshot(null);
    if (!actor) return;
    let active = true;
    let dirty = false;
    let inFlight: AbortController | null = null;

    const refresh = (invalidate = false) => {
      if (!active) return;
      if (invalidate) dirty = true;
      if (document.visibilityState !== "visible" || inFlight) return;
      dirty = false;
      const controller = new AbortController();
      inFlight = controller;
      const timeout = window.setTimeout(() => controller.abort(), 10_000);
      void getMarketWallet(controller.signal).then((wallet) => {
        if (wallet?.assetCode !== "USD" || typeof wallet.availableUnits !== "string"
          || !/^-?\d{1,19}$/.test(wallet.availableUnits)) {
          throw new Error("Invalid platform balance response");
        }
        // An invalidation during this request requires a new authoritative read.
        if (!active || dirty) return;
        setSnapshot({ actor, wallet, unavailable: false });
      }).catch(() => {
        if (!active || dirty) return;
        setSnapshot((previous) => ({
          actor,
          wallet: previous?.actor === actor ? previous.wallet : null,
          unavailable: true,
        }));
      }).finally(() => {
        window.clearTimeout(timeout);
        inFlight = null;
        if (active && dirty) refresh();
      });
    };

    const onVisible = () => refresh();
    const unsubscribe = subscribeMarketBalanceChanged(actor, () => refresh(true));
    const timer = window.setInterval(onVisible, 15_000);
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onVisible);
    window.addEventListener("online", onVisible);
    refresh();
    return () => {
      active = false;
      inFlight?.abort();
      window.clearInterval(timer);
      unsubscribe();
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", onVisible);
      window.removeEventListener("online", onVisible);
    };
  }, [actor]);

  const value = actor && snapshot?.actor === actor ? snapshot : EMPTY_BALANCE;
  return <WalletBalanceContext.Provider value={value}>{children}</WalletBalanceContext.Provider>;
}

export function useMarketWalletBalance() {
  return React.useContext(WalletBalanceContext);
}
