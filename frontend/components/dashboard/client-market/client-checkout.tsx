"use client";
/*
 * Renting a Client: one dialog.
 *
 * The buyer sees the price, how the rental starts (trial only, or keep it
 * after the trial), what happens next, and confirms that the seller's machine
 * can read its disk. Balance details, the early-exit rate and the billing rule
 * sit in the collapsed details. A trial-only rental needs no balance.
 */
import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { ChevronDown, Info, Loader2, RefreshCw, ShieldAlert, Wallet } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { InPlaceTopup } from "@/components/market/in-place-topup";
import { ApiError, createMarketAccessRequest } from "@/lib/api";
import { trialDuration } from "@/lib/client-checkout";
import { formatUsdMoney } from "@/lib/market-money";
import { formatWalletUnits, suggestedTopupAmount } from "@/lib/market-wallet-money";
import { validPaidWalletQuoteFunding } from "@/lib/market-wallet-quote";
import {
  clientDisplayName,
  getOnlineListings,
  onlineMarketError,
  onlineRequest,
  type OnlineListing,
  type OnlineQuote,
} from "@/lib/online-client-market";
import { cn } from "@/lib/utils";
import { onlineListingRentable, ROUTER_OPEN_LOGIN_EVENT } from "./host-utils";
import { ClientEgressLine } from "./online-shop";

type AfterTrial = "end" | "continue";
type Pending = { quoteId: string; afterTrial: AfterTrial | null };

function readPending(key: string): Pending | null {
  try {
    const raw = key ? sessionStorage.getItem(key) : null;
    if (!raw) return null;
    // Earlier builds stored the bare quote id.
    if (!raw.startsWith("{")) return { quoteId: raw, afterTrial: null };
    const value = JSON.parse(raw) as Pending;
    return typeof value?.quoteId === "string" ? value : null;
  } catch {
    return null;
  }
}

