"use client";

/* Hallmark · component: wallet checkout (cashier redirect + in-site direct payment)
 * genre: modern-minimal · theme: existing Router system
 * states: checking · payable · confirming · unavailable · closed · credited · rate-limited · error
 */

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { AlertTriangle, ArrowLeft, CheckCircle2, Clock3, Loader2, XCircle } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { QrCodeSvg } from "@/components/market/qr-code-svg";
import {
  CopyTextButton,
  WALLET_PRIMARY_BUTTON,
  WALLET_SECONDARY_BUTTON,
  WalletBadge,
  WalletSectionLabel,
} from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import {
  getMarketWalletTopup,
  getMarketWalletTopupGatewayStatus,
  getMarketWalletTopupPaymentInstructions,
  type MarketTopupGatewayStatus,
  type MarketTopupPaymentInstructions,
  type MarketWalletTopup,
} from "@/lib/market-wallet-api";
import { formatTopupCountdown, verifiedPayableCheckoutUrl } from "@/lib/market-wallet-money";

type Translate = ReturnType<typeof useLocaleText>["t"];

const CLOSED_STATUS = new Set(["credited", "expired", "cancelled"]);
const PAYABILITY_KEYS = new Set([
  "buyer_stopped", "cancel_pending", "order_changed", "gateway_closed", "payment_processing",
  "gateway_unknown", "payment_evidence", "deadline_passed", "checkout_unavailable",
]);
const INSTRUCTIONS_INTERVAL_MS = 5_000;
const MAX_BACKOFF_MS = 30_000;
/** Refresh the order record on every Nth instructions poll (~15 s). */
const ORDER_REFRESH_EVERY = 3;

function walletHref(topupId: string | null) {
  return topupId ? `/account/wallet/?topup=${encodeURIComponent(topupId)}` : "/account/wallet/";
}

function payabilityText(t: Translate, reason: string | null | undefined) {
  const key = reason === "payable" ? "checkout_unavailable" : reason && PAYABILITY_KEYS.has(reason) ? reason : "gateway_unknown";
  return t(`marketWallet.payability.${key}` as Parameters<Translate>[0]);
}

function usdText(minor: number, locale: string) {
  return new Intl.NumberFormat(locale, { style: "currency", currency: "USD" }).format(minor / 100);
}

function retryAfterMs(cause: ApiError): number | null {
  const details = cause.details ?? {};
  for (const key of ["retryAfterSeconds", "retryAfter", "retry_after"]) {
    const value = Number(details[key]);
    if (Number.isFinite(value) && value > 0) return Math.min(value * 1000, MAX_BACKOFF_MS);
  }
  return null;
}

function CheckoutRedirect() {
  const { t } = useLocaleText();
  const topupId = useSearchParams().get("topup");
  const [unavailable, setUnavailable] = React.useState(false);
  const [reason, setReason] = React.useState<MarketTopupGatewayStatus["payabilityReason"]>(undefined);
  const [directOrder, setDirectOrder] = React.useState<MarketWalletTopup | null>(null);

  React.useEffect(() => {
    let active = true;
    setUnavailable(false);
    setReason(undefined);
    setDirectOrder(null);
    if (!topupId) {
      setUnavailable(true);
      return;
    }
    const redirect = async () => {
      try {
        const order = await getMarketWalletTopup(topupId);
        if (!active) return;
        if (order.orderMode === "direct") {
          // In-site payment: the direct page owns polling and every state.
          setDirectOrder(order);
          return;
        }
        if (order.buyerAbandonedAt || order.status !== "pending") {
          setReason(order.buyerAbandonedAt ? "buyer_stopped" : "order_changed");
          setUnavailable(true);
          return;
        }
        const gateway = await getMarketWalletTopupGatewayStatus(topupId);
        if (!active) return;
        const serverNow = Date.parse(gateway.serverNow);
        const checkoutUrl = verifiedPayableCheckoutUrl(
          order.status,
          order.checkoutUrl,
          gateway,
          !!order.buyerAbandonedAt,
          Number.isFinite(serverNow) ? serverNow : Date.now(),
        );
        if (!checkoutUrl) {
          setReason(gateway.payabilityReason === "payable"
            ? "checkout_unavailable"
            : gateway.payabilityReason ?? "gateway_unknown");
          setUnavailable(true);
          return;
        }
        window.location.replace(checkoutUrl);
      } catch {
        if (active) {
          setReason("gateway_unknown");
          setUnavailable(true);
        }
      }
    };
    void redirect();
    return () => { active = false; };
  }, [topupId]);

  if (directOrder && topupId) return <DirectCheckout topupId={topupId} initialOrder={directOrder} />;

  return <main className="mx-auto grid min-h-screen w-full max-w-lg content-center gap-4 px-5 py-12 text-center">
    <h1 className="text-xl font-semibold">{t("marketWallet.checkoutRedirectTitle")}</h1>
    <p role="status" className="text-sm text-muted-foreground">
      {t(unavailable ? "marketWallet.checkoutRedirectUnavailable" : "marketWallet.checkoutRedirectChecking")}
    </p>
    {unavailable && reason ? (
      <p className="text-sm text-amber-700">{t(`marketWallet.payability.${reason}` as Parameters<typeof t>[0])}</p>
    ) : null}
    <Link
      className="inline-flex min-h-11 items-center justify-center font-medium text-primary underline underline-offset-2"
      href={walletHref(topupId)}
    >
      {t("marketWallet.checkoutRedirectReturn")}
    </Link>
  </main>;
}

