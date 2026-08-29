"use client";

import * as React from "react";
import Link from "next/link";
import { Button } from "@heroui/react";
import {
  Check,
  Loader2,
  RotateCcw,
  X,
} from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  acceptShareMarketPriceChange,
  rejectShareMarketPriceChange,
  releaseShareMarketSubscription,
} from "@/lib/api";
import { shareMarketHref } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { ShareMarketSubscription } from "@/lib/types";
import {
  isCoreShareApp,
  refundStatusKey,
  shareMarketMutationError,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";

type Translate = ReturnType<typeof useLocaleText>["t"];

function formatDate(value: string | undefined, locale: string) {
  if (!value) return "-";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(timestamp))
    : value;
}

function rentalApps(subscription: ShareMarketSubscription) {
  return [...new Set(
    (subscription.apps?.length ? subscription.apps : [subscription.appType]).filter(isCoreShareApp),
  )];
}

export function rentalPrice(subscription: ShareMarketSubscription, locale: string, t: Translate) {
  return subscription.dailyRateMinor == null
    ? t("shareMarket.free")
    : `${formatUsdMoney(subscription.dailyRateMinor, locale)} / ${t("marketBilling.day")}`;
}

function historyNote(subscription: ShareMarketSubscription, locale: string, t: Translate) {
  const parts: string[] = [];
  if (subscription.terminationAdjustment) {
    parts.push(t("shareMarket.refund.summary", {
      amount: formatUsdMoney(subscription.terminationAdjustment.amountMinor, locale),
      status: t(refundStatusKey(subscription.terminationAdjustment.status)),
    }));
  }
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
  onAcceptPrice,
  onRejectPrice,
}: {
  subscription: ShareMarketSubscription;
  t: Translate;
  busy?: boolean;
  onRelease?: () => void;
  onAcceptPrice?: () => void;
  onRejectPrice?: () => void;
}) {
  const pendingPrice = subscription.priceChange?.status === "pending";
  return (
    <div className="flex shrink-0 flex-wrap items-center justify-end gap-1">
      {pendingPrice && onAcceptPrice ? (
        <Button size="sm" variant="primary" className="h-7 min-w-0 px-2 text-xs" isDisabled={busy} onClick={onAcceptPrice}>
          <Check className="h-3.5 w-3.5" />
          {t("shareMarket.priceChange.accept")}
        </Button>
      ) : null}
      {pendingPrice && onRejectPrice ? (
        <Button size="sm" variant="outline" className="h-7 min-w-0 px-2 text-xs" isDisabled={busy} onClick={onRejectPrice}>
          <X className="h-3.5 w-3.5" />
          {t("shareMarket.priceChange.reject")}
        </Button>
      ) : null}
      {subscription.canRelease && onRelease ? (
        <Button size="sm" variant="outline" className="h-7 min-w-0 px-2 text-xs" isDisabled={busy} onClick={onRelease}>
          {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RotateCcw className="h-3.5 w-3.5" />}
          {t("shareMarket.release")}
        </Button>
      ) : null}
    </div>
  );
}

