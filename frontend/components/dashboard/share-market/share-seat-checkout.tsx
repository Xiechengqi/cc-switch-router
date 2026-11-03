"use client";
import * as React from "react";
import Link from "next/link";
import { Button, Modal } from "@heroui/react";
import { ChevronDown, Info, Loader2, RefreshCw, ShoppingCart } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { marketEligibilityFromError } from "@/components/common/seller-approval-dialog";
import { ApiError, quoteShareMarketSeat, rentShareMarketSeat } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import { sameLoginContext } from "@/lib/password-auth";
import { formatWalletUnits, walletTopupHref } from "@/lib/market-wallet-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { shareCheckoutState, isWalletQuoteConflict, isUnknownShareCheckoutError } from "@/lib/share-checkout";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { MarketEligibility, ShareMarketListing, ShareMarketRentAppService, ShareMarketRentQuote, ShareMarketSeat } from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";
import { formatUsdLimit } from "@/lib/share-value-quota";
import { cn } from "@/lib/utils";
import { formatSeatPrice, formatSeatQuota, isCoreShareApp, shareMarketMutationError } from "./market-utils";
import { providerCategory, providerCategoryLabel } from "@/lib/provider-presentation";
import { showRentQuoteCountdown } from "./buyer-catalog-utils";
import { USD_SEAT_PARALLEL_CAP, UsdSeatRentTerms } from "./seat-value-quota";
import { useQuotaActor } from "./seat-value-quota";
import { valueReadinessActor } from "@/lib/use-value-quota-readiness";
import { clearShareCheckoutRecovery, readShareCheckoutRecovery, saveShareCheckoutRecovery, type ShareCheckoutDraft, type SharePendingPurchase } from "@/lib/share-checkout-recovery";

export type ShareCheckoutSeat = { listing: ShareMarketListing; seat: ShareMarketSeat };
type RentTarget = ShareCheckoutSeat & { quote: ShareMarketRentQuote; idempotencyKey: string };
function rentAppLabel(app: string) {
  return isCoreShareApp(app) ? SHARE_APP_LABELS[app] : app;
}

function rentAppModel(service: ShareMarketRentAppService, t: ReturnType<typeof useLocaleText>["t"]) {
  if (service.modelMode === "passthrough") return t("shareMarket.modelPassthrough");
  return service.upstreamModel || service.models?.join(" / ") || t("shareMarket.catalog.modelUnknown");
}


