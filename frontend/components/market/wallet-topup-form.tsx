"use client";

/* Hallmark · component: wallet top-up form · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * Shared by the balance page and the navbar balance dialog so payment-tab,
 * idempotency and quote rules exist exactly once.
 */

import * as React from "react";
import { Button } from "@heroui/react";
import { CheckCircle2, CreditCard, Loader2 } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CompactSelect } from "@/components/common/compact-select";
import { InfoHint } from "@/components/common/info-hint";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletBadge } from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { paymentTokenLabel } from "@/lib/market-payment-tokens";
import {
  createMarketWalletTopup,
  getMarketWalletChannels,
  type MarketWallet,
  type MarketWalletChannel,
  type MarketWalletTopup,
} from "@/lib/market-wallet-api";
import {
  TOPUP_PRESET_MINOR,
  formatMinorUsd,
  parseTopupMinor,
  parseUsdMinor,
  shouldResetTopupIdempotency,
  topupBounds,
  walletTopupCheckoutHref,
} from "@/lib/market-wallet-money";
import { topupMethodOptions } from "@/lib/topup-methods";
import { quoteWalletTopup, type TopupQuote } from "@/lib/wallet-policy-api";

const CLOSED_TOPUP_STATUS = new Set(["credited", "expired", "cancelled"]);

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Localized message for a failed top-up creation. */
function topupErrorKey(cause: unknown): MessageKey | null {
  if (!(cause instanceof ApiError)) return null;
  switch (cause.code) {
    case "MARKET_TOPUP_PENDING_LIMIT": return "marketWallet.pendingLimit";
    case "MARKET_TOPUP_ABANDONED_LIMIT": return "marketWallet.abandonedLimit";
    case "MARKET_TOPUP_REVIEW_LIMIT": return "marketWallet.reviewLimit";
    case "MARKET_TOPUP_CHECKOUT_UNAVAILABLE": return "marketWallet.checkoutUnavailable";
    case "MARKET_TOPUP_PAUSED": return "marketWallet.error.topupPaused";
    case "DUJIAOPAY_RATE_LIMITED": return "marketWallet.error.gatewayRateLimited";
    case "DUJIAOPAY_NO_CAPACITY": return "marketWallet.error.gatewayNoCapacity";
    case "DUJIAOPAY_UNAVAILABLE": return "marketWallet.error.gatewayUnavailable";
    case "DUJIAOPAY_REJECTED": return "marketWallet.error.gatewayRejected";
    case "DUJIAOPAY_AUTH_FAILED": return "marketWallet.error.gatewayAuthFailed";
    default: return null;
  }
}

