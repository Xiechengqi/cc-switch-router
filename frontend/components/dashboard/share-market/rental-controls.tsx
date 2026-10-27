"use client";
import { walletLifecycleKey, walletReleaseScheduled, trialHours } from "@/lib/client-checkout";
import { shareTrialSummary } from "@/lib/share-trial";

import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import Link from "next/link";
import { Button } from "@heroui/react";
import { Loader2, RotateCcw } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import {
  listingForRentalShare,
  rentalTermProgress,
} from "@/components/dashboard/share-market/subscription-utils";
import {
  MARKET_RENTAL_HISTORY_PAGE_SIZE,
  pageCountForSize,
  paginateListings,
  rentalHistoryPageStart,
} from "@/components/dashboard/share-market/buyer-catalog-utils";
import { MarketPagination } from "@/components/dashboard/share-market/market-pagination";
import { MarketShareIdentity } from "@/components/dashboard/share-market/market-share-identity";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { releaseShareMarketSubscription } from "@/lib/api";
import { shareMarketHref } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { ShareMarketListing, ShareMarketSubscription } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  isCoreShareApp,
  shareMarketMutationError,
  subscriptionStatusKey,
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

function historyNote(subscription: ShareMarketSubscription, locale: string, t: Translate) {
  const parts: string[] = [];

  if (subscription.releaseReason) {
    parts.push(t("shareMarket.subscription.statusDetail", { reason: subscription.releaseReason }));
  }
  return parts.join(" · ");
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

function historyShareSource(subscription: ShareMarketSubscription, listing?: ShareMarketListing) {
  const apps = listing?.supportedApps?.length
    ? listing.supportedApps
    : (subscription.apps?.length ? subscription.apps : [subscription.appType].filter(Boolean));
  return {
    subdomain: listing?.subdomain || subscription.subdomain || subscription.shareName,
    shareName: listing?.shareName || subscription.shareName,
    supportedApps: apps,
    apps,
    appCapabilities: listing?.appCapabilities,
  };
}

export function ShareMarketRentalHistory({
  subscriptions,
  listings = [],
  nextCursor,
  historyTotal,
  loadingMore = false,
  onLoadMore,
}: {
  subscriptions: ShareMarketSubscription[];
  listings?: ShareMarketListing[];
  nextCursor?: string | null;
  historyTotal?: number;
  loadingMore?: boolean;
  onLoadMore?: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const [page, setPage] = React.useState(1);
  const pendingPageRef = React.useRef<number | null>(null);
  const total = Math.max(historyTotal ?? 0, subscriptions.length);
  const paged = React.useMemo(
    () => paginateListings(
      subscriptions,
      page,
      MARKET_RENTAL_HISTORY_PAGE_SIZE,
      total,
    ),
    [page, subscriptions, total],
  );
  const needsMoreForPage = (nextPage: number) =>
    nextPage >= 1
    && nextPage <= paged.pageCount
    && rentalHistoryPageStart(nextPage) >= subscriptions.length
    && !!nextCursor
    && !!onLoadMore;

  React.useEffect(() => {
    if (pendingPageRef.current != null) {
      if (rentalHistoryPageStart(pendingPageRef.current) < subscriptions.length) {
        setPage(pendingPageRef.current);
        pendingPageRef.current = null;
      }
      return;
    }
    setPage((current) => {
      if (rentalHistoryPageStart(current) >= subscriptions.length) {
        return Math.min(
          current,
          pageCountForSize(subscriptions.length, MARKET_RENTAL_HISTORY_PAGE_SIZE),
        );
      }
      return Math.min(current, paged.pageCount);
    });
  }, [paged.pageCount, subscriptions.length]);

  const notes = paged.items.map((subscription) => historyNote(subscription, locale, t));
  const showNote = notes.some(Boolean);
  const startedAt = (subscription: ShareMarketSubscription) => (
    formatUtcDateTime(subscription.serviceStartedAt || subscription.activatedAt || subscription.createdAt, locale)
  );
  const endedAt = (subscription: ShareMarketSubscription) => (
    formatUtcDateTime(subscription.releasedAt || subscription.updatedAt, locale)
  );
  const statusLabel = (subscription: ShareMarketSubscription) => {
    const key = subscriptionStatusKey(subscription.status);
    return key ? t(key) : subscription.status;
  };
  const goToPage = async (nextPage: number) => {
    if (nextPage < 1 || nextPage > paged.pageCount) return;
    if (!needsMoreForPage(nextPage)) {
      pendingPageRef.current = null;
      setPage(nextPage);
      return;
    }
    if (loadingMore) return;
    pendingPageRef.current = nextPage;
    await onLoadMore?.();
  };

  return (
    <section className="grid gap-2 border-t border-slate-200 pt-5" aria-labelledby="share-rentals-history">
      <h3 id="share-rentals-history" className="text-xs font-semibold uppercase tracking-wide text-slate-500">
        {t("account.share.history")}
        {total ? <span className="ml-1.5 tabular-nums text-slate-400">{total}</span> : null}
      </h3>
      {total ? (
        <>
          <div className="grid gap-2 lg:hidden">
            {paged.items.map((subscription, index) => {
              const listing = listingForRentalShare(listings, subscription) as ShareMarketListing | undefined;
              return (
                <div key={subscription.id} className="grid gap-0.5 border-b border-slate-100 py-2 last:border-0">
                  <div className="flex min-w-0 items-center gap-2">
                    <Link
                      href={shareMarketHref({ shareId: subscription.shareId })}
                      className="min-w-0"
                    >
                      <MarketShareIdentity source={historyShareSource(subscription, listing)} size={14} />
                    </Link>
                    <span className="shrink-0 text-xs tabular-nums text-slate-400">#{subscription.seatPosition}</span>
                  </div>
                  <p className="flex min-w-0 flex-wrap gap-x-2 text-[11px] text-slate-500">
                    <span>{statusLabel(subscription)}</span>
                    <span className="tabular-nums">{rentalPrice(subscription, locale, t)}</span>
                    <span className="tabular-nums">{startedAt(subscription)}</span>
                    <span className="tabular-nums">{endedAt(subscription)}</span>
                  </p>
                  {notes[index] ? <p className="text-[11px] leading-4 text-slate-400">{notes[index]}</p> : null}
                </div>
              );
            })}
          </div>
          <div className="hidden overflow-hidden rounded-md border border-slate-200 lg:block">
            <table className="w-full table-fixed border-collapse text-left text-xs">
              <thead className="bg-slate-50 text-[10px] font-semibold uppercase tracking-[0.08em] text-slate-500">
                <tr>
                  <th className="px-3 py-2">{t("shareMarket.col.share")}</th>
                  <th className="w-14 px-2 py-2">{t("shareMarket.col.seat")}</th>
                  <th className="w-28 px-2 py-2">{t("shareMarket.col.status")}</th>
                  <th className="w-32 px-2 py-2">{t("shareMarket.col.amount")}</th>
                  <th className="w-40 px-2 py-2">{t("shareMarket.col.started")}</th>
                  <th className="w-40 px-2 py-2">{t("shareMarket.col.ended")}</th>
                  {showNote ? <th className="px-3 py-2">{t("shareMarket.col.note")}</th> : null}
                </tr>
              </thead>
              <tbody>
                {paged.items.map((subscription, index) => {
                  const listing = listingForRentalShare(listings, subscription) as ShareMarketListing | undefined;
                  return (
                    <tr key={subscription.id} className="border-t border-slate-100 text-slate-700">
                      <td className="min-w-0 px-3 py-2">
                        <Link
                          href={shareMarketHref({ shareId: subscription.shareId })}
                          className="min-w-0 block"
                        >
                          <MarketShareIdentity source={historyShareSource(subscription, listing)} size={14} />
                        </Link>
                      </td>
                      <td className="px-2 py-2 tabular-nums text-slate-500">#{subscription.seatPosition}</td>
                      <td className="px-2 py-2 text-slate-500">{statusLabel(subscription)}</td>
                      <td className="px-2 py-2 tabular-nums">{rentalPrice(subscription, locale, t)}</td>
                      <td className="px-2 py-2 tabular-nums text-slate-500">{startedAt(subscription)}</td>
                      <td className="px-2 py-2 tabular-nums text-slate-500">{endedAt(subscription)}</td>
                      {showNote ? (
                        <td className="px-3 py-2">
                          <span className="block truncate text-slate-400" title={notes[index] || undefined}>{notes[index] || "—"}</span>
                        </td>
                      ) : null}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <MarketPagination
            page={paged.page}
            pageCount={paged.pageCount}
            onPageChange={(nextPage) => void goToPage(nextPage)}
          />
        </>
      ) : <p className="py-1 text-sm text-slate-400">{t("account.share.historyEmpty")}</p>}
    </section>
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
