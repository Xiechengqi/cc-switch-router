"use client";

/* Hallmark · component: platform wallet panel · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P4 H5 E4 S5 R5 V4
 */

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Button } from "@heroui/react";
import { ArrowUpRight, Ban, CheckCircle2, Clock3, CreditCard, Loader2, RefreshCw, WalletCards } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { MarketWalletRefundPanel } from "@/components/dashboard/market-wallet-refund-panel";
import { ApiError } from "@/lib/api";
import { DASHBOARD_ACCOUNT_BILLING_PATH } from "@/lib/dashboard-nav";
import {
  createMarketWalletTopup,
  cancelMarketWalletTopup,
  getMarketWallet,
  getMarketWalletChannels,
  getMarketWalletTopup,
  getMarketWalletTopupGatewayStatus,
  getMarketWalletTopups,
  getMarketWalletTransactions,
  recheckMarketWalletTopup,
  stopMarketWalletTopup,
  type MarketWalletChannel,
  type MarketWallet,
  type MarketWalletTopup,
  type MarketTopupGatewayStatus,
  type MarketWalletTransaction,
} from "@/lib/market-wallet-api";
import { formatTopupCountdown, formatWalletUnits, formatWalletUnitsAsCny, parseUsdMinor, payableCheckoutUrl, safeMarketReturnPath, shouldResetTopupIdempotency, topupAutoCheckCountdown, topupCancelAction, topupDisplayStatus, verifiedPayableCheckoutUrl, walletTopupCheckoutHref } from "@/lib/market-wallet-money";

const CLOSED_TOPUP_STATUS = new Set(["credited", "expired", "cancelled"]);

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

function closePaymentWindow(paymentWindow: Window | null) {
  try { paymentWindow?.close(); } catch { /* The payment tab may have navigated away. */ }
}

