"use client";

/*
 * Top up without leaving the current task. The payment page opens in a new
 * tab; this panel keeps watching the balance and reports back once it covers
 * what the task needs. It never continues the task by itself.
 */

import * as React from "react";
import { Button } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletTopupForm } from "@/components/market/wallet-topup-form";
import { notifyMarketBalanceChanged, subscribeMarketBalanceChanged } from "@/lib/market-balance-events";
import {
  getMarketWallet,
  getMarketWalletTopups,
  type MarketWallet,
  type MarketWalletTopup,
} from "@/lib/market-wallet-api";
import { formatUnitsCentsDown, formatUnitsCentsUp, safeMarketReturnPath } from "@/lib/market-wallet-money";

type Snapshot = { actor: string; wallet: MarketWallet; topups: MarketWalletTopup[] };

/**
 * The buyer's balance and recent top-ups while `active`. Refreshes on balance
 * signals, focus and a bounded interval (faster while `waiting`). Reopening
 * is a new read barrier: cached history must not authorize a payment.
 */
export function useWalletSnapshot(active: boolean, waiting = false) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id : undefined;
  const [snapshot, setSnapshot] = React.useState<Snapshot | null>(null);
  const [error, setError] = React.useState("");
  const [ready, setReady] = React.useState(false);
  const loadVersion = React.useRef(0);

  const load = React.useCallback(async () => {
    if (!actor) return;
    const version = ++loadVersion.current;
    let wallet: MarketWallet;
    let topups: Awaited<ReturnType<typeof getMarketWalletTopups>>;
    try {
      [wallet, topups] = await Promise.all([getMarketWallet(), getMarketWalletTopups()]);
    } catch (cause) {
      if (version === loadVersion.current) {
        setReady(false);
        setError(cause instanceof Error ? cause.message : String(cause));
      }
      return;
    }
    if (version !== loadVersion.current) return;
    setSnapshot((current) => ({
      actor,
      wallet,
      // Keep orders created here that the first page may not list yet.
      topups: current?.actor === actor
        ? [...topups.items, ...current.topups.filter((order) => !topups.items.some((item) => item.id === order.id))]
        : topups.items,
    }));
    setError("");
    setReady(true);
  }, [actor]);

  React.useEffect(() => {
    if (!active) return;
    setReady(false);
    const refresh = () => { if (document.visibilityState === "visible") void load(); };
    refresh();
    const timer = window.setInterval(refresh, waiting ? 10_000 : 30_000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    const off = actor ? subscribeMarketBalanceChanged(actor, refresh) : () => {};
    return () => {
      loadVersion.current++;
      window.clearInterval(timer);
      off();
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [actor, load, active, waiting]);

  const reload = React.useCallback(async () => {
    if (actor) notifyMarketBalanceChanged(actor);
    await load();
  }, [actor, load]);

  const addOrder = React.useCallback((order: MarketWalletTopup) => {
    loadVersion.current++;
    setSnapshot((value) => value && value.actor === actor
      ? { ...value, topups: [order, ...value.topups.filter((item) => item.id !== order.id)] }
      : value);
  }, [actor]);

  const current = snapshot?.actor === actor ? snapshot : null;
  return { actor, current, error, ready, load, reload, addOrder };
}

/**
 * Inline top-up for a task that needs `requiredUnits` of available balance.
 * `onFunded` fires once when the balance covers it.
 */
export function InPlaceTopup({
  active,
  purpose,
  requiredUnits,
  suggestedAmount,
  idPrefix = "in-place-topup",
  onBusyChange,
  onFunded,
}: {
  active: boolean;
  /** One line on what the money is for, e.g. "Seat #1 needs $0.91 more". */
  purpose: string;
  requiredUnits: string;
  /** Dollars with two decimals; already clamped to the top-up range. */
  suggestedAmount: string | null;
  idPrefix?: string;
  onBusyChange?: (busy: boolean) => void;
  onFunded: (wallet: MarketWallet) => void;
}) {
  const { t, locale } = useLocaleText();
  const [ordered, setOrdered] = React.useState(false);
  const { actor, current, error, ready, load, reload, addOrder } = useWalletSnapshot(active, ordered);
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const funded = React.useRef(false);
  const onFundedRef = React.useRef(onFunded);
  onFundedRef.current = onFunded;

  React.useEffect(() => {
    if (active) setReturnPath(safeMarketReturnPath(`${window.location.pathname}${window.location.search}`));
  }, [active]);

  React.useEffect(() => {
    if (!active || !current || funded.current) return;
    if (BigInt(current.wallet.availableUnits) >= BigInt(requiredUnits)) {
      funded.current = true;
      onFundedRef.current(current.wallet);
    }
  }, [active, current, requiredUnits]);

  const missing = current ? BigInt(requiredUnits) - BigInt(current.wallet.availableUnits) : null;

  return (
    <div className="grid min-w-0 gap-4">
      <div className="grid gap-1 rounded-xl border border-accent/20 bg-accent/5 px-4 py-3">
        <p className="text-sm font-medium text-foreground">{purpose}</p>
        <p className="text-xs leading-5 text-muted-foreground">
          {current
            ? t("shareRent.topup.balanceLine", {
              balance: formatUnitsCentsDown(current.wallet.availableUnits, "$", locale),
              missing: formatUnitsCentsUp(missing && missing > 0n ? missing.toString() : "0", "$", locale),
            })
            : error ? t("walletQuick.unavailable") : t("common.loading")}
        </p>
      </div>
      {error ? (
        <div className="grid justify-items-start gap-2">
          <p role="alert" className="text-sm text-destructive">{t("walletQuick.unavailable")} {error}</p>
          <Button size="sm" variant="outline" onPress={() => void load()}>{t("common.retry")}</Button>
        </div>
      ) : null}
      {current && actor ? (
        <WalletTopupForm
          wallet={current.wallet}
          topups={current.topups}
          ready={ready}
          active={active}
          returnPath={returnPath}
          suggestedAmount={suggestedAmount}
          idPrefix={idPrefix}
          onBusyChange={onBusyChange}
          onOrderCreated={(order) => { setOrdered(true); addOrder(order); }}
          onReload={reload}
        />
      ) : !error ? (
        <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden />{t("common.loading")}
        </p>
      ) : null}
      <p role="status" aria-live="polite" className="flex items-center gap-2 text-xs leading-5 text-muted-foreground">
        {ordered ? <Loader2 className="size-3.5 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
        {t(ordered ? "shareRent.topup.waiting" : "shareRent.topup.note")}
      </p>
    </div>
  );
}
