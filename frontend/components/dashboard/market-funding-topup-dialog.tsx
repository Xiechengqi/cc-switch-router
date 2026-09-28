"use client";

/* Hallmark · component: market funding top-up flow · genre: modern-minimal · theme: existing router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: pass (40–41) · pre-emit critique: P5 H5 E5 S5 R5 V4
 */

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { AlertTriangle, Ban, ChevronDown, CircleCheckBig, Copy, Loader2, RefreshCw } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
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
  isRecurringFunding,
  MAX_MARKET_FUNDING_TOPUP_MINOR,
  marketFundingDecisionState,
  marketFundingTopupErrorKey,
  marketFundingTopupUnavailableKey,
  marketFundingUsesCredit,
  type MarketFundingDecisionState,
  type MarketTopupFunding,
} from "@/lib/market-funding";
import type { MessageKey } from "@/lib/i18n";
import type {
  BinanceFundingIntent,
  MarketFundingSummary,
  MarketRecurringFundingSummary,
} from "@/lib/types";
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

export function MarketRecurringFundingSummaryCard({
  funding,
  className = "",
}: {
  funding: MarketRecurringFundingSummary;
  className?: string;
}) {
  const { locale, t } = useLocaleText();
  return (
    <section className={`grid gap-3 rounded-md border border-slate-200 bg-slate-50 p-3 ${className}`.trim()}>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <strong className="block text-sm">{t("marketRecurringFunding.title")}</strong>
          <span className="block truncate text-xs text-slate-500" title={funding.supplierEmail}>
            {funding.supplierEmail}
          </span>
        </div>
        <span className="rounded-full bg-white px-2 py-1 text-[11px] text-slate-600 ring-1 ring-slate-200">
          {t("marketRecurringFunding.prepaidOnly")}
        </span>
      </div>
      <dl className="grid grid-cols-2 gap-x-4 gap-y-3 border-y border-slate-200 py-3 text-sm sm:grid-cols-4">
        <div><dt className="text-xs text-slate-500">{t("marketFunding.prepaidTotal")}</dt><dd className="font-semibold tabular-nums">{formatUsdMoney(funding.prepaidBalanceMinor, locale)}</dd></div>
        <div><dt className="text-xs text-slate-500">{t("marketFunding.prepaidHeld")}</dt><dd className="font-semibold tabular-nums">{formatUsdMoney(funding.prepaidHeldMinor, locale)}</dd></div>
        <div><dt className="text-xs text-slate-500">{t("marketFunding.prepaidAvailable")}</dt><dd className="font-semibold tabular-nums text-emerald-700">{formatUsdMoney(funding.prepaidAvailableMinor, locale)}</dd></div>
        <div><dt className="text-xs text-slate-500">{t("marketRecurringFunding.shortfall")}</dt><dd className={`font-semibold tabular-nums ${funding.requiredTopupMinor > 0 ? "text-rose-700" : "text-emerald-700"}`}>{formatUsdMoney(funding.requiredTopupMinor, locale)}</dd></div>
        <div><dt className="text-xs text-slate-500">{t("marketRecurringFunding.firstPeriod")}</dt><dd className="font-semibold tabular-nums">{formatUsdMoney(funding.initialHoldMinor, locale)}</dd></div>
        <div><dt className="text-xs text-slate-500">{t("marketRecurringFunding.nextPeriod")}</dt><dd className="font-semibold tabular-nums">{formatUsdMoney(funding.renewalHoldMinor, locale)}</dd></div>
        <div className="col-span-2"><dt className="text-xs text-slate-500">{t("marketRecurringFunding.totalReserved")}</dt><dd className="font-semibold tabular-nums">{formatUsdMoney(funding.totalRequiredHoldMinor, locale)}</dd></div>
      </dl>
      <p className="text-xs leading-5 text-slate-600">{t("marketRecurringFunding.policy")}</p>
    </section>
  );
}

