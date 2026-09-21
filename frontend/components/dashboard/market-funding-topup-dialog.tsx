"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { AlertTriangle, Ban, CircleCheckBig, Copy, Loader2, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  cancelBinanceFundingIntent,
  createBinanceFundingIntent,
  getBinanceFundingIntent,
  refreshBinanceFundingIntent,
} from "@/lib/api";
import {
  defaultMarketFundingTopupMinor,
  formatFundingRunway,
  MAX_MARKET_FUNDING_TOPUP_MINOR,
  marketFundingDecisionState,
  marketFundingTopupErrorKey,
  marketFundingTopupUnavailableKey,
  marketFundingUsesCredit,
  type MarketFundingDecisionState,
} from "@/lib/market-funding";
import type { MessageKey } from "@/lib/i18n";
import type { BinanceFundingIntent, MarketFundingSummary } from "@/lib/types";
import { formatUsdMoney } from "@/lib/market-money";

function statusLabel(status: string, t: ReturnType<typeof useLocaleText>["t"]) {
  switch (status) {
    case "pending": return t("marketBilling.binance.status.pending");
    case "credited": return t("marketBilling.prepaid.topup.status.credited");
    case "expired": return t("marketBilling.binance.status.expired");
    case "cancelled": return t("marketBilling.binance.status.cancelled");
    case "review_required": return t("marketBilling.binance.status.review");
    default: return status.replaceAll("_", " ");
  }
}

const FUNDING_DECISION_KEYS: Record<MarketFundingDecisionState, MessageKey> = {
  ready: "marketFunding.decision.ready",
  low_runway: "marketFunding.decision.lowRunway",
  uses_credit: "marketFunding.decision.usesCredit",
  shortfall: "marketFunding.decision.shortfall",
};