type PollState = "checking" | "live" | "rate_limited" | "error";

function DirectCheckout({ topupId, initialOrder }: { topupId: string; initialOrder: MarketWalletTopup }) {
  const { locale, t } = useLocaleText();
  const [order, setOrder] = React.useState(initialOrder);
  const [instructions, setInstructions] = React.useState<MarketTopupPaymentInstructions | null>(null);
  const [pollState, setPollState] = React.useState<PollState>("checking");
  const [lastCheckedAt, setLastCheckedAt] = React.useState<number | null>(null);
  const [serverOffsetMs, setServerOffsetMs] = React.useState(0);
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const orderRef = React.useRef(initialOrder);
  const headingRef = React.useRef<HTMLHeadingElement>(null);
  const closed = CLOSED_STATUS.has(order.status);

  React.useEffect(() => {
    if (closed) return;
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [closed]);

  // Move focus to the outcome so screen-reader users hear the final state.
  React.useEffect(() => {
    if (closed) headingRef.current?.focus();
  }, [closed]);

  React.useEffect(() => {
    let active = true;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let delayMs = INSTRUCTIONS_INTERVAL_MS;
    let tick = 0;

    const refreshOrder = async () => {
      const next = await getMarketWalletTopup(topupId);
      if (!active) return next;
      orderRef.current = next;
      setOrder(next);
      return next;
    };

    const poll = async () => {
      if (!active) return;
      tick += 1;
      try {
        let current = orderRef.current;
        if (tick % ORDER_REFRESH_EVERY === 0 || current.status !== "pending") {
          current = await refreshOrder();
          if (!active) return;
        }
        if (CLOSED_STATUS.has(current.status)) {
          setInstructions(null);
          setPollState("live");
          return; // Terminal: stop polling.
        }
        const payableOrder = current.status === "pending" && !current.buyerAbandonedAt
          && current.hasPaymentInstructions !== false;
        if (payableOrder) {
          const next = await getMarketWalletTopupPaymentInstructions(topupId);
          if (!active) return;
          setInstructions(next);
          const offset = Date.parse(next.serverNow) - Date.now();
          if (Number.isFinite(offset)) setServerOffsetMs(offset);
          if (!next.canPay && !["pending", "awaiting_payment", "confirming"].includes(next.gatewayStatus)) {
            // The gateway order is ending; pick up the local outcome promptly.
            current = await refreshOrder();
            if (!active) return;
            if (CLOSED_STATUS.has(current.status)) {
              setInstructions(null);
              setPollState("live");
              return;
            }
          }
        } else {
          setInstructions(null);
        }
        setPollState("live");
        setLastCheckedAt(Date.now());
        delayMs = INSTRUCTIONS_INTERVAL_MS;
      } catch (cause) {
        if (!active) return;
        if (cause instanceof ApiError && cause.status === 429) {
          setPollState("rate_limited");
          delayMs = retryAfterMs(cause) ?? Math.min(delayMs * 2, MAX_BACKOFF_MS);
        } else if (cause instanceof ApiError && cause.status >= 400 && cause.status < 500 && cause.status !== 408) {
          // The order is gone or not ours: polling cannot recover.
          setPollState("error");
          setInstructions(null);
          return;
        } else {
          setPollState("error");
          delayMs = Math.min(delayMs * 2, MAX_BACKOFF_MS);
        }
      }
      if (active) timeout = setTimeout(() => void poll(), delayMs);
    };

    void poll();
    return () => {
      active = false;
      if (timeout) clearTimeout(timeout);
    };
  }, [topupId]);

  const serverNowMs = nowMs + serverOffsetMs;
  const expiresMs = instructions?.expiresAt ? Date.parse(instructions.expiresAt) : Number.NaN;
  const remainingSeconds = Number.isFinite(expiresMs) ? Math.max(0, Math.ceil((expiresMs - serverNowMs) / 1000)) : null;
  const windowEnded = remainingSeconds === 0;
  const confirming = instructions?.gatewayStatus === "confirming";
  const payable = !!instructions?.canPay && !!instructions.payAddress && !!instructions.payableAmount
    && !windowEnded && !closed && !order.buyerAbandonedAt && order.status === "pending";
  const fiatMinor = instructions?.fiatAmountMinor ?? order.amountMinor;
  const symbol = instructions?.tokenSymbol ?? "";
  const chainName = instructions?.chainName ?? instructions?.chain ?? "";

  const statusLine = pollState === "checking"
    ? t("marketCheckout.poll.checking")
    : pollState === "rate_limited"
      ? t("marketCheckout.poll.rateLimited")
      : pollState === "error"
        ? t("marketCheckout.poll.error")
        : closed
          ? t("marketCheckout.poll.stopped")
          : lastCheckedAt
            ? t("marketCheckout.poll.live", { time: new Intl.DateTimeFormat(locale, { timeStyle: "medium" }).format(lastCheckedAt) })
            : t("marketCheckout.poll.checking");

  return (
    <main className="relative mx-auto grid min-h-screen w-full max-w-5xl content-start gap-8 overflow-hidden px-4 py-10 sm:px-6 sm:py-14" aria-labelledby="direct-checkout-title">
      {/* Ambient accent glow, felt more than seen. */}
      <div aria-hidden="true" className="pointer-events-none absolute -right-40 -top-40 size-[28rem] rounded-full bg-[#0052FF]/[0.06] blur-[120px]" />

      <header className="relative grid gap-3">
        <WalletSectionLabel pulse={!closed && payable}>{t("marketCheckout.label")}</WalletSectionLabel>
        <h1 id="direct-checkout-title" ref={headingRef} tabIndex={-1} className="font-display text-3xl leading-tight tracking-[-0.02em] outline-none sm:text-[2.75rem]">
          {order.status === "credited"
            ? t("marketCheckout.creditedTitle")
            : closed
              ? t("marketCheckout.closedTitle")
              : <>{t("marketCheckout.title")}{" "}<span className="bg-gradient-to-r from-[#0052FF] to-[#4D7CFF] bg-clip-text text-transparent">{usdText(fiatMinor, locale)}</span></>}
        </h1>
        {!closed ? <p className="max-w-2xl text-base leading-relaxed text-muted-foreground">{t("marketCheckout.subtitle")}</p> : null}
      </header>

      {closed ? (
        <ClosedState order={order} topupId={topupId} />
      ) : payable && instructions ? (
        <div className="relative grid min-w-0 gap-6 lg:grid-cols-[0.9fr_1.1fr]">
          <section aria-label={t("marketCheckout.qrSection")} className="grid min-w-0 content-start justify-items-center gap-4 rounded-2xl border border-border bg-white p-6 shadow-[0_10px_15px_rgba(0,0,0,0.08)] sm:p-8">
            <div className="rounded-xl border border-border p-2">
              <QrCodeSvg value={instructions.payAddress!} size={232} label={t("marketCheckout.qrLabel", { chain: chainName, symbol })} />
            </div>
            <p className="text-center text-sm text-muted-foreground">{t("marketCheckout.qrHelp", { chain: chainName })}</p>
            {remainingSeconds != null ? (
              <p className={`inline-flex items-center gap-2 rounded-full px-3 py-1.5 text-sm font-medium tabular-nums ${remainingSeconds <= 300 ? "bg-amber-100 text-amber-900" : "bg-muted text-foreground"}`}>
                <Clock3 className="size-4" aria-hidden="true" />
                <span>{t("marketCheckout.expiresIn", { time: formatTopupCountdown(remainingSeconds) })}</span>
              </p>
            ) : null}
          </section>

          <section aria-label={t("marketCheckout.detailsSection")} className="grid min-w-0 content-start gap-5 rounded-2xl border border-border bg-card p-6 shadow-[0_4px_6px_rgba(0,0,0,0.07)] sm:p-8">
            <div className="flex flex-wrap gap-2">
              <WalletBadge tone="accent">{symbol}</WalletBadge>
              <WalletBadge>{chainName}</WalletBadge>
            </div>
            <div className="grid gap-2">
              <p className="font-mono text-xs uppercase tracking-[0.15em] text-muted-foreground">{t("marketCheckout.exactAmount")}</p>
              <div className="flex min-w-0 items-start gap-3">
                <p className="min-w-0 flex-1 break-all font-mono text-3xl font-semibold tabular-nums tracking-tight sm:text-4xl">
                  {instructions.payableAmount}<span className="ml-2 text-lg font-medium text-muted-foreground sm:text-xl">{symbol}</span>
                </p>
                <CopyTextButton value={instructions.payableAmount!} label={t("marketCheckout.copyAmount")} />
              </div>
              <p className="text-xs text-muted-foreground">{t("marketCheckout.fiatEquivalent", { amount: usdText(fiatMinor, locale) })}</p>
              <dl className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-1 text-xs tabular-nums">
                <dt className="text-muted-foreground">{t("walletPolicy.credit")}</dt><dd>{usdText(order.creditAmountMinor ?? order.amountMinor, locale)}</dd>
                <dt className="text-muted-foreground">{t("walletPolicy.fee", { rate: (order.platformFeeRateBps ?? 0) / 100 })}</dt><dd>{usdText(order.platformFeeMinor ?? 0, locale)}</dd>
                <dt className="font-medium">{t("walletPolicy.total")}</dt><dd>{usdText(order.amountMinor, locale)}</dd>
              </dl>
            </div>
            <div className="grid gap-2">
              <p className="font-mono text-xs uppercase tracking-[0.15em] text-muted-foreground">{t("marketCheckout.address", { chain: chainName })}</p>
              <div className="flex min-w-0 items-start gap-3 rounded-xl bg-muted/60 p-3">
                <p className="min-w-0 flex-1 break-all font-mono text-sm leading-6">{instructions.payAddress}</p>
                <CopyTextButton value={instructions.payAddress!} label={t("marketCheckout.copyAddress")} />
              </div>
            </div>
            <div role="note" className="flex gap-3 rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm leading-relaxed text-amber-950">
              <AlertTriangle className="mt-0.5 size-5 shrink-0 text-amber-700" aria-hidden="true" />
              <div className="grid gap-1">
                <p className="font-semibold">{t("marketCheckout.warningTitle", { amount: instructions.payableAmount ?? "", symbol, chain: chainName })}</p>
                <p>{t("marketCheckout.warningBody")}</p>
              </div>
            </div>
            {confirming ? <ConfirmationProgress instructions={instructions} /> : null}
          </section>
        </div>
      ) : (
        <section className="relative grid max-w-2xl gap-4 rounded-2xl border border-border bg-card p-6 shadow-[0_4px_6px_rgba(0,0,0,0.07)] sm:p-8">
          {confirming && instructions ? <ConfirmationProgress instructions={instructions} /> : null}
          {pollState === "checking" && !instructions ? (
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />{t("marketCheckout.loading")}
            </p>
          ) : (
            <p className="flex items-start gap-2 text-sm text-amber-800">
              <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {order.buyerAbandonedAt
                ? payabilityText(t, "buyer_stopped")
                : order.status === "verifying"
                  ? t("marketCheckout.verifying")
                  : order.status === "review_required"
                    ? t("marketWallet.reviewHelp")
                    : ["created", "creating", "create_unknown"].includes(order.status)
                      ? t("marketCheckout.creating")
                      : windowEnded
                        ? payabilityText(t, "deadline_passed")
                        : instructions
                          ? payabilityText(t, instructions.payabilityReason)
                          : pollState === "error"
                            ? t("marketCheckout.loadFailed")
                            : payabilityText(t, "gateway_unknown")}
            </p>
          )}
          <Link href={walletHref(topupId)} className={`${WALLET_SECONDARY_BUTTON} justify-self-start`}>
            <ArrowLeft className="size-4" aria-hidden="true" />{t("marketWallet.checkoutRedirectReturn")}
          </Link>
        </section>
      )}

      <footer className="relative flex flex-wrap items-center justify-between gap-3 border-t border-border pt-5">
        <p role="status" aria-live="polite" className="flex items-center gap-2 text-xs text-muted-foreground">
          {!closed && pollState !== "error" ? <span className="size-1.5 rounded-full bg-accent pulse-dot" aria-hidden="true" /> : null}
          {statusLine}
        </p>
        {payable ? (
          <Link href={walletHref(topupId)} className="inline-flex min-h-11 items-center gap-1 text-sm font-medium text-muted-foreground underline-offset-2 hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
            <ArrowLeft className="size-4" aria-hidden="true" />{t("marketWallet.checkoutRedirectReturn")}
          </Link>
        ) : null}
      </footer>
    </main>
  );
}

function ConfirmationProgress({ instructions }: { instructions: MarketTopupPaymentInstructions }) {
  const { t } = useLocaleText();
  const confirmations = Math.max(0, instructions.confirmations ?? 0);
  const required = instructions.minConfirmations && instructions.minConfirmations > 0 ? instructions.minConfirmations : null;
  const percent = required ? Math.min(100, Math.round((confirmations / required) * 100)) : null;
  return (
    <div className="grid gap-2 rounded-xl border border-accent/20 bg-accent/5 p-4">
      <p className="text-sm font-semibold text-foreground">{t("marketCheckout.confirmingTitle")}</p>
      <div
        role="progressbar"
        aria-label={t("marketCheckout.confirmingTitle")}
        aria-valuemin={0}
        aria-valuemax={required ?? undefined}
        aria-valuenow={required ? Math.min(confirmations, required) : undefined}
        aria-valuetext={required
          ? t("marketCheckout.confirmations", { count: confirmations, required })
          : t("marketCheckout.confirmationsUnknown", { count: confirmations })}
        className="h-2 overflow-hidden rounded-full bg-muted"
      >
        <div
          className={`h-full rounded-full bg-gradient-to-r from-[#0052FF] to-[#4D7CFF] transition-[width] duration-700 ease-out motion-reduce:transition-none ${percent == null ? "w-1/3 animate-pulse motion-reduce:animate-none" : ""}`}
          style={percent == null ? undefined : { width: `${Math.max(percent, 4)}%` }}
        />
      </div>
      <p className="text-xs tabular-nums text-muted-foreground">
        {required
          ? t("marketCheckout.confirmations", { count: confirmations, required })
          : t("marketCheckout.confirmationsUnknown", { count: confirmations })}
      </p>
      <p className="text-xs text-muted-foreground">{t("marketCheckout.confirmingHelp")}</p>
    </div>
  );
}

function ClosedState({ order, topupId }: { order: MarketWalletTopup; topupId: string }) {
  const { t } = useLocaleText();
  const credited = order.status === "credited";
  return (
    <section className={`relative grid max-w-2xl gap-4 rounded-2xl border p-6 shadow-[0_4px_6px_rgba(0,0,0,0.07)] sm:p-8 ${credited ? "border-emerald-200 bg-emerald-50/60" : "border-border bg-card"}`}>
      <p className="flex items-start gap-3 text-base">
        {credited
          ? <CheckCircle2 className="mt-0.5 size-5 shrink-0 text-emerald-700" aria-hidden="true" />
          : <XCircle className="mt-0.5 size-5 shrink-0 text-muted-foreground" aria-hidden="true" />}
        <span>
          {credited
            ? t("marketCheckout.credited")
            : order.localCloseReason === "never_created"
              ? t("marketCheckout.closedNeverCreated")
              : order.status === "expired"
                ? t("marketCheckout.closedExpired")
                : t("marketCheckout.closedCancelled")}
        </span>
      </p>
      {!credited ? <p className="text-sm text-muted-foreground">{t("marketCheckout.closedHelp")}</p> : null}
      <Link href={walletHref(topupId)} className={`${credited ? WALLET_PRIMARY_BUTTON : WALLET_SECONDARY_BUTTON} justify-self-start`}>
        {credited ? null : <ArrowLeft className="size-4" aria-hidden="true" />}
        {t("marketWallet.checkoutRedirectReturn")}
      </Link>
    </section>
  );
}

export default function WalletCheckoutPage() {
  return <React.Suspense fallback={null}><CheckoutRedirect /></React.Suspense>;
}
