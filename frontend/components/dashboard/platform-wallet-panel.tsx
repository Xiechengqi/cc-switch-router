"use client";

/* Hallmark · component: platform wallet panel · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P4 H5 E4 S5 R5 V4
 */

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Button } from "@heroui/react";
import { ArrowUpRight, Ban, CheckCircle2, ChevronDown, Clock3, Loader2, RefreshCw, WalletCards } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletTransferPanel } from "@/components/dashboard/wallet-transfer-panel";
import { useAuth } from "@/components/auth/auth-provider";
import { C2CBuyPanel } from "@/components/dashboard/c2c-buy-panel";
import { C2COrdersPanel } from "@/components/dashboard/c2c-orders-panel";
import { C2CAction } from "@/components/market/c2c-primitives";
import { subscribeMarketBalanceChanged, notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { MarketBuyerDisputesList } from "@/components/dashboard/market-buyer-disputes-list";
import { WalletTopupForm } from "@/components/market/wallet-topup-form";
import { getTransferCapacity, type TransferCapacity } from "@/lib/wallet-policy-api";
import { ApiError } from "@/lib/api";
import { WALLET_ACTIONS_ID } from "@/lib/dashboard-nav";
import { messages, type MessageKey } from "@/lib/i18n";
import { paymentTokenLabel } from "@/lib/market-payment-tokens";
import {
  cancelMarketWalletTopup,
  getMarketWallet,
  getMarketWalletTopup,
  getMarketWalletTopupGatewayStatus,
  getMarketWalletTopups,
  getMarketWalletTransactions,
  recheckMarketWalletTopup,
  stopMarketWalletTopup,
  type MarketWallet,
  type MarketWalletTopup,
  type MarketTopupGatewayStatus,
  type MarketWalletTransaction,
  type MarketWalletPage,
} from "@/lib/market-wallet-api";
import { formatMinorUsd, formatTopupCountdown, formatWalletUnits, formatWalletUnitsAsCny, safeMarketReturnPath, topupAutoCheckCountdown, topupCancelAction, topupDisplayStatus, verifiedPayableCheckoutUrl, walletTopupCheckoutHref } from "@/lib/market-wallet-money";

export { WalletActionButton } from "@/components/market/wallet-topup-form";

const CLOSED_TOPUP_STATUS = new Set(["credited", "expired", "cancelled"]);
const HISTORY_PREVIEW_LIMIT = 5;
const WALLET_TASKS = ["topup", "c2c", "transfer"] as const;
type WalletTask = (typeof WALLET_TASKS)[number];

function isWalletTask(value: string | null): value is WalletTask {
  return WALLET_TASKS.includes(value as WalletTask);
}

function initialWalletTask(searchParams: { get(name: string): string | null }): WalletTask {
  if (searchParams.get("c2cOrder") || searchParams.get("c2cSell") === "1") return "c2c";
  const requested = searchParams.get("task");
  return isWalletTask(requested) ? requested : "topup";
}

function moneyText(units: string, displayCurrency: string, fxRateFresh: boolean, fxRateMicros: number, locale: string) {
  return displayCurrency === "CNY" && fxRateFresh
    ? formatWalletUnitsAsCny(units, fxRateMicros, locale)
    : formatWalletUnits(units);
}

function signedWalletUnits(raw: string) {
  const text = formatWalletUnits(raw);
  return text.startsWith("−") || text.startsWith("<") ? text : `+${text}`;
}

function mergeHistoryItems<T extends { id: string }>(first: T[], rest: T[]): T[] {
  const seen = new Set(first.map((item) => item.id));
  return [...first, ...rest.filter((item) => !seen.has(item.id))];
}

async function refreshHistoryHead<T extends { id: string }>(
  fetchPage: (cursor?: string) => Promise<MarketWalletPage<T>>,
  current: T[],
): Promise<MarketWalletPage<T>> {
  const known = new Set(current.map((item) => item.id));
  let page = await fetchPage();
  let items = page.items;
  // If more than a page arrived since the last refresh, bridge the gap before
  // joining the new head to the previously loaded history.
  while (page.nextCursor && known.size && !page.items.some((item) => known.has(item.id))) {
    page = await fetchPage(page.nextCursor);
    items = mergeHistoryItems(items, page.items);
  }
  return { items, nextCursor: page.nextCursor };
}

function WalletHistorySection({ title, children, collapsed, hasMore, loading, disabled, onShowEarlier, onLoadMore }: {
  title: string;
  children: React.ReactNode;
  collapsed: boolean;
  hasMore: boolean;
  loading: boolean;
  disabled: boolean;
  onShowEarlier: () => void;
  onLoadMore: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const titleId = React.useId();
  const listRef = React.useRef<HTMLDivElement>(null);
  // Keep the preview height after reveal so older rows scroll inside the card.
  const [lockedHeight, setLockedHeight] = React.useState<number | null>(null);
  const revealEarlier = () => {
    const height = listRef.current?.getBoundingClientRect().height;
    if (height) setLockedHeight(Math.ceil(height));
    onShowEarlier();
  };
  const locked = lockedHeight != null && !collapsed;
  return (
    <section className="min-w-0 rounded-xl border border-border bg-card p-4">
      <h2 id={titleId} className="text-base font-semibold">{title}</h2>
      <div
        ref={listRef}
        role="region" aria-labelledby={titleId} tabIndex={locked ? 0 : undefined}
        className={locked
          ? "mt-2 overflow-y-auto overscroll-y-contain [scrollbar-gutter:stable] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          : "mt-2"}
        style={locked ? { height: lockedHeight } : undefined}
        onScroll={(event) => {
          if (!locked) return;
          const list = event.currentTarget;
          if (hasMore && !disabled && list.scrollHeight - list.scrollTop - list.clientHeight < 96) {
            void onLoadMore();
          }
        }}
      >
        {children}
        {collapsed ? <Button size="sm" variant="ghost" className="mt-1" isDisabled={disabled} onPress={revealEarlier}>
          {t("marketWallet.showEarlier")}
        </Button> : null}
        {!collapsed && hasMore ? <Button size="sm" variant="ghost" className="mt-1" isDisabled={disabled} onPress={() => void onLoadMore()}>
          {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
          {t(loading ? "common.loading" : "marketWallet.more")}
        </Button> : null}
      </div>
    </section>
  );
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

function walletEntryKey(entry: MarketWalletTransaction): MessageKey {
  if (entry.c2cOrderId) {
    // Hold journals use action "release" for returning a reservation. That is
    // not the C2C settlement that moves available balance to the buyer.
    if (entry.action.startsWith("reserve:")) return "marketWallet.entry.c2cHold";
    if (entry.sourceKind === "hold" || entry.action === "release_hold") return "marketWallet.entry.c2cReleaseHold";
    if (entry.action === "release") return safeBigInt(entry.deltaUnits) > 0n ? "marketWallet.entry.c2cReleaseIn" : "marketWallet.entry.c2cReleaseOut";
  }
  if (entry.sourceKind === "wallet_transfer") {
    return safeBigInt(entry.deltaUnits) > 0n ? "marketWallet.entry.transferIn" : "marketWallet.entry.transferOut";
  }
  if (entry.action.startsWith("reserve:")) return "marketWallet.entry.holdPlaced";
  const key = `marketWallet.entry.${entry.sourceKind}`;
  return (key in messages.en ? key : "marketWallet.entry.other") as MessageKey;
}

function safeBigInt(raw: string) {
  try { return BigInt(raw); } catch { return 0n; }
}

function dateText(value: string, locale: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

export function PlatformWalletPanel({ initialWallet }: { initialWallet: MarketWallet }) {
  const { session } = useAuth();
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const searchParams = useSearchParams();
  const returnedTopupId = searchParams.get("topup");
  const [task, setTask] = React.useState<WalletTask>(() => initialWalletTask(searchParams));
  const [c2cOrderId, setC2COrderId] = React.useState<string | null>(searchParams.get("c2cOrder"));
  const [c2cMode, setC2CMode] = React.useState<"buy" | "sell">(searchParams.get("c2cSell") === "1" ? "sell" : "buy");
  const linkedC2COrder = searchParams.get("c2cOrder");
  React.useEffect(() => { if (linkedC2COrder) { setTask("c2c"); setC2COrderId(linkedC2COrder); } }, [linkedC2COrder]);
  const linkedTask = searchParams.get("task");
  React.useEffect(() => { if (isWalletTask(linkedTask)) setTask(linkedTask); }, [linkedTask]);
  const returnedFromCheckoutExit = searchParams.get("cancelled") === "1";
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const [wallet, setWallet] = React.useState(initialWallet);
  const [topups, setTopups] = React.useState<MarketWalletTopup[]>([]);
  const [gatewayStatuses, setGatewayStatuses] = React.useState<Record<string, MarketTopupGatewayStatus | null>>({});
  const [statusBusyId, setStatusBusyId] = React.useState<string | null>(null);
  const [cancelConfirmId, setCancelConfirmId] = React.useState<string | null>(null);
  const [abandonConfirmId, setAbandonConfirmId] = React.useState<string | null>(null);
  const [cancelUnavailableId, setCancelUnavailableId] = React.useState<string | null>(null);
  const [transactions, setTransactions] = React.useState<MarketWalletTransaction[]>([]);
  const [topupCursor, setTopupCursor] = React.useState<string | null>(null);
  const [transactionCursor, setTransactionCursor] = React.useState<string | null>(null);
  const [historyExpanded, setHistoryExpanded] = React.useState({ topups: false, transactions: false });
  const [returnedTopup, setReturnedTopup] = React.useState<MarketWalletTopup | null>(null);
  const [busy, setBusy] = React.useState("");
  const [historyError, setHistoryError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [transferCapacity, setTransferCapacity] = React.useState<TransferCapacity | null>(null);
  const [transferCapacityUnavailable, setTransferCapacityUnavailable] = React.useState(false);
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const [serverOffsetMs, setServerOffsetMs] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  const [historyReady, setHistoryReady] = React.useState(false);
  const busyRef = React.useRef(false);
  // The top-up form owns its own submission; page actions must not overlap it.
  const formBusyRef = React.useRef(false);
  const [formBusy, setFormBusy] = React.useState(false);
  const [suggestedAmount, setSuggestedAmount] = React.useState<string | null>(null);
  const statusRequestVersionRef = React.useRef<Record<string, number>>({});
  const reloadVersionRef = React.useRef(0);
  const historyExpandedRef = React.useRef({ topups: false, transactions: false });
  const historiesRef = React.useRef({ topups, transactions });
  const balanceFingerprint = `${wallet.version}:${wallet.postedUnits}:${wallet.heldUnits}:${wallet.availableUnits}`;
  const lastBalanceFingerprintRef = React.useRef(balanceFingerprint);
  const hasUnresolvedTopups = topups.some((order) =>
    ["created", "creating", "create_unknown", "pending", "verifying", "review_required"].includes(order.status));
  const hasQueuedRecheck = topups.some((order) => order.buyerRecheckResult === "queued");

  React.useEffect(() => {
    const changed = lastBalanceFingerprintRef.current !== balanceFingerprint;
    lastBalanceFingerprintRef.current = balanceFingerprint;
    // Payment reconciliation may credit the wallet while this page is open.
    // Publish only a change signal, never amounts or a second credit request.
    if (changed && session?.user?.id) notifyMarketBalanceChanged(session.user.id);
  }, [balanceFingerprint, session?.user?.id]);

  React.useEffect(() => {
    historiesRef.current = { topups, transactions };
  }, [topups, transactions]);

  React.useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);

  React.useEffect(() => {
    const requested = safeMarketReturnPath(searchParams.get("returnTo"));
    let saved: string | null = null;
    try {
      if (returnedTopupId) {
        const key = `marketWalletReturnTo:${returnedTopupId}`;
        saved = safeMarketReturnPath(window.sessionStorage.getItem(key));
        if (!saved) saved = safeMarketReturnPath(window.localStorage.getItem(key));
      }
    } catch {
      // Storage is optional; the wallet and checkout remain usable without it.
    }
    setReturnPath(requested ?? saved);
    setSuggestedAmount(searchParams.get("amount"));
  }, [returnedTopupId, searchParams]);

  React.useEffect(() => {
    const pendingIds = topups.filter((order) => order.status === "pending").map((order) => order.id);
    if (!pendingIds.length) return;
    let active = true;
    const requests = pendingIds.map((id) => {
      const version = (statusRequestVersionRef.current[id] ?? 0) + 1;
      statusRequestVersionRef.current[id] = version;
      return { id, version };
    });
    void Promise.all(requests.map(async ({ id, version }) => {
      try {
        const status = await getMarketWalletTopupGatewayStatus(id);
        return { id, version, status, offset: Date.parse(status.serverNow) - Date.now() };
      } catch {
        return { id, version, status: null, offset: null };
      }
    })).then((results) => {
      if (!active) return;
      const offset = results.find(({ offset }) => offset != null && Number.isFinite(offset))?.offset;
      if (offset != null) setServerOffsetMs(offset);
      setGatewayStatuses((current) => ({
        ...current,
        ...Object.fromEntries(results
          .filter(({ id, version }) => statusRequestVersionRef.current[id] === version)
          .map(({ id, status }) => [id, status])),
      }));
    });
    return () => { active = false; };
  }, [topups]);

  const refreshTransferCapacity = React.useCallback(async () => {
    try {
      setTransferCapacity(await getTransferCapacity());
      setTransferCapacityUnavailable(false);
    } catch {
      setTransferCapacity(null);
      setTransferCapacityUnavailable(true);
    }
  }, []);

  const reload = React.useCallback(async () => {
    const version = ++reloadVersionRef.current;
    const [nextWallet, nextTopups, nextTransactions] = await Promise.all([
      getMarketWallet(),
      refreshHistoryHead(getMarketWalletTopups, historiesRef.current.topups),
      refreshHistoryHead(getMarketWalletTransactions, historiesRef.current.transactions),
    ]);
    if (version !== reloadVersionRef.current) return;
    setWallet(nextWallet);
    setGatewayStatuses({});
    // Refresh the newest entries without discarding history already loaded by scrolling.
    setTopups((current) => mergeHistoryItems(nextTopups.items, current));
    setTopupCursor((current) => historyExpandedRef.current.topups ? current : nextTopups.nextCursor);
    setTransactions((current) => mergeHistoryItems(nextTransactions.items, current));
    setTransactionCursor((current) => historyExpandedRef.current.transactions ? current : nextTransactions.nextCursor);
    setLoaded(true);
    setHistoryReady(true);
    void refreshTransferCapacity();
  }, [refreshTransferCapacity]);

  React.useEffect(() => session?.user?.id ? subscribeMarketBalanceChanged(session.user.id, () => { void reload().catch(() => setHistoryError(t("earningConversion.refreshFailed"))); }) : undefined, [session?.user?.id, reload, t]);

  React.useEffect(() => {
    if (!hasUnresolvedTopups) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void reload().catch(() => {});
    }, hasQueuedRecheck ? 5_000 : 30_000);
    const onVisible = () => {
      if (document.visibilityState === "visible") void reload().catch(() => {});
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [hasUnresolvedTopups, hasQueuedRecheck, reload]);

  React.useEffect(() => {
    let active = true;
    const version = ++reloadVersionRef.current;
    Promise.all([
      getMarketWallet(),
      getMarketWalletTopups(), getMarketWalletTransactions(),
    ]).then(([nextWallet, nextTopups, nextTransactions]) => {
      if (!active || version !== reloadVersionRef.current) return;
      setWallet(nextWallet);
      setTopups(nextTopups.items);
      setTopupCursor(nextTopups.nextCursor);
      setTransactions(nextTransactions.items);
      setTransactionCursor(nextTransactions.nextCursor);
      setLoaded(true);
      setHistoryReady(true);
      void refreshTransferCapacity();
    }).catch((cause) => {
      if (!active || version !== reloadVersionRef.current) return;
      setHistoryError(errorText(cause));
      setLoaded(true);
    });
    return () => { active = false; };
  }, [refreshTransferCapacity]);

  React.useEffect(() => {
    if (!returnedTopupId) return;
    let active = true;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const retry = (delayMs = 5_000) => {
      if (active) timeout = setTimeout(poll, delayMs);
    };
    const poll = async () => {
      let credited = false;
      try {
        const current = await getMarketWalletTopup(returnedTopupId);
        if (!active) return;
        setReturnedTopup(current);
        credited = current.status === "credited";
        if (CLOSED_TOPUP_STATUS.has(current.status)) {
          try { window.localStorage.removeItem(`marketWalletReturnTo:${returnedTopupId}`); } catch { /* Optional storage. */ }
        }
        if (credited) {
          await reload();
        }
        setHistoryError("");
        if (current.status === "review_required") retry(60_000);
        else if (!CLOSED_TOPUP_STATUS.has(current.status)) retry();
      } catch (cause) {
        if (!active) return;
        setHistoryError(errorText(cause));
        // A temporary API failure (including one during the balance refresh)
        // must not strand a paid order on a stale balance. A rejected order ID
        // is terminal and should not spin forever.
        if (credited || !(cause instanceof ApiError) || cause.status >= 500 || cause.status === 429) retry();
      }
    };
    void poll();
    return () => { active = false; if (timeout) clearTimeout(timeout); };
  }, [returnedTopupId, reload]);

  const handleFormBusy = React.useCallback((next: boolean) => {
    formBusyRef.current = next;
    setFormBusy(next);
  }, []);
  const handleOrderCreated = React.useCallback((order: MarketWalletTopup) => {
    reloadVersionRef.current += 1;
    setTopups((current) => [order, ...current.filter((item) => item.id !== order.id)]);
  }, []);
  const anyBusy = !!busy || formBusy;

  const refreshGatewayStatus = async (id: string) => {
    if (statusBusyId) return;
    const version = (statusRequestVersionRef.current[id] ?? 0) + 1;
    statusRequestVersionRef.current[id] = version;
    setStatusBusyId(id);
    try {
      const status = await getMarketWalletTopupGatewayStatus(id);
      if (statusRequestVersionRef.current[id] === version) {
        setGatewayStatuses((current) => ({ ...current, [id]: status }));
      }
      const offset = Date.parse(status.serverNow) - Date.now();
      if (Number.isFinite(offset)) setServerOffsetMs(offset);
      setHistoryError("");
    } catch {
      if (statusRequestVersionRef.current[id] === version) {
        setGatewayStatuses((current) => ({ ...current, [id]: null }));
      }
      setHistoryError(t("marketWallet.gatewayStatusUnavailable"));
    } finally {
      setStatusBusyId(null);
    }
  };

  const chooseCancelAction = async (order: MarketWalletTopup) => {
    if (statusBusyId || busyRef.current || formBusyRef.current) return;
    const version = (statusRequestVersionRef.current[order.id] ?? 0) + 1;
    statusRequestVersionRef.current[order.id] = version;
    setStatusBusyId(order.id);
    try {
      const status = await getMarketWalletTopupGatewayStatus(order.id);
      if (statusRequestVersionRef.current[order.id] === version) {
        setGatewayStatuses((current) => ({ ...current, [order.id]: status }));
      }
      const offset = Date.parse(status.serverNow) - Date.now();
      if (Number.isFinite(offset)) setServerOffsetMs(offset);
      const action = topupCancelAction(order.status, status, !!order.buyerAbandonedAt);
      if (action === "gateway_cancel") setCancelConfirmId(order.id);
      else if (action === "abandon") setAbandonConfirmId(order.id);
      else setCancelUnavailableId(order.id);
    } catch {
      if (statusRequestVersionRef.current[order.id] === version) {
        setGatewayStatuses((current) => ({ ...current, [order.id]: null }));
      }
      setCancelUnavailableId(order.id);
    } finally {
      setStatusBusyId(null);
    }
  };

  const requestRecheck = async (id: string) => {
    if (statusBusyId || busyRef.current || formBusyRef.current) return;
    setStatusBusyId(id);
    setHistoryError("");
    setNotice("");
    try {
      const result = await recheckMarketWalletTopup(id);
      reloadVersionRef.current += 1;
      setTopups((current) => current.map((order) => order.id === id ? result.order : order));
      if (!result.queued) setNotice(t("marketWallet.recheckAlreadyQueued"));
    } catch (cause) {
      setHistoryError(errorText(cause));
    } finally {
      setStatusBusyId(null);
    }
  };

  const cancelSelectedTopup = async () => {
    const id = cancelConfirmId;
    if (!id || busyRef.current || formBusyRef.current) return;
    busyRef.current = true;
    setBusy("cancelTopup");
    setHistoryError("");
    setGatewayStatuses((current) => ({ ...current, [id]: null }));
    try {
      const updated = await cancelMarketWalletTopup(id);
      setTopups((current) => current.map((order) => order.id === id ? updated : order));
      setCancelConfirmId(null);
      try {
        await reload();
      } catch {
        setHistoryError(t("marketWallet.cancelRefreshFailed"));
      }
    } catch (cause) {
      setCancelConfirmId(null);
      setHistoryError(cause instanceof ApiError && cause.status === 409
        ? t("marketWallet.cancelChanged")
        : errorText(cause));
      await reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const abandonSelectedTopup = async () => {
    const id = abandonConfirmId;
    if (!id || busyRef.current || formBusyRef.current) return;
    busyRef.current = true;
    setBusy("abandonTopup");
    setHistoryError("");
    setNotice("");
    setGatewayStatuses((current) => ({ ...current, [id]: null }));
    try {
      const updated = await stopMarketWalletTopup(id);
      setTopups((current) => current.map((order) => order.id === id ? updated : order));
      setAbandonConfirmId(null);
      setNotice(t("marketWallet.abandonedHelp"));
      try {
        await reload();
      } catch {
        setHistoryError(t("marketWallet.abandonRefreshFailed"));
      }
    } catch (cause) {
      setAbandonConfirmId(null);
      setHistoryError(cause instanceof ApiError && cause.status === 409
        ? t("marketWallet.abandonChanged") : errorText(cause));
      await reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const loadMore = async (kind: "topups" | "transactions") => {
    if (busyRef.current || formBusyRef.current) return;
    const cursor = kind === "topups" ? topupCursor : transactionCursor;
    if (!cursor) return;
    busyRef.current = true;
    setBusy(kind);
    setHistoryError("");
    const version = reloadVersionRef.current;
    try {
      if (kind === "topups") {
        const page = await getMarketWalletTopups(cursor);
        if (version !== reloadVersionRef.current) return;
        historyExpandedRef.current.topups = true;
        setTopups((current) => mergeHistoryItems(current, page.items));
        setTopupCursor(page.nextCursor);
      } else {
        const page = await getMarketWalletTransactions(cursor);
        if (version !== reloadVersionRef.current) return;
        historyExpandedRef.current.transactions = true;
        setTransactions((current) => mergeHistoryItems(current, page.items));
        setTransactionCursor(page.nextCursor);
      }
    } catch (cause) {
      setHistoryError(errorText(cause));
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const cancelUnavailableGateway = cancelUnavailableId ? gatewayStatuses[cancelUnavailableId] : null;
  const cancelUnavailableDescription = ["confirming", "paid"].includes(cancelUnavailableGateway?.gatewayStatus ?? "")
    ? t("marketWallet.cancelPaymentStarted")
    : ["expired", "canceled", "cancelled"].includes(cancelUnavailableGateway?.gatewayStatus ?? "")
      ? t("marketWallet.cancelGatewayEnding")
      : cancelUnavailableGateway?.gatewayStatus === "pending" || cancelUnavailableGateway?.gatewayStatus === "awaiting_payment"
        ? t("marketWallet.cancelSafetyHold")
        : t("marketWallet.gatewayStatusUnavailable");

  const balanceText = (units: string) => moneyText(units, displayCurrency, fxRateFresh, fxRateMicros, locale);
  const topupClosedReason = !wallet.canTopup
    ? t(wallet.mode === "shadow" ? "marketWallet.topupStatus.shadow" : wallet.mode === "enabled" ? "marketWallet.topupStatus.unavailable" : "marketWallet.topupStatus.closed")
    : null;
  const heldUnits = safeBigInt(wallet.heldUnits);
  const owedUnits = safeBigInt(wallet.committedUnits ?? "0");
  const transferableUnits = transferCapacity ? safeBigInt(transferCapacity.transferableUnits) : null;
  const balanceIsClear = heldUnits === 0n && owedUnits === 0n && transferableUnits !== null
    && transferableUnits === safeBigInt(wallet.availableUnits);
  const balanceNote = transferCapacityUnavailable
    ? t("marketWallet.balanceNoteUnavailable")
    : !transferCapacity
      ? t("common.loading")
      : balanceIsClear
        ? t("marketWallet.balanceClear")
        : t("marketWallet.balanceNote", {
            held: balanceText(wallet.heldUnits),
            owed: balanceText(wallet.committedUnits ?? "0"),
            transferable: balanceText(transferCapacity.transferableUnits),
          });

  return (
    <main className="mx-auto grid w-full max-w-5xl min-w-0 gap-6 px-4 py-6 sm:px-6" aria-label={t("account.nav.wallet")}>
      <header className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <p className="max-w-2xl text-sm leading-6 text-muted-foreground">{t("marketWallet.subtitle")}</p>
        <Button variant="outline" size="sm" isDisabled={anyBusy} onPress={() => void reload().catch((cause) => setHistoryError(errorText(cause)))}>
          <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
        </Button>
      </header>

      {notice ? <p role="status" className="rounded-md bg-emerald-50 px-4 py-3 text-sm text-emerald-800">{notice}</p> : null}
      {returnedTopup || returnPath ? (
        <div role="status" aria-live="polite" className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-card px-4 py-3 text-sm">
          <div className="grid gap-1">
            {returnedTopup ? <span className="flex items-center gap-2">
              {returnedTopup.status === "credited" ? <CheckCircle2 className="size-4 text-emerald-700" aria-hidden="true" /> : <Clock3 className="size-4 text-muted-foreground" aria-hidden="true" />}
              {t(`marketWallet.status.${topupDisplayStatus(returnedTopup.status, undefined, !!returnedTopup.buyerAbandonedAt)}` as Parameters<typeof t>[0])}
            </span> : null}
            {returnedFromCheckoutExit && returnedTopup && !CLOSED_TOPUP_STATUS.has(returnedTopup.status) ? <span className="text-xs text-amber-700">{t("marketWallet.checkoutExitIsNotCancellation")}</span> : null}
          </div>
          {returnPath ? (
            <Link href={returnPath} className="inline-flex min-h-11 items-center whitespace-nowrap font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t(returnedTopup?.status === "credited" ? "marketWallet.returnToMarket" : "marketWallet.returnWhilePending")}<ArrowUpRight className="ml-1 size-4" aria-hidden="true" />
            </Link>
          ) : null}
        </div>
      ) : null}

      <section className="min-w-0 rounded-xl border border-border bg-card px-5 py-4" aria-label={t("marketWallet.balanceBreakdown")}>
        <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
          <p className="flex items-center gap-2 text-sm text-muted-foreground"><WalletCards className="size-4" aria-hidden="true" />{t("marketWallet.available")}</p>
          <p className="break-all text-3xl font-semibold tabular-nums tracking-tight">{balanceText(wallet.availableUnits)}</p>
        </div>
        {displayCurrency === "CNY" && fxRateFresh ? <p className="mt-1 text-right text-xs text-muted-foreground">{t("marketWallet.displayOnly", { amount: formatWalletUnits(wallet.availableUnits) })}</p> : null}
        <p className="mt-2 text-sm leading-6 text-muted-foreground">{balanceNote}</p>
      </section>

      <section id={WALLET_ACTIONS_ID} tabIndex={-1} aria-label={t("marketWallet.actions")} className="grid min-w-0 scroll-mt-20 gap-4 rounded-xl border border-border bg-card p-5 outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring sm:p-6">
        <div role="group" aria-label={t("marketWallet.actions")} className="flex flex-wrap gap-2">
          {WALLET_TASKS.map((item) => (
            <C2CAction key={item} type="button" secondary={task !== item} aria-pressed={task === item} disabled={anyBusy} onClick={() => setTask(item)}>
              {t(`marketWallet.action.${item}`)}
              {item === "topup" && topupClosedReason ? <span className="ml-2 text-xs font-medium text-muted-foreground">{topupClosedReason}</span> : null}
            </C2CAction>
          ))}
        </div>
        {task === "transfer" ? <WalletTransferPanel embedded onBusyChange={handleFormBusy} onTransferred={async () => { if (session?.user?.id) notifyMarketBalanceChanged(session.user.id); await reload(); }} walletVersion={wallet.version} /> : null}
        {task === "c2c" && session?.user?.id ? <div className="grid min-w-0 gap-4">
          <div role="group" aria-label={t("c2c.mode")} className="flex flex-wrap gap-2">
            <C2CAction type="button" secondary={c2cMode !== "buy"} aria-pressed={c2cMode === "buy"} disabled={anyBusy} onClick={() => setC2CMode("buy")}>{t("c2c.buy")}</C2CAction>
            <C2CAction type="button" secondary={c2cMode !== "sell"} aria-pressed={c2cMode === "sell"} disabled={anyBusy} onClick={() => setC2CMode("sell")}>{t("c2c.seller")}</C2CAction>
          </div>
          {c2cMode === "buy" ? <C2CBuyPanel key={`${session.user.id}:buy`} actor={session.user.id} onBusyChange={handleFormBusy} onOrder={order => { setC2CMode("buy"); setC2COrderId(order.id); }} /> : null}
          <C2COrdersPanel key={session.user.id} actor={session.user.id} selectedId={c2cOrderId} onSelect={setC2COrderId} embedded sellOpen={c2cMode === "sell"} onSellOpenChange={open => setC2CMode(open ? "sell" : "buy")} />
        </div> : null}
        <div hidden={task !== "topup"}><WalletTopupForm
          wallet={wallet}
          topups={topups}
          ready={historyReady}
          active={task === "topup"}
          returnPath={returnPath}
          suggestedAmount={suggestedAmount}
          externalBusy={!!busy}
          onBusyChange={handleFormBusy}
          onOrderCreated={handleOrderCreated}
          onReload={reload}
        /></div>
      </section>

      {(wallet.canTopup || wallet.hasWalletActivity) && !loaded ? <p className="text-sm text-muted-foreground">{t("common.loading")}</p> : null}
      {historyError ? <p role="alert" className="rounded-md border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">{historyError}</p> : null}
      {wallet.canTopup || wallet.hasWalletActivity ? <div className="grid min-w-0 gap-6 lg:grid-cols-2">
        <WalletHistorySection title={t("marketWallet.topups")} collapsed={!historyExpanded.topups && topups.length > HISTORY_PREVIEW_LIMIT}
          hasMore={!!topupCursor} loading={busy === "topups"} disabled={!!busy}
          onShowEarlier={() => setHistoryExpanded((current) => ({ ...current, topups: true }))} onLoadMore={() => loadMore("topups")}>
          <ul>
            {(historyExpanded.topups ? topups : topups.slice(0, HISTORY_PREVIEW_LIMIT)).map((order) => {
              const gateway = gatewayStatuses[order.id];
              const abandoned = !!order.buyerAbandonedAt;
              const shownStatus = abandoned && order.status === "pending" && (gateway === undefined || gateway === null)
                ? "buyer_abandoned"
                : order.status === "pending" && gateway === undefined
                ? "gateway_checking"
                : order.status === "pending" && gateway === null
                  ? "gateway_unavailable"
                  : order.status === "pending" && gateway && !gateway.canPay
                    && ["awaiting_payment", "pending"].includes(gateway.gatewayStatus)
                    ? "payment_unavailable"
                  : topupDisplayStatus(order.status, gateway?.gatewayStatus, abandoned);
              const deadline = gateway?.gatewayStatus === "awaiting_payment"
                ? gateway.selectionDeadline
                : gateway?.expiresAt;
              const activePayment = !abandoned && order.status === "pending"
                && !["confirming", "paid", "expired", "canceled", "cancelled"].includes(gateway?.gatewayStatus ?? "");
              const countdown = activePayment ? topupAutoCheckCountdown(order.autoCloseAt, nowMs + serverOffsetMs) : null;
              const directOrder = order.orderMode === "direct";
              const checkoutUrl = directOrder
                ? (order.status === "pending" && !abandoned && order.hasPaymentInstructions ? walletTopupCheckoutHref(order.id) : null)
                : verifiedPayableCheckoutUrl(order.status, order.checkoutUrl, gateway, abandoned, nowMs + serverOffsetMs);
              const neverCreated = order.localCloseReason === "never_created";
              const pendingVerification = order.status === "pending" && gateway
                && !gateway.canPay && !gateway.canCancel && !gateway.canAbandon;
              const canStopPayment = !abandoned && ["created", "creating", "create_unknown", "pending", "verifying", "review_required"].includes(order.status);
              const recheckResultIsCurrent = order.buyerRecheckResult === order.status
                || (order.status === "create_unknown" && order.buyerRecheckResult === "pending");
              const lastGatewayLabel = order.lastGatewayStatus === "expired" ? t("marketWallet.status.expired")
                : ["canceled", "cancelled"].includes(order.lastGatewayStatus ?? "") ? t("marketWallet.status.cancelled")
                  : order.lastGatewayStatus === "paid" ? t("marketWallet.status.gateway_paid")
                    : order.lastGatewayStatus === "confirming" ? t("marketWallet.status.verifying")
                      : order.lastGatewayStatus === "pending" ? t("marketWallet.status.gateway_pending")
                        : order.lastGatewayStatus === "awaiting_payment" ? t("marketWallet.status.awaiting_payment")
                          : t("marketWallet.status.gateway_unknown");
              const statusTone = shownStatus === "credited" ? "text-emerald-800"
                : ["review_required", "payment_unavailable", "gateway_unavailable", "buyer_abandoned"].includes(shownStatus) || countdown?.phase === "soon" ? "text-amber-800"
                  : checkoutUrl || shownStatus === "pending" || shownStatus === "awaiting_payment" ? "text-primary"
                    : "text-muted-foreground";
              const showRecheck = order.status === "review_required" || countdown?.phase === "checking" || pendingVerification || (abandoned && order.status === "pending") || order.buyerRecheckResult === "retrying";
              const showStop = canStopPayment && !(order.status === "pending" && gateway?.canCancel);
              const needsAttention = !CLOSED_TOPUP_STATUS.has(order.status);
              return <li key={order.id} className="border-b border-border/70 text-sm last:border-b-0">
                <details className="group" open={needsAttention || undefined}>
                  <summary className="flex min-h-9 cursor-pointer list-none items-center gap-2 py-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
                    <ChevronDown className="size-3.5 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" aria-hidden="true" />
                    <span className="sr-only">{t("marketWallet.topupDetails")}</span>
                    <span className="flex min-w-0 items-center gap-1.5 font-medium tabular-nums">${((order.creditAmountMinor ?? order.amountMinor) / 100).toFixed(2)}
                      {directOrder && order.paymentTokenId ? <span className="rounded-full bg-accent/10 px-1.5 py-px text-[0.7rem] font-medium text-accent">{paymentTokenLabel(order.paymentTokenId).label}</span> : null}
                    </span>
                    <span className={`min-w-0 truncate text-xs font-medium ${statusTone}`}>{neverCreated ? t("marketWallet.status.never_created") : t(`marketWallet.status.${shownStatus}` as Parameters<typeof t>[0])}</span>
                    <span className="ml-auto shrink-0 text-xs text-muted-foreground">{dateText(order.createdAt, locale)}</span>
                  </summary>
                  <div className="grid gap-1 pb-2 pl-5 text-xs text-muted-foreground">
                    {checkoutUrl || showRecheck || showStop || order.status === "pending" ? <div className="flex flex-wrap items-center gap-1 pb-1">
                      {checkoutUrl ? <a className="inline-flex items-center gap-1 whitespace-nowrap text-xs font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" href={walletTopupCheckoutHref(order.id)} target="_blank" rel="noopener noreferrer">
                        {t("marketWallet.continuePay")}<ArrowUpRight className="size-3" aria-hidden="true" />
                      </a> : null}
                      {showRecheck ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => void requestRecheck(order.id)}>
                        <RefreshCw className={`size-4 ${statusBusyId === order.id ? "animate-spin" : ""}`} aria-hidden="true" />{t("marketWallet.recheck")}
                      </Button> : null}
                      {showStop ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => setAbandonConfirmId(order.id)}>
                        {t("marketWallet.abandonTopup")}
                      </Button> : null}
                      {order.status === "pending" ? <Button size="sm" variant="ghost" isIconOnly aria-label={t("marketWallet.refreshGatewayStatus")} isDisabled={statusBusyId === order.id || !!busy} onPress={() => void refreshGatewayStatus(order.id)}>
                        <RefreshCw className={`size-4 ${statusBusyId === order.id ? "animate-spin" : ""}`} aria-hidden="true" />
                      </Button> : null}
                    </div> : null}
                    {order.buyerRecheckResult === "queued" ? <p role="status" className="font-medium text-primary">{t("marketWallet.recheckQueued")}</p>
                      : order.buyerRecheckResult === "retrying" ? <p role="status" className="text-amber-700">{t("marketWallet.recheckRetrying")}</p>
                        : order.buyerRecheckCompletedAt && order.buyerRecheckResult && recheckResultIsCurrent ? <p role="status">{t(`marketWallet.recheckResult.${order.buyerRecheckResult}` as Parameters<typeof t>[0], { time: dateText(order.buyerRecheckCompletedAt, locale) })}</p> : null}
                    {order.platformFeeMinor > 0 ? <p>{t("walletPolicy.orderBreakdown", { fee: formatMinorUsd(order.platformFeeMinor), total: formatMinorUsd(order.amountMinor) })}</p> : null}
                    {neverCreated ? <p>{t("marketWallet.neverCreatedHelp")}</p> : null}
                    {order.paidAt ? <p>{t("marketWallet.paidAt", { time: dateText(order.paidAt, locale) })}</p> : null}
                    {deadline && order.status === "pending" && !abandoned ? <p>{t("marketWallet.gatewayDeadline", { time: dateText(deadline, locale) })}</p> : null}
                    {countdown ? <p className={`font-medium tabular-nums ${countdown.phase === "soon" ? "text-amber-700" : ""}`}>
                      {countdown.phase === "checking"
                        ? t("marketWallet.autoCheckPending")
                        : t("marketWallet.autoCheckCountdown", { time: formatTopupCountdown(countdown.remainingSeconds) })}
                    </p> : null}
                    {countdown?.phase === "soon" ? <p className="text-amber-700">{t("marketWallet.autoCheckSoon")}</p> : null}
                    {gateway?.gatewayStatus === "awaiting_payment" && gateway.canPay && order.status === "pending" && !abandoned ? <p>{t("marketWallet.awaitingMethodHelp")}</p> : null}
                    {order.status === "pending" && gateway && !gateway.canPay && !abandoned ? <p className="text-amber-700">{t(`marketWallet.payability.${gateway.payabilityReason ?? "gateway_unknown"}` as Parameters<typeof t>[0])}</p> : null}
                    {abandoned && !CLOSED_TOPUP_STATUS.has(order.status) ? <p>{t("marketWallet.abandonedHelp")}</p> : null}
                    {order.status === "review_required" ? <p className="text-amber-700">{t(order.reviewReason === "terminal_payment_marker" ? "marketWallet.reviewTerminalMarker" : order.reviewReason === "paid_receipt_invalid" ? "marketWallet.reviewInvalidReceipt" : order.reviewReason === "paid_terms_review" ? "marketWallet.reviewPaidTerms" : "marketWallet.reviewHelp")}</p> : null}
                    {order.lastGatewayCheckedAt ? <p>{t("marketWallet.lastGatewayCheck", { time: dateText(order.lastGatewayCheckedAt, locale), status: lastGatewayLabel })}</p> : null}
                    {order.status === "pending" && gateway?.canCancel ? <div className="pt-1">
                      <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => void chooseCancelAction(order)}>
                        <Ban className="size-4" aria-hidden="true" />{t("marketWallet.cancelPayment")}
                      </Button>
                    </div> : null}
                  </div>
                </details>
              </li>;
            })}
            {loaded && !topups.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTopups")}</li> : null}
          </ul>
        </WalletHistorySection>
        <WalletHistorySection title={t("marketWallet.transactions")} collapsed={!historyExpanded.transactions && transactions.length > HISTORY_PREVIEW_LIMIT}
          hasMore={!!transactionCursor} loading={busy === "transactions"} disabled={!!busy}
          onShowEarlier={() => setHistoryExpanded((current) => ({ ...current, transactions: true }))} onLoadMore={() => loadMore("transactions")}>
          <ul>
            {(historyExpanded.transactions ? transactions : transactions.slice(0, HISTORY_PREVIEW_LIMIT)).map((entry) => {
              const delta = safeBigInt(entry.deltaUnits);
              const heldDelta = safeBigInt(entry.heldDeltaUnits);
              // A hold or release moves money between available and held without
              // changing the posted balance; show that movement instead of $0.00.
              const holdOnly = delta === 0n && heldDelta !== 0n;
              const entryKey = walletEntryKey(entry);
              const entryLabel = t(entryKey, entry.counterpartyEmail ? { email: entry.counterpartyEmail } : undefined);
              const amountTone = holdOnly ? "text-muted-foreground" : delta > 0n ? "text-emerald-800" : delta < 0n ? "text-foreground" : "text-muted-foreground";
              const counterparty = entry.counterpartyEmail && !entryKey.startsWith("marketWallet.entry.transfer") ? entry.counterpartyEmail : null;
              const holdNote = holdOnly ? t(heldDelta > 0n ? "marketWallet.entryHeld" : "marketWallet.entryReleased") : null;
              const entryMain = <>
                {entry.c2cOrderId ? <button type="button" onClick={() => { setTask("c2c"); setC2COrderId(entry.c2cOrderId ?? null); }} className="min-w-0 truncate text-left font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">{entryLabel}</button> : <span className="min-w-0 truncate font-medium">{entryLabel}</span>}
                <span className="ml-auto shrink-0 text-xs text-muted-foreground">{dateText(entry.createdAt, locale)}</span>
                {holdOnly ? <span className="shrink-0 tabular-nums text-muted-foreground">{formatWalletUnits((heldDelta < 0n ? -heldDelta : heldDelta).toString())}</span>
                  : <span className={`shrink-0 tabular-nums ${amountTone}`}>{signedWalletUnits(entry.deltaUnits)}</span>}
              </>;
              return <li key={entry.id} className="border-b border-border/70 text-sm last:border-b-0">
                {counterparty || holdNote ? <details className="group">
                  <summary className="flex min-h-9 cursor-pointer list-none items-center gap-2 py-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
                    <ChevronDown className="size-3.5 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" aria-hidden="true" />
                    <span className="sr-only">{t("marketWallet.entryDetails")}</span>
                    {entryMain}
                  </summary>
                  <div className="grid gap-1 pb-2 pl-5 text-xs text-muted-foreground">
                    {counterparty ? <p className="break-all">{counterparty}</p> : null}
                    {holdNote ? <p>{holdNote}</p> : null}
                  </div>
                </details> : <div className="flex min-h-9 items-center gap-2 py-1 pl-5">{entryMain}</div>}
              </li>;
            })}
            {loaded && !transactions.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTransactions")}</li> : null}
          </ul>
        </WalletHistorySection>
      </div> : null}
      {wallet.hasWalletActivity ? <MarketBuyerDisputesList /> : null}
      <ConfirmAlertDialog
        open={!!cancelConfirmId}
        title={t("marketWallet.cancelPaymentTitle")}
        description={t("marketWallet.cancelPaymentNote")}
        confirmLabel={t("marketWallet.cancelPayment")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "cancelTopup"}
        onConfirm={() => void cancelSelectedTopup()}
        onOpenChange={(open) => { if (busy !== "cancelTopup" && !open) setCancelConfirmId(null); }}
      />
      <ConfirmAlertDialog
        open={!!abandonConfirmId}
        title={t("marketWallet.abandonTopupTitle")}
        description={t("marketWallet.abandonTopupNote")}
        confirmLabel={t("marketWallet.abandonTopup")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "abandonTopup"}
        onConfirm={() => void abandonSelectedTopup()}
        onOpenChange={(open) => { if (busy !== "abandonTopup" && !open) setAbandonConfirmId(null); }}
      />
      <ConfirmAlertDialog
        open={!!cancelUnavailableId}
        title={t("marketWallet.cancelUnavailableTitle")}
        description={cancelUnavailableDescription}
        confirmLabel={t("marketWallet.recheck")}
        cancelLabel={t("common.close")}
        tone="warning"
        busy={!!statusBusyId}
        onConfirm={() => {
          const id = cancelUnavailableId;
          setCancelUnavailableId(null);
          if (id) void requestRecheck(id);
        }}
        onOpenChange={(open) => { if (!open) setCancelUnavailableId(null); }}
      />
    </main>
  );
}