export function MarketFundingTopupFlow({
  active,
  funding,
  onClose,
  onBack,
  onDismissBlockedChange,
  onCredited,
  variant = "standalone",
}: {
  active: boolean;
  funding?: MarketTopupFunding;
  onClose: () => void;
  onBack?: () => void;
  onDismissBlockedChange?: (blocked: boolean) => void;
  onCredited: (intent: BinanceFundingIntent) => void | Promise<void>;
  variant?: "standalone" | "checkout";
}) {
  const { locale, t } = useLocaleText();
  const [amount, setAmount] = React.useState("");
  const [intent, setIntent] = React.useState<BinanceFundingIntent | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState("");
  const [refreshConfirmOpen, setRefreshConfirmOpen] = React.useState(false);
  const [cancelConfirmOpen, setCancelConfirmOpen] = React.useState(false);
  const [clockMs, setClockMs] = React.useState(() => Date.now());
  const idempotencyRef = React.useRef<{ amountMinor: number; key: string } | null>(null);
  const creditedRef = React.useRef("");
  const intentEpochRef = React.useRef(0);
  const intentMutationRef = React.useRef(false);
  const intentPollInFlightRef = React.useRef(false);
  const dismissalBlocked = !!busy
    || refreshConfirmOpen
    || cancelConfirmOpen
    || (variant === "checkout" && intent?.status === "pending");
  const fundingErrorText = React.useCallback((reason: unknown) => {
    const messageKey = marketFundingTopupErrorKey(reason);
    return messageKey ? t(messageKey) : reason instanceof Error ? reason.message : String(reason);
  }, [t]);

  React.useEffect(() => {
    onDismissBlockedChange?.(dismissalBlocked);
    return () => onDismissBlockedChange?.(false);
  }, [dismissalBlocked, onDismissBlockedChange]);

  React.useEffect(() => {
    if (!active) return;
    intentEpochRef.current += 1;
    intentMutationRef.current = false;
    intentPollInFlightRef.current = false;
    const initialMinor = funding
      ? variant === "checkout" && funding.requiredTopupMinor > 0
        ? Math.min(funding.requiredTopupMinor, MAX_MARKET_FUNDING_TOPUP_MINOR)
        : defaultMarketFundingTopupMinor(funding)
      : 0;
    setAmount(initialMinor > 0 ? (initialMinor / 100).toFixed(2) : "");
    setIntent(null);
    setError("");
    setBusy("");
    setRefreshConfirmOpen(false);
    setCancelConfirmOpen(false);
    setClockMs(Date.now());
    idempotencyRef.current = null;
    creditedRef.current = "";
  }, [active, funding, variant]);

  React.useEffect(() => {
    if (!active || !intent?.id || intent.status !== "pending") return;
    const controller = new AbortController();
    let pollActive = true;
    const intentId = intent.id;
    const epoch = ++intentEpochRef.current;
    intentPollInFlightRef.current = false;
    const accept = (next: BinanceFundingIntent) => {
      if (!pollActive || intentEpochRef.current !== epoch) return;
      setIntent(next);
      setError("");
    };
    const poll = window.setInterval(() => {
      if (intentMutationRef.current || intentPollInFlightRef.current) return;
      intentPollInFlightRef.current = true;
      void getBinanceFundingIntent(intentId, controller.signal)
        .then(accept)
        .catch((reason) => {
          if (!pollActive || controller.signal.aborted || intentEpochRef.current !== epoch) return;
          setIntent((current) => current?.id === intentId
            ? { ...current, accountStatus: "unavailable" }
            : current);
          setError(fundingErrorText(reason));
        })
        .finally(() => {
          if (pollActive && intentEpochRef.current === epoch) {
            intentPollInFlightRef.current = false;
          }
        });
    }, 3_000);
    const clock = window.setInterval(() => setClockMs(Date.now()), 1_000);
    return () => {
      pollActive = false;
      controller.abort();
      window.clearInterval(poll);
      window.clearInterval(clock);
      intentPollInFlightRef.current = false;
    };
  }, [active, fundingErrorText, intent?.id, intent?.status]);

  React.useEffect(() => {
    if (!active || intent?.status !== "credited" || creditedRef.current === intent.id) return;
    creditedRef.current = intent.id;
    if (variant === "standalone") toast.success(t("marketBilling.prepaid.topup.credited"));
    void onCredited(intent);
  }, [active, intent?.id, intent?.status, onCredited, t, variant]);

  React.useEffect(() => {
    if (intent?.status !== "pending") setCancelConfirmOpen(false);
  }, [intent?.status]);

  React.useEffect(() => {
    if (!["pending", "expired", "cancelled"].includes(intent?.status ?? "")) {
      setRefreshConfirmOpen(false);
    }
  }, [intent?.status]);

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
  const amountInvalid = amount.trim().length > 0 && amountMinor == null;
  const minimumPresetMinor = Math.min(
    funding?.requiredTopupMinor ?? 0,
    MAX_MARKET_FUNDING_TOPUP_MINOR,
  );
  const recurringFunding = funding && isRecurringFunding(funding) ? funding : undefined;
  const meteredFunding = funding && !isRecurringFunding(funding) ? funding : undefined;
  const requiredNowMinor = recurringFunding?.totalRequiredHoldMinor
    ?? meteredFunding?.requiredCoverageMinor
    ?? 0;
  const recommendedTopupMinor = Math.max(
    meteredFunding?.recommendedTopupMinor ?? recurringFunding?.requiredTopupMinor ?? 0,
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
    if (!intent || busy) return;
    setRefreshConfirmOpen(false);
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
    if (!intent || busy) return;
    setCancelConfirmOpen(false);
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
    <>
      <Modal.Header>
        <div className="min-w-0 pr-8">
          <Modal.Heading>
            {t(variant === "checkout" ? "marketFunding.topup.checkoutTitle" : "marketBilling.dialog.topup")}
          </Modal.Heading>
          {variant === "checkout" && funding ? (
            <p className="mt-1 truncate text-xs text-slate-500" title={funding.supplierEmail}>
              {funding.supplierEmail}
            </p>
          ) : null}
        </div>
      </Modal.Header>
      <Modal.Body className="grid max-h-[min(72dvh,640px)] gap-4 overflow-y-auto">
        {funding ? variant === "checkout" ? (
          <dl className="grid grid-cols-3 gap-3 py-3 text-sm">
            <div className="min-w-0">
              <dt className="text-[11px] leading-4 text-slate-500">{t("marketFunding.prepaidAvailable")}</dt>
              <dd className="mt-1 truncate font-semibold tabular-nums text-slate-950">{formatUsdMoney(funding.prepaidAvailableMinor, locale)}</dd>
            </div>
            <div className="min-w-0">
              <dt className="text-[11px] leading-4 text-slate-500">{t("marketFunding.topup.requiredNow")}</dt>
              <dd className="mt-1 truncate font-semibold tabular-nums text-slate-950">{formatUsdMoney(requiredNowMinor, locale)}</dd>
            </div>
            <div className="min-w-0">
              <dt className="text-[11px] leading-4 text-slate-500">{t("marketFunding.shortfall")}</dt>
              <dd className="mt-1 truncate font-semibold tabular-nums text-rose-700">{formatUsdMoney(funding.requiredTopupMinor, locale)}</dd>
            </div>
          </dl>
        ) : (
          <div className="grid grid-cols-2 gap-3 rounded-md border border-slate-200 bg-slate-50 p-3 text-sm sm:grid-cols-4">
            <div><span className="block text-xs text-slate-500">{t("marketBilling.supplier")}</span><strong className="break-all">{funding.supplierEmail}</strong></div>
            <div><span className="block text-xs text-slate-500">{t("marketFunding.prepaidAvailable")}</span><strong>{formatUsdMoney(funding.prepaidAvailableMinor, locale)}</strong></div>
            <div><span className="block text-xs text-slate-500">{t("marketFunding.shortfall")}</span><strong>{formatUsdMoney(funding.requiredTopupMinor, locale)}</strong></div>
            <div><span className="block text-xs text-slate-500">{recurringFunding ? t("marketRecurringFunding.topupTarget") : t("marketFunding.topup.recommended", { days: meteredFunding?.recommendedCoverageDays ?? 0 })}</span><strong>{formatUsdMoney(recommendedTopupMinor, locale)}</strong></div>
          </div>
        ) : null}

        {!intent ? variant === "checkout" ? (
          <div className="grid gap-4">
            <div className="py-2 text-center">
              <span className="block text-xs font-medium text-slate-500">{t("marketFunding.topup.transferAmount")}</span>
              <strong className="mt-1 block text-3xl font-semibold tracking-tight tabular-nums text-slate-950">
                {amountMinor == null ? "—" : formatUsdMoney(amountMinor, locale)}
              </strong>
              <span className="mt-1 block text-xs text-slate-500">
                {t(amountMinor === funding?.requiredTopupMinor
                  ? "marketFunding.topup.exactShortfall"
                  : "marketFunding.topup.customSelected")}
              </span>
            </div>
            <details className="group rounded-md bg-slate-50 px-3">
              <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between py-2 text-sm font-medium text-slate-700 hover:text-slate-950 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary active:text-slate-950 [&::-webkit-details-marker]:hidden">
                {t("marketFunding.topup.adjustAmount")}
                <span className="flex items-center gap-2 text-xs font-normal text-slate-500">
                  <span className="group-open:hidden">{t("marketFunding.topup.optional")}</span>
                  <ChevronDown className="h-4 w-4 transition-transform group-open:rotate-180" aria-hidden="true" />
                </span>
              </summary>
              <div className="grid gap-2 pb-2">
                <label className="grid gap-1 text-sm">
                  <span className="text-slate-500">{t("marketFunding.topup.custom")}</span>
                  <input value={amount} onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }} inputMode="decimal" max={MAX_MARKET_FUNDING_TOPUP_MINOR / 100} aria-invalid={amountInvalid} className="min-h-11 rounded-md border border-slate-300 px-3 tabular-nums focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary aria-invalid:border-rose-400" />
                  <span className={`min-h-5 text-xs ${amountInvalid ? "text-rose-700" : "text-slate-500"}`}>
                    {amountInvalid
                      ? t("marketBilling.prepaid.amountInvalid")
                      : t("marketFunding.topup.transferLimit", { amount: formatUsdMoney(MAX_MARKET_FUNDING_TOPUP_MINOR, locale) })}
                  </span>
                </label>
                <p className="text-xs leading-5 text-slate-500">{t("marketFunding.topup.providerScope")}</p>
                <p className="text-xs leading-5 text-slate-500">{t("marketFunding.topup.refundDisclosure")}</p>
              </div>
            </details>
            {partialRemainingMinor > 0 ? <p className="rounded-md bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-900">{t("marketFunding.topup.partialNotice", { remaining: formatUsdMoney(partialRemainingMinor, locale) })}</p> : null}
            {!funding?.topupAvailable ? <p className="rounded-md bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-800">{t(marketFundingTopupUnavailableKey(funding?.topupUnavailableReason))}</p> : null}
          </div>
        ) : (
          <div className="grid gap-3">
            <div className="flex flex-wrap gap-2">
              {funding && minimumPresetMinor > 0 ? <Button size="sm" variant="outline" onClick={() => { setAmount((minimumPresetMinor / 100).toFixed(2)); idempotencyRef.current = null; }}>{t(funding.requiredTopupMinor > MAX_MARKET_FUNDING_TOPUP_MINOR ? "marketFunding.topup.maximumPreset" : "marketFunding.topup.minimumPreset", { amount: formatUsdMoney(minimumPresetMinor, locale) })}</Button> : null}
              {meteredFunding && recommendedPresetMinor > 0 && recommendedPresetMinor !== minimumPresetMinor ? <Button size="sm" variant="outline" onClick={() => { setAmount((recommendedPresetMinor / 100).toFixed(2)); idempotencyRef.current = null; }}>{t(recommendedTopupMinor > MAX_MARKET_FUNDING_TOPUP_MINOR ? "marketFunding.topup.maximumPreset" : "marketFunding.topup.recommendedPreset", { days: meteredFunding.recommendedCoverageDays, amount: formatUsdMoney(recommendedPresetMinor, locale) })}</Button> : null}
            </div>
            <label className="grid gap-1 text-sm">
              <span className="text-slate-500">{t("marketFunding.topup.custom")}</span>
              <input value={amount} onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }} inputMode="decimal" max={MAX_MARKET_FUNDING_TOPUP_MINOR / 100} aria-invalid={amountInvalid} className="min-h-11 rounded-md border border-slate-200 px-3 tabular-nums aria-invalid:border-rose-400" autoFocus />
              <span className={`min-h-5 text-xs ${amountInvalid ? "text-rose-700" : "text-slate-500"}`}>
                {amountInvalid
                  ? t("marketBilling.prepaid.amountInvalid")
                  : t("marketFunding.topup.transferLimit", { amount: formatUsdMoney(MAX_MARKET_FUNDING_TOPUP_MINOR, locale) })}
              </span>
            </label>
            {partialRemainingMinor > 0 ? <p className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-900">{t("marketFunding.topup.partialNotice", { remaining: formatUsdMoney(partialRemainingMinor, locale) })}</p> : null}
            <p className="rounded-md border border-sky-200 bg-sky-50 px-3 py-2 text-xs leading-5 text-sky-900">{t("marketFunding.topup.providerScope")}</p>
            <p className="text-xs leading-5 text-slate-500">{t("marketFunding.topup.refundDisclosure")}</p>
            {!funding?.topupAvailable ? <p className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-800">{t(marketFundingTopupUnavailableKey(funding?.topupUnavailableReason))}</p> : null}
          </div>
        ) : null}

        {error ? <div className="flex gap-2 rounded-md bg-rose-50 p-3 text-xs leading-5 text-rose-800"><AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />{error}</div> : null}
        {intent ? (
          <>
            <div className={`rounded-lg px-3 py-4 text-center ${intent.status === "credited" ? "bg-emerald-50" : "bg-amber-50/70"}`}>
              {intent.status === "credited" ? <CircleCheckBig className="mx-auto h-8 w-8 text-emerald-600" /> : null}
              <p className="text-xs font-medium text-slate-500">{canTransfer ? t("marketBilling.binance.exactAmount") : statusLabel(intent.status, t)}</p>
              <strong className="mt-1 block text-3xl tabular-nums">{intent.payAmount}<span className="ml-2 text-base">{intent.asset}</span></strong>
              {canTransfer ? <Button size="sm" variant="outline" className="mt-3 min-h-11" onClick={() => void copy(intent.payAmount)}><Copy className="h-4 w-4" />{t("marketBilling.binance.copyAmount")}</Button> : null}
            </div>
            <div className="grid gap-1 rounded-lg bg-slate-50 px-3 py-2 text-sm">
              {canTransfer ? <>
                <div className="flex min-h-12 items-center justify-between gap-3 py-2"><span className="text-slate-500">{t("marketBilling.binance.receiverUid")}</span><span className="flex min-w-0 items-center gap-1"><strong className="truncate text-right">{intent.receiverUid}</strong><Button isIconOnly size="sm" variant="ghost" className="min-h-11 min-w-11" aria-label={t("marketBilling.binance.copyUid")} onClick={() => void copy(intent.receiverUid)}><Copy className="h-4 w-4" /></Button></span></div>
                <div className="flex min-h-12 items-center justify-between gap-3 py-2"><span className="text-slate-500">{t("marketBilling.binance.noteCode")}</span><span className="flex items-center gap-1"><strong>{intent.noteCode}</strong><Button isIconOnly size="sm" variant="ghost" className="min-h-11 min-w-11" aria-label={t("marketBilling.binance.copyNote")} onClick={() => void copy(intent.noteCode)}><Copy className="h-4 w-4" /></Button></span></div>
              </> : null}
              <div className="flex min-h-12 items-center justify-between gap-3 py-2"><span className="text-slate-500">{t("marketBilling.binance.remaining")}</span><strong className="tabular-nums">{countdown}</strong></div>
              <div className="flex min-h-12 items-center justify-between gap-3 py-2"><span className="text-slate-500">{t("marketBilling.binance.statusLabel")}</span><strong>{statusLabel(intent.status, t)}</strong></div>
            </div>
            {canTransfer ? <p className="rounded-md bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-950">{t("marketBilling.binance.path")}</p> : null}
            <div className="flex flex-wrap justify-center gap-2">
              {["pending", "expired", "cancelled"].includes(intent.status) ? <Button size="sm" variant="outline" className="min-h-11" isDisabled={!!busy || intent.accountStatus !== "verified"} onClick={() => setRefreshConfirmOpen(true)}>{busy === "refresh" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}{t("marketBilling.binance.refresh")}</Button> : null}
              {intent.status === "pending" ? <Button size="sm" variant="outline" className="min-h-11" isDisabled={!!busy} onClick={() => setCancelConfirmOpen(true)}>{busy === "cancel" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Ban className="h-4 w-4" />}{t("marketBilling.binance.cancel")}</Button> : null}
            </div>
          </>
        ) : null}
      </Modal.Body>
      <Modal.Footer className="flex-wrap justify-between">
        <Button className="min-h-11 whitespace-nowrap" variant="ghost" isDisabled={dismissalBlocked} onClick={onBack ?? onClose}>
          {onBack ? t("marketFunding.topup.backToRental") : intent ? t("common.close") : t("common.cancel")}
        </Button>
        {!intent ? <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={!!busy || amountMinor == null || !funding?.topupAvailable} onClick={() => void createIntent()}>{busy === "create" ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("marketBilling.prepaid.topup.create")}</Button> : null}
      </Modal.Footer>
      <ConfirmAlertDialog
        open={refreshConfirmOpen}
        title={t("marketBilling.binance.refresh")}
        description={t("marketBilling.binance.refreshConfirm")}
        confirmLabel={t("marketBilling.binance.refresh")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "refresh"}
        onConfirm={() => void refreshIntent()}
        onOpenChange={(open) => !busy && setRefreshConfirmOpen(open)}
      />
      <ConfirmAlertDialog
        open={cancelConfirmOpen}
        title={t("marketBilling.binance.cancel")}
        description={t("marketBilling.binance.cancelConfirm")}
        confirmLabel={t("marketBilling.binance.cancel")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "cancel"}
        onConfirm={() => void cancelIntent()}
        onOpenChange={(open) => !busy && setCancelConfirmOpen(open)}
      />
    </>
  );
}

export function MarketFundingTopupDialog({
  open,
  funding,
  onClose,
  onCredited,
}: {
  open: boolean;
  funding?: MarketTopupFunding;
  onClose: () => void;
  onCredited: (intent: BinanceFundingIntent) => void | Promise<void>;
}) {
  const [dismissBlocked, setDismissBlocked] = React.useState(false);
  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !next && !dismissBlocked && onClose()}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light max-h-[calc(100dvh-1rem)] w-[min(560px,calc(100vw-1rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
          <MarketFundingTopupFlow
            active={open}
            funding={funding}
            onClose={onClose}
            onDismissBlockedChange={setDismissBlocked}
            onCredited={onCredited}
          />
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
