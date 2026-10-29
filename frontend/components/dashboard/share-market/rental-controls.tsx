"use client";
import { walletLifecycleKey, walletReleaseScheduled, trialHours } from "@/lib/client-checkout";
import { shareTrialSummary } from "@/lib/share-trial";

import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { Button } from "@heroui/react";
import { Loader2, RotateCcw } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { rentalTermProgress } from "@/components/dashboard/share-market/subscription-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { releaseShareMarketSubscription } from "@/lib/api";
import { formatUsdMoney } from "@/lib/market-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { ShareMarketSubscription } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  isCoreShareApp,
  shareMarketMutationError,
} from "@/components/dashboard/share-market/market-utils";

type Translate = ReturnType<typeof useLocaleText>["t"];

function formatUtcDateTime(value: string | null | undefined, locale: string) {
  if (!value) return "-";
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return value;
  return `${new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone: "UTC",
  }).format(new Date(timestamp))} UTC`;
}

function rentalApps(subscription: ShareMarketSubscription) {
  return [...new Set(
    (subscription.apps?.length ? subscription.apps : [subscription.appType]).filter(isCoreShareApp),
  )];
}

export function rentalPrice(subscription: ShareMarketSubscription, locale: string, t: Translate) {
  if (
    subscription.pricingModel === "prepaid_calendar_month"
    && subscription.cyclePriceMinor != null
  ) {
    return `${formatUsdMoney(subscription.cyclePriceMinor, locale)} / ${t("market.month")}`;
  }
  if (
    subscription.pricingModel === "free"
    || subscription.dailyRateMinor == null
  ) {
    return t("shareMarket.free");
  }
  return `${formatUsdMoney(subscription.dailyRateMinor, locale)} / ${t("market.day")}`;
}

/** The term frozen into the rental at rent time, independent of what the listing says today. */
export function rentalServiceTerm(subscription: ShareMarketSubscription, t: Translate) {
  if (subscription.pricingModel === "prepaid_calendar_month") {
    return t("shareMarket.serviceDuration.monthlyManaged");
  }
  return subscription.serviceDurationDays == null
    ? t("shareMarket.serviceDuration.permanent")
    : t("shareMarket.serviceDuration.daysValue", { count: subscription.serviceDurationDays });
}

/**
 * What the rider pays and how long the seat still runs, on the card itself.
 * The rate is the one frozen into their rental — the listing price next to it can already
 * have moved — and the countdown replaces an expiry timestamp nobody converts correctly.
 * It ticks once a minute, which is as fast as a day/hour readout can ever change.
 */
export function RentalTermLine({
  subscription,
  className,
}: {
  subscription: ShareMarketSubscription;
  className?: string;
}) {
  const { locale, t } = useLocaleText();
  const [nowMs, setNowMs] = React.useState(() => Date.now());

  React.useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const contract = subscription.walletContract;
  const progress = rentalTermProgress(subscription, nowMs);
  const remaining = contract
    ? t(walletLifecycleKey(contract), { date: formatUtcDateTime(contract.periodEnd, locale), hours: trialHours(contract.trialSecondsRemaining, locale) })
    : progress.pending
      ? t("account.share.activationPending")
      : progress.expired
        ? t("shareMarket.serviceDuration.expired")
        : progress.days != null
          ? t("shareMarket.serviceDuration.remainingDays", { count: progress.days })
          : progress.hours != null
            ? t("shareMarket.serviceDuration.remainingHours", { count: progress.hours })
            : t("shareMarket.serviceDuration.permanent");
  const urgent = contract
    ? ["suspended", "ending", "review_required"].includes(contract.status)
    : progress.expired || progress.hours != null || (progress.days ?? 99) <= 2;

  return (
    <p className={cn("flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5 text-[10px] leading-4 text-slate-500", className)}>
      <strong className="font-semibold tabular-nums text-slate-700">{rentalPrice(subscription, locale, t)}</strong>
      <span>{rentalServiceTerm(subscription, t)}</span>
      <span className={cn("tabular-nums", urgent ? "font-medium text-amber-700" : null)}>{remaining}</span>
      {shareTrialSummary(subscription, locale, t) ? <span>{shareTrialSummary(subscription, locale, t)}</span> : null}
    </p>
  );
}


export function RentalApps({ subscription, size = 16 }: { subscription: ShareMarketSubscription; size?: number }) {
  const apps = rentalApps(subscription);
  if (!apps.length) return null;
  return (
    <span className="inline-flex shrink-0 items-center gap-1" title={apps.map((app) => SHARE_APP_LABELS[app]).join(" / ")}>
      {apps.map((app) => <ShareAppLogo key={app} app={app} size={size} />)}
    </span>
  );
}

export function RentalActions({
  subscription,
  t,
  busy,
  onRelease,

}: {
  subscription: ShareMarketSubscription;
  t: Translate;
  busy?: boolean;
  onRelease?: () => void;

}) {

  return (
    <div className="flex shrink-0 flex-wrap items-center justify-end gap-1">

      {subscription.canRelease && !walletReleaseScheduled(subscription.walletContract, subscription.pricingModel) && onRelease ? (
        <Button size="sm" variant="outline" className="h-7 min-w-0 px-2 text-xs" isDisabled={busy} onClick={onRelease}>
          {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RotateCcw className="h-3.5 w-3.5" />}
          {t("shareMarket.release")}
        </Button>
      ) : null}
    </div>
  );
}


type PendingAction = {
  subscriptionId: string;
  title: string;
  description: string;
  label: string;
  run: () => Promise<unknown>;
};

export function useShareMarketRentalActions(onChanged: () => Promise<void> | void) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const [busyId, setBusyId] = React.useState("");
  const [action, setAction] = React.useState<PendingAction | null>(null);
  const [error, setError] = React.useState("");

  const run = async (subscriptionId: string, operation: () => Promise<unknown>) => {
    if (busyId) return;
    setBusyId(subscriptionId);
    setError("");
    try {
      await operation();
      if (session?.user?.id) notifyMarketBalanceChanged(session.user.id);
      setAction(null);
      await onChanged();
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusyId("");
    }
  };

  const rowActions = (subscription: ShareMarketSubscription) => ({
    onRelease: subscription.canRelease
      && !walletReleaseScheduled(subscription.walletContract, subscription.pricingModel)
      ? () => setAction({
        subscriptionId: subscription.id,
        title: t(subscription.walletContract?.status === "active" && subscription.pricingModel === "prepaid_calendar_month"
          ? "shareMarket.confirm.releaseMonthlyTitle" : "shareMarket.confirm.releaseTitle"),
        description: subscription.walletContract?.status === "active" && subscription.pricingModel === "prepaid_calendar_month"
          ? t("shareMarket.confirm.releaseMonthlyDescription", { share: subscription.shareName, time: formatUtcDateTime(subscription.walletContract.periodEnd, locale) })
          : t("shareMarket.confirm.releaseDescription", { share: subscription.shareName }),
        label: t("shareMarket.release"),
        run: () => releaseShareMarketSubscription(subscription.id),
      })
      : undefined,

  });

  const dialog = (
    <ConfirmAlertDialog
      open={!!action}
      title={action?.title || ""}
      description={action?.description || ""}
      confirmLabel={action?.label || ""}
      cancelLabel={t("common.cancel")}
      tone="warning"
      busy={!!busyId}
      onConfirm={() => action && void run(action.subscriptionId, action.run)}
      onOpenChange={(open) => !open && !busyId && setAction(null)}
    />
  );

  return {
    busyId,
    error,
    setError,
    rowActions,
    dialog,
    interactionActive: !!busyId || !!action,
  };
}