export function useClientCheckout({
  listings,
  authenticated,
  onReceipt,
}: {
  listings: OnlineListing[];
  authenticated: boolean;
  /** A lease now exists for the buyer: show it where they are. */
  onReceipt: (leaseId: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const recoveryKey = session?.user?.id ? "cc-client-order:" + session.user.id : "";
  const [selection, setSelection] = React.useState<OnlineListing | null>(null);
  const [quote, setQuote] = React.useState<OnlineQuote | null>(null);
  const [auto, setAuto] = React.useState(false);
  const [afterTrial, setAfterTrial] = React.useState<AfterTrial>("end");
  const [ack, setAck] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [uncertain, setUncertain] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [accessRequired, setAccessRequired] = React.useState(false);
  const [topupOpen, setTopupOpen] = React.useState(false);
  const [topupBusy, setTopupBusy] = React.useState(false);
  const [pending, setPending] = React.useState<Pending | null>(null);
  const attempted = React.useRef<string | null>(null);
  const quoteVersion = React.useRef(0);
  const mounted = React.useRef(true);
  React.useEffect(() => {
    mounted.current = true;
    setPending(readPending(recoveryKey));
    return () => {
      mounted.current = false;
    };
  }, [recoveryKey]);
  const remember = (value: Pending | null) => {
    if (mounted.current) setPending(value);
    try {
      if (!recoveryKey) return;
      if (value) sessionStorage.setItem(recoveryKey, JSON.stringify(value));
      else sessionStorage.removeItem(recoveryKey);
    } catch {
      /* In-memory recovery remains available. */
    }
  };
  const selected = selection
    ? (listings.find((item) => item.installationId === selection.installationId && item.owned === selection.owned) ?? selection)
    : null;
  const paid = !!quote && quote.terms.cyclePriceMinor > 0;
  const selfRental = quote?.terms.selfRental === true;
  const trialChoice = paid && quote.terms.trialSeconds > 0;
  const trialOnly = trialChoice && afterTrial === "end";
  const funding = quote?.walletFunding ?? undefined;
  const fundingValid = !paid || validPaidWalletQuoteFunding(funding, quote.terms.cyclePriceMinor);
  const shortfall = paid && !trialOnly && fundingValid && funding?.shortfallUnits !== "0";
  const funded = !paid || trialOnly || (fundingValid && funding?.shortfallUnits === "0");

  const reset = () => {
    quoteVersion.current++;
    attempted.current = null;
    setSelection(null);
    setQuote(null);
    setAuto(false);
    setAfterTrial("end");
    setAck(false);
    setUncertain(false);
    setError("");
    setNotice("");
    setAccessRequired(false);
    setTopupOpen(false);
  };
  async function run(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await fn();
    } catch (cause) {
      setAccessRequired(cause instanceof ApiError && cause.code === "MARKET_ACCESS_REQUIRED");
      setError(onlineMarketError(cause, t));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }
  async function requote(listing: OnlineListing, renew: boolean, latest = false) {
    const version = ++quoteVersion.current;
    const current = latest
      ? (await getOnlineListings()).listings.find((item) => item.installationId === listing.installationId)
      : listing;
    if (!current || !onlineListingRentable(current)) throw new Error(t("lease.unavailable"));
    const next = await onlineRequest<OnlineQuote>(`installations/${current.installationId}/quote`, "POST", {
      revision: current.revision,
      autoRenew: renew,
    });
    if (version !== quoteVersion.current) return;
    setSelection(current);
    setAccessRequired(false);
    setQuote(next);
    setAuto(next.terms.autoRenew);
  }
  // An expired quote is refreshed in place; the buyer is told the terms were re-checked.
  React.useEffect(() => {
    if (!quote || !selected || uncertain || busy || topupOpen || topupBusy) return;
    const timer = setTimeout(
      () => {
        if (attempted.current === quote.id) return;
        void run(async () => {
          await requote(selected, auto, true);
          setNotice(t("lease.requoted"));
        });
      },
      Math.max(0, quote.expiresAt * 1000 - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [quote, selected, uncertain, busy, topupOpen, topupBusy, auto]);

  function open(listing: OnlineListing) {
    if (!authenticated) {
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    if (pending) {
      toast.warning(t("lease.receipt"));
      return;
    }
    reset();
    setSelection(listing);
    void run(() => requote(listing, false));
  }
  function receipt(leaseId: string) {
    remember(null);
    if (!mounted.current) return;
    reset();
    onReceipt(leaseId);
  }
  async function commit(quoteId: string, choice: AfterTrial | null) {
    const result = await onlineRequest<{ leaseId: string }>(`quotes/${encodeURIComponent(quoteId)}/commit`, "POST", {
      acknowledged: true,
      ...(choice ? { afterTrial: choice } : {}),
    });
    if (!result?.leaseId) throw new Error(t("lease.receipt"));
    return result.leaseId;
  }
  async function confirm() {
    if (!selected || !quote || !funded || (!selfRental && !ack)) return;
    const choice = trialChoice ? afterTrial : null;
    attempted.current = quote.id;
    remember({ quoteId: quote.id, afterTrial: choice });
    try {
      receipt(await commit(quote.id, choice));
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 409 && !cause.code?.startsWith("CLIENT_TRIAL")) {
        remember(null);
        attempted.current = null;
        setUncertain(false);
        setAck(false);
        await requote(selected, auto, true);
        setNotice(t("lease.requoted"));
        return;
      }
      if (!(cause instanceof ApiError) || cause.status >= 500) {
        setUncertain(true);
        throw new Error(t("lease.receipt"));
      }
      if (cause.status !== 401) remember(null);
      attempted.current = null;
      setUncertain(false);
      throw cause;
    }
  }
  function retryPending() {
    if (!pending) return;
    void run(async () => {
      try {
        receipt(await commit(pending.quoteId, pending.afterTrial));
      } catch (cause) {
        if (cause instanceof ApiError && cause.status === 409) remember(null);
        throw cause;
      }
    });
  }

  const price = paid ? formatUsdMoney(quote.terms.cyclePriceMinor, locale) : t("online.free");
  const trialLength = quote ? trialDuration(quote.terms.trialSeconds, locale) : "";
  const trialEnds = quote
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(new Date(Date.now() + quote.terms.trialSeconds * 1000))
    : "";
  const timeline = !quote
    ? []
    : selfRental
      ? [t("clientMarket.checkout.timeline.self")]
      : !paid
        ? [
            quote.terms.freeDays
              ? t("clientMarket.checkout.timeline.freeDays", { days: quote.terms.freeDays })
              : t("clientMarket.checkout.timeline.freeOngoing"),
          ]
        : trialOnly
          ? [
              t("clientMarket.checkout.timeline.trialStart", { duration: trialLength }),
              t("clientMarket.checkout.timeline.trialOnlyEnd", { date: trialEnds }),
            ]
          : trialChoice
            ? [
                t("clientMarket.checkout.timeline.trialStart", { duration: trialLength }),
                t("clientMarket.checkout.timeline.trialThenCharge", { date: trialEnds, price }),
                t(auto ? "clientMarket.checkout.timeline.renewOn" : "clientMarket.checkout.timeline.renewOff"),
              ]
            : [
                t("clientMarket.checkout.timeline.chargeAtActivation", { price }),
                t(auto ? "clientMarket.checkout.timeline.renewOn" : "clientMarket.checkout.timeline.renewOff"),
              ];
  const confirmLabel = uncertain
    ? t("common.retry")
    : selfRental || !paid
      ? t("clientMarket.shop.confirmFree")
      : trialOnly
        ? t("clientMarket.checkout.startTrial")
        : t("online.confirm");
  const isOpen = !!selection;
  const locked = busy || topupBusy || uncertain;

  const dialog = (
    <Modal.Backdrop isOpen={isOpen} isDismissable={!busy && !topupBusy} onOpenChange={(next) => !next && !busy && !topupBusy && reset()}>
      <Modal.Container placement="center" className="p-0 sm:p-4">
        <Modal.Dialog className="light flex h-dvh w-full max-w-none flex-col overflow-hidden rounded-none bg-card text-card-foreground sm:h-auto sm:max-h-[calc(100dvh-2rem)] sm:max-w-[32rem] sm:rounded-2xl">
          <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" isDisabled={busy || topupBusy} />
          <Modal.Header className="grid shrink-0 gap-1.5">
            <Modal.Heading className="flex items-center gap-2 break-all pr-8">
              {topupOpen ? <Wallet className="size-5 shrink-0 text-muted-foreground" aria-hidden /> : null}
              {selected ? clientDisplayName(selected) : ""}
            </Modal.Heading>
            {selected ? <ClientEgressLine listing={selected} /> : null}
          </Modal.Header>
          <Modal.Body className="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto overscroll-contain text-sm">
            {busy && !quote ? (
              <p role="status" className="flex items-center gap-2 text-muted-foreground">
                <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden />
                {t("common.loading")}
              </p>
            ) : null}
            {topupOpen && quote && funding && selected && fundingValid ? (
              <InPlaceTopup
                active
                purpose={t("marketWallet.shortfall", { amount: formatWalletUnits(funding.shortfallUnits, "$", locale) })}
                requiredUnits={(BigInt(funding.requiredUnits) + BigInt(funding.committedUnits)).toString()}
                suggestedAmount={suggestedTopupAmount(funding.shortfallUnits)}
                idPrefix="client-rent-topup"
                onBusyChange={setTopupBusy}
                onFunded={() =>
                  void run(async () => {
                    setTopupOpen(false);
                    await requote(selected, auto, true);
                  })
                }
              />
            ) : quote ? (
              <>
                <section className="grid gap-1.5 rounded-xl border border-border bg-muted/40 px-4 py-3">
                  <div className="flex min-w-0 items-baseline justify-between gap-3">
                    <strong className="text-2xl font-semibold tabular-nums tracking-tight">
                      {selfRental ? t("clientMarket.shop.selfFree") : price}
                    </strong>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {paid ? t("clientMarket.shop.monthly") : quote.terms.freeDays
                        ? new Intl.NumberFormat(locale, { style: "unit", unit: "day", unitDisplay: "long" }).format(quote.terms.freeDays)
                        : t("clientMarket.shop.ongoing")}
                    </span>
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {t(trialOnly || (!paid && !selfRental) || selfRental ? "clientMarket.checkout.dueNone" : trialChoice ? "clientMarket.checkout.dueAfterTrial" : "clientMarket.shop.priceAtActivation")}
                  </p>
                </section>

                {trialChoice ? (
                  <fieldset className="grid gap-2" disabled={locked}>
                    <legend className="mb-2 text-xs font-medium text-muted-foreground">{t("clientMarket.checkout.mode")}</legend>
                    <div className="grid grid-cols-2 gap-2">
                      {([
                        ["end", t("clientMarket.checkout.trialOnly"), t("clientMarket.checkout.trialOnlyHint", { duration: trialLength })],
                        ["continue", t("clientMarket.checkout.continue"), t("clientMarket.checkout.continueHint", { price })],
                      ] as const).map(([value, label, hint]) => (
                        <label
                          key={value}
                          className={cn(
                            "grid min-h-11 cursor-pointer content-start gap-0.5 rounded-xl border px-3 py-2.5 transition-all duration-200 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent has-[:focus-visible]:ring-offset-2",
                            afterTrial === value ? "border-accent bg-accent/5 shadow-[0_4px_14px_rgba(0,82,255,0.12)]" : "border-border hover:border-accent/30",
                            locked && "cursor-not-allowed opacity-60",
                          )}
                        >
                          <input
                            type="radio"
                            name="client-rent-after-trial"
                            value={value}
                            className="sr-only"
                            checked={afterTrial === value}
                            onChange={() => {
                              setAfterTrial(value);
                              setNotice("");
                              if (value === "end" && auto) {
                                setQuote(null);
                                void run(() => requote(selected!, false));
                              }
                            }}
                          />
                          <span className={cn("text-sm font-semibold", afterTrial === value && "text-accent")}>{label}</span>
                          <span className="text-xs leading-5 text-muted-foreground">{hint}</span>
                        </label>
                      ))}
                    </div>
                  </fieldset>
                ) : null}

                <ol className="grid gap-2" aria-label={t("clientMarket.checkout.timeline")}>
                  {timeline.map((line, index) => (
                    <li key={line} className="flex min-w-0 items-start gap-2.5 leading-5">
                      <span className={cn("mt-1.5 size-2 shrink-0 rounded-full", index === 0 ? "bg-accent" : "bg-border")} aria-hidden />
                      <span className="min-w-0">{line}</span>
                    </li>
                  ))}
                </ol>

                {paid && !trialOnly ? (
                  <label className={cn("flex min-h-11 items-center justify-between gap-3 rounded-xl border border-border px-3 py-2.5", locked ? "cursor-not-allowed opacity-60" : "cursor-pointer")}>
                    <span className="min-w-0">
                      <strong className="block font-medium">{t("online.renew")}</strong>
                      <span className="block truncate text-xs text-muted-foreground">{t("marketWallet.autoRenewAtDue")}</span>
                    </span>
                    <span className="relative shrink-0">
                      <input
                        type="checkbox"
                        className="peer sr-only"
                        checked={auto}
                        disabled={locked}
                        onChange={(event) => {
                          const renew = event.target.checked;
                          setAuto(renew);
                          setQuote(null);
                          void run(() => requote(selected!, renew));
                        }}
                      />
                      <span className="block h-6 w-11 rounded-full bg-slate-300 transition-colors after:absolute after:left-1 after:top-1 after:size-4 after:rounded-full after:bg-white after:content-[''] after:transition-transform peer-checked:bg-accent peer-checked:after:translate-x-5 peer-focus-visible:ring-2 peer-focus-visible:ring-accent peer-focus-visible:ring-offset-2" aria-hidden />
                    </span>
                  </label>
                ) : null}

                {paid && !trialOnly && fundingValid && funding ? (
                  <p className={cn("text-xs leading-5 tabular-nums", shortfall ? "font-medium text-destructive" : "text-muted-foreground")} aria-live="polite">
                    {shortfall
                      ? t("clientMarket.checkout.fundingShort", { balance: formatWalletUnits(funding.availableUnits, "$", locale), missing: formatWalletUnits(funding.shortfallUnits, "$", locale) })
                      : t("clientMarket.checkout.fundingOk", { balance: formatWalletUnits(funding.availableUnits, "$", locale) })}
                  </p>
                ) : null}
                {paid && !trialOnly && !fundingValid ? <p role="alert" className="text-destructive">{t("marketWallet.shortcutUnavailable")}</p> : null}

                {selfRental ? (
                  <p className="text-muted-foreground">{t("clientMarket.shop.selfNote")}</p>
                ) : (
                  <label className="flex items-start gap-2.5 rounded-xl bg-amber-50 px-3 py-2.5 leading-5 text-amber-950">
                    <input
                      className="mt-0.5 size-4 shrink-0 accent-[rgb(var(--router-primary))] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
                      type="checkbox"
                      checked={ack}
                      disabled={locked}
                      onChange={(event) => setAck(event.target.checked)}
                    />
                    <span className="flex gap-1.5">
                      <ShieldAlert className="mt-0.5 size-4 shrink-0 text-amber-600" aria-hidden />
                      {t("clientMarket.shop.rootConsent")}
                    </span>
                  </label>
                )}

                {notice ? <p role="status" className="rounded-lg bg-accent/5 px-3 py-2 text-accent">{notice}</p> : null}

                {paid ? (
                  <details className="group rounded-xl bg-muted/40 px-3">
                    <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-3 py-2 font-medium text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 [&::-webkit-details-marker]:hidden">
                      <span className="flex items-center gap-2">
                        <Info className="size-4 shrink-0" aria-hidden />
                        {t("clientMarket.shop.billingDetails")}
                      </span>
                      <ChevronDown className="size-4 shrink-0 transition-transform group-open:rotate-180 motion-reduce:transition-none" aria-hidden />
                    </summary>
                    <div className="grid gap-3 pb-3 pt-1 text-xs leading-5 text-muted-foreground">
                      <p>{t("clientMarket.checkout.billingRule")}</p>
                      <p>{t("clientTermination.quote", { rate: quote.terms.terminationPolicy.rateBps / 100 })}</p>
                      {fundingValid && funding ? (
                        <dl className="grid gap-1.5 tabular-nums">
                          <div className="flex justify-between gap-3">
                            <dt>{t("clientMarket.shop.firstPeriod")}</dt>
                            <dd>{formatWalletUnits(funding.requiredUnits, "$", locale)}</dd>
                          </div>
                          <div className="flex justify-between gap-3">
                            <dt>{t("marketWallet.available")}</dt>
                            <dd>{formatWalletUnits(funding.availableUnits, "$", locale)}</dd>
                          </div>
                          {funding.committedUnits !== "0" ? (
                            <div className="flex justify-between gap-3">
                              <dt>{t("clientMarket.shop.otherPayments")}</dt>
                              <dd>{formatWalletUnits(funding.committedUnits, "$", locale)}</dd>
                            </div>
                          ) : null}
                          {!trialOnly && funding.shortfallUnits !== "0" ? (
                            <div className="flex justify-between gap-3 font-semibold text-foreground">
                              <dt>{t("clientMarket.shop.balanceGap")}</dt>
                              <dd>{formatWalletUnits(funding.shortfallUnits, "$", locale)}</dd>
                            </div>
                          ) : null}
                        </dl>
                      ) : null}
                    </div>
                  </details>
                ) : null}
              </>
            ) : null}
            {error ? <p role="alert" className="text-destructive">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer className="shrink-0 flex-wrap justify-between gap-2 border-t border-border pt-3">
            {topupOpen ? (
              <Button className="min-h-11" variant="ghost" isDisabled={topupBusy} onPress={() => setTopupOpen(false)}>
                {t("clientMarket.shop.backToQuote")}
              </Button>
            ) : (
              <>
                <Button className="min-h-11" variant="ghost" isDisabled={busy || topupBusy} onPress={reset}>
                  {t("common.cancel")}
                </Button>
                {accessRequired && selected ? (
                  <Button
                    className="min-h-11"
                    variant="primary"
                    isDisabled={busy}
                    onPress={() =>
                      void run(async () => {
                        await createMarketAccessRequest({ targetKind: "client_host", targetId: selected.installationId });
                        toast.success(t("marketApproval.requested"));
                        reset();
                      })
                    }
                  >
                    {t("marketApproval.apply")}
                  </Button>
                ) : !quote || (paid && !trialOnly && !fundingValid) ? (
                  <Button className="min-h-11" variant="primary" isDisabled={busy} onPress={() => selected && void run(() => requote(selected, auto, true))}>
                    <RefreshCw className="size-4" aria-hidden />
                    {t("online.quote")}
                  </Button>
                ) : shortfall && funding ? (
                  <Button className="min-h-11" variant="primary" isDisabled={busy || topupBusy} onPress={() => setTopupOpen(true)}>
                    <Wallet className="size-4" aria-hidden />
                    {t("clientMarket.shop.topup")}
                  </Button>
                ) : (
                  <Button
                    className="min-h-11"
                    variant="primary"
                    isDisabled={busy || topupBusy || (!selfRental && !ack)}
                    onPress={() => void run(confirm)}
                  >
                    {busy ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
                    {confirmLabel}
                  </Button>
                )}
              </>
            )}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );

  const pendingBanner =
    pending && !selection ? (
      <div role="status" className="flex flex-wrap items-center gap-2 rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-950">
        <span className="min-w-0 flex-1">{t("lease.receipt")}</span>
        <Button size="sm" variant="outline" isDisabled={busy} onPress={retryPending}>
          {t("common.retry")}
        </Button>
        {error ? <span role="alert" className="w-full text-xs text-destructive">{error}</span> : null}
      </div>
    ) : null;

  return { open, dialog, pendingBanner, busy: busy && !!selection };
}