export function ShareMarketRentalHistory({
  subscriptions,
  nextCursor,
  loadingMore = false,
  onLoadMore,
}: {
  subscriptions: ShareMarketSubscription[];
  nextCursor?: string | null;
  loadingMore?: boolean;
  onLoadMore?: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const notes = subscriptions.map((subscription) => historyNote(subscription, locale, t));
  const showNote = notes.some(Boolean);
  const endedAt = (subscription: ShareMarketSubscription) => formatDate(subscription.releasedAt || subscription.updatedAt, locale);
  const statusLabel = (subscription: ShareMarketSubscription) => {
    const key = subscriptionStatusKey(subscription.status);
    return key ? t(key) : subscription.status;
  };

  return (
    <section className="grid gap-2 border-t border-slate-200 pt-5" aria-labelledby="share-rentals-history">
      <h3 id="share-rentals-history" className="text-xs font-semibold uppercase tracking-wide text-slate-500">
        {t("account.share.history")}
        {subscriptions.length ? <span className="ml-1.5 tabular-nums text-slate-400">{subscriptions.length}</span> : null}
      </h3>
      {subscriptions.length ? (
        <>
          <div className="grid gap-2 lg:hidden">
            {subscriptions.map((subscription, index) => (
              <div key={subscription.id} className="grid gap-0.5 border-b border-slate-100 py-2 last:border-0">
                <div className="flex min-w-0 items-center gap-2">
                  <RentalApps subscription={subscription} size={14} />
                  <Link
                    href={shareMarketHref({ shareId: subscription.shareId })}
                    className="min-w-0 truncate text-sm font-medium text-slate-800 hover:underline"
                  >
                    {subscription.shareName}
                  </Link>
                  <span className="shrink-0 text-xs tabular-nums text-slate-400">#{subscription.seatPosition}</span>
                </div>
                <p className="flex min-w-0 flex-wrap gap-x-2 text-[11px] text-slate-500">
                  <span>{statusLabel(subscription)}</span>
                  <span className="tabular-nums">{rentalPrice(subscription, locale, t)}</span>
                  <span className="tabular-nums">{endedAt(subscription)}</span>
                </p>
                {notes[index] ? <p className="text-[11px] leading-4 text-slate-400">{notes[index]}</p> : null}
              </div>
            ))}
          </div>
          <div className="hidden overflow-hidden rounded-md border border-slate-200 lg:block">
            <table className="w-full table-fixed border-collapse text-left text-xs">
              <thead className="bg-slate-50 text-[10px] font-semibold uppercase tracking-[0.08em] text-slate-500">
                <tr>
                  <th className="px-3 py-2">{t("shareMarket.col.share")}</th>
                  <th className="w-14 px-2 py-2">{t("shareMarket.col.seat")}</th>
                  <th className="w-28 px-2 py-2">{t("shareMarket.col.status")}</th>
                  <th className="w-32 px-2 py-2">{t("shareMarket.col.amount")}</th>
                  <th className="w-40 px-2 py-2">{t("shareMarket.col.ended")}</th>
                  {showNote ? <th className="px-3 py-2">{t("shareMarket.col.note")}</th> : null}
                </tr>
              </thead>
              <tbody>
                {subscriptions.map((subscription, index) => (
                  <tr key={subscription.id} className="border-t border-slate-100 text-slate-700">
                    <td className="min-w-0 px-3 py-2">
                      <div className="flex min-w-0 items-center gap-1.5">
                        <RentalApps subscription={subscription} size={14} />
                        <Link
                          href={shareMarketHref({ shareId: subscription.shareId })}
                          className="min-w-0 truncate font-medium text-slate-800 hover:underline"
                          title={subscription.shareName}
                        >
                          {subscription.shareName}
                        </Link>
                      </div>
                    </td>
                    <td className="px-2 py-2 tabular-nums text-slate-500">#{subscription.seatPosition}</td>
                    <td className="px-2 py-2 text-slate-500">{statusLabel(subscription)}</td>
                    <td className="px-2 py-2 tabular-nums">{rentalPrice(subscription, locale, t)}</td>
                    <td className="px-2 py-2 tabular-nums text-slate-500">{endedAt(subscription)}</td>
                    {showNote ? (
                      <td className="px-3 py-2">
                        <span className="block truncate text-slate-400" title={notes[index] || undefined}>{notes[index] || "—"}</span>
                      </td>
                    ) : null}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {nextCursor && onLoadMore ? (
            <Button variant="outline" className="justify-self-center" isDisabled={loadingMore} onClick={() => void onLoadMore()}>
              {loadingMore ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("account.share.loadMore")}
            </Button>
          ) : null}
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
  const [busyId, setBusyId] = React.useState("");
  const [action, setAction] = React.useState<PendingAction | null>(null);
  const [error, setError] = React.useState("");

  const run = async (subscriptionId: string, operation: () => Promise<unknown>) => {
    if (busyId) return;
    setBusyId(subscriptionId);
    setError("");
    try {
      await operation();
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
      ? () => setAction({
        subscriptionId: subscription.id,
        title: t("shareMarket.confirm.releaseTitle"),
        description: t("shareMarket.confirm.releaseDescription", { share: subscription.shareName }),
        label: t("shareMarket.release"),
        run: () => releaseShareMarketSubscription(subscription.id),
      })
      : undefined,
    onAcceptPrice: subscription.priceChange
      ? () => setAction({
        subscriptionId: subscription.id,
        title: t("shareMarket.priceChange.acceptTitle"),
        description: t("shareMarket.priceChange.acceptDescription", {
          previous: formatUsdMoney(subscription.priceChange!.previousDailyRateMinor, locale),
          proposed: formatUsdMoney(subscription.priceChange!.proposedDailyRateMinor, locale),
        }),
        label: t("shareMarket.priceChange.accept"),
        run: () => acceptShareMarketPriceChange(subscription.priceChange!.id),
      })
      : undefined,
    onRejectPrice: subscription.priceChange
      ? () => void run(subscription.id, () => rejectShareMarketPriceChange(subscription.priceChange!.id))
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
