"use client";

import * as React from "react";
import { Button, Chip, toast } from "@heroui/react";
import { CalendarClock, Loader2, RotateCcw, WalletCards } from "lucide-react";
import { MarketFundingTopupDialog } from "@/components/dashboard/market-funding-topup-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getMarketBillingDashboard,
  reserveNextMarketRecurringPeriod,
  updateMarketRecurringRenewal,
} from "@/lib/api";
import { ApiError } from "@/lib/api";
import { formatUsdMoney } from "@/lib/market-money";
import type {
  MarketPrepaidAccount,
  MarketRecurringContract,
  MarketRecurringFundingSummary,
} from "@/lib/types";

function formatDateTime(value: string | undefined, locale: string) {
  if (!value) return "—";
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return value;
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(parsed));
}

function topupFundingForContract(
  contract: MarketRecurringContract,
  account?: MarketPrepaidAccount,
): MarketRecurringFundingSummary {
  const available = account?.availableMinor ?? 0;
  const nextHeld = Math.min(contract.cyclePriceMinor, contract.nextPeriodHeldMinor);
  // When the next period is already funded, a voluntary top-up creates one
  // further month of buffer instead of claiming that the current hold is short.
  const target = nextHeld >= contract.cyclePriceMinor
    ? contract.cyclePriceMinor
    : contract.cyclePriceMinor - nextHeld;
  return {
    supplierUserId: contract.supplierUserId,
    supplierEmail: contract.supplierEmail,
    currency: "USD",
    pricingModel: "prepaid_calendar_month",
    billingInterval: "calendar_month",
    cyclePriceMinor: contract.cyclePriceMinor,
    renewalPolicy: contract.renewalPolicy,
    prepaidAccountId: account?.id,
    prepaidBalanceMinor: account?.balanceMinor ?? 0,
    prepaidHeldMinor: account?.heldMinor ?? 0,
    prepaidAvailableMinor: available,
    initialHoldMinor: 0,
    renewalHoldMinor: nextHeld,
    totalRequiredHoldMinor: target,
    requiredTopupMinor: Math.max(0, target - available),
    topupAvailable: account?.topupAvailable ?? false,
    topupUnavailableReason: account?.topupUnavailableReason ?? "supplier_unavailable",
  };
}

