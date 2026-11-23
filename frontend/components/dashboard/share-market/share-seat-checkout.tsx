"use client";
/*
 * Renting a Share seat: one dialog, three steps.
 *
 *   confirm → (topup) → success
 *
 * The confirm step shows only what a buyer decides on: the price, how the
 * rental starts (trial only, or keep it after the trial), what happens when
 * the trial or month ends, and whether it can be ended early. Everything else
 * sits in the collapsed details. An expired quote is refreshed silently; the
 * buyer is only told when the price or terms actually changed.
 */
import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { CheckCircle2, ChevronDown, CircleSlash, Info, Loader2, RefreshCw, ShieldCheck, Wallet } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { marketEligibilityFromError } from "@/components/common/seller-approval-dialog";
import { ShareConnectCredentials } from "@/components/dashboard/share-connect-dialog";
import { InPlaceTopup } from "@/components/market/in-place-topup";
import { ApiError, quoteShareMarketSeat, rentShareMarketSeat } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import { sameLoginContext } from "@/lib/password-auth";
import { formatUnitsCentsDown, formatUnitsCentsUp, suggestedTopupAmount } from "@/lib/market-wallet-money";
import { formatMarketPrice } from "@/lib/market-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { shareCheckoutState, isWalletQuoteConflict, isUnknownShareCheckoutError } from "@/lib/share-checkout";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { MarketEligibility, ShareMarketListing, ShareMarketRentAppService, ShareMarketRentQuote, ShareMarketSeat } from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";
import { formatUsdLimit } from "@/lib/share-value-quota";
import { cn } from "@/lib/utils";
import { formatSeatPrice, formatSeatQuota, isCoreShareApp, shareMarketMutationError } from "./market-utils";
import { providerCategory, providerCategoryLabel } from "@/lib/provider-presentation";
import { USD_SEAT_PARALLEL_CAP, UsdSeatRentTerms } from "./seat-value-quota";
import { useQuotaActor } from "./seat-value-quota";
import { valueReadinessActor } from "@/lib/use-value-quota-readiness";
import {
  clearShareCheckoutRecovery,
  readShareCheckoutRecovery,
  saveShareCheckoutRecovery,
  type ShareAfterTrial,
  type ShareCheckoutDraft,
  type SharePendingPurchase,
} from "@/lib/share-checkout-recovery";

export type ShareCheckoutSeat = { listing: ShareMarketListing; seat: ShareMarketSeat };
type RentTarget = ShareCheckoutSeat & { quote: ShareMarketRentQuote; idempotencyKey: string };
type Step = "confirm" | "topup";
type Outcome = "trialOnly" | "trial" | "paid" | "free";
type Success = { item?: ShareCheckoutSeat; outcome: Outcome; trialHours: string };

function rentAppLabel(app: string) {
  return isCoreShareApp(app) ? SHARE_APP_LABELS[app] : app;
}

function rentAppModel(service: ShareMarketRentAppService, t: ReturnType<typeof useLocaleText>["t"]) {
  if (service.modelMode === "passthrough") return t("shareMarket.modelPassthrough");
  return service.upstreamModel || service.models?.join(" / ") || t("shareMarket.catalog.modelUnknown");
}

function isFreeOffer(offer: ShareMarketRentQuote["offer"]) {
  return offer.pricingModel === "free" || (offer.dailyRateMinor == null && offer.cyclePriceMinor == null);
}

/** A paid seat with trial time left for this buyer can be rented as trial only. */
function offersTrialOnly(quote: ShareMarketRentQuote) {
  return !isFreeOffer(quote.offer) && quote.trialSecondsRemaining > 0;
}

/** What the buyer agrees to. A refreshed quote that keeps these is the same deal. */
function termsFingerprint(quote: ShareMarketRentQuote) {
  const o = quote.offer;
  return JSON.stringify([
    o.pricingModel, o.dailyRateMinor ?? null, o.cyclePriceMinor ?? null, o.serviceDurationDays ?? null,
    o.parallelLimit ?? null, o.tokenLimit ?? null, o.tokenPeriod, o.valueQuota ?? null,
    o.trial?.tokenBudget ?? null, o.trial?.valueBudgetUsdMicros ?? null, quote.trialSecondsRemaining > 0,
    o.terminationPolicy?.rateBps ?? null, o.terminationPolicy?.earlyTermination ?? null,
  ]);
}

const newKey = (quote: ShareMarketRentQuote) => `share-rent:${quote.id}:${crypto.randomUUID()}`;