function closePaymentWindow(paymentWindow: Window | null) {
  try { paymentWindow?.close(); } catch { /* The payment tab may have navigated away. */ }
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

export function WalletTopupForm({
  wallet,
  topups,
  ready,
  returnPath = null,
  suggestedAmount = null,
  externalBusy = false,
  active = true,
  idPrefix = "market-wallet-topup",
  onBusyChange,
  onOrderCreated,
  onReload,
}: {
  wallet: MarketWallet;
  /** Newest top-up orders, used to avoid paying twice for an unsettled amount. */
  topups: MarketWalletTopup[];
  /** False until `topups` reflects the server; submitting waits for it. */
  ready: boolean;
  returnPath?: string | null;
  suggestedAmount?: string | null;
  externalBusy?: boolean;
  /** Keep the draft/idempotency key mounted while another wallet tab is shown. */
  active?: boolean;
  idPrefix?: string;
  onBusyChange?: (busy: boolean) => void;
  onOrderCreated: (order: MarketWalletTopup) => void;
  onReload: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const [channels, setChannels] = React.useState<MarketWalletChannel[]>([]);
  const [channelId, setChannelId] = React.useState("");
  const [tokenId, setTokenId] = React.useState("");
  const [amount, setAmount] = React.useState("");
  const [amountTouched, setAmountTouched] = React.useState(false);
  const [quote, setQuote] = React.useState<{ fingerprint: string; value: TopupQuote } | null>(null);
  const [quoteError, setQuoteError] = React.useState("");
  const [quoteRefresh, setQuoteRefresh] = React.useState(0);
  const [busy, setBusy] = React.useState(false);
  const [topupError, setTopupError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [checkoutFallback, setCheckoutFallback] = React.useState<{ orderId: string; url: string } | null>(null);
  const [reviewRetryConfirm, setReviewRetryConfirm] = React.useState(false);
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const idempotencyRef = React.useRef<{ fingerprint: string; key: string; quoteId: string; orderId?: string } | null>(null);
  const busyRef = React.useRef(false);
  const channelReloadVersionRef = React.useRef(0);
  const mountedRef = React.useRef(true);
  const paymentWindowRef = React.useRef<Window | null>(null);
  const onBusyChangeRef = React.useRef(onBusyChange);
  onBusyChangeRef.current = onBusyChange;
  const amountId = `${idPrefix}-amount`;
  const amountHelpId = `${idPrefix}-amount-help`;
  const disabled = busy || externalBusy;
  React.useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      channelReloadVersionRef.current++;
      closePaymentWindow(paymentWindowRef.current);
      onBusyChangeRef.current?.(false);
    };
  }, []);

  const methodOptions = React.useMemo(() => topupMethodOptions(channels), [channels]);
  const selectedChannel = methodOptions.find((option) => option.channel.id === channelId)?.channel ?? null;
  const directChannel = selectedChannel?.orderMode === "direct";
  const channelMethods = React.useMemo(() => directChannel ? selectedChannel?.methods ?? [] : [], [directChannel, selectedChannel]);
  const bounds = topupBounds(selectedChannel);
  const parsedAmount = bounds ? parseTopupMinor(amount, bounds) : null;
  const amountError = !!bounds && amountTouched && amount && parsedAmount == null;
  const methodReady = !directChannel || channelMethods.some((method) => method.tokenId === tokenId);
  const quoteFingerprint = `${channelId}:${parsedAmount}:${directChannel ? tokenId : ""}:${selectedChannel?.policyVersion}:${selectedChannel?.maintenanceFeeBps}:${selectedChannel?.channelRevision}:${selectedChannel?.channelSettingsVersion}:${bounds?.min}:${bounds?.max}`;
  const activeQuote = quote?.fingerprint === quoteFingerprint && Date.parse(quote.value.expiresAt) > nowMs ? quote.value : null;
  const quoteExpired = quote?.fingerprint === quoteFingerprint && Date.parse(quote.value.expiresAt) <= nowMs;

  React.useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [active]);

  React.useEffect(() => {
    if (suggestedAmount && parseUsdMinor(suggestedAmount) != null) {
      setAmount((current) => current || suggestedAmount);
    }
  }, [suggestedAmount]);

  React.useEffect(() => {
    const attemptedOrderId = idempotencyRef.current?.orderId;
    if (attemptedOrderId && topups.some((order) =>
      order.id === attemptedOrderId && shouldResetTopupIdempotency(order))) {
      idempotencyRef.current = null;
    }
    // A fallback link to an order that was since closed or abandoned would
    // only lead the buyer to a payment page that refuses to pay.
    setCheckoutFallback((current) => current && topups.some((order) => order.id === current.orderId
      && (CLOSED_TOPUP_STATUS.has(order.status) || !!order.buyerAbandonedAt)) ? null : current);
  }, [topups]);

  const refreshChannels = React.useCallback(async () => {
    const version = ++channelReloadVersionRef.current;
    const nextChannels = await getMarketWalletChannels();
    if (version !== channelReloadVersionRef.current) return;
    const options = topupMethodOptions(nextChannels);
    setChannels(nextChannels);
    setChannelId((current) => options.some((option) => option.channel.id === current) ? current : options[0]?.channel.id || "");
  }, []);

  React.useEffect(() => {
    if (!wallet.canTopup || !active) return;
    void refreshChannels().catch((cause) => setQuoteError(errorText(cause)));
    // Limits can change even with no pending order. Revalidate when returning
    // from another tab and at a bounded cadence while the form is visible.
    const refresh = () => { if (document.visibilityState === "visible") void refreshChannels().catch(() => {}); };
    const timer = window.setInterval(refresh, 30_000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
      channelReloadVersionRef.current++;
    };
  }, [active, wallet.canTopup, refreshChannels]);

  React.useEffect(() => {
    let current = true;
    setQuoteError("");
    if (!active || !wallet.canTopup || !channelId || parsedAmount == null || !methodReady) { setQuote(null); return; }
    const timer = window.setTimeout(() => {
      void quoteWalletTopup(channelId, parsedAmount, directChannel ? tokenId : undefined)
        .then(async (value) => {
          if (!current) return;
          if (value.policyVersion !== selectedChannel?.policyVersion
            || value.feeBps !== selectedChannel.maintenanceFeeBps
            || value.channelRevision !== selectedChannel.channelRevision
            || value.channelSettingsVersion !== selectedChannel.channelSettingsVersion) {
            setQuote(null);
            setQuoteError(t("walletPolicy.quoteExpired"));
            await refreshChannels();
            return;
          }
          setQuote({ fingerprint: quoteFingerprint, value });
        })
        .catch((cause) => { if (current) {
          setQuote(null);
          const range = cause instanceof ApiError ? topupBounds(cause.details) : null;
          setQuoteError(cause instanceof ApiError && cause.code === "MARKET_TOPUP_AMOUNT_OUT_OF_RANGE"
            ? range ? t("marketWallet.error.amountOutOfRange", { min: formatMinorUsd(range.min), max: formatMinorUsd(range.max) }) : t("marketWallet.amountRangeUnavailable")
            : errorText(cause));
          if (cause instanceof ApiError && ["MARKET_TOPUP_AMOUNT_OUT_OF_RANGE", "MARKET_TOPUP_QUOTE_CHANGED"].includes(cause.code ?? "")) void refreshChannels().catch(() => {});
        } });
    }, 450);
    return () => { current = false; window.clearTimeout(timer); };
  // The fingerprint captures the channel's monetary configuration, without
  // re-quoting just because polling returned an equivalent channel object.
  }, [active, channelId, parsedAmount, directChannel, tokenId, methodReady, quoteFingerprint, wallet.canTopup, quoteRefresh, quoteExpired, refreshChannels, t]);

  React.useEffect(() => {
    // A payment method belongs to one channel; a single method is preselected,
    // several require an explicit choice because they settle on different chains.
    setTokenId((current) => channelMethods.some((method) => method.tokenId === current)
      ? current
      : channelMethods.length === 1 ? channelMethods[0].tokenId : "");
  }, [channelMethods]);

  const setBusyState = (next: boolean) => {
    busyRef.current = next;
    setBusy(next);
    onBusyChangeRef.current?.(next);
  };

  const matchingUnsettled = topups.find((order) =>
    (order.creditAmountMinor ?? order.amountMinor) === parsedAmount
      && (["verifying", "review_required"].includes(order.status)
        || (!!order.buyerAbandonedAt && !CLOSED_TOPUP_STATUS.has(order.status))));

  const startTopup = async () => {
    if (!active || !ready || busyRef.current || externalBusy || !wallet.canTopup || !channelId || parsedAmount == null || !methodReady || !activeQuote) return;
    const attemptedOrderId = idempotencyRef.current?.orderId;
    if (attemptedOrderId && topups.some((order) => order.id === attemptedOrderId
      && shouldResetTopupIdempotency(order))) {
      idempotencyRef.current = null;
    }
    setReviewRetryConfirm(false);
    setBusyState(true);
    setTopupError("");
    setNotice("");
    setCheckoutFallback(null);
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
    const fingerprint = `${quoteFingerprint}:${activeQuote.policyVersion}:${activeQuote.channelRevision}:${activeQuote.channelSettingsVersion}:${activeQuote.feeMinor}`;
    if (idempotencyRef.current?.fingerprint !== fingerprint) {
      idempotencyRef.current = { fingerprint, key: crypto.randomUUID(), quoteId: activeQuote.id };
    }
    const attempt = idempotencyRef.current;
    paymentWindowRef.current = paymentWindow;
    try {
      const order = await createMarketWalletTopup(channelId, parsedAmount, attempt.key, attempt.quoteId, directChannel ? tokenId : undefined);
      if (!mountedRef.current) { closePaymentWindow(paymentWindow); return; }
      if (idempotencyRef.current === attempt) attempt.orderId = order.id;
      if (shouldResetTopupIdempotency(order)) idempotencyRef.current = null;
      onOrderCreated(order);
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
      if (order.buyerAbandonedAt) {
        closePaymentWindow(paymentWindow);
        idempotencyRef.current = null;
        setNotice(t("marketWallet.abandonedHelp"));
      } else if (order.status === "credited") {
        closePaymentWindow(paymentWindow);
        void onReload().catch(() => {});
      } else {
        // The same-origin checkout page owns the one fresh gateway preflight.
        // Do not close a newly opened tab merely because payment is unavailable:
        // it must show the buyer why the existing order cannot be paid.
        const localCheckout = walletTopupCheckoutHref(order.id);
        if (!paymentWindow || paymentWindow.closed) {
          setCheckoutFallback({ orderId: order.id, url: localCheckout });
        } else {
          try {
            paymentWindow.location.replace(localCheckout);
            setNotice(t("marketWallet.openedPaymentTab"));
          } catch {
            closePaymentWindow(paymentWindow);
            setCheckoutFallback({ orderId: order.id, url: localCheckout });
          }
        }
      }
    } catch (cause) {
      closePaymentWindow(paymentWindow);
      if (!mountedRef.current) return;
      if (cause instanceof ApiError && cause.code === "MARKET_TOPUP_QUOTE_CHANGED") {
        idempotencyRef.current = null;
        setQuote(null);
        setQuoteRefresh((value) => value + 1);
        setTopupError(t("walletPolicy.quoteExpired"));
        void refreshChannels().catch(() => {});
        return;
      }
      const range = cause instanceof ApiError ? topupBounds(cause.details) : null;
      const key = topupErrorKey(cause);
      setTopupError(cause instanceof ApiError && cause.code === "MARKET_TOPUP_AMOUNT_OUT_OF_RANGE"
        ? range ? t("marketWallet.error.amountOutOfRange", {
          min: formatMinorUsd(range.min), max: formatMinorUsd(range.max),
        }) : t("marketWallet.amountRangeUnavailable")
        : key ? t(key) : errorText(cause));
      void onReload().catch(() => {});
    } finally {
      paymentWindowRef.current = null;
      if (mountedRef.current) setBusyState(false);
    }
  };

  const submitTopup = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setAmountTouched(true);
    if (busyRef.current || externalBusy || !ready || !wallet.canTopup || !channelId || parsedAmount == null || !methodReady || !activeQuote) return;
    if (matchingUnsettled) setReviewRetryConfirm(true);
    else void startTopup();
  };

  if (!wallet.canTopup) {
    return <div className="grid content-start gap-3 rounded-lg bg-muted/50 p-4 text-sm text-muted-foreground">
      <p>{t(wallet.mode === "shadow" ? "marketWallet.shadow" : wallet.mode === "enabled" ? "marketWallet.topupUnavailable" : "marketWallet.notOpen")}</p>
    </div>;
  }

  return (
    <form className="grid min-w-0 content-start gap-3" data-topup-channel={channelId} onSubmit={submitTopup}>
      {wallet.mode === "shadow" ? <p className="text-sm text-muted-foreground">{t("marketWallet.shadow")}</p> : null}
      <div className="grid min-w-0 gap-1 text-sm">
        <span className="flex flex-wrap items-center gap-2">{t("marketWallet.topupMethod")}
          {selectedChannel?.canary ? <><WalletBadge tone="warning" pulse>{t("marketWallet.canaryBadge")}</WalletBadge><InfoHint>{t("marketWallet.canaryHelp")}</InfoHint></> : null}
        </span>
        <CompactSelect
          ariaLabel={t("marketWallet.topupMethod")} className="w-full" triggerClassName="h-11"
          value={channelId} onChange={(value) => { setChannelId(value); idempotencyRef.current = null; }}
          disabled={!methodOptions.length || disabled}
          options={methodOptions.length
            ? methodOptions.map((option) => ({ value: option.channel.id, label: t(`marketWallet.topupMethod.${option.method}`) }))
            : [{ value: "", label: t("marketWallet.noTopupMethod") }]}
        />
      </div>
      <fieldset className="grid min-w-0 gap-2 text-sm" aria-describedby={amountHelpId}>
        <legend className="mb-1">{t("marketWallet.amount")}</legend>
        <div className="grid grid-cols-3 gap-2 sm:grid-cols-5">
          {TOPUP_PRESET_MINOR.map((minor) => {
            const allowed = !!bounds && minor >= bounds.min && minor <= bounds.max;
            const pressed = parsedAmount === minor;
            return <button
              key={minor} type="button" aria-pressed={pressed}
              disabled={!allowed || disabled}
              onClick={() => { setAmount((minor / 100).toFixed(2)); setAmountTouched(true); idempotencyRef.current = null; }}
              className={`min-h-11 rounded-md border text-sm font-semibold tabular-nums focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:cursor-not-allowed disabled:opacity-40 ${pressed
                ? "border-primary bg-primary text-primary-foreground"
                : "border-border bg-background hover:bg-muted"}`}
            >
              ${minor / 100}
            </button>;
          })}
        </div>
        <label className="grid min-w-0 gap-1">
          <span className="text-xs text-muted-foreground">{t("marketWallet.customAmount")}</span>
          <span className="relative grid">
            <span className="pointer-events-none absolute inset-y-0 left-3 flex items-center text-muted-foreground" aria-hidden="true">$</span>
            <input
              id={amountId}
              aria-label={t("marketWallet.customAmount")}
              inputMode="decimal" autoComplete="off" value={amount}
              onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }}
              onBlur={() => setAmountTouched(true)}
              aria-invalid={!!amountError} aria-describedby={amountHelpId}
              className="h-11 min-w-0 rounded-md border border-input bg-background pl-7 pr-3 tabular-nums outline-2 outline-transparent focus-visible:outline-ring aria-invalid:border-destructive disabled:cursor-not-allowed disabled:opacity-50"
              placeholder="10.00" disabled={!bounds || disabled}
            />
          </span>
        </label>
        <span className="flex min-h-[1lh] items-start gap-1">
          <span id={amountHelpId} className={`text-xs ${amountError ? "text-destructive" : "text-muted-foreground"}`}>
            {!bounds ? t(selectedChannel ? "marketWallet.amountRangeUnavailable" : "marketWallet.noChannelHelp") : amountError
              ? t("marketWallet.amountRangeError", { min: formatMinorUsd(bounds.min), max: formatMinorUsd(bounds.max) })
              : t("marketWallet.amountRangeHelp", { min: formatMinorUsd(bounds.min), max: formatMinorUsd(bounds.max) })}
          </span>
          {bounds && !amountError ? <InfoHint>{t("marketWallet.amountRangeDetail")}</InfoHint> : null}
        </span>
      </fieldset>
      {directChannel ? <fieldset className="grid min-w-0 gap-2 text-sm">
        <legend className="mb-1 flex items-center gap-1">{t("marketWallet.paymentMethod")}<InfoHint>{t("marketWallet.directMethodHelp")}</InfoHint></legend>
        {channelMethods.length ? <div role="radiogroup" aria-label={t("marketWallet.paymentMethod")} className="grid gap-2 sm:grid-cols-2">
          {channelMethods.map((method) => (
            <label key={method.tokenId} className="flex min-h-11 cursor-pointer items-center gap-3 rounded-md border border-border bg-background px-3 py-2 hover:bg-muted has-[:checked]:border-primary has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-ring has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-50">
              <input
                type="radio" name={`${idPrefix}-method`} value={method.tokenId}
                checked={tokenId === method.tokenId} disabled={disabled}
                onChange={() => { setTokenId(method.tokenId); idempotencyRef.current = null; }}
                className="size-4 accent-primary"
              />
              <span className="grid min-w-0">
                <span className="font-semibold">{method.symbol || paymentTokenLabel(method.tokenId).symbol}</span>
                <span className="truncate text-xs text-muted-foreground">{method.chainName || paymentTokenLabel(method.tokenId).chainName}</span>
              </span>
            </label>
          ))}
        </div> : <p className="text-xs text-muted-foreground">{t("marketWallet.noPaymentMethods")}</p>}
      </fieldset> : null}
      {parsedAmount != null && methodReady ? <dl aria-live="polite" className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-1.5 rounded-xl bg-muted/60 p-4 text-sm tabular-nums">
        <dt className="text-slate-600">{t("walletPolicy.credit")}</dt><dd className="text-right">{activeQuote ? formatMinorUsd(activeQuote.creditMinor) : t("marketWallet.quotingTotal")}</dd>
        <dt className="text-slate-600">{t("walletPolicy.fee", { rate: (activeQuote?.feeBps ?? selectedChannel?.maintenanceFeeBps ?? 0) / 100 })}</dt><dd className="text-right">{activeQuote ? formatMinorUsd(activeQuote.feeMinor) : t("marketWallet.quotingTotal")}</dd>
        <dt className="mt-1.5 border-t border-border pt-2 font-semibold">{t("walletPolicy.total")}</dt><dd className="mt-1.5 border-t border-border pt-2 text-right text-lg font-semibold">{activeQuote ? formatMinorUsd(activeQuote.payMinor) : t("marketWallet.quotingTotal")}</dd>
      </dl> : null}
      {quoteError ? <div className="grid justify-items-start gap-2"><p role="alert" className="text-sm text-destructive">{quoteError}</p><Button variant="outline" onPress={() => setQuoteRefresh((value) => value + 1)}>{t("common.retry")}</Button></div> : null}
      <p className="flex items-center gap-1 text-xs text-muted-foreground">{t("walletPolicy.disclosureShort")}<InfoHint>{t("walletPolicy.disclosure")}</InfoHint></p>
      <WalletActionButton
        label={busy ? t("marketWallet.creating") : activeQuote ? t("walletPolicy.pay", { amount: formatMinorUsd(activeQuote.payMinor) }) : t("marketWallet.pay")}
        phase={busy ? "loading" : topupError ? "error" : "default"}
        disabled={!ready || !methodOptions.length || parsedAmount == null || !methodReady || !activeQuote || disabled}
      />
      {topupError ? <p role="alert" className="text-sm text-destructive">{topupError}</p> : null}
      {notice ? <p role="status" className="rounded-md bg-emerald-50 px-3 py-2 text-sm text-emerald-800">{notice}</p> : null}
      {checkoutFallback ? <p role="status" className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900">
        {t("marketWallet.paymentTabBlocked")} <a href={checkoutFallback.url} target="_blank" rel="noopener noreferrer" className="font-semibold underline underline-offset-2">{t("marketWallet.openPaymentTab")}</a>
      </p> : null}
      {matchingUnsettled ? <p className="text-xs text-amber-700">{t("marketWallet.reviewRetryNotice")}</p> : null}
      {!methodOptions.length ? <p className="text-xs text-muted-foreground">{t("marketWallet.noChannelHelp")}</p> : null}
      <ConfirmAlertDialog
        open={reviewRetryConfirm}
        title={t("marketWallet.reviewRetryTitle")}
        description={t("marketWallet.reviewRetryWarning")}
        confirmLabel={t("marketWallet.startSeparateTopup")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy}
        onConfirm={() => void startTopup()}
        onOpenChange={(open) => { if (!busy) setReviewRetryConfirm(open); }}
      />
    </form>
  );
}