export function MarketRecurringContractCard({
  contract,
  onChanged,
  compact = false,
  className = "",
}: {
  contract: MarketRecurringContract;
  onChanged: () => Promise<void> | void;
  compact?: boolean;
  className?: string;
}) {
  const { locale, t } = useLocaleText();
  const [current, setCurrent] = React.useState(contract);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [topupFunding, setTopupFunding] = React.useState<MarketRecurringFundingSummary>();

  React.useEffect(() => setCurrent(contract), [contract]);

  const ended = current.status === "ended" || current.status === "activation_failed";
  const nextFunded = current.nextPeriodHeldMinor >= current.cyclePriceMinor;
  const automatic = current.renewalPolicy === "automatic";
  const compactLifecycle = current.cancelAtPeriodEnd
    ? t("marketRecurring.compact.cancelAt", {
        time: formatDateTime(current.currentPeriodEnd, locale),
      })
    : current.status === "recovery"
      ? t("marketRecurring.compact.recoveryBy", {
          time: formatDateTime(current.recoveryDeadline, locale),
        })
      : current.status === "trial"
        ? t("marketRecurring.compact.trialUntil", {
            time: formatDateTime(current.trialEndsAt, locale),
          })
        : t("marketRecurring.compact.renewsAt", {
            time: formatDateTime(current.currentPeriodEnd, locale),
          });

  const loadTopup = React.useCallback(async () => {
    setBusy("topup");
    setError("");
    try {
      const dashboard = await getMarketBillingDashboard();
      const account = dashboard.prepaidAccounts.find((item) =>
        item.isBuyer
        && item.supplierUserId === current.supplierUserId
        && item.currency === current.currency,
      );
      setTopupFunding(topupFundingForContract(current, account));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy("");
    }
  }, [current]);

  const run = async (
    kind: string,
    operation: () => Promise<MarketRecurringContract>,
    successKey: "marketRecurring.updated" | "marketRecurring.reserved",
  ) => {
    if (busy) return;
    setBusy(kind);
    setError("");
    try {
      const next = await operation();
      setCurrent(next);
      toast.success(t(successKey));
      await onChanged();
    } catch (reason) {
      if (reason instanceof ApiError && reason.code === "MARKET_PREPAID_REQUIRED") {
        setError(t("marketRecurring.fundingRequired"));
        await loadTopup();
      } else {
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      setBusy("");
    }
  };

  const setAutomatic = (enabled: boolean) => run(
    "renewal",
    () => updateMarketRecurringRenewal(current.id, {
      renewalPolicy: enabled ? "automatic" : "manual",
      autoRenewMaxPriceMinor: current.cyclePriceMinor,
      renewalPriority: current.renewalPriority,
    }),
    "marketRecurring.updated",
  );

  const reactivate = () => run(
    "reactivate",
    () => updateMarketRecurringRenewal(current.id, {
      renewalPolicy: current.renewalPolicy === "automatic" ? "automatic" : "manual",
      autoRenewMaxPriceMinor: current.cyclePriceMinor,
      renewalPriority: current.renewalPriority,
    }),
    "marketRecurring.updated",
  );

  const reserve = () => run(
    "reserve",
    () => reserveNextMarketRecurringPeriod(current.id),
    "marketRecurring.reserved",
  );

  return (
    <>
      <section className={`grid gap-3 rounded-lg border border-slate-200 bg-slate-50 p-3 ${className}`.trim()}>
        <div className="flex min-w-0 flex-wrap items-start justify-between gap-2">
          <div className="flex min-w-0 items-start gap-2">
            <CalendarClock className="mt-0.5 h-4 w-4 shrink-0 text-slate-500" aria-hidden />
            <div className="min-w-0">
              <strong className="block text-sm text-slate-900">
                {formatUsdMoney(current.cyclePriceMinor, locale)} / {t("marketBilling.month")}
              </strong>
              <span className="block truncate text-xs text-slate-500" title={current.supplierEmail}>
                {current.supplierEmail}
              </span>
            </div>
          </div>
          <Chip size="sm" variant="soft">
            {t(`marketRecurring.status.${current.status}` as Parameters<typeof t>[0])}
          </Chip>
        </div>

        {!compact ? (
          <dl className="grid grid-cols-2 gap-x-4 gap-y-2 border-y border-slate-200 py-3 text-xs sm:grid-cols-4">
            <div className="min-w-0"><dt className="text-slate-500">{t("marketRecurring.periodStart")}</dt><dd className="mt-0.5 font-medium text-slate-800">{formatDateTime(current.currentPeriodStart, locale)}</dd></div>
            <div className="min-w-0"><dt className="text-slate-500">{t("marketRecurring.periodEnd")}</dt><dd className="mt-0.5 font-medium text-slate-800">{formatDateTime(current.currentPeriodEnd, locale)}</dd></div>
            <div className="min-w-0"><dt className="text-slate-500">{t("marketRecurring.nextHeld")}</dt><dd className={`mt-0.5 font-medium ${nextFunded ? "text-emerald-700" : "text-amber-700"}`}>{formatUsdMoney(current.nextPeriodHeldMinor, locale)}</dd></div>
            <div className="min-w-0"><dt className="text-slate-500">{t("marketRecurring.renewalMode")}</dt><dd className="mt-0.5 font-medium text-slate-800">{t(automatic ? "marketRecurring.automatic" : "marketRecurring.manual")}</dd></div>
            {current.trialEndsAt ? <div className="col-span-2"><dt className="text-slate-500">{t("marketRecurring.trialEnds")}</dt><dd className="mt-0.5 font-medium text-slate-800">{formatDateTime(current.trialEndsAt, locale)}</dd></div> : null}
            {current.recoveryDeadline ? <div className="col-span-2"><dt className="text-rose-600">{t("marketRecurring.recoveryDeadline")}</dt><dd className="mt-0.5 font-medium text-rose-700">{formatDateTime(current.recoveryDeadline, locale)}</dd></div> : null}
          </dl>
        ) : (
          <p className="text-xs leading-5 text-slate-600">{compactLifecycle}</p>
        )}

        {current.cancelAtPeriodEnd ? (
          <p className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-900">
            {t("marketRecurring.cancelScheduled", { time: formatDateTime(current.currentPeriodEnd, locale) })}
          </p>
        ) : current.status === "recovery" ? (
          <p className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-800">
            {t("marketRecurring.recoveryHint")}
          </p>
        ) : null}

        {!ended ? (
          <div className="flex flex-wrap items-center gap-2">
            {current.cancelAtPeriodEnd ? (
              <Button size="sm" variant="outline" isDisabled={!!busy} onClick={() => void reactivate()}>
                {busy === "reactivate" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
                {t("marketRecurring.keepService")}
              </Button>
            ) : (
              <Button size="sm" variant="outline" isDisabled={!!busy} onClick={() => void setAutomatic(!automatic)}>
                {busy === "renewal" ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t(automatic ? "marketRecurring.disableAutoRenew" : "marketRecurring.enableAutoRenew")}
              </Button>
            )}
            {!automatic && !nextFunded && !current.cancelAtPeriodEnd ? (
              <Button size="sm" variant="outline" isDisabled={!!busy} onClick={() => void reserve()}>
                {busy === "reserve" ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("marketRecurring.reserveNext")}
              </Button>
            ) : null}
            <Button size="sm" variant="ghost" isDisabled={!!busy} onClick={() => void loadTopup()}>
              {busy === "topup" ? <Loader2 className="h-4 w-4 animate-spin" /> : <WalletCards className="h-4 w-4" />}
              {t("marketFunding.topup.optionalAction")}
            </Button>
          </div>
        ) : null}
        {error ? <p role="alert" className="text-xs leading-5 text-rose-700">{error}</p> : null}
      </section>

      <MarketFundingTopupDialog
        open={!!topupFunding}
        funding={topupFunding}
        onClose={() => setTopupFunding(undefined)}
        onCredited={async () => {
          setTopupFunding(undefined);
          setError("");
          await onChanged();
        }}
      />
    </>
  );
}