export function useShareSeatCheckout({
  onChanged, onPresented, onAccessRequired, returnTo, restoreTarget,
}: {
  onChanged: () => Promise<void> | void;
  onPresented?: () => void;
  onAccessRequired?: (item: ShareCheckoutSeat, eligibility: MarketEligibility) => void;
  returnTo?: (target: ShareCheckoutSeat) => string;
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
  const [busySeatId, setBusySeatId] = React.useState("");
  const [error, setError] = React.useState("");
  const [rentNotice, setRentNotice] = React.useState("");
  const [rentQuoteInvalidated, setRentQuoteInvalidated] = React.useState(false);
  const [quoteNowMs, setQuoteNowMs] = React.useState(Date.now);
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
    const recovery = scope ? readShareCheckoutRecovery(scope) : null;
    if (recovery?.pending) { pending.current = recovery.pending; setUnknown(true); }
    return () => { ++generation.current; quoteController.current?.abort(); };
  }, [scope]);
  const guard = () => {
    const epoch = generation.current;
    const auth = readAuthState();
    return () => epoch === generation.current && sameLoginContext(auth, readAuthState());
  };
  React.useEffect(() => {
    if (!rentTarget) return;
    setQuoteNowMs(Date.now());
    const timer = window.setInterval(() => setQuoteNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [rentTarget]);
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
      setRentQuoteInvalidated(false); setRentAutoRenew(draft?.shareId === item.listing.shareId && draft.seatId === item.seat.id ? draft.autoRenew : false); setUnknown(false);
      onPresented?.();
      setRentTarget({ ...item, quote, idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}` });
    } catch (reason) {
      if (!current()) return;
      const eligibility = marketEligibilityFromError(reason);
      if (eligibility && onAccessRequired) { onPresented?.(); onAccessRequired(item, eligibility); }
      else setError(shareMarketMutationError(reason, t));
    } finally { if (current()) { flight.current = false; setBusySeatId(""); setQuoting(null); } }
  };
  const refreshQuote = async () => {
    if (!rentTarget || flight.current || pending.current) return false;
    const current = guard();
    flight.current = true; setBusySeatId(rentTarget.seat.id); setError(""); setRentNotice(""); setRentQuoteInvalidated(true);
    try {
      const quote = await verifiedQuote(rentTarget);
      if (!current()) return false;
      setRentQuoteInvalidated(false);
      setRentTarget({ ...rentTarget, quote, idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}` });
      return true;
    } catch (reason) { if (current()) setError(shareMarketMutationError(reason, t)); return false; }
    finally { if (current()) { flight.current = false; setBusySeatId(""); } }
  };
  const confirmRent = async () => {
    if (flight.current || (!rentTarget && !pending.current)) return;
    if (!pending.current && rentTarget && shareCheckoutState(rentTarget.quote).unsupported) return;
    if (!pending.current && rentTarget && (rentQuoteInvalidated || Date.parse(rentTarget.quote.expiresAt) <= Date.now())) { await refreshQuote(); return; }
    if (!pending.current && rentTarget && shareCheckoutState(rentTarget.quote).shortfall) return;
    const current = guard();
    const request = pending.current ?? { shareId: rentTarget!.listing.shareId, seatId: rentTarget!.seat.id, quoteId: rentTarget!.quote.id, idempotencyKey: rentTarget!.idempotencyKey, autoRenew: rentAutoRenew };
    if (!saveShareCheckoutRecovery(scope, request, request)) { setError(t("shareCheckout.storageError")); return; }
    pending.current = request;
    flight.current = true; setBusySeatId(request.seatId); setError(""); setRentNotice("");
    try {
      const result = await rentShareMarketSeat(request.seatId, request.quoteId, request.idempotencyKey, request.autoRenew);
      if (!result?.ok || typeof result.subscriptionId !== "string" || !result.subscriptionId) throw new Error("Invalid purchase receipt");
      if (!current()) return;
      pending.current = null; setUnknown(false);
      clearShareCheckoutRecovery(scope);
      if (actor) notifyMarketBalanceChanged(actor);
      setRentTarget(null); setRentNotice(""); setRentQuoteInvalidated(false);
      // A refresh failure must not turn a successful purchase into an unknown purchase.
      void Promise.resolve().then(onChanged).catch(() => undefined);
    } catch (reason) {
      if (!current()) return;
      if (isUnknownShareCheckoutError(reason)) { setUnknown(true); setError(shareMarketMutationError(reason, t)); }
      else {
        pending.current = null; setUnknown(false);
        clearShareCheckoutRecovery(scope);
        if (reason instanceof ApiError && reason.status === 410) { setRentQuoteInvalidated(true); setError(t("shareMarket.rentConfirm.expired")); }
        else if (isWalletQuoteConflict(reason) && rentTarget) {
          setRentQuoteInvalidated(true);
          try {
            const quote = await verifiedQuote(rentTarget);
            if (!current()) return;
            setRentTarget({ ...rentTarget, quote, idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}` });
            setRentQuoteInvalidated(false); setRentNotice(t("shareMarket.rentConfirm.fundingChanged"));
          } catch (refreshReason) { if (current()) setError(shareMarketMutationError(refreshReason, t)); }
        } else setError(shareMarketMutationError(reason, t));
      }
    } finally { if (current()) { flight.current = false; setBusySeatId(""); } }
  };
  const closeRentDialog = () => {
    if (flight.current || pending.current) return;
    clearShareCheckoutRecovery(scope);
    setRentTarget(null); setRentAutoRenew(false); setRentNotice(""); setRentQuoteInvalidated(false); setError("");
  };
  const restored = React.useRef("");
  React.useEffect(() => {
    if (!scope || rentTarget || quoting || unknown || flight.current || pending.current) return;
    const draft = readShareCheckoutRecovery(scope)?.draft;
    if (!draft) return;
    const key = `${scope}:${draft.shareId}:${draft.seatId}`;
    const target = restoreTarget?.(draft);
    if (target && target.seat.canRent && restored.current !== key) {
      restored.current = key;
      void open(target);
    }
  }, [scope, restoreTarget, rentTarget, quoting, unknown]);
  const quoteRemainingSeconds = rentTarget
    ? Math.max(0, Math.ceil((Date.parse(rentTarget.quote.expiresAt) - quoteNowMs) / 1_000))
    : 0;
  const quoteExpired = !!rentTarget && quoteRemainingSeconds <= 0;
  const quoteRequiresRefresh = !unknown && (quoteExpired || rentQuoteInvalidated);
  const rentCheckout = shareCheckoutState(rentTarget?.quote);
  const rentWalletFunding = rentCheckout.paid && !rentCheckout.unsupported ? rentTarget?.quote.walletFunding : undefined;
  const rentWalletShortfall = rentCheckout.shortfall;
  const rentPrimaryAction: "refresh" | "topup" | "confirm" = unknown ? "confirm" : quoteRequiresRefresh || rentCheckout.unsupported
    ? "refresh" : rentWalletShortfall ? "topup" : "confirm";
  const showQuoteCountdown = showRentQuoteCountdown(quoteRequiresRefresh, quoteRemainingSeconds);
  const rentQuoteStatusDetails = !rentTarget
    ? ""
    : quoteExpired
      ? t("shareMarket.rentConfirm.expired")
      : rentQuoteInvalidated
        ? t("shareMarket.rentConfirm.refreshRequired")
        : t("shareMarket.rentConfirm.quoteExpiry", {
          time: new Intl.DateTimeFormat(locale, { timeStyle: "medium" }).format(new Date(rentTarget.quote.expiresAt)),
        });
  const rentOffer = rentTarget?.quote.offer;
  const rentIsFree = rentOffer?.pricingModel === "free"
    || (rentOffer?.dailyRateMinor == null && rentOffer?.cyclePriceMinor == null);
  const rentPrice = rentOffer
    ? formatSeatPrice(
      {
        isFree: rentIsFree,
        dailyRateMinor: rentOffer.dailyRateMinor,
        cyclePriceMinor: rentOffer.cyclePriceMinor,
      },
      locale,
      t("shareMarket.free"),
      t("market.day"),
    )
    : "";
  const rentTrialBudget = rentOffer?.trial?.tokenBudget ?? rentOffer?.trialTokenLimit ?? 0;
  const rentTrialValue = rentOffer?.trial?.valueBudgetUsdMicros ?? rentOffer?.trialValueLimitUsdMicros;
  const rentTrialTokens = rentTrialValue != null && rentTrialValue !== "0"
    ? t("shareMarket.rentConfirm.trialValueCompact", { amount: formatUsdLimit(rentTrialValue) })
    : rentTrialBudget > 0
    ? t("shareMarket.rentConfirm.trialTokenCompact", {
      tokens: formatTokenMillions(rentTrialBudget, locale),
    })
    : "";
  const rentTrialSummary = !rentOffer || rentIsFree || rentTarget.quote.trialSecondsRemaining <= 0
    ? ""
    : t("shareMarket.rentConfirm.remainingTrialCompact", {
      hours: (rentTarget.quote.trialSecondsRemaining / 3_600).toFixed(1),
      tokens: rentTrialTokens,
    });
  const rentQuotaSummary = rentOffer
    ? t(rentOffer.valueQuota ? "shareMarket.rentConfirm.quotaSummaryUsd" : "shareMarket.rentConfirm.quotaSummary", {
      // A USD-equivalent seat runs at most USD_SEAT_PARALLEL_CAP requests at once.
      parallel: rentOffer.valueQuota
        ? Math.min(rentOffer.parallelLimit ?? USD_SEAT_PARALLEL_CAP, USD_SEAT_PARALLEL_CAP)
        : rentOffer.parallelLimit == null
          ? t("common.unlimited")
          : rentOffer.parallelLimit,
      tokens: formatSeatQuota(
        rentOffer,
        locale,
        t("common.unlimited"),
        (period) => t(`shareMarket.period.${period}`),
        (amount) => t("shareMarket.quota.usdAmount", { amount }),
      ),
    })
    : "";
  const rentTermSummary = !rentOffer
    ? ""
    : rentOffer.pricingModel === "prepaid_calendar_month"
      ? t("shareMarket.rentConfirm.termMonthlyCompact")
    : rentOffer.serviceDurationDays == null
      ? t("shareMarket.rentConfirm.termPermanentCompact")
      : t("shareMarket.rentConfirm.termFixedCompact", {
        days: rentOffer.serviceDurationDays,
      });
  const rentAppsSummary = rentOffer
    ? rentOffer.service.apps.map((service) => rentAppLabel(service.app)).join(" · ")
    : "";
  const rentTermsSummary = [rentQuotaSummary, rentTermSummary].filter(Boolean).join(" · ");
  const rentServiceDetails = !rentTarget
    ? ""
    : rentTarget.quote.offer.pricingModel === "prepaid_calendar_month"
      ? t("shareMarket.rentConfirm.serviceMonthly")
      : rentTarget.quote.offer.serviceDurationDays == null
        ? t("shareMarket.rentConfirm.servicePermanent")
        : t("shareMarket.rentConfirm.serviceFixed", {
          days: rentTarget.quote.offer.serviceDurationDays,
        });
  const visible = stateScope === scope && login === valueReadinessActor();
  const dialog = (<Modal.Backdrop isOpen={visible && !passwordSettingsOpen && (!!rentTarget || !!quoting || unknown)} isDismissable={!busySeatId && !unknown} onOpenChange={(open) => !open && closeRentDialog()}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light max-h-[calc(100dvh-1rem)] w-[min(500px,calc(100vw-1rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
            <>
                <Modal.Header>
                  {rentTarget ? (
                    <div className="flex min-w-0 flex-1 items-start justify-between gap-3 pr-8">
                      <div className="min-w-0 flex-1">
                        <Modal.Heading className="break-words">
                          {t("shareMarket.rentConfirm.titleSeat", {
                            position: rentTarget.quote.offer.seatPosition,
                          })}
                        </Modal.Heading>
                        <p className="mt-1 truncate text-xs text-slate-500" title={rentTarget.quote.offer.shareName}>{rentTarget.quote.offer.shareName}</p>
                      </div>
                      {showQuoteCountdown ? (
                        <span
                          className={cn(
                            "shrink-0 whitespace-nowrap px-2 py-1 text-xs font-medium tabular-nums",
                            quoteRequiresRefresh ? "bg-rose-100 text-rose-700" : "bg-amber-100 text-amber-800",
                          )}
                          role={quoteRequiresRefresh ? "alert" : undefined}
                          title={rentQuoteStatusDetails}
                        >
                          {quoteExpired
                            ? t("shareMarket.rentConfirm.expiredShort")
                            : rentQuoteInvalidated
                              ? t("shareMarket.rentConfirm.refreshRequiredShort")
                              : t("shareMarket.rentConfirm.expiresInShort", { seconds: quoteRemainingSeconds })}
                        </span>
                      ) : null}
                    </div>
                  ) : <Modal.Heading>{t("shareMarket.rentConfirm.title")}</Modal.Heading>}
                </Modal.Header>
                <Modal.Body className="grid max-h-[min(72dvh,640px)] gap-4 overflow-y-auto">
                  {quoting ? <p role="status" className="flex items-center gap-2"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</p> : null}
                  {unknown ? <p role="alert" className="text-sm text-amber-800">{t("shareCheckout.unknown")}</p> : null}
                  {error ? <p role="alert" className="text-sm text-rose-700">{error}</p> : null}
                  {rentTarget ? (
                    <>
                      <section className="grid gap-3 py-2">
                        <div className="flex min-w-0 flex-wrap items-end justify-between gap-3">
                          <div className="min-w-0">
                            <span className="block text-xs text-slate-500">{t("shareMarket.rentConfirm.priceLabel")}</span>
                            <strong className="mt-1 block break-words text-3xl font-semibold tracking-tight text-slate-950 tabular-nums">{rentPrice}</strong>
                          </div>
                          {rentTrialSummary ? (
                            <span className="whitespace-nowrap bg-emerald-50 px-2.5 py-1 text-xs font-medium text-emerald-700">{rentTrialSummary}</span>
                          ) : null}
                        </div>
                        <p className="truncate text-sm font-medium text-slate-800" title={rentAppsSummary}>{rentAppsSummary}</p>
                        <p className="truncate text-xs text-slate-500" title={rentTermsSummary}>{rentTermsSummary}</p>
                      </section>

                      {rentTarget.quote.trialSecondsRemaining > 0 ? <p className="rounded-lg bg-emerald-50 px-3 py-3 text-xs leading-6 text-emerald-900">{t("marketTrial.checkout", { hours: (rentTarget.quote.trialSecondsRemaining / 3600).toFixed(1), tokens: rentTrialValue != null && rentTrialValue !== "0" ? `${formatUsdLimit(rentTrialValue)} ${t("shareMarket.quota.apiValue")}` : rentTrialBudget > 0 ? formatTokenMillions(rentTrialBudget, locale) : t("marketTrial.timeOnly") })}</p> : null}
                      {rentOffer?.trial?.unitLocked ? <p className="text-xs leading-5 text-muted-foreground">{t("shareMarket.rentConfirm.trialUnitLocked")}</p> : null}
                      {rentOffer ? <p className="text-xs leading-5 text-muted-foreground">{rentOffer.terminationPolicy ? t("shareTermination.quote", { rate: rentOffer.terminationPolicy.rateBps / 100 }) : t("shareTermination.legacy")}</p> : null}
                      {rentOffer?.valueQuota ? <UsdSeatRentTerms offer={rentOffer} /> : null}
                      {rentCheckout.unsupported ? <p role="alert" className="rounded-md bg-rose-50 p-3 text-sm text-rose-800">{t("clientCheckout.unsupported")}</p> : null}
                      {rentWalletFunding ? (
                        <section className="grid gap-2 rounded-lg bg-muted/50 px-3 py-3 text-sm" aria-live="polite">
                          <h3 className="font-semibold">{t("marketWallet.title")}</h3>
                          <div className="flex min-w-0 items-center justify-between gap-3">
                            <span className="text-muted-foreground">{t("marketWallet.available")}</span>
                            <strong className="min-w-0 break-all text-right tabular-nums">{formatWalletUnits(rentWalletFunding.availableUnits, "$", locale)}</strong>
                          </div>
                          {BigInt(rentWalletFunding.committedUnits ?? "0") > 0n ? <div className="flex min-w-0 items-center justify-between gap-3">
                            <span className="text-muted-foreground">{t("marketWallet.existingCommitments")}</span>
                            <strong className="min-w-0 break-all text-right tabular-nums">{formatWalletUnits(rentWalletFunding.committedUnits, "$", locale)}</strong>
                          </div> : null}
                          <div className="flex min-w-0 items-center justify-between gap-3">
                            <span className="text-muted-foreground">{t("marketWallet.requiredNow")}</span>
                            <strong className="min-w-0 break-all text-right tabular-nums">{formatWalletUnits(rentWalletFunding.requiredUnits, "$", locale)}</strong>
                          </div>
                          {rentWalletShortfall ? (
                            <p role="alert" className="font-medium text-destructive">
                              {t("marketWallet.shortfall", { amount: formatWalletUnits(rentWalletFunding.shortfallUnits, "$", locale) })}
                            </p>
                          ) : null}
                        </section>
                      ) : null}

                      {rentWalletFunding && rentOffer?.pricingModel === "prepaid_calendar_month" ? (
                        <label className={cn(
                          "flex min-h-11 items-center justify-between gap-3 rounded-lg bg-slate-50 px-3 py-3 text-sm",
                          busySeatId || unknown || quoteRequiresRefresh
                            ? "cursor-not-allowed opacity-60"
                            : "cursor-pointer",
                        )}>
                          <span className="min-w-0">
                            <strong className="block text-slate-900">{t("marketWallet.autoRenew")}</strong>
                            <span className="block truncate text-xs text-slate-500">{t("marketWallet.autoRenewAtDue")}</span>
                          </span>
                          <span className="relative shrink-0">
                            <input
                              type="checkbox"
                              className="peer sr-only"
                              checked={rentAutoRenew}
                              disabled={!!busySeatId || unknown || quoteRequiresRefresh}
                              onChange={(event) => setRentAutoRenew(event.target.checked)}
                            />
                            <span className="block h-6 w-11 rounded-full bg-slate-300 transition-colors after:absolute after:left-1 after:top-1 after:h-4 after:w-4 after:rounded-full after:bg-white after:content-[''] after:transition-transform peer-checked:bg-primary peer-checked:after:translate-x-5 peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-primary" aria-hidden="true" />
                          </span>
                        </label>
                      ) : null}

                      {rentNotice ? <p role="status" className="rounded-md bg-amber-50 px-3 py-2 text-sm leading-5 text-amber-900">{rentNotice}</p> : null}


                      <details className="group rounded-lg bg-slate-50 px-3">
                        <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-3 rounded-sm py-2 text-sm font-medium text-slate-700 hover:text-slate-950 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary active:text-slate-950 [&::-webkit-details-marker]:hidden">
                          <span className="flex min-w-0 items-center gap-2">
                            <Info className="h-4 w-4 shrink-0 text-slate-400" aria-hidden="true" />
                            <span className="whitespace-nowrap">{t("shareMarket.rentConfirm.details")}</span>
                          </span>
                          <ChevronDown className="h-4 w-4 shrink-0 text-slate-400 group-open:rotate-180" aria-hidden="true" />
                        </summary>
                        <div className="grid gap-5 pb-3 pt-2">
                          <section className="grid gap-2">
                            <h3 className="text-sm font-semibold text-slate-900">{t("shareMarket.rentConfirm.billingDetails")}</h3>
                            <div className="grid gap-2 text-xs leading-5 text-slate-600">
                              <p>{t("marketWallet.serviceBilling")}</p>
                              <p>{rentServiceDetails}</p>
                            </div>
                          </section>

                          <section className="grid gap-2">
                            <h3 className="text-sm font-semibold text-slate-900">{t("shareMarket.rentConfirm.technicalDetails")}</h3>
                            <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-3 text-xs leading-5">
                              <dt className="text-slate-500">{t("shareMarket.catalog.enabledApps")}</dt>
                              <dd className="grid min-w-0 gap-2">
                                {rentTarget.quote.offer.service.apps.map((service) => (
                                  <div key={service.app} className="grid min-w-0 gap-0.5">
                                    <strong className="font-medium text-slate-800">{rentAppLabel(service.app)}</strong>
                                    <span className="break-words text-slate-600">{providerCategoryLabel(providerCategory(rentTarget.listing.providerIdentity), locale)}{service.providerType ? ` · ${service.providerType}` : ""}</span>
                                    <span className="break-words text-slate-500">{rentAppModel(service, t)}</span>
                                  </div>
                                ))}
                              </dd>
                              <dt className="text-slate-500">{t("shareMarket.catalog.shareCapacity")}</dt>
                              <dd>{rentTarget.quote.offer.service.shareParallelLimit == null ? t("common.unlimited") : t("shareMarket.parallelShort", { value: rentTarget.quote.offer.service.shareParallelLimit })}</dd>
                              <dt className="text-slate-500">{t("shareMarket.catalog.shareTokens")}</dt>
                              <dd>{rentTarget.quote.offer.service.shareTokenLimit == null ? t("common.unlimited") : `${formatTokenMillions(rentTarget.quote.offer.service.shareTokensUsed, locale)} / ${formatTokenMillions(rentTarget.quote.offer.service.shareTokenLimit, locale)}`}</dd>
                            </dl>
                          </section>

                          <p className="text-xs leading-5 text-slate-500">{rentQuoteStatusDetails}</p>
                        </div>
                      </details>
                    </>
                  ) : null}
                </Modal.Body>
                <Modal.Footer className="flex-wrap justify-between">
                  {<Button className="min-h-11 whitespace-nowrap" variant="ghost" isDisabled={!!busySeatId || unknown} onClick={closeRentDialog}>{t("common.cancel")}</Button>}
                  {rentPrimaryAction === "refresh" ? (
                    <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={!!busySeatId} onClick={() => void refreshQuote()}>
                      {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
                      {t("shareMarket.rentConfirm.requote")}
                    </Button>
                  ) : null}

                  {rentPrimaryAction === "topup" && rentWalletFunding && rentTarget ? (
                    <Link
                      href={walletTopupHref(rentWalletFunding.shortfallUnits, returnTo?.(rentTarget) ?? `/share-market/?focus=${encodeURIComponent(rentTarget.quote.offer.shareId)}`)}
                      aria-disabled={!!busySeatId}
                      tabIndex={busySeatId ? -1 : undefined}
                      onClick={(event) => {
                        if (busySeatId || !saveShareCheckoutRecovery(scope, { shareId: rentTarget.listing.shareId, seatId: rentTarget.seat.id, autoRenew: rentAutoRenew })) {
                          event.preventDefault(); setError(t("shareCheckout.storageError"));
                        }
                      }}
                      className="inline-flex min-h-11 items-center justify-center rounded-lg bg-primary px-4 font-semibold text-primary-foreground whitespace-nowrap transition-opacity hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:opacity-80 aria-disabled:pointer-events-none aria-disabled:opacity-50"
                    >
                      {t("marketWallet.topupShortfall")}
                    </Link>
                  ) : null}
                  {rentPrimaryAction === "confirm" ? (
                    <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={!!busySeatId || rentCheckout.unsupported} onClick={() => void confirmRent()}>
                      {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShoppingCart className="h-4 w-4" />}
                      {unknown ? t("shareCheckout.retry") : rentIsFree
                        ? t("shareMarket.rentConfirm.confirmFree")
                        : t("shareMarket.rentConfirm.confirm")}
                    </Button>
                  ) : null}

                </Modal.Footer>
              </>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>);
  return { open, dialog, busySeatId: visible ? busySeatId : "", error: visible ? error : "", setError, interactionActive: visible && (!!rentTarget || !!quoting || !!busySeatId || unknown) };
}