export function useShareSeatCheckout({
  onChanged, onPresented, onAccessRequired, restoreTarget,
}: {
  onChanged: () => Promise<void> | void;
  onPresented?: () => void;
  onAccessRequired?: (item: ShareCheckoutSeat, eligibility: MarketEligibility) => void;
  restoreTarget?: (draft: ShareCheckoutDraft) => ShareCheckoutSeat | undefined;
}) {
  const { locale, t } = useLocaleText();
  const { session, passwordSettingsOpen } = useAuth();
  const actor = session?.authenticated ? session.user?.id || "" : "";
  const login = useQuotaActor();
  const scope = actor ? `${actor}:${login}` : "";
  const [stateScope, setStateScope] = React.useState(scope);
  const [rentTarget, setRentTarget] = React.useState<RentTarget | null>(null);
  const [quoting, setQuoting] = React.useState<ShareCheckoutSeat | null>(null);
  const [rentAutoRenew, setRentAutoRenew] = React.useState(false);
  // Trial only is the default whenever it is offered.
  const [afterTrial, setAfterTrial] = React.useState<ShareAfterTrial>("end");
  const [step, setStep] = React.useState<Step>("confirm");
  const [topupBusy, setTopupBusy] = React.useState(false);
  const [success, setSuccess] = React.useState<Success | null>(null);
  const [busySeatId, setBusySeatId] = React.useState("");
  const [error, setError] = React.useState("");
  const [rentNotice, setRentNotice] = React.useState("");
  const [stale, setStale] = React.useState(false);
  const [unknown, setUnknown] = React.useState(false);
  const flight = React.useRef(false);
  const generation = React.useRef(0);
  const pending = React.useRef<SharePendingPurchase | null>(null);
  const quoteController = React.useRef<AbortController | null>(null);
  React.useEffect(() => {
    ++generation.current;
    quoteController.current?.abort();
    flight.current = false;
    pending.current = null;
    setStateScope(scope);
    setRentTarget(null); setQuoting(null); setBusySeatId(""); setUnknown(false); setError(""); setRentNotice("");
    setStep("confirm"); setSuccess(null); setTopupBusy(false);
    const recovery = scope ? readShareCheckoutRecovery(scope) : null;
    if (recovery?.pending) { pending.current = recovery.pending; setUnknown(true); }
    return () => { ++generation.current; quoteController.current?.abort(); };
  }, [scope]);
  const guard = () => {
    const epoch = generation.current;
    const auth = readAuthState();
    return () => epoch === generation.current && sameLoginContext(auth, readAuthState());
  };
  const verifiedQuote = async (item: ShareCheckoutSeat) => {
    const controller = new AbortController();
    quoteController.current?.abort();
    quoteController.current = controller;
    const quote = await quoteShareMarketSeat(item.seat.id, controller.signal);
    if (!quote?.id || quote.offer?.shareId !== item.listing.shareId || quote.offer.seatId !== item.seat.id
      || quote.offer.listingId !== item.listing.id || !Number.isFinite(Date.parse(quote.expiresAt))) {
      throw new Error("Invalid Share quote identity");
    }
    return quote;
  };
  const open = async (item: ShareCheckoutSeat) => {
    if (flight.current || pending.current) return;
    const current = guard();
    flight.current = true; setBusySeatId(item.seat.id); setQuoting(item); setError(""); setRentNotice("");
    try {
      const quote = await verifiedQuote(item);
      if (!current()) return;
      const draft = readShareCheckoutRecovery(scope)?.draft;
      const sameSeat = draft?.shareId === item.listing.shareId && draft.seatId === item.seat.id;
      setStale(false); setUnknown(false); setStep("confirm"); setSuccess(null);
      setRentAutoRenew(sameSeat ? draft.autoRenew : false);
      setAfterTrial(sameSeat && draft.afterTrial ? draft.afterTrial : "end");
      onPresented?.();
      setRentTarget({ ...item, quote, idempotencyKey: newKey(quote) });
    } catch (reason) {
      if (!current()) return;
      const eligibility = marketEligibilityFromError(reason);
      if (eligibility && onAccessRequired) { onPresented?.(); onAccessRequired(item, eligibility); }
      else setError(shareMarketMutationError(reason, t));
    } finally { if (current()) { flight.current = false; setBusySeatId(""); setQuoting(null); } }
  };

  /**
   * Re-quote `target`. Returns the fresh target, or null on failure (the error
   * is shown). When the terms changed the buyer is told and must look again.
   */
  const requote = async (target: RentTarget, current: () => boolean) => {
    try {
      const quote = await verifiedQuote(target);
      if (!current()) return null;
      const next = { ...target, quote, idempotencyKey: newKey(quote) };
      const changed = termsFingerprint(quote) !== termsFingerprint(target.quote);
      setRentTarget(next); setStale(false);
      if (changed) setRentNotice(t("shareRent.termsChanged"));
      return { next, changed };
    } catch (reason) {
      if (current()) { setStale(true); setError(shareMarketMutationError(reason, t)); }
      return null;
    }
  };
  const refreshQuote = async () => {
    if (!rentTarget || flight.current || pending.current) return null;
    const current = guard();
    flight.current = true; setBusySeatId(rentTarget.seat.id); setError("");
    try { return await requote(rentTarget, current); }
    finally { if (current()) { flight.current = false; setBusySeatId(""); } }
  };

  // Keep the quote fresh while the buyer reads; they never see a countdown.
  React.useEffect(() => {
    if (!rentTarget || step !== "confirm" || unknown) return;
    const wait = Math.max(0, Date.parse(rentTarget.quote.expiresAt) - Date.now());
    const timer = window.setTimeout(() => { if (!flight.current) void refreshQuote(); }, wait + 250);
    return () => window.clearTimeout(timer);
    // Re-armed per quote; refreshQuote reads the latest state when it fires.
  }, [rentTarget?.quote.id, step, unknown]);

  const trialOnlyFor = (target: RentTarget | null) => !!target && offersTrialOnly(target.quote) && afterTrial === "end";
  const autoRenewFor = (target: RentTarget | null) => !!target && target.quote.offer.pricingModel === "prepaid_calendar_month" && !trialOnlyFor(target);
  const outcomeFor = (target: RentTarget | null, request: { afterTrial?: ShareAfterTrial }): Outcome => {
    if (request.afterTrial === "end") return "trialOnly";
    // Only seats with trial time left are sent with an after-trial choice.
    if (request.afterTrial === "continue") return "trial";
    if (!target) return "paid";
    if (isFreeOffer(target.quote.offer)) return "free";
    return target.quote.trialSecondsRemaining > 0 ? "trial" : "paid";
  };

  const confirmRent = async () => {
    if (flight.current || (!rentTarget && !pending.current)) return;
    const current = guard();
    let target = rentTarget;
    if (!pending.current && target) {
      if (shareCheckoutState(target.quote).unsupported) return;
      if (stale || Date.parse(target.quote.expiresAt) <= Date.now() + 2_000) {
        flight.current = true; setBusySeatId(target.seat.id); setError(""); setRentNotice("");
        const fresh = await requote(target, current);
        if (!current()) return;
        flight.current = false; setBusySeatId("");
        if (!fresh || fresh.changed) return;
        target = fresh.next;
      }
      if (!trialOnlyFor(target) && shareCheckoutState(target.quote).shortfall) { openTopup(); return; }
    }
    const trialOnly = trialOnlyFor(target);
    const request = pending.current ?? {
      shareId: target!.listing.shareId,
      seatId: target!.seat.id,
      quoteId: target!.quote.id,
      idempotencyKey: target!.idempotencyKey,
      autoRenew: autoRenewFor(target) && rentAutoRenew,
      ...(trialOnly ? { afterTrial: "end" as const } : offersTrialOnly(target!.quote) ? { afterTrial: "continue" as const } : {}),
    };
    if (!saveShareCheckoutRecovery(scope, request, request)) { setError(t("shareCheckout.storageError")); return; }
    pending.current = request;
    flight.current = true; setBusySeatId(request.seatId); setError(""); setRentNotice("");
    try {
      const result = await rentShareMarketSeat(request.seatId, request.quoteId, request.idempotencyKey, request.autoRenew, request.afterTrial ?? "continue");
      if (!result?.ok || typeof result.subscriptionId !== "string" || !result.subscriptionId) throw new Error("Invalid purchase receipt");
      if (!current()) return;
      pending.current = null; setUnknown(false);
      clearShareCheckoutRecovery(scope);
      if (actor) notifyMarketBalanceChanged(actor);
      const item = target ?? restoreTarget?.(request);
      setSuccess({
        item: item ? { listing: item.listing, seat: item.seat } : undefined,
        outcome: outcomeFor(target, request),
        trialHours: target ? (target.quote.trialSecondsRemaining / 3_600).toFixed(1) : "",
      });
      setRentTarget(null); setRentNotice(""); setStale(false); setStep("confirm");
      // A refresh failure must not turn a successful purchase into an unknown purchase.
      void Promise.resolve().then(onChanged).catch(() => undefined);
    } catch (reason) {
      if (!current()) return;
      if (isUnknownShareCheckoutError(reason)) { setUnknown(true); setError(shareMarketMutationError(reason, t)); }
      else {
        pending.current = null; setUnknown(false);
        clearShareCheckoutRecovery(scope);
        if (reason instanceof ApiError && reason.status === 410 && target) {
          const fresh = await requote(target, current);
          if (fresh && !fresh.changed && current()) setRentNotice(t("shareRent.quoteRefreshed"));
        } else if (isWalletQuoteConflict(reason) && target) {
          const fresh = await requote(target, current);
          if (fresh && current()) setRentNotice(t("shareMarket.rentConfirm.fundingChanged"));
        } else setError(shareMarketMutationError(reason, t));
      }
    } finally { if (current()) { flight.current = false; setBusySeatId(""); } }
  };

  const openTopup = () => {
    if (!rentTarget) return;
    // Survives a reload of this page while paying in the other tab.
    saveShareCheckoutRecovery(scope, {
      shareId: rentTarget.listing.shareId, seatId: rentTarget.seat.id, autoRenew: rentAutoRenew,
      ...(offersTrialOnly(rentTarget.quote) ? { afterTrial } : {}),
    });
    setError(""); setRentNotice(""); setStep("topup");
  };
  const funded = async () => {
    if (!rentTarget || flight.current) return;
    const current = guard();
    // Hold the flight so the expired-quote timer re-armed by this step change
    // does not abort the re-quote below.
    flight.current = true; setBusySeatId(rentTarget.seat.id); setError("");
    setStep("confirm");
    try {
      const fresh = await requote(rentTarget, current);
      if (fresh && !fresh.changed && current()) setRentNotice(t("shareRent.topup.funded"));
    } finally { if (current()) { flight.current = false; setBusySeatId(""); } }
  };

  const closeRentDialog = () => {
    if (flight.current || pending.current || topupBusy) return;
    clearShareCheckoutRecovery(scope);
    setRentTarget(null); setRentAutoRenew(false); setRentNotice(""); setStale(false); setError("");
    setStep("confirm"); setSuccess(null);
  };
  const restored = React.useRef("");
  React.useEffect(() => {
    if (!scope || rentTarget || quoting || unknown || success || flight.current || pending.current) return;
    const draft = readShareCheckoutRecovery(scope)?.draft;
    if (!draft) return;
    const key = `${scope}:${draft.shareId}:${draft.seatId}`;
    const target = restoreTarget?.(draft);
    if (target && target.seat.canRent && restored.current !== key) {
      restored.current = key;
      void open(target);
    }
  }, [scope, restoreTarget, rentTarget, quoting, unknown, success]);

  const visible = stateScope === scope && login === valueReadinessActor();
  const busy = !!busySeatId || topupBusy;
  const offer = rentTarget?.quote.offer;
  const checkout = shareCheckoutState(rentTarget?.quote);
  const isFree = !!offer && isFreeOffer(offer);
  const trialChoice = !!rentTarget && offersTrialOnly(rentTarget.quote);
  const trialOnly = trialOnlyFor(rentTarget);
  const showAutoRenew = autoRenewFor(rentTarget);
  const funding = rentTarget && checkout.paid && !checkout.unsupported && !trialOnly ? rentTarget.quote.walletFunding : undefined;
  const shortfall = !!funding && checkout.shortfall;
  const trialHours = rentTarget ? (rentTarget.quote.trialSecondsRemaining / 3_600).toFixed(1) : "";
  const price = offer
    ? formatSeatPrice({ isFree, dailyRateMinor: offer.dailyRateMinor, cyclePriceMinor: offer.cyclePriceMinor }, locale, t("shareMarket.free"), t("market.day"), t("market.month"))
    : "";
  const cyclePrice = offer?.cyclePriceMinor != null ? formatMarketPrice(offer.cyclePriceMinor, locale) : price;
  const trialValue = offer?.trial?.valueBudgetUsdMicros ?? offer?.trialValueLimitUsdMicros;
  const trialBudget = offer?.trial?.tokenBudget ?? offer?.trialTokenLimit ?? 0;
  const trialAllowance = trialValue != null && trialValue !== "0"
    ? t("shareMarket.rentConfirm.trialValueCompact", { amount: formatUsdLimit(trialValue) })
    : trialBudget > 0 ? t("shareMarket.rentConfirm.trialTokenCompact", { tokens: formatTokenMillions(trialBudget, locale) }) : "";
  const quotaSummary = offer
    ? t(offer.valueQuota ? "shareMarket.rentConfirm.quotaSummaryUsd" : "shareMarket.rentConfirm.quotaSummary", {
      // A USD-equivalent seat runs at most USD_SEAT_PARALLEL_CAP requests at once.
      parallel: offer.valueQuota
        ? Math.min(offer.parallelLimit ?? USD_SEAT_PARALLEL_CAP, USD_SEAT_PARALLEL_CAP)
        : offer.parallelLimit == null ? t("common.unlimited") : offer.parallelLimit,
      tokens: formatSeatQuota(offer, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`), (amount) => t("shareMarket.quota.usdAmount", { amount })),
    })
    : "";
  const termSummary = !offer ? ""
    : offer.pricingModel === "prepaid_calendar_month" ? t("shareMarket.rentConfirm.termMonthlyCompact")
    : offer.serviceDurationDays == null ? t("shareMarket.rentConfirm.termPermanentCompact")
    : t("shareMarket.rentConfirm.termFixedCompact", { days: offer.serviceDurationDays });
  const appsSummary = offer ? offer.service.apps.map((service) => rentAppLabel(service.app)).join(" · ") : "";

  // What happens, in order.
  const timeline: string[] = [];
  if (offer && rentTarget) {
    const trial = rentTarget.quote.trialSecondsRemaining > 0 && !isFree;
    if (isFree) timeline.push(t("shareRent.step.free"));
    else if (trial) {
      timeline.push(t("shareRent.step.trialNow", { hours: trialHours, budget: trialAllowance }));
      if (trialOnly) timeline.push(t("shareRent.step.trialEnds"), t("shareRent.step.switchLater"));
      else timeline.push(t("shareRent.step.trialConverts", { price: cyclePrice }));
    } else if (offer.pricingModel === "prepaid_calendar_month") timeline.push(t("shareRent.step.payNowMonthly", { amount: cyclePrice }));
    else timeline.push(t("shareRent.step.payDaily", { price }));
    if (showAutoRenew) timeline.push(t(rentAutoRenew ? "shareRent.step.renewOn" : "shareRent.step.renewOff"));
  }
  const policy = offer?.terminationPolicy;
  const termsLine = !offer ? null
    : trialOnly ? { tone: "calm" as const, text: t("shareRent.terms.trialOnly") }
    : isFree ? { tone: "calm" as const, text: t("shareRent.terms.free") }
    : !policy ? { tone: "calm" as const, text: t("shareTermination.legacy") }
    : policy.earlyTermination === "forbidden" ? { tone: "warn" as const, text: t("shareRent.terms.forbidden") }
    : { tone: "calm" as const, text: t("shareRent.terms.allowed", { rate: policy.rateBps / 100 }) };

  const action: "retry" | "topup" | "confirm" = unknown ? "confirm"
    : checkout.unsupported ? "retry" : shortfall ? "topup" : "confirm";
  const confirmLabel = unknown ? t("shareCheckout.retry")
    : isFree ? t("shareRent.action.free")
    : trialOnly ? t("shareRent.action.trialOnly")
    : rentTarget && rentTarget.quote.trialSecondsRemaining > 0 ? t("shareRent.action.trial")
    : offer?.pricingModel === "prepaid_calendar_month" ? t("shareRent.action.pay", { amount: cyclePrice })
    : t("shareRent.action.rent");
  const topupAmount = funding ? suggestedTopupAmount(funding.shortfallUnits) : null;
  const requiredUnits = funding ? (BigInt(funding.availableUnits) + BigInt(funding.shortfallUnits)).toString() : "0";

  const isOpen = visible && !passwordSettingsOpen && (!!rentTarget || !!quoting || unknown || !!success);
  const title = success
    ? t(success.outcome === "trialOnly" ? "shareRent.success.trialOnly" : success.outcome === "trial" ? "shareRent.success.trial" : "shareRent.success.rented")
    : step === "topup" ? t("shareRent.topup.title")
    : rentTarget ? t("shareRent.title", { position: rentTarget.quote.offer.seatPosition })
    : t("shareMarket.rentConfirm.title");

  const dialog = (<Modal.Backdrop isOpen={isOpen} isDismissable={!busy && !unknown} onOpenChange={(next) => !next && closeRentDialog()}>
    <Modal.Container placement="center" className="p-0 sm:p-4">
      <Modal.Dialog className="light flex h-dvh w-full max-w-none flex-col overflow-hidden rounded-none bg-card text-card-foreground sm:h-auto sm:max-h-[calc(100dvh-2rem)] sm:max-w-[32rem] sm:rounded-2xl">
        <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" isDisabled={busy || unknown} />
        <Modal.Header>
          <div className="min-w-0 pr-8">
            <Modal.Heading className="flex items-center gap-2 break-words">
              {success ? <CheckCircle2 className="size-5 shrink-0 text-success" aria-hidden /> : step === "topup" ? <Wallet className="size-5 shrink-0 text-muted-foreground" aria-hidden /> : null}
              {title}
            </Modal.Heading>
            {rentTarget && !success ? <p className="mt-1 truncate text-xs text-muted-foreground" title={rentTarget.quote.offer.shareName}>{rentTarget.quote.offer.shareName}</p> : null}
          </div>
        </Modal.Header>
        <Modal.Body className="grid min-h-0 min-w-0 flex-1 content-start gap-4 overflow-y-auto overscroll-contain sm:max-h-[76dvh]">
          {quoting ? <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden />{t("common.loading")}</p> : null}
          {unknown ? <p role="alert" className="rounded-lg bg-warning/10 px-3 py-2 text-sm text-amber-900">{t("shareCheckout.unknown")}</p> : null}
          {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}

          {success ? (
            <SuccessStep success={success} />
          ) : step === "topup" && rentTarget && funding ? (
            <InPlaceTopup
              active={isOpen && step === "topup"}
              purpose={t("shareRent.topup.purpose", { position: rentTarget.quote.offer.seatPosition, missing: formatUnitsCentsUp(funding.shortfallUnits, "$", locale) })}
              requiredUnits={requiredUnits}
              suggestedAmount={topupAmount}
              idPrefix="share-rent-topup"
              onBusyChange={setTopupBusy}
              onFunded={() => void funded()}
            />
          ) : rentTarget && offer ? (
            <>
              <section className="grid gap-1.5 rounded-xl border border-border bg-muted/40 px-4 py-3">
                <div className="flex min-w-0 items-baseline justify-between gap-3">
                  <strong className="break-words text-2xl font-semibold tracking-tight tabular-nums">{price}</strong>
                  <span className="shrink-0 text-xs text-muted-foreground">{termSummary}</span>
                </div>
                <p className="truncate text-sm font-medium" title={appsSummary}>{appsSummary}</p>
                <p className="truncate text-xs text-muted-foreground" title={quotaSummary}>{quotaSummary}</p>
              </section>

              {trialChoice ? (
                <fieldset className="grid gap-2" disabled={busy || unknown}>
                  <legend className="mb-2 text-xs font-medium text-muted-foreground">{t("shareRent.mode.label")}</legend>
                  <div className="grid grid-cols-2 gap-2">
                    {([
                      ["end", t("shareRent.mode.trialOnly"), t("shareRent.mode.trialOnlyHint", { hours: trialHours })],
                      ["continue", t("shareRent.mode.continue"), t("shareRent.mode.continueHint", { price })],
                    ] as const).map(([value, label, hint]) => (
                      <label
                        key={value}
                        className={cn(
                          "grid min-h-11 cursor-pointer content-start gap-0.5 rounded-xl border px-3 py-2.5 transition-all duration-200 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent has-[:focus-visible]:ring-offset-2",
                          afterTrial === value ? "border-accent bg-accent/5 shadow-[0_4px_14px_rgba(0,82,255,0.12)]" : "border-border hover:border-accent/30",
                          (busy || unknown) && "cursor-not-allowed opacity-60",
                        )}
                      >
                        <input type="radio" name="share-rent-after-trial" value={value} className="sr-only" checked={afterTrial === value} onChange={() => { setAfterTrial(value); setRentNotice(""); }} />
                        <span className={cn("text-sm font-semibold", afterTrial === value && "text-accent")}>{label}</span>
                        <span className="text-xs leading-5 text-muted-foreground">{hint}</span>
                      </label>
                    ))}
                  </div>
                </fieldset>
              ) : null}

              <ol className="grid gap-2" aria-label={t("shareRent.timeline")}>
                {timeline.map((line, index) => (
                  <li key={line} className="flex min-w-0 items-start gap-2.5 text-sm leading-5">
                    <span className={cn("mt-1.5 size-2 shrink-0 rounded-full", index === 0 ? "bg-accent" : "bg-border")} aria-hidden />
                    <span className="min-w-0">{line}</span>
                  </li>
                ))}
              </ol>

              {showAutoRenew ? (
                <label className={cn("flex min-h-11 items-center justify-between gap-3 rounded-xl border border-border px-3 py-2.5 text-sm", busy || unknown ? "cursor-not-allowed opacity-60" : "cursor-pointer")}>
                  <span className="min-w-0">
                    <strong className="block font-medium">{t("marketWallet.autoRenew")}</strong>
                    <span className="block truncate text-xs text-muted-foreground">{t("marketWallet.autoRenewAtDue")}</span>
                  </span>
                  <span className="relative shrink-0">
                    <input type="checkbox" className="peer sr-only" checked={rentAutoRenew} disabled={busy || unknown} onChange={(event) => setRentAutoRenew(event.target.checked)} />
                    <span className="block h-6 w-11 rounded-full bg-slate-300 transition-colors after:absolute after:left-1 after:top-1 after:size-4 after:rounded-full after:bg-white after:content-[''] after:transition-transform peer-checked:bg-accent peer-checked:after:translate-x-5 peer-focus-visible:ring-2 peer-focus-visible:ring-accent peer-focus-visible:ring-offset-2" aria-hidden />
                  </span>
                </label>
              ) : null}

              {termsLine ? (
                <p className={cn("flex items-start gap-2 text-xs leading-5", termsLine.tone === "warn" ? "rounded-lg bg-warning/10 px-3 py-2 text-amber-900" : "text-muted-foreground")}>
                  {termsLine.tone === "warn" ? <CircleSlash className="mt-0.5 size-3.5 shrink-0" aria-hidden /> : <ShieldCheck className="mt-0.5 size-3.5 shrink-0" aria-hidden />}
                  <span>{termsLine.text}</span>
                </p>
              ) : null}

              {funding ? (
                <p className={cn("text-xs leading-5 tabular-nums", shortfall ? "font-medium text-destructive" : "text-muted-foreground")} aria-live="polite">
                  {shortfall
                    ? t("shareRent.funding.short", { balance: formatUnitsCentsDown(funding.availableUnits, "$", locale), missing: formatUnitsCentsUp(funding.shortfallUnits, "$", locale) })
                    : t("shareRent.funding.ok", { balance: formatUnitsCentsDown(funding.availableUnits, "$", locale), required: formatUnitsCentsUp(funding.requiredUnits, "$", locale) })}
                </p>
              ) : null}
              {checkout.unsupported ? <p role="alert" className="rounded-lg bg-destructive/10 p-3 text-sm text-destructive">{t("clientCheckout.unsupported")}</p> : null}
              {rentNotice ? <p role="status" className="rounded-lg bg-accent/5 px-3 py-2 text-sm leading-5 text-accent">{rentNotice}</p> : null}

              <details className="group rounded-xl bg-muted/40 px-3">
                <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-3 rounded-sm py-2 text-sm font-medium text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
                  <span className="flex min-w-0 items-center gap-2">
                    <Info className="size-4 shrink-0" aria-hidden />
                    <span className="whitespace-nowrap">{t("shareMarket.rentConfirm.details")}</span>
                  </span>
                  <ChevronDown className="size-4 shrink-0 transition-transform group-open:rotate-180 motion-reduce:transition-none" aria-hidden />
                </summary>
                <div className="grid gap-5 pb-3 pt-2 text-xs leading-5">
                  <section className="grid gap-2">
                    <h3 className="text-sm font-semibold">{t("shareMarket.rentConfirm.billingDetails")}</h3>
                    {!isFree ? <p className="text-muted-foreground">{t("marketWallet.serviceBilling")}</p> : null}
                    <p className="text-muted-foreground">{
                      offer.pricingModel === "prepaid_calendar_month" ? t("shareMarket.rentConfirm.serviceMonthly")
                        : offer.serviceDurationDays == null ? t("shareMarket.rentConfirm.servicePermanent")
                        : t("shareMarket.rentConfirm.serviceFixed", { days: offer.serviceDurationDays })
                    }</p>
                    {policy && !isFree ? <p className="text-muted-foreground">{t("shareTermination.quote", { rate: policy.rateBps / 100 })}</p> : null}
                    {offer.trial?.unitLocked ? <p className="text-muted-foreground">{t("shareMarket.rentConfirm.trialUnitLocked")}</p> : null}
                  </section>
                  {offer.valueQuota ? <UsdSeatRentTerms offer={offer} /> : null}
                  <section className="grid gap-2">
                    <h3 className="text-sm font-semibold">{t("shareMarket.rentConfirm.technicalDetails")}</h3>
                    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-3">
                      <dt className="text-muted-foreground">{t("shareMarket.catalog.enabledApps")}</dt>
                      <dd className="grid min-w-0 gap-2">
                        {offer.service.apps.map((service) => (
                          <div key={service.app} className="grid min-w-0 gap-0.5">
                            <strong className="font-medium">{rentAppLabel(service.app)}</strong>
                            <span className="break-words text-muted-foreground">{providerCategoryLabel(providerCategory(rentTarget.listing.providerIdentity), locale)}{service.providerType ? ` · ${service.providerType}` : ""}</span>
                            <span className="break-words text-muted-foreground">{rentAppModel(service, t)}</span>
                          </div>
                        ))}
                      </dd>
                      {offer.service.measurementPolicy === "text-tokens-v1" ? <>
                        <dt className="text-muted-foreground">{t("shareRent.capability.title")}</dt>
                        <dd>{t("shareRent.capability.body")}</dd>
                      </> : null}
                      <dt className="text-muted-foreground">{t("shareMarket.catalog.shareCapacity")}</dt>
                      <dd>{offer.service.shareParallelLimit == null ? t("common.unlimited") : t("shareMarket.parallelShort", { value: offer.service.shareParallelLimit })}</dd>
                      <dt className="text-muted-foreground">{t("shareMarket.catalog.shareTokens")}</dt>
                      <dd>{offer.service.shareTokenLimit == null ? t("common.unlimited") : `${formatTokenMillions(offer.service.shareTokensUsed, locale)} / ${formatTokenMillions(offer.service.shareTokenLimit, locale)}`}</dd>
                    </dl>
                  </section>
                </div>
              </details>
            </>
          ) : null}
        </Modal.Body>
        <Modal.Footer className="flex-wrap justify-between gap-2">
          {success ? (
            <Button className="ml-auto min-h-11" variant="primary" onPress={closeRentDialog}>{t("shareRent.done")}</Button>
          ) : step === "topup" ? (
            <Button className="min-h-11" variant="ghost" isDisabled={busy} onPress={() => { setStep("confirm"); setError(""); }}>{t("shareRent.topup.back")}</Button>
          ) : (
            <>
              <Button className="min-h-11 whitespace-nowrap" variant="ghost" isDisabled={busy || unknown} onPress={closeRentDialog}>{t("common.cancel")}</Button>
              {action === "retry" ? (
                <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={busy} onPress={() => void refreshQuote()}>
                  {busy ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : <RefreshCw className="size-4" aria-hidden />}
                  {t("shareRent.action.retry")}
                </Button>
              ) : action === "topup" && funding ? (
                <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={busy} onPress={openTopup}>
                  <Wallet className="size-4" aria-hidden />
                  {t("shareRent.action.topup", { amount: formatUnitsCentsUp(funding.shortfallUnits, "$", locale) })}
                </Button>
              ) : (
                <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={busy || (!unknown && (!rentTarget || checkout.unsupported))} onPress={() => void confirmRent()}>
                  {busySeatId ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
                  {confirmLabel}
                </Button>
              )}
            </>
          )}
        </Modal.Footer>
      </Modal.Dialog>
    </Modal.Container>
  </Modal.Backdrop>);
  return { open, dialog, busySeatId: visible ? busySeatId : "", error: visible ? error : "", setError, interactionActive: visible && (!!rentTarget || !!quoting || !!busySeatId || unknown || !!success) };
}

function SuccessStep({ success }: { success: Success }) {
  const { t } = useLocaleText();
  const subdomain = success.item?.listing.subdomain;
  return (
    <div className="grid gap-4">
      <p className="text-sm leading-6">
        {success.outcome === "trialOnly" ? (success.trialHours ? t("shareRent.success.trialOnlyBody", { hours: success.trialHours }) : t("shareRent.success.trialOnlyBodyNoHours"))
          : success.outcome === "trial" ? (success.trialHours ? t("shareRent.success.trialBody", { hours: success.trialHours }) : t("shareRent.success.trialBodyNoHours"))
          : t("shareRent.success.paidBody")}
      </p>
      {subdomain ? (
        <section className="grid gap-3 rounded-xl border border-border px-4 py-3">
          <h3 className="text-sm font-semibold">{t("shareRent.success.connect")}</h3>
          <ShareConnectCredentials subdomain={subdomain} />
          <p className="text-xs leading-5 text-muted-foreground">{t("shareRent.success.activation")}</p>
        </section>
      ) : null}
      <p className="text-xs leading-5 text-muted-foreground">{t("shareRent.success.manage")}</p>
    </div>
  );
}
