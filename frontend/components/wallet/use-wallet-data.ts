"use client";

import * as React from "react";
import { getC2COrders } from "@/lib/market-c2c";
import { notifyMarketBalanceChanged, subscribeMarketBalanceChanged } from "@/lib/market-balance-events";
import {
  getMarketWallet,
  getMarketWalletTopups,
  getMarketWalletTransactions,
  recheckMarketWalletTopup,
  type MarketWallet,
  type MarketWalletPage,
  type MarketWalletTopup,
  type MarketWalletTransaction,
} from "@/lib/market-wallet-api";
import { rememberView } from "@/lib/cached-resource";
import { isOpenTopup } from "@/lib/wallet-view";
export { isOpenTopup };
import { getAwaitingReturns, getPendingTransfers, type AwaitingReturn } from "@/lib/wallet-transfer-returns";
import type { WalletTransfer } from "@/lib/wallet-policy-api";

function mergeById<T extends { id: string }>(first: T[], rest: T[]): T[] {
  const seen = new Set(first.map((item) => item.id));
  return [...first, ...rest.filter((item) => !seen.has(item.id))];
}

/** Clients ask the server to re-check a confirming order at most this often. */
const RECHECK_INTERVAL_MS = 60_000;

export type WalletData = {
  wallet: MarketWallet;
  topups: MarketWalletTopup[];
  topupCursor: string | null;
  transactions: MarketWalletTransaction[];
  transactionCursor: string | null;
  pendingTransfers: WalletTransfer[];
  awaitingReturns: AwaitingReturn[];
  c2cActive: number;
  ready: boolean;
  error: string;
  reload: () => Promise<void>;
  loadMore: (kind: "topups" | "transactions") => Promise<void>;
  replaceTopup: (order: MarketWalletTopup) => void;
  setC2CActive: (count: number) => void;
};

/**
 * One loader for the balance page: balance, recent top-ups and activity, the
 * sender's pending transfers, legacy return requests and active C2C trades.
 * It refreshes on balance-change signals and, while a top-up is open, on a
 * gentle interval and whenever the tab becomes visible.
 */
export function useWalletData(actorId: string, initialWallet: MarketWallet, rememberKey?: string): WalletData {
  const [wallet, setWallet] = React.useState(initialWallet);
  const [topups, setTopups] = React.useState<MarketWalletTopup[]>([]);
  const [topupCursor, setTopupCursor] = React.useState<string | null>(null);
  const [transactions, setTransactions] = React.useState<MarketWalletTransaction[]>([]);
  const [transactionCursor, setTransactionCursor] = React.useState<string | null>(null);
  const [pendingTransfers, setPendingTransfers] = React.useState<WalletTransfer[]>([]);
  const [awaitingReturns, setAwaitingReturns] = React.useState<AwaitingReturn[]>([]);
  const [c2cActive, setC2CActive] = React.useState(0);
  const [ready, setReady] = React.useState(false);
  const [error, setError] = React.useState("");
  const version = React.useRef(0);
  const rechecked = React.useRef(new Map<string, number>());
  // Once older pages were loaded, a refresh only merges the newest page and
  // keeps the cursor where scrolling left it.
  const expanded = React.useRef({ topups: false, transactions: false });
  const lastBalance = React.useRef(`${initialWallet.version}:${initialWallet.availableUnits}`);

  const reload = React.useCallback(async () => {
    const current = ++version.current;
    const [nextWallet, nextTopups, nextTransactions, nextPending, nextReturns, nextC2C] = await Promise.all([
      getMarketWallet(),
      getMarketWalletTopups(),
      getMarketWalletTransactions(),
      // Secondary sections must not block the balance and top-up.
      getPendingTransfers().catch(() => null),
      getAwaitingReturns().catch(() => null),
      getC2COrders({ status: "active" }).then((page) => page.items.length).catch(() => null),
    ]);
    if (current !== version.current) return;
    setWallet(nextWallet);
    setTopups((existing) => mergeById(nextTopups.items, existing));
    if (!expanded.current.topups) setTopupCursor(nextTopups.nextCursor);
    setTransactions((existing) => mergeById(nextTransactions.items, existing));
    if (!expanded.current.transactions) setTransactionCursor(nextTransactions.nextCursor);
    if (nextPending) setPendingTransfers(nextPending);
    if (nextReturns) setAwaitingReturns(nextReturns);
    if (nextC2C != null) setC2CActive(nextC2C);
    setReady(true);
    setError("");
  }, []);

  React.useEffect(() => {
    void reload().catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)));
  }, [reload]);

  React.useEffect(() => {
    if (rememberKey) rememberView(rememberKey, wallet);
    const fingerprint = `${wallet.version}:${wallet.availableUnits}`;
    if (fingerprint !== lastBalance.current) {
      lastBalance.current = fingerprint;
      // Other views (navbar balance) refresh on this signal; never amounts.
      notifyMarketBalanceChanged(actorId);
    }
  }, [actorId, rememberKey, wallet]);

  React.useEffect(() => subscribeMarketBalanceChanged(actorId, () => { void reload().catch(() => {}); }), [actorId, reload]);

  const openTopups = topups.filter(isOpenTopup);
  const hasOpenTopup = openTopups.length > 0;
  // Ask the server to look again at confirming orders when the buyer is here;
  // the server throttles too, this only avoids needless signed requests.
  const recheckConfirming = React.useCallback(() => {
    const now = Date.now();
    for (const order of topups.filter(isOpenTopup)) {
      if (order.buyerView?.state !== "confirming") continue;
      const last = rechecked.current.get(order.id) ?? 0;
      if (now - last < RECHECK_INTERVAL_MS) continue;
      rechecked.current.set(order.id, now);
      void recheckMarketWalletTopup(order.id).catch(() => {});
    }
  }, [topups]);

  React.useEffect(() => {
    if (!hasOpenTopup) return;
    const refresh = () => {
      if (document.visibilityState !== "visible") return;
      recheckConfirming();
      void reload().catch(() => {});
    };
    const timer = window.setInterval(refresh, 30_000);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [hasOpenTopup, recheckConfirming, reload]);

  const loadMore = React.useCallback(async (kind: "topups" | "transactions") => {
    const cursor = kind === "topups" ? topupCursor : transactionCursor;
    if (!cursor) return;
    expanded.current[kind] = true;
    if (kind === "topups") {
      const page: MarketWalletPage<MarketWalletTopup> = await getMarketWalletTopups(cursor);
      setTopups((existing) => mergeById(existing, page.items));
      setTopupCursor(page.nextCursor);
    } else {
      const page: MarketWalletPage<MarketWalletTransaction> = await getMarketWalletTransactions(cursor);
      setTransactions((existing) => mergeById(existing, page.items));
      setTransactionCursor(page.nextCursor);
    }
  }, [topupCursor, transactionCursor]);

  const replaceTopup = React.useCallback((order: MarketWalletTopup) => {
    version.current += 1;
    setTopups((existing) => [order, ...existing.filter((item) => item.id !== order.id)]);
  }, []);

  return {
    wallet, topups, topupCursor, transactions, transactionCursor, pendingTransfers, awaitingReturns,
    c2cActive, ready, error, reload, loadMore, replaceTopup, setC2CActive,
  };
}