function dateText(value: string, locale: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

export function WalletActionButton({
  label,
  phase = "default",
  disabled = false,
  previewState,
}: {
  label: string;
  phase?: "default" | "loading" | "error" | "success";
  disabled?: boolean;
  previewState?: "hover" | "focus" | "active";
}) {
  return (
    <button
      type="submit"
      disabled={disabled || phase === "loading"}
      data-phase={phase}
      data-preview={previewState}
      className="inline-flex h-11 min-w-0 items-center justify-center gap-2 whitespace-nowrap rounded-md bg-primary px-4 text-sm font-semibold text-primary-foreground transition-transform duration-100 hover:bg-primary/90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:cursor-not-allowed disabled:opacity-50 data-[phase=error]:bg-destructive data-[preview=hover]:bg-primary/90 data-[preview=focus]:outline-2 data-[preview=focus]:outline-offset-2 data-[preview=focus]:outline-ring data-[preview=active]:translate-y-px motion-reduce:transition-none"
    >
      {phase === "loading" ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        : phase === "success" ? <CheckCircle2 className="size-4" aria-hidden="true" />
          : <CreditCard className="size-4" aria-hidden="true" />}
      {label}
    </button>
  );
}

export function PlatformWalletPanel({ initialWallet }: { initialWallet: MarketWallet }) {
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const searchParams = useSearchParams();
  const returnedTopupId = searchParams.get("topup");
  const returnedFromCheckoutExit = searchParams.get("cancelled") === "1";
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const [wallet, setWallet] = React.useState(initialWallet);
  const [channels, setChannels] = React.useState<MarketWalletChannel[]>([]);
  const [topups, setTopups] = React.useState<MarketWalletTopup[]>([]);
  const [gatewayStatuses, setGatewayStatuses] = React.useState<Record<string, MarketTopupGatewayStatus | null>>({});
  const [statusBusyId, setStatusBusyId] = React.useState<string | null>(null);
  const [cancelConfirmId, setCancelConfirmId] = React.useState<string | null>(null);
  const [abandonConfirmId, setAbandonConfirmId] = React.useState<string | null>(null);
  const [cancelUnavailableId, setCancelUnavailableId] = React.useState<string | null>(null);
  const [reviewRetryConfirm, setReviewRetryConfirm] = React.useState(false);
  const [checkoutFallbackUrl, setCheckoutFallbackUrl] = React.useState<string | null>(null);
  const [transactions, setTransactions] = React.useState<MarketWalletTransaction[]>([]);
  const [topupCursor, setTopupCursor] = React.useState<string | null>(null);
  const [transactionCursor, setTransactionCursor] = React.useState<string | null>(null);
  const [returnedTopup, setReturnedTopup] = React.useState<MarketWalletTopup | null>(null);
  const [channelId, setChannelId] = React.useState("");
  const [amount, setAmount] = React.useState("");
  const [amountTouched, setAmountTouched] = React.useState(false);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const [serverOffsetMs, setServerOffsetMs] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  const idempotencyRef = React.useRef<{ fingerprint: string; key: string; orderId?: string } | null>(null);
  const busyRef = React.useRef(false);
  const statusRequestVersionRef = React.useRef<Record<string, number>>({});
  const reloadVersionRef = React.useRef(0);
  const hasUnresolvedTopups = topups.some((order) =>
    ["created", "creating", "create_unknown", "pending", "verifying", "review_required"].includes(order.status));
  const hasQueuedRecheck = topups.some((order) => order.buyerRecheckResult === "queued");

  React.useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);

  React.useEffect(() => {
    const attemptedOrderId = idempotencyRef.current?.orderId;
    if (attemptedOrderId && topups.some((order) =>
      order.id === attemptedOrderId && shouldResetTopupIdempotency(order))) {
      idempotencyRef.current = null;
    }
  }, [topups]);

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
    const suggested = searchParams.get("amount");
    if (suggested && parseUsdMinor(suggested) != null) {
      setAmount((current) => current || suggested);
    }
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

  const reload = React.useCallback(async () => {
    const version = ++reloadVersionRef.current;
    const [nextWallet, nextChannels, nextTopups, nextTransactions] = await Promise.all([
      getMarketWallet(), getMarketWalletChannels(),
      getMarketWalletTopups(), getMarketWalletTransactions(),
    ]);
    if (version !== reloadVersionRef.current) return;
    setWallet(nextWallet);
    setChannels(nextChannels);
    setGatewayStatuses({});
    setTopups(nextTopups.items);
    setTopupCursor(nextTopups.nextCursor);
    setTransactions(nextTransactions.items);
    setTransactionCursor(nextTransactions.nextCursor);
    setChannelId((current) => current || nextChannels[0]?.id || "");
    setLoaded(true);
  }, []);

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
      getMarketWallet(), getMarketWalletChannels(),
      getMarketWalletTopups(), getMarketWalletTransactions(),
    ]).then(([nextWallet, nextChannels, nextTopups, nextTransactions]) => {
      if (!active || version !== reloadVersionRef.current) return;
      setWallet(nextWallet);
      setChannels(nextChannels);
      setTopups(nextTopups.items);
      setTopupCursor(nextTopups.nextCursor);
      setTransactions(nextTransactions.items);
      setTransactionCursor(nextTransactions.nextCursor);
      setChannelId((current) => current || nextChannels[0]?.id || "");
      setLoaded(true);
    }).catch((cause) => {
      if (!active || version !== reloadVersionRef.current) return;
      setError(errorText(cause));
      setLoaded(true);
    });
    return () => { active = false; };
  }, []);

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
        setError("");
        if (current.status === "review_required") retry(60_000);
        else if (!CLOSED_TOPUP_STATUS.has(current.status)) retry();
      } catch (cause) {
        if (!active) return;
        setError(errorText(cause));
        // A temporary API failure (including one during the balance refresh)
        // must not strand a paid order on a stale balance. A rejected order ID
        // is terminal and should not spin forever.
        if (credited || !(cause instanceof ApiError) || cause.status >= 500 || cause.status === 429) retry();
      }
    };
    void poll();
    return () => { active = false; if (timeout) clearTimeout(timeout); };
  }, [returnedTopupId, reload]);

  const parsedAmount = parseUsdMinor(amount);
  const amountError = amountTouched && amount && parsedAmount == null;
  const matchingUnsettled = topups.find((order) =>
    order.amountMinor === parsedAmount
      && (["verifying", "review_required"].includes(order.status)
        || (!!order.buyerAbandonedAt && !CLOSED_TOPUP_STATUS.has(order.status))));
  const startTopup = async () => {
    if (busyRef.current || !wallet.canTopup || !channelId || parsedAmount == null) return;
    const attemptedOrderId = idempotencyRef.current?.orderId;
    if (attemptedOrderId && topups.some((order) => order.id === attemptedOrderId
      && shouldResetTopupIdempotency(order))) {
      idempotencyRef.current = null;
    }
    setReviewRetryConfirm(false);
    busyRef.current = true;
    setBusy("topup");
    setError("");
    setNotice("");
    setCheckoutFallbackUrl(null);
    let paymentWindow: Window | null = null;
    try {
      // Open during the user gesture; opening after the awaited API call is
      // commonly blocked by browsers. No third-party page gets an opener.
      paymentWindow = window.open("about:blank", "_blank");
      if (paymentWindow) {
        paymentWindow.opener = null;
        paymentWindow.document.title = t("marketWallet.creating");
        paymentWindow.document.body.textContent = t("marketWallet.creating");
      }
    } catch {
      try { paymentWindow?.close(); } catch { /* The manual payment link remains available. */ }
      paymentWindow = null;
    }
    const fingerprint = `${channelId}:${parsedAmount}`;
    if (idempotencyRef.current?.fingerprint !== fingerprint) {
      idempotencyRef.current = { fingerprint, key: crypto.randomUUID() };
    }
    const attempt = idempotencyRef.current;
    try {
      const order = await createMarketWalletTopup(channelId, parsedAmount, attempt.key);
      if (idempotencyRef.current === attempt) attempt.orderId = order.id;
      if (shouldResetTopupIdempotency(order)) idempotencyRef.current = null;
      reloadVersionRef.current += 1;
      setTopups((current) => [order, ...current.filter((item) => item.id !== order.id)]);
      if (returnPath) {
        const key = `marketWalletReturnTo:${order.id}`;
        try {
          window.sessionStorage.setItem(key, returnPath);
        } catch {
          // A blocked storage API must not block payment.
        }
        try {
          // A noopener fallback tab cannot read this tab's sessionStorage.
          window.localStorage.setItem(key, returnPath);
        } catch {
          // Return navigation is optional.
        }
        try {
          // A new tab has its own sessionStorage. Its initial copy is made
          // before the API returns, so write the per-order return path there.
          paymentWindow?.sessionStorage.setItem(key, returnPath);
        } catch {
          // A blocked storage API must not block payment.
        }
      }
      const checkoutUrl = payableCheckoutUrl(order.status, order.checkoutUrl, undefined, !!order.buyerAbandonedAt);
      if (checkoutUrl) {
        try {
          const gateway = await getMarketWalletTopupGatewayStatus(order.id);
          setGatewayStatuses((current) => ({ ...current, [order.id]: gateway }));
          const offset = Date.parse(gateway.serverNow) - Date.now();
          if (Number.isFinite(offset)) setServerOffsetMs(offset);
          const verifiedCheckout = verifiedPayableCheckoutUrl(order.status, checkoutUrl, gateway, !!order.buyerAbandonedAt, Date.now() + (Number.isFinite(offset) ? offset : 0));
          if (!verifiedCheckout) {
            closePaymentWindow(paymentWindow);
            setNotice(t("marketWallet.paymentNoLongerOpen"));
            void reload().catch(() => {});
            return;
          }
          const localCheckout = walletTopupCheckoutHref(order.id);
          if (!paymentWindow || paymentWindow.closed) {
            setCheckoutFallbackUrl(localCheckout);
          } else {
            try {
              paymentWindow.location.replace(localCheckout);
              setNotice(t("marketWallet.openedPaymentTab"));
            } catch {
              closePaymentWindow(paymentWindow);
              setCheckoutFallbackUrl(localCheckout);
            }
          }
        } catch {
          closePaymentWindow(paymentWindow);
          setError(t("marketWallet.checkoutPending"));
          void reload().catch(() => {});
        }
      } else if (order.buyerAbandonedAt) {
        closePaymentWindow(paymentWindow);
        idempotencyRef.current = null;
        setNotice(t("marketWallet.abandonedHelp"));
      } else if (order.status !== "credited") {
        closePaymentWindow(paymentWindow);
        setError(t(order.status === "review_required" ? "marketWallet.reviewHelp" : "marketWallet.checkoutPending"));
      } else {
        closePaymentWindow(paymentWindow);
      }
    } catch (cause) {
      closePaymentWindow(paymentWindow);
      setError(cause instanceof ApiError && cause.code === "MARKET_TOPUP_PENDING_LIMIT"
        ? t("marketWallet.pendingLimit")
        : cause instanceof ApiError && cause.code === "MARKET_TOPUP_ABANDONED_LIMIT"
          ? t("marketWallet.abandonedLimit")
        : cause instanceof ApiError && cause.code === "MARKET_TOPUP_REVIEW_LIMIT"
          ? t("marketWallet.reviewLimit")
        : cause instanceof ApiError && cause.code === "MARKET_TOPUP_CHECKOUT_UNAVAILABLE"
          ? t("marketWallet.checkoutUnavailable")
          : errorText(cause));
      void reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const submitTopup = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setAmountTouched(true);
    if (busyRef.current || !loaded || !wallet.canTopup || !channelId || parsedAmount == null) return;
    if (matchingUnsettled) setReviewRetryConfirm(true);
    else void startTopup();
  };

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
      setError("");
    } catch {
      if (statusRequestVersionRef.current[id] === version) {
        setGatewayStatuses((current) => ({ ...current, [id]: null }));
      }
      setError(t("marketWallet.gatewayStatusUnavailable"));
    } finally {
      setStatusBusyId(null);
    }
  };

  const chooseCancelAction = async (order: MarketWalletTopup) => {
    if (statusBusyId || busyRef.current) return;
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
    if (statusBusyId || busyRef.current) return;
    setStatusBusyId(id);
    setError("");
    setNotice("");
    try {
      const result = await recheckMarketWalletTopup(id);
      reloadVersionRef.current += 1;
      setTopups((current) => current.map((order) => order.id === id ? result.order : order));
      if (!result.queued) setNotice(t("marketWallet.recheckAlreadyQueued"));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setStatusBusyId(null);
    }
  };

  const cancelSelectedTopup = async () => {
    const id = cancelConfirmId;
    if (!id || busyRef.current) return;
    busyRef.current = true;
    setBusy("cancelTopup");
    if (idempotencyRef.current?.orderId === id) idempotencyRef.current = null;
    setError("");
    setGatewayStatuses((current) => ({ ...current, [id]: null }));
    setCheckoutFallbackUrl(null);
    try {
      const updated = await cancelMarketWalletTopup(id);
      setTopups((current) => current.map((order) => order.id === id ? updated : order));
      setCancelConfirmId(null);
      try {
        await reload();
      } catch {
        setError(t("marketWallet.cancelRefreshFailed"));
      }
    } catch (cause) {
      setCancelConfirmId(null);
      setError(cause instanceof ApiError && cause.status === 409
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
    if (!id || busyRef.current) return;
    busyRef.current = true;
    setBusy("abandonTopup");
    if (idempotencyRef.current?.orderId === id) idempotencyRef.current = null;
    setError("");
    setNotice("");
    setGatewayStatuses((current) => ({ ...current, [id]: null }));
    setCheckoutFallbackUrl(null);
    try {
      const updated = await stopMarketWalletTopup(id);
      setTopups((current) => current.map((order) => order.id === id ? updated : order));
      setAbandonConfirmId(null);
      setNotice(t("marketWallet.abandonedHelp"));
      try {
        await reload();
      } catch {
        setError(t("marketWallet.abandonRefreshFailed"));
      }
    } catch (cause) {
      setAbandonConfirmId(null);
      setError(cause instanceof ApiError && cause.status === 409
        ? t("marketWallet.abandonChanged") : errorText(cause));
      await reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const loadMore = async (kind: "topups" | "transactions") => {
    if (busyRef.current) return;
    const cursor = kind === "topups" ? topupCursor : transactionCursor;
    if (!cursor) return;
    busyRef.current = true;
    setBusy(kind);
    setError("");
    try {
      if (kind === "topups") {
        const page = await getMarketWalletTopups(cursor);
        setTopups((current) => [...current, ...page.items]);
        setTopupCursor(page.nextCursor);
      } else {
        const page = await getMarketWalletTransactions(cursor);
        setTransactions((current) => [...current, ...page.items]);
        setTransactionCursor(page.nextCursor);
      }
    } catch (cause) {
      setError(errorText(cause));
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

  return (
    <main className="mx-auto grid w-full max-w-5xl min-w-0 gap-6 px-4 py-6 sm:px-6" aria-label={t("marketWallet.title")}>
      <header className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="break-words text-2xl font-semibold tracking-tight">{t("marketWallet.title")}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t("marketWallet.subtitle")}</p>
        </div>
        <Button variant="outline" size="sm" isDisabled={!!busy} onPress={() => void reload().catch((cause) => setError(errorText(cause)))}>
          <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
        </Button>
      </header>

      {error ? <p role="alert" className="rounded-md border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">{error}</p> : null}
      {notice ? <p role="status" className="rounded-md bg-emerald-50 px-4 py-3 text-sm text-emerald-800">{notice}</p> : null}
      {checkoutFallbackUrl ? <p role="status" className="rounded-md bg-amber-50 px-4 py-3 text-sm text-amber-900">
        {t("marketWallet.paymentTabBlocked")} <a href={checkoutFallbackUrl} target="_blank" rel="noopener noreferrer" className="font-semibold underline underline-offset-2">{t("marketWallet.openPaymentTab")}</a>
      </p> : null}
      {wallet.mode === "shadow" ? <p className="rounded-md border border-amber-300 bg-amber-50 px-4 py-3 text-sm text-amber-900">{t("marketWallet.shadow")}</p> : null}
      {returnedTopup ? (
        <div role="status" aria-live="polite" className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-card px-4 py-3 text-sm">
          <div className="grid gap-1">
            <span className="flex items-center gap-2">
              {returnedTopup.status === "credited" ? <CheckCircle2 className="size-4 text-emerald-700" aria-hidden="true" /> : <Clock3 className="size-4 text-muted-foreground" aria-hidden="true" />}
              {t(`marketWallet.status.${topupDisplayStatus(returnedTopup.status, undefined, !!returnedTopup.buyerAbandonedAt)}` as Parameters<typeof t>[0])}
            </span>
            {returnedFromCheckoutExit && !CLOSED_TOPUP_STATUS.has(returnedTopup.status) ? <span className="text-xs text-amber-700">{t("marketWallet.checkoutExitIsNotCancellation")}</span> : null}
          </div>
          {returnedTopup.status === "credited" && returnPath ? (
            <Link href={returnPath} className="inline-flex min-h-11 items-center whitespace-nowrap font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t("marketWallet.returnToMarket")}<ArrowUpRight className="ml-1 size-4" aria-hidden="true" />
            </Link>
          ) : null}
        </div>
      ) : null}

      <section className="grid min-w-0 gap-5 rounded-xl border border-border bg-card p-5 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)] sm:p-6">
        <div className="min-w-0">
          <p className="flex items-center gap-2 text-sm text-muted-foreground"><WalletCards className="size-4" aria-hidden="true" />{t("marketWallet.available")}</p>
          <p className="mt-2 break-all text-3xl font-semibold tabular-nums tracking-tight sm:text-4xl">{displayCurrency === "CNY" && fxRateFresh ? formatWalletUnitsAsCny(wallet.availableUnits, fxRateMicros, locale) : formatWalletUnits(wallet.availableUnits)}</p>
          {displayCurrency === "CNY" && fxRateFresh ? <p className="mt-1 text-xs text-muted-foreground">{t("marketWallet.displayOnly", { amount: formatWalletUnits(wallet.availableUnits) })}</p> : null}
          <p className="mt-2 text-sm text-muted-foreground">{t("marketWallet.held")}: {displayCurrency === "CNY" && fxRateFresh ? formatWalletUnitsAsCny(wallet.heldUnits, fxRateMicros, locale) : formatWalletUnits(wallet.heldUnits)}</p>
        </div>
        {wallet.canTopup ? <form className="grid min-w-0 content-start gap-3" onSubmit={(event) => void submitTopup(event)}>
          <p className="text-sm font-semibold">{t("marketWallet.topup")}</p>
          <label className="grid min-w-0 gap-1 text-sm">
            <span>{t("marketWallet.channel")}</span>
            <select
              className="h-11 min-w-0 rounded-md border border-input bg-background px-3 outline-2 outline-transparent focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50"
              value={channelId} onChange={(event) => { setChannelId(event.target.value); idempotencyRef.current = null; }}
              disabled={!wallet.canTopup || !channels.length || !!busy}
            >
              {!channels.length ? <option value="">{t("marketWallet.noChannel")}</option> : null}
              {channels.map((channel) => <option key={channel.id} value={channel.id}>{channel.displayName}</option>)}
            </select>
          </label>
          <label className="grid min-w-0 gap-1 text-sm">
            <span>{t("marketWallet.amount")}</span>
            <input
              id="market-wallet-topup-amount"
              inputMode="decimal" autoComplete="off" value={amount}
              onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }}
              onBlur={() => setAmountTouched(true)}
              aria-invalid={!!amountError} aria-describedby="wallet-amount-help"
              className="h-11 min-w-0 rounded-md border border-input bg-background px-3 outline-2 outline-transparent focus-visible:outline-ring aria-invalid:border-destructive disabled:cursor-not-allowed disabled:opacity-50"
              placeholder="10.00" disabled={!wallet.canTopup || !!busy}
            />
            <span id="wallet-amount-help" className={`min-h-[1lh] text-xs ${amountError ? "text-destructive" : "text-muted-foreground"}`}>
              {amountError ? t("marketWallet.amountError") : t("marketWallet.amountHelp")}
            </span>
          </label>
          <WalletActionButton
            label={busy === "topup" ? t("marketWallet.creating") : t("marketWallet.pay")}
            phase={busy === "topup" ? "loading" : error ? "error" : "default"}
            disabled={!loaded || !wallet.canTopup || !channels.length || parsedAmount == null || !!busy}
          />
          {matchingUnsettled ? <p className="text-xs text-amber-700">{t("marketWallet.reviewRetryNotice")}</p> : null}
          {!channels.length ? <p className="text-xs text-muted-foreground">{t("marketWallet.noChannelHelp")}</p> : null}
        </form> : <div className="grid content-start gap-3 rounded-lg bg-muted/50 p-4 text-sm text-muted-foreground">
          <p>{t(wallet.mode === "enabled" ? "marketWallet.topupUnavailable" : "marketWallet.notOpen")}</p>
          {wallet.mode !== "enabled" ? <Link href={DASHBOARD_ACCOUNT_BILLING_PATH} className="font-medium text-foreground underline underline-offset-2">{t("marketWallet.legacyBilling")}</Link> : null}
        </div>}
      </section>

      {(wallet.canTopup || wallet.hasWalletActivity) && !loaded ? <p className="text-sm text-muted-foreground">{t("common.loading")}</p> : null}
      {wallet.canTopup || wallet.hasWalletActivity ? <div className="grid min-w-0 gap-6 lg:grid-cols-2">
        <section className="min-w-0 rounded-xl border border-border bg-card p-5" aria-label={t("marketWallet.topups")}>
          <h2 className="text-base font-semibold">{t("marketWallet.topups")}</h2>
          <ul className="mt-3 grid gap-2">
            {topups.map((order) => {
              const gateway = gatewayStatuses[order.id];
              const abandoned = !!order.buyerAbandonedAt;
              const shownStatus = abandoned && order.status === "pending" && (gateway === undefined || gateway === null)
                ? "buyer_abandoned"
                : order.status === "pending" && gateway === undefined
                ? "gateway_checking"
                : order.status === "pending" && gateway === null
                  ? "gateway_unavailable"
                  : topupDisplayStatus(order.status, gateway?.gatewayStatus, abandoned);
              const deadline = gateway?.gatewayStatus === "awaiting_payment"
                ? gateway.selectionDeadline
                : gateway?.expiresAt;
              const activePayment = !abandoned && order.status === "pending"
                && !["confirming", "paid", "expired", "canceled", "cancelled"].includes(gateway?.gatewayStatus ?? "");
              const countdown = activePayment ? topupAutoCheckCountdown(order.autoCloseAt, nowMs + serverOffsetMs) : null;
              const checkoutUrl = verifiedPayableCheckoutUrl(order.status, order.checkoutUrl, gateway, abandoned, nowMs + serverOffsetMs);
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
              return <li key={order.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-md bg-muted/50 px-3 py-2 text-sm">
                <div className="min-w-0">
                  <p className="font-medium tabular-nums">${(order.amountMinor / 100).toFixed(2)}</p>
                  <p className="text-xs text-muted-foreground">{dateText(order.createdAt, locale)}</p>
                  {deadline && order.status === "pending" && !abandoned ? <p className="text-xs text-muted-foreground">{t("marketWallet.gatewayDeadline", { time: dateText(deadline, locale) })}</p> : null}
                  {countdown ? <p className={`mt-1 text-xs font-medium tabular-nums ${countdown.phase === "soon" ? "text-amber-700" : "text-muted-foreground"}`}>
                    {countdown.phase === "checking"
                      ? t("marketWallet.autoCheckPending")
                      : t("marketWallet.autoCheckCountdown", { time: formatTopupCountdown(countdown.remainingSeconds) })}
                  </p> : null}
                  {countdown?.phase === "soon" ? <p className="text-xs text-amber-700">{t("marketWallet.autoCheckSoon")}</p> : null}
                  {gateway?.gatewayStatus === "awaiting_payment" && order.status === "pending" && !abandoned ? <p className="text-xs text-muted-foreground">{t("marketWallet.awaitingMethodHelp")}</p> : null}
                  {pendingVerification && !abandoned ? <p className="text-xs text-amber-700">{t("marketWallet.pendingVerificationHelp")}</p> : null}
                  {abandoned && !CLOSED_TOPUP_STATUS.has(order.status) ? <p className="text-xs text-muted-foreground">{t("marketWallet.abandonedHelp")}</p> : null}
                  {order.status === "review_required" ? <p className="mt-1 text-xs text-amber-700">{t(order.reviewReason === "terminal_payment_marker" ? "marketWallet.reviewTerminalMarker" : order.reviewReason === "paid_receipt_invalid" ? "marketWallet.reviewInvalidReceipt" : "marketWallet.reviewHelp")}</p> : null}
                  {order.buyerRecheckResult === "queued" ? <p role="status" className="mt-1 text-xs font-medium text-primary">{t("marketWallet.recheckQueued")}</p>
                    : order.buyerRecheckResult === "retrying" ? <p role="status" className="mt-1 text-xs text-amber-700">{t("marketWallet.recheckRetrying")}</p>
                      : order.buyerRecheckCompletedAt && order.buyerRecheckResult && recheckResultIsCurrent ? <p role="status" className="mt-1 text-xs text-muted-foreground">{t(`marketWallet.recheckResult.${order.buyerRecheckResult}` as Parameters<typeof t>[0], { time: dateText(order.buyerRecheckCompletedAt, locale) })}</p> : null}
                  {order.lastGatewayCheckedAt ? <p className="text-xs text-muted-foreground">{t("marketWallet.lastGatewayCheck", { time: dateText(order.lastGatewayCheckedAt, locale), status: lastGatewayLabel })}</p> : null}
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs text-muted-foreground">{t(`marketWallet.status.${shownStatus}` as Parameters<typeof t>[0])}</span>
                  {order.status === "pending" ? <Button size="sm" variant="ghost" isIconOnly aria-label={t("marketWallet.refreshGatewayStatus")} isDisabled={statusBusyId === order.id || !!busy} onPress={() => void refreshGatewayStatus(order.id)}>
                    <RefreshCw className={`size-4 ${statusBusyId === order.id ? "animate-spin" : ""}`} aria-hidden="true" />
                  </Button> : null}
                  {order.status === "pending" && gateway?.canCancel ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => void chooseCancelAction(order)}>
                    <Ban className="size-4" aria-hidden="true" />{t("marketWallet.cancelPayment")}
                  </Button> : null}
                  {canStopPayment && !(order.status === "pending" && gateway?.canCancel) ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => setAbandonConfirmId(order.id)}>
                    {t("marketWallet.abandonTopup")}
                  </Button> : null}
                  {(order.status === "review_required" || countdown?.phase === "checking" || pendingVerification || (abandoned && order.status === "pending") || order.buyerRecheckResult === "retrying") ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => void requestRecheck(order.id)}>
                    <RefreshCw className={`size-4 ${statusBusyId === order.id ? "animate-spin" : ""}`} aria-hidden="true" />{t("marketWallet.recheck")}
                  </Button> : null}
                  {checkoutUrl ? <a className="inline-flex min-h-11 items-center gap-1 whitespace-nowrap text-xs font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" href={walletTopupCheckoutHref(order.id)} target="_blank" rel="noopener noreferrer">
                    {t("marketWallet.continuePay")}<ArrowUpRight className="size-3" aria-hidden="true" />
                  </a> : null}
                  {wallet.canTopup && ["expired", "cancelled"].includes(order.status) ? <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => {
                    idempotencyRef.current = null;
                    setAmount((order.amountMinor / 100).toFixed(2));
                    setAmountTouched(false);
                    if (channels.some((channel) => channel.id === order.channelId)) setChannelId(order.channelId);
                    document.getElementById("market-wallet-topup-amount")?.focus();
                  }}>
                    {t("marketWallet.restartTopup")}
                  </Button> : null}
                </div>
              </li>;
            })}
            {loaded && !topups.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTopups")}</li> : null}
          </ul>
          {topupCursor ? <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => void loadMore("topups")}>{t("marketWallet.more")}</Button> : null}
        </section>
        <section className="min-w-0 rounded-xl border border-border bg-card p-5" aria-label={t("marketWallet.transactions")}>
          <h2 className="text-base font-semibold">{t("marketWallet.transactions")}</h2>
          <ul className="mt-3 grid gap-2">
            {transactions.map((entry) => (
              <li key={entry.id} className="flex min-w-0 items-center justify-between gap-2 rounded-md bg-muted/50 px-3 py-2 text-sm">
                <div className="min-w-0">
                  <p className="truncate font-medium">{t(`marketWallet.entry.${entry.sourceKind}` as Parameters<typeof t>[0])}</p>
                  <p className="text-xs text-muted-foreground">{dateText(entry.createdAt, locale)}</p>
                </div>
                <span className="shrink-0 tabular-nums">{formatWalletUnits(entry.deltaUnits)}</span>
              </li>
            ))}
            {loaded && !transactions.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTransactions")}</li> : null}
          </ul>
          {transactionCursor ? <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => void loadMore("transactions")}>{t("marketWallet.more")}</Button> : null}
        </section>
      </div> : null}
      {wallet.hasWalletActivity ? <MarketWalletRefundPanel availableUnits={wallet.availableUnits} onSubmitted={reload} /> : null}
      <ConfirmAlertDialog
        open={reviewRetryConfirm}
        title={t("marketWallet.reviewRetryTitle")}
        description={t("marketWallet.reviewRetryWarning")}
        confirmLabel={t("marketWallet.startSeparateTopup")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "topup"}
        onConfirm={() => void startTopup()}
        onOpenChange={(open) => { if (busy !== "topup") setReviewRetryConfirm(open); }}
      />
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