export function MarketFundingDecisionCard({
  funding,
  onTopup,
  topupDisabled = false,
}: {
  funding: MarketFundingSummary;
  onTopup?: () => void;
  topupDisabled?: boolean;
}) {
  const { locale, t } = useLocaleText();
  const state = marketFundingDecisionState(funding);
  const credit = funding.creditKind === "unlimited"
    ? t("marketFunding.credit.unlimited")
    : funding.creditKind === "none"
      ? t("marketFunding.credit.none")
      : formatUsdMoney(funding.creditAvailableMinor ?? 0, locale);
  const runway = funding.projectedDailyRateMinor > 0
    ? formatFundingRunway(
      funding.estimatedRunwaySeconds,
      locale,
      funding.creditKind === "unlimited" ? t("marketFunding.runway.unlimited") : "-",
    )
    : "-";
  const statusTone = state === "shortfall"
    ? "border-rose-200 bg-rose-50/80"
    : state === "uses_credit" || state === "low_runway"
      ? "border-amber-200 bg-amber-50/70"
      : "border-slate-200 bg-slate-50";
  const statusIconTone = state === "shortfall"
    ? "text-rose-700"
    : state === "uses_credit" || state === "low_runway"
      ? "text-amber-700"
      : "text-emerald-700";
  const guidance = state === "shortfall"
    ? funding.topupAvailable
      ? t("marketFunding.topup.minimum", {
        amount: formatUsdMoney(funding.requiredTopupMinor, locale),
      })
      : t(marketFundingTopupUnavailableKey(funding.topupUnavailableReason))
    : state === "uses_credit"
      ? t("marketFunding.decision.creditWarning", {
        amount: formatUsdMoney(funding.creditCoverageMinor, locale),
      })
      : state === "low_runway"
        ? t("marketFunding.decision.lowRunwayWarning", { runway })
        : "";

  return (
    <section className={`grid gap-3 rounded-md border p-3 ${statusTone}`} aria-live="polite">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-2">
        <div className="flex min-w-0 items-start gap-2">
          {state === "ready"
            ? <CircleCheckBig className={`mt-0.5 h-4 w-4 shrink-0 ${statusIconTone}`} aria-hidden="true" />
            : <AlertTriangle className={`mt-0.5 h-4 w-4 shrink-0 ${statusIconTone}`} aria-hidden="true" />}
          <div className="min-w-0">
            <strong className="block text-sm text-slate-900">{t(FUNDING_DECISION_KEYS[state])}</strong>
            <span className="block truncate text-xs text-slate-600" title={funding.supplierEmail}>{funding.supplierEmail}</span>
          </div>
        </div>
        {onTopup ? (
          <Button
            size="sm"
            variant="ghost"
            className="min-h-11 shrink-0 whitespace-nowrap"
            isDisabled={topupDisabled}
            onClick={onTopup}
          >
            {t("marketFunding.topup.optionalAction")}
          </Button>
        ) : null}
      </div>
      <p className="text-sm leading-6 text-slate-700">
        {t("marketFunding.decision.runway", {
          rate: formatUsdMoney(funding.projectedDailyRateMinor, locale),
          runway,
        })}
      </p>
      <dl className="grid grid-cols-2 gap-3 border-t border-slate-200/80 pt-3 text-sm">
        <div className="min-w-0">
          <dt className="text-xs text-slate-600">{t("marketFunding.prepaidAvailable")}</dt>
          <dd className="mt-0.5 truncate font-semibold tabular-nums text-slate-950">
            {formatUsdMoney(funding.prepaidAvailableMinor, locale)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-xs text-slate-600">{t("marketFunding.creditAvailable")}</dt>
          <dd className="mt-0.5 truncate font-semibold tabular-nums text-slate-950">{credit}</dd>
        </div>
      </dl>
      {guidance ? <p className="text-xs leading-5 text-slate-700">{guidance}</p> : null}
    </section>
  );
}

export function MarketFundingSummaryCard({
  funding,
  className = "",
}: {
  funding: MarketFundingSummary;
  className?: string;
}) {
  const { locale, t } = useLocaleText();
  const credit = funding.creditKind === "unlimited"
    ? t("marketFunding.credit.unlimited")
    : funding.creditKind === "none"
      ? t("marketFunding.credit.none")
      : formatUsdMoney(funding.creditAvailableMinor ?? 0, locale);
  const creditLimit = funding.creditKind === "unlimited"
    ? t("marketFunding.credit.unlimited")
    : funding.creditKind === "none"
      ? t("marketFunding.credit.none")
      : formatUsdMoney(funding.creditLimitMinor ?? 0, locale);
  const prepaidRunway = funding.projectedDailyRateMinor > 0
    ? formatFundingRunway(funding.prepaidRunwaySeconds, locale, "-")
    : "-";
  const totalRunway = funding.projectedDailyRateMinor > 0
    ? formatFundingRunway(
      funding.estimatedRunwaySeconds,
      locale,
      funding.creditKind === "unlimited" ? t("marketFunding.runway.unlimited") : "-",
    )
    : "-";
  const usesCredit = marketFundingUsesCredit(funding);

  return (
    <section className={`grid gap-3 rounded-md border border-slate-200 bg-slate-50 p-3 ${className}`.trim()}>
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <strong className="block text-sm">{t("marketFunding.title")}</strong>
          <span className="block break-all text-xs text-slate-500">{funding.supplierEmail}</span>
        </div>
        <span className="rounded-full bg-white px-2 py-1 text-[11px] text-slate-600 ring-1 ring-slate-200">
          {funding.fundingMode === "prepaid" ? t("marketFunding.mode.prepaid") : t("marketFunding.mode.hybrid")}
        </span>
      </div>
      <div className="grid gap-3 border-y border-slate-200 py-3 text-sm lg:grid-cols-3">
        <div className="grid gap-2 sm:grid-cols-3">
          <div><span className="block text-xs text-slate-500">{t("marketFunding.prepaidTotal")}</span><strong className="tabular-nums">{formatUsdMoney(funding.prepaidBalanceMinor, locale)}</strong></div>
          <div><span className="block text-xs text-slate-500">{t("marketFunding.prepaidHeld")}</span><strong className="tabular-nums">{formatUsdMoney(funding.prepaidHeldMinor, locale)}</strong></div>
          <div><span className="block text-xs text-slate-500">{t("marketFunding.prepaidAvailable")}</span><strong className="tabular-nums text-emerald-700">{formatUsdMoney(funding.prepaidAvailableMinor, locale)}</strong></div>
        </div>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-2">
          <div><span className="block text-xs text-slate-500">{t("marketFunding.creditLimit")}</span><strong className="tabular-nums">{creditLimit}</strong></div>
          <div><span className="block text-xs text-slate-500">{t("marketFunding.creditUsed")}</span><strong className="tabular-nums">{formatUsdMoney(funding.creditOutstandingMinor, locale)}</strong></div>
          <div><span className="block text-xs text-slate-500">{t("marketFunding.creditReserved")}</span><strong className="tabular-nums">{formatUsdMoney(funding.creditReservedMinor, locale)}</strong></div>
          <div><span className="block text-xs text-slate-500">{t("marketFunding.creditAvailable")}</span><strong className="tabular-nums">{credit}</strong></div>
        </div>
        <div className="grid gap-2 sm:grid-cols-3">
          <div><span className="block text-xs text-slate-500">{t("marketFunding.rate.current")}</span><strong className="tabular-nums">{formatUsdMoney(funding.activeDailyRateMinor, locale)}</strong></div>
          <div><span className="block text-xs text-slate-500">{t("marketFunding.rate.new")}</span><strong className="tabular-nums">{formatUsdMoney(funding.additionalDailyRateMinor, locale)}</strong></div>
          <div><span className="block text-xs text-slate-500">{t("marketFunding.rate.projected")}</span><strong className="tabular-nums">{formatUsdMoney(funding.projectedDailyRateMinor, locale)}</strong></div>
        </div>
      </div>
      <div className="grid grid-cols-2 gap-x-4 gap-y-2 text-sm sm:grid-cols-4">
        <div><span className="block text-xs text-slate-500">{t("marketFunding.runway.prepaid")}</span><strong className="tabular-nums">{prepaidRunway}</strong></div>
        <div><span className="block text-xs text-slate-500">{t("marketFunding.runway.total")}</span><strong className="tabular-nums">{totalRunway}</strong></div>
        <div><span className="block text-xs text-slate-500">{t("marketFunding.coverage")}</span><strong className="tabular-nums">{formatUsdMoney(funding.requiredCoverageMinor, locale)}</strong></div>
        <div><span className="block text-xs text-slate-500">{t("marketFunding.shortfall")}</span><strong className={`tabular-nums ${funding.requiredTopupMinor > 0 ? "text-rose-700" : "text-emerald-700"}`}>{formatUsdMoney(funding.requiredTopupMinor, locale)}</strong></div>
      </div>
      <p className="text-xs leading-5 text-slate-600">{t("marketFunding.coverageSplit", {
        prepaid: formatUsdMoney(funding.prepaidCoverageMinor, locale),
        credit: formatUsdMoney(funding.creditCoverageMinor, locale),
      })}</p>
      {usesCredit ? <p className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-900">{t("marketFunding.creditWarning", { amount: formatUsdMoney(funding.creditCoverageMinor, locale) })}</p> : null}
      <p className="text-xs leading-5 text-slate-600">{t("marketFunding.policy")}</p>
    </section>
  );
}

export function MarketFundingTopupDialog({
  open,
  funding,
  onClose,
  onCredited,
}: {
  open: boolean;
  funding?: MarketFundingSummary;
  onClose: () => void;
  onCredited: (intent: BinanceFundingIntent) => void | Promise<void>;
}) {
  const { locale, t } = useLocaleText();
  const [amount, setAmount] = React.useState("");
  const [intent, setIntent] = React.useState<BinanceFundingIntent | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState("");
  const [clockMs, setClockMs] = React.useState(() => Date.now());
  const idempotencyRef = React.useRef<{ amountMinor: number; key: string } | null>(null);
  const creditedRef = React.useRef("");
  const intentEpochRef = React.useRef(0);
  const intentMutationRef = React.useRef(false);
  const intentPollInFlightRef = React.useRef(false);
  const fundingErrorText = React.useCallback((reason: unknown) => {
    const messageKey = marketFundingTopupErrorKey(reason);
    return messageKey ? t(messageKey) : reason instanceof Error ? reason.message : String(reason);
  }, [t]);

  React.useEffect(() => {
    if (!open) return;
    intentEpochRef.current += 1;
    intentMutationRef.current = false;
    intentPollInFlightRef.current = false;
    setAmount(funding ? (defaultMarketFundingTopupMinor(funding) / 100).toFixed(2) : "");
    setIntent(null);
    setError("");
    setBusy("");
    setClockMs(Date.now());
    idempotencyRef.current = null;
    creditedRef.current = "";
  }, [funding, open]);

  React.useEffect(() => {
    if (!open || !intent?.id || intent.status !== "pending") return;
    const controller = new AbortController();
    let active = true;
    const intentId = intent.id;
    const epoch = ++intentEpochRef.current;
    intentPollInFlightRef.current = false;
    const accept = (next: BinanceFundingIntent) => {
      if (!active || intentEpochRef.current !== epoch) return;
      setIntent(next);
      setError("");
    };
    const poll = window.setInterval(() => {
      if (intentMutationRef.current || intentPollInFlightRef.current) return;
      intentPollInFlightRef.current = true;
      void getBinanceFundingIntent(intentId, controller.signal)
        .then(accept)
        .catch((reason) => {
          if (!active || controller.signal.aborted || intentEpochRef.current !== epoch) return;
          setIntent((current) => current?.id === intentId
            ? { ...current, accountStatus: "unavailable" }
            : current);
          setError(fundingErrorText(reason));
        })
        .finally(() => {
          if (active && intentEpochRef.current === epoch) {
            intentPollInFlightRef.current = false;
          }
        });
    }, 3_000);
    const clock = window.setInterval(() => setClockMs(Date.now()), 1_000);
    return () => {
      active = false;
      controller.abort();
      window.clearInterval(poll);
      window.clearInterval(clock);
      intentPollInFlightRef.current = false;
    };
  }, [fundingErrorText, intent?.id, intent?.status, open]);

  React.useEffect(() => {
    if (!open || intent?.status !== "credited" || creditedRef.current === intent.id) return;
    creditedRef.current = intent.id;
    toast.success(t("marketBilling.prepaid.topup.credited"));
    void onCredited(intent);
  }, [intent?.id, intent?.status, onCredited, open, t]);

  const amountMinor = React.useMemo(() => {
    const normalized = amount.trim();
    if (!/^\d+(?:\.\d{1,2})?$/.test(normalized)) return null;
    const value = Number(normalized);
    const minor = Math.round(value * 100);
    return Number.isSafeInteger(minor)
      && minor > 0
      && minor <= MAX_MARKET_FUNDING_TOPUP_MINOR
      ? minor
      : null;
  }, [amount]);
  const minimumPresetMinor = Math.min(
    funding?.requiredTopupMinor ?? 0,
    MAX_MARKET_FUNDING_TOPUP_MINOR,
  );
  const recommendedTopupMinor = Math.max(
    funding?.recommendedTopupMinor ?? 0,
    funding?.requiredTopupMinor ?? 0,
  );
  const recommendedPresetMinor = Math.min(
    recommendedTopupMinor,
    MAX_MARKET_FUNDING_TOPUP_MINOR,
  );
  const partialRemainingMinor = funding && amountMinor != null
    ? Math.max(0, funding.requiredTopupMinor - amountMinor)
    : 0;
  const remainingSeconds = intent
    ? Math.max(0, Math.floor((Date.parse(intent.expiresAt) - clockMs) / 1_000))
    : 0;
  const canTransfer = intent?.status === "pending"
    && intent.accountStatus === "verified"
    && remainingSeconds > 0;
  const countdown = `${String(Math.floor(remainingSeconds / 60)).padStart(2, "0")}:${String(remainingSeconds % 60).padStart(2, "0")}`;

  const createIntent = async () => {
    if (!funding || amountMinor == null || busy) return;
    if (!idempotencyRef.current || idempotencyRef.current.amountMinor !== amountMinor) {
      idempotencyRef.current = { amountMinor, key: `market-funding:${crypto.randomUUID()}` };
    }
    const mutationEpoch = ++intentEpochRef.current;
    intentMutationRef.current = true;
    intentPollInFlightRef.current = false;
    setBusy("create");
    setError("");
    try {
      const next = await createBinanceFundingIntent({
        supplierUserId: funding.supplierUserId,
        amountMinor,
        idempotencyKey: idempotencyRef.current.key,
      });
      if (intentEpochRef.current !== mutationEpoch) return;
      setIntent(next);
      setClockMs(Date.now());
    } catch (reason) {
      if (intentEpochRef.current !== mutationEpoch) return;
      setError(fundingErrorText(reason));
    } finally {
      if (intentEpochRef.current === mutationEpoch) {
        intentMutationRef.current = false;
        setBusy("");
      }
    }
  };

  const refreshIntent = async () => {
    if (!intent || busy || !window.confirm(t("marketBilling.binance.refreshConfirm"))) return;
    const mutationEpoch = ++intentEpochRef.current;
    intentMutationRef.current = true;
    intentPollInFlightRef.current = false;
    setBusy("refresh");
    try {
      const next = await refreshBinanceFundingIntent(intent.id);
      if (intentEpochRef.current !== mutationEpoch) return;
      setIntent(next);
      idempotencyRef.current = null;
      setError("");
      setClockMs(Date.now());
    } catch (reason) {
      if (intentEpochRef.current !== mutationEpoch) return;
      setError(fundingErrorText(reason));
    } finally {
      if (intentEpochRef.current === mutationEpoch) {
        intentMutationRef.current = false;
        setBusy("");
      }
    }
  };

  const cancelIntent = async () => {
    if (!intent || busy || !window.confirm(t("marketBilling.binance.cancelConfirm"))) return;
    const mutationEpoch = ++intentEpochRef.current;
    intentMutationRef.current = true;
    intentPollInFlightRef.current = false;
    setBusy("cancel");
    try {
      const next = await cancelBinanceFundingIntent(intent.id);
      if (intentEpochRef.current !== mutationEpoch) return;
      setIntent(next);
      setError("");
    } catch (reason) {
      if (intentEpochRef.current !== mutationEpoch) return;
      setError(fundingErrorText(reason));
    } finally {
      if (intentEpochRef.current === mutationEpoch) {
        intentMutationRef.current = false;
        setBusy("");
      }
    }
  };

  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(t("marketBilling.binance.copied"));
    } catch {
      toast.danger(t("marketBilling.binance.copyFailed"));
    }
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !next && !busy && onClose()}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("marketBilling.dialog.topup")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[70vh] gap-4 overflow-y-auto">
            {funding ? (
              <div className="grid grid-cols-2 gap-3 rounded-md border border-slate-200 bg-slate-50 p-3 text-sm sm:grid-cols-4">
                <div><span className="block text-xs text-slate-500">{t("marketBilling.supplier")}</span><strong className="break-all">{funding.supplierEmail}</strong></div>
                <div><span className="block text-xs text-slate-500">{t("marketFunding.prepaidAvailable")}</span><strong>{formatUsdMoney(funding.prepaidAvailableMinor, locale)}</strong></div>
                <div><span className="block text-xs text-slate-500">{t("marketFunding.shortfall")}</span><strong>{formatUsdMoney(funding.requiredTopupMinor, locale)}</strong></div>
                <div><span className="block text-xs text-slate-500">{t("marketFunding.topup.recommended", { days: funding.recommendedCoverageDays })}</span><strong>{formatUsdMoney(recommendedTopupMinor, locale)}</strong></div>
              </div>
            ) : null}
            {!intent ? (
              <div className="grid gap-3">
                <div className="flex flex-wrap gap-2">
                  {funding && minimumPresetMinor > 0 ? <Button size="sm" variant="outline" onClick={() => { setAmount((minimumPresetMinor / 100).toFixed(2)); idempotencyRef.current = null; }}>{t(funding.requiredTopupMinor > MAX_MARKET_FUNDING_TOPUP_MINOR ? "marketFunding.topup.maximumPreset" : "marketFunding.topup.minimumPreset", { amount: formatUsdMoney(minimumPresetMinor, locale) })}</Button> : null}
                  {funding && recommendedPresetMinor > 0 && recommendedPresetMinor !== minimumPresetMinor ? <Button size="sm" variant="outline" onClick={() => { setAmount((recommendedPresetMinor / 100).toFixed(2)); idempotencyRef.current = null; }}>{t(recommendedTopupMinor > MAX_MARKET_FUNDING_TOPUP_MINOR ? "marketFunding.topup.maximumPreset" : "marketFunding.topup.recommendedPreset", { days: funding.recommendedCoverageDays, amount: formatUsdMoney(recommendedPresetMinor, locale) })}</Button> : null}
                </div>
                <label className="grid gap-1 text-sm">
                  <span className="text-slate-500">{t("marketFunding.topup.custom")}</span>
                  <input value={amount} onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }} inputMode="decimal" max={MAX_MARKET_FUNDING_TOPUP_MINOR / 100} className="h-10 rounded-md border border-slate-200 px-3 tabular-nums" autoFocus />
                  <span className="text-xs text-slate-500">{t("marketFunding.topup.transferLimit", { amount: formatUsdMoney(MAX_MARKET_FUNDING_TOPUP_MINOR, locale) })}</span>
                </label>
                {partialRemainingMinor > 0 ? <p className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-900">{t("marketFunding.topup.partialNotice", { remaining: formatUsdMoney(partialRemainingMinor, locale) })}</p> : null}
                <p className="rounded-md border border-sky-200 bg-sky-50 px-3 py-2 text-xs leading-5 text-sky-900">{t("marketFunding.topup.providerScope")}</p>
                <p className="text-xs leading-5 text-slate-500">{t("marketFunding.topup.refundDisclosure")}</p>
                {!funding?.topupAvailable ? <p className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-800">{t(marketFundingTopupUnavailableKey(funding?.topupUnavailableReason))}</p> : null}
              </div>
            ) : null}
            {error ? <div className="flex gap-2 rounded-md border border-rose-200 bg-rose-50 p-3 text-xs leading-5 text-rose-800"><AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />{error}</div> : null}
            {intent ? (
              <>
                <div className={`rounded-xl border p-4 text-center ${intent.status === "credited" ? "border-emerald-200 bg-emerald-50" : "border-amber-200 bg-amber-50/70"}`}>
                  {intent.status === "credited" ? <CircleCheckBig className="mx-auto h-8 w-8 text-emerald-600" /> : null}
                  <p className="text-xs font-medium text-slate-500">{canTransfer ? t("marketBilling.binance.exactAmount") : statusLabel(intent.status, t)}</p>
                  <strong className="mt-1 block text-3xl tabular-nums">{intent.payAmount}<span className="ml-2 text-base">{intent.asset}</span></strong>
                  {canTransfer ? <Button size="sm" variant="outline" className="mt-3" onClick={() => void copy(intent.payAmount)}><Copy className="h-4 w-4" />{t("marketBilling.binance.copyAmount")}</Button> : null}
                </div>
                <div className="divide-y divide-dashed divide-slate-200 rounded-lg border border-slate-200 px-3 text-sm">
                  {canTransfer ? <>
                    <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.receiverUid")}</span><span className="flex items-center gap-2"><strong className="break-all text-right">{intent.receiverUid}</strong><Button isIconOnly size="sm" variant="ghost" aria-label={t("marketBilling.binance.copyUid")} onClick={() => void copy(intent.receiverUid)}><Copy className="h-4 w-4" /></Button></span></div>
                    <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.noteCode")}</span><span className="flex items-center gap-2"><strong>{intent.noteCode}</strong><Button isIconOnly size="sm" variant="ghost" aria-label={t("marketBilling.binance.copyNote")} onClick={() => void copy(intent.noteCode)}><Copy className="h-4 w-4" /></Button></span></div>
                  </> : null}
                  <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.remaining")}</span><strong className="tabular-nums">{countdown}</strong></div>
                  <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.statusLabel")}</span><strong>{statusLabel(intent.status, t)}</strong></div>
                </div>
                {canTransfer ? <p className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-950">{t("marketBilling.binance.path")}</p> : null}
                <div className="flex justify-center gap-2">
                  {["pending", "expired", "cancelled"].includes(intent.status) ? <Button size="sm" variant="outline" isDisabled={!!busy || intent.accountStatus !== "verified"} onClick={() => void refreshIntent()}>{busy === "refresh" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}{t("marketBilling.binance.refresh")}</Button> : null}
                  {intent.status === "pending" ? <Button size="sm" variant="outline" isDisabled={!!busy} onClick={() => void cancelIntent()}>{busy === "cancel" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Ban className="h-4 w-4" />}{t("marketBilling.binance.cancel")}</Button> : null}
                </div>
              </>
            ) : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={!!busy} onClick={onClose}>{intent ? t("common.close") : t("common.cancel")}</Button>
            {!intent ? <Button variant="primary" isDisabled={!!busy || amountMinor == null || !funding?.topupAvailable} onClick={() => void createIntent()}>{busy === "create" ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("marketBilling.prepaid.topup.create")}</Button> : null}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
