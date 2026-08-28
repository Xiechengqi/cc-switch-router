"use client";

import * as React from "react";
import Link from "next/link";
import { Button, Chip, Drawer } from "@heroui/react";
import {
  ArrowUpRight,
  Check,
  CircleDollarSign,
  ExternalLink,
  Loader2,
  RotateCcw,
  X,
} from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { drawerDialogClassName, subdomainTunnelUrl } from "@/components/dashboard/share-dashboard-utils";
import {
  CatalogSeatPreviewList,
  MARKET_SHARE_CARD_GRID_CLASS,
  MarketShareCard,
  listingCardId,
  shouldOpenMarketShareCard,
} from "@/components/dashboard/share-market/market-share-card";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  acceptShareMarketPriceChange,
  rejectShareMarketPriceChange,
  releaseShareMarketSubscription,
} from "@/lib/api";
import {
  DASHBOARD_ACCOUNT_BILLING_PATH,
  shareMarketHref,
} from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { ShareMarketListing, ShareMarketSubscription } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  formatTokenLimit,
  grantFailureMessageKey,
  integrityReasonText,
  integrityStatusKey,
  isCoreShareApp,
  refundStatusKey,
  shareMarketMutationError,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";
import {
  groupActiveRentalsByShare,
  listingForRentalShare,
  partitionShareMarketSubscriptions,
  sortShareMarketSubscriptions,
} from "@/components/dashboard/share-market/subscription-utils";

export { sortShareMarketSubscriptions };

type PendingAction = {
  subscriptionId: string;
  title: string;
  description: string;
  label: string;
  run: () => Promise<unknown>;
};

type Translate = ReturnType<typeof useLocaleText>["t"];

function isAnomalous(status: string) {
  return [
    "grant_pending",
    "billing_suspend_pending",
    "billing_suspended",
    "billing_resume_pending",
    "billing_control_failed",
    "revoke_pending",
    "revoke_failed",
    "grant_failed",
  ].includes(status);
}

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

function rentalPrice(subscription: ShareMarketSubscription, locale: string, t: Translate) {
  return subscription.dailyRateMinor == null
    ? t("shareMarket.free")
    : `${formatUsdMoney(subscription.dailyRateMinor, locale)} / ${t("marketBilling.day")}`;
}

function rentalServiceTerm(subscription: ShareMarketSubscription, t: Translate) {
  return subscription.serviceDurationDays == null
    ? t("shareMarket.serviceDuration.permanent")
    : t("shareMarket.serviceDuration.daysValue", { count: subscription.serviceDurationDays });
}

function rentalQuota(subscription: ShareMarketSubscription, locale: string, t: Translate) {
  return [
    t("shareMarket.parallelShort", { value: subscription.parallelLimit == null ? "∞" : subscription.parallelLimit }),
    formatTokenLimit(subscription, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`)),
  ].join(" · ");
}

function rentalServiceTiming(subscription: ShareMarketSubscription, locale: string, t: Translate) {
  if (!subscription.serviceStartedAt) return t("account.share.activationPending");
  return subscription.expiresAt
    ? `${t("shareMarket.serviceDuration.started")}: ${formatDate(subscription.serviceStartedAt, locale)} · ${t("shareMarket.serviceDuration.expires")}: ${formatDate(subscription.expiresAt, locale)}`
    : `${t("shareMarket.serviceDuration.started")}: ${formatDate(subscription.serviceStartedAt, locale)} · ${t("shareMarket.serviceDuration.permanent")}`;
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

function RentalApps({ subscription, size = 16 }: { subscription: ShareMarketSubscription; size?: number }) {
  const apps = rentalApps(subscription);
  if (!apps.length) return null;
  return (
    <span className="inline-flex shrink-0 items-center gap-1" title={apps.map((app) => SHARE_APP_LABELS[app]).join(" / ")}>
      {apps.map((app) => <ShareAppLogo key={app} app={app} size={size} />)}
    </span>
  );
}

function StatusDetails({
  subscription,
  locale,
  t,
}: {
  subscription: ShareMarketSubscription;
  locale: string;
  t: Translate;
}) {
  const grantFailed = subscription.status === "grant_failed";
  const grantContractViolation = subscription.failureCode === "share_market_grant_contract_violation";
  const hasStatusDetail = grantFailed
    || !!subscription.releaseReason
    || !!subscription.failureCode
    || subscription.grantAttempts != null
    || subscription.integrityState !== "compatible"
    || !!subscription.terminationAdjustment;
  if (!hasStatusDetail) return null;
  return (
    <div className="grid gap-0.5 border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-900">
      {grantFailed || grantContractViolation ? <p className="font-medium">{t(grantFailureMessageKey(subscription.failureCode))}</p> : null}
      {subscription.failureCode ? <p className="break-all font-mono text-[10px] text-rose-800/70">{t("shareMarket.authorizationFailure.code", { code: subscription.failureCode })}</p> : null}
      {subscription.grantAttempts != null ? <p>{t("shareMarket.authorizationFailure.attempts", { count: subscription.grantAttempts })}</p> : null}
      {subscription.releaseReason ? <p className="break-words text-rose-800/80">{t(grantFailed ? "shareMarket.authorizationFailure.reason" : "shareMarket.subscription.statusDetail", { reason: subscription.releaseReason })}</p> : null}
      {subscription.integrityState !== "compatible" ? <p>{t(integrityStatusKey(subscription.integrityState))}{subscription.integrityReason ? ` · ${integrityReasonText(subscription.integrityReason, t)}` : ""}</p> : null}
      {subscription.terminationAdjustment ? <p>{t("shareMarket.refund.summary", { amount: formatUsdMoney(subscription.terminationAdjustment.amountMinor, locale), status: t(refundStatusKey(subscription.terminationAdjustment.status)) })}</p> : null}
    </div>
  );
}

function PriceChangeDetails({
  subscription,
  locale,
  t,
  busy,
  onAcceptPrice,
  onRejectPrice,
}: {
  subscription: ShareMarketSubscription;
  locale: string;
  t: Translate;
  busy?: boolean;
  onAcceptPrice?: () => void;
  onRejectPrice?: () => void;
}) {
  const priceChange = subscription.priceChange;
  if (!priceChange) return null;
  return (
    <div className="grid gap-2 border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-sm text-amber-950">
      <strong>{t(`shareMarket.priceChange.status.${priceChange.status}`)}</strong>
      <span>{t("shareMarket.priceChange.summary", {
        previous: formatUsdMoney(priceChange.previousDailyRateMinor, locale),
        proposed: formatUsdMoney(priceChange.proposedDailyRateMinor, locale),
      })}</span>
      {priceChange.status === "pending" && (onAcceptPrice || onRejectPrice) ? (
        <div className="flex flex-wrap gap-2">
          {onAcceptPrice ? <Button size="sm" variant="primary" isDisabled={busy} onClick={onAcceptPrice}><Check className="h-4 w-4" />{t("shareMarket.priceChange.accept")}</Button> : null}
          {onRejectPrice ? <Button size="sm" variant="outline" isDisabled={busy} onClick={onRejectPrice}><X className="h-4 w-4" />{t("shareMarket.priceChange.reject")}</Button> : null}
        </div>
      ) : null}
    </div>
  );
}

function RentalActions({
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
  const openUrl = subdomainTunnelUrl(subscription.subdomain);
  return (
    <div className="flex shrink-0 flex-wrap items-center justify-end gap-1">
      {subscription.dailyRateMinor != null ? (
        <Link
          href={DASHBOARD_ACCOUNT_BILLING_PATH}
          className="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 hover:text-slate-800 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          title={t("marketBilling.open")}
          aria-label={t("marketBilling.open")}
        >
          <CircleDollarSign className="h-3.5 w-3.5" />
        </Link>
      ) : null}
      {openUrl ? (
        <a
          href={openUrl}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex h-7 items-center gap-1 rounded-md px-2 text-xs font-medium text-slate-700 hover:bg-slate-100 hover:text-slate-900 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
        >
          {t("account.share.openShare")}
          <ExternalLink className="h-3.5 w-3.5" />
        </a>
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

function RentalIdentity({
  subscription,
  statusLabel,
  online,
}: {
  subscription: ShareMarketSubscription;
  statusLabel: string;
  online: boolean;
}) {
  const { t } = useLocaleText();
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
      <RentalApps subscription={subscription} />
      <strong className="min-w-0 truncate text-sm text-slate-900">{subscription.shareName}</strong>
      <span className="shrink-0 text-xs tabular-nums text-slate-500">#{subscription.seatPosition}</span>
      <span className={cn("shrink-0 text-[11px] font-medium", online ? "text-emerald-700" : "text-rose-700")}>
        {online ? t("shareMarket.online") : t("shareMarket.offline")}
      </span>
      <span className="min-w-0 truncate text-[11px] text-slate-500">{statusLabel}</span>
    </div>
  );
}

export function ShareMarketSubscriptionCard({
  subscription,
  perspective = "user",
  busy,
  onRelease,
  onAcceptPrice,
  onRejectPrice,
}: {
  subscription: ShareMarketSubscription;
  perspective?: "user" | "provider";
  busy: boolean;
  onRelease?: () => void;
  onAcceptPrice?: () => void;
  onRejectPrice?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const anomalous = isAnomalous(subscription.status) || subscription.integrityState !== "compatible";
  const openUrl = subdomainTunnelUrl(subscription.subdomain);
  const manageHref = perspective === "provider"
    ? shareMarketHref({ workspace: "selling", shareId: subscription.shareId })
    : undefined;
  const statusKey = subscriptionStatusKey(subscription.status);
  const price = rentalPrice(subscription, locale, t);
  const serviceTerm = rentalServiceTerm(subscription, t);
  const serviceTiming = rentalServiceTiming(subscription, locale, t);

  return (
    <section
      aria-busy={busy}
      className={`grid gap-3 rounded-md border bg-card p-4 shadow-sm sm:p-5 ${anomalous ? "border-rose-200 ring-1 ring-rose-100" : "border-border"}`}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <RentalApps subscription={subscription} size={18} />
        <strong className="truncate text-sm">{subscription.shareName}</strong>
        <Chip size="sm" variant={anomalous ? "primary" : "tertiary"}>{statusKey ? t(statusKey) : subscription.status}</Chip>
        <Chip size="sm" variant="tertiary">{subscription.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}</Chip>
        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {perspective === "user"
            ? t("account.share.provider", { owner: subscription.ownerEmail })
            : t("account.share.renter", { email: subscription.renterEmail || "-" })}
        </span>
      </div>

      <dl className="grid gap-3 text-sm sm:grid-cols-2 xl:grid-cols-4">
        <div>
          <dt className="text-xs text-muted-foreground">{t("account.share.seat")}</dt>
          <dd className="mt-0.5 font-medium">#{subscription.seatPosition}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">{t("account.share.offer")}</dt>
          <dd className="mt-0.5 font-medium">{price} · {serviceTerm}</dd>
          <dd className="mt-0.5 text-xs text-muted-foreground">{serviceTiming}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">{t("account.share.quota")}</dt>
          <dd className="mt-0.5 font-medium">
            {t("shareMarket.parallelShort", { value: subscription.parallelLimit == null ? "∞" : subscription.parallelLimit })}
          </dd>
          <dd className="mt-0.5 text-xs text-muted-foreground">
            {formatTokenLimit(subscription, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`))}
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">{t("account.share.updated")}</dt>
          <dd className="mt-0.5 text-muted-foreground">{formatDate(subscription.updatedAt, locale)}</dd>
        </div>
      </dl>

      <StatusDetails subscription={subscription} locale={locale} t={t} />
      <PriceChangeDetails
        subscription={subscription}
        locale={locale}
        t={t}
        busy={busy}
        onAcceptPrice={perspective === "user" ? onAcceptPrice : undefined}
        onRejectPrice={perspective === "user" ? onRejectPrice : undefined}
      />

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <p className="text-xs text-muted-foreground">
          {perspective === "user" ? t("account.share.manageHint") : t("account.share.providerManageHint")}
        </p>
        <div className="flex flex-wrap gap-2">
          {subscription.dailyRateMinor != null ? (
            <Link href={DASHBOARD_ACCOUNT_BILLING_PATH} className="inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t("marketBilling.open")}<ArrowUpRight className="h-3.5 w-3.5" />
            </Link>
          ) : null}
          {openUrl ? (
            <a href={openUrl} target="_blank" rel="noopener noreferrer" className="inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t("account.share.openShare")}<ExternalLink className="h-3.5 w-3.5" />
            </a>
          ) : null}
          {perspective === "user" && subscription.canRelease && onRelease ? (
            <Button size="sm" variant="outline" isDisabled={busy} onClick={onRelease}><RotateCcw className="h-4 w-4" />{t("shareMarket.release")}</Button>
          ) : null}
          {manageHref ? (
            <Link href={manageHref} className="inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t("account.share.manageInMarket")}<ArrowUpRight className="h-3.5 w-3.5" />
            </Link>
          ) : null}
        </div>
      </div>
    </section>
  );
}

function ActiveRentalRow({
  subscription,
  busy,
  attention,
  onRelease,
  onAcceptPrice,
  onRejectPrice,
}: {
  subscription: ShareMarketSubscription;
  busy: boolean;
  attention?: boolean;
  onRelease?: () => void;
  onAcceptPrice?: () => void;
  onRejectPrice?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const statusKey = subscriptionStatusKey(subscription.status);
  const statusLabel = statusKey ? t(statusKey) : subscription.status;
  const price = rentalPrice(subscription, locale, t);
  const quota = rentalQuota(subscription, locale, t);
  const timing = rentalServiceTiming(subscription, locale, t);
  const owner = t("account.share.provider", { owner: subscription.ownerEmail });
  const priceOnlyAttention = attention
    && subscription.priceChange?.status === "pending"
    && subscription.integrityState === "compatible"
    && !isAnomalous(subscription.status);

  return (
    <article
      aria-busy={busy}
      title={`${owner} · ${timing}`}
      className={cn(
        "grid gap-2 px-3 py-2.5",
        attention && (priceOnlyAttention
          ? "rounded-md border border-amber-200 bg-amber-50/40"
          : "rounded-md border border-rose-200 bg-rose-50/40"),
      )}
    >
      <div className="flex min-w-0 flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="min-w-0">
          <RentalIdentity
            subscription={subscription}
            statusLabel={statusLabel}
            online={!!subscription.shareOnline}
          />
          <p className="mt-1 flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5 text-xs text-slate-500">
            <strong className="font-medium tabular-nums text-slate-800">{price}</strong>
            <span>{rentalServiceTerm(subscription, t)}</span>
            <span className="min-w-0 truncate">{quota}</span>
          </p>
        </div>
        <RentalActions subscription={subscription} t={t} busy={busy} onRelease={onRelease} />
      </div>
      {attention ? (
        <>
          <StatusDetails subscription={subscription} locale={locale} t={t} />
          <PriceChangeDetails
            subscription={subscription}
            locale={locale}
            t={t}
            busy={busy}
            onAcceptPrice={onAcceptPrice}
            onRejectPrice={onRejectPrice}
          />
        </>
      ) : null}
    </article>
  );
}

function HistoryTable({
  subscriptions,
  locale,
  t,
}: {
  subscriptions: ShareMarketSubscription[];
  locale: string;
  t: Translate;
}) {
  const notes = subscriptions.map((subscription) => historyNote(subscription, locale, t));
  const showNote = notes.some(Boolean);
  const endedAt = (subscription: ShareMarketSubscription) => formatDate(subscription.releasedAt || subscription.updatedAt, locale);
  const statusLabel = (subscription: ShareMarketSubscription) => {
    const key = subscriptionStatusKey(subscription.status);
    return key ? t(key) : subscription.status;
  };

  return (
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
    </>
  );
}

export function ShareMarketBuyerRentals({
  subscriptions,
  listings = [],
  loading,
  onChanged,
  onInteractionChange,
  nextCursor,
  loadingMore = false,
  onLoadMore,
  showHeading = true,
}: {
  subscriptions: ShareMarketSubscription[];
  listings?: ShareMarketListing[];
  loading: boolean;
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
  nextCursor?: string | null;
  loadingMore?: boolean;
  onLoadMore?: () => Promise<void> | void;
  showHeading?: boolean;
}) {
  const { locale, t } = useLocaleText();
  const [busyId, setBusyId] = React.useState("");
  const [action, setAction] = React.useState<PendingAction | null>(null);
  const [error, setError] = React.useState("");
  const [selectedShareId, setSelectedShareId] = React.useState<string | null>(null);
  const interactionActive = !!busyId || !!action || loadingMore || !!selectedShareId;

  React.useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);

  const { history } = React.useMemo(
    () => partitionShareMarketSubscriptions(subscriptions),
    [subscriptions],
  );
  const activeGroups = React.useMemo(
    () => groupActiveRentalsByShare(subscriptions),
    [subscriptions],
  );
  const selectedGroup = selectedShareId
    ? activeGroups.find((group) => group.shareId === selectedShareId) || null
    : null;
  const selectedListing = selectedGroup
    ? listingForRentalShare(listings, selectedGroup) as ShareMarketListing | undefined
    : undefined;

  React.useEffect(() => {
    if (!selectedShareId) return;
    if (!activeGroups.some((group) => group.shareId === selectedShareId)) setSelectedShareId(null);
  }, [activeGroups, selectedShareId]);

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

  if (loading && !subscriptions.length) {
    return <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  }

  return (
    <div className="grid min-w-0 gap-5">
      {showHeading ? (
        <div>
          <h2 className="text-sm font-semibold text-foreground">{t("shareMarket.workspace.rentals")}</h2>
          <p className="mt-0.5 text-xs text-muted-foreground">{t("shareMarket.workspace.rentalsHint")}</p>
        </div>
      ) : null}
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}

      <section className="grid gap-2" aria-labelledby="share-rentals-active">
        <h3 id="share-rentals-active" className="text-xs font-semibold uppercase tracking-wide text-slate-500">
          {t("account.share.active")}
          {activeGroups.length ? <span className="ml-1.5 tabular-nums text-slate-400">{activeGroups.length}</span> : null}
        </h3>
        {activeGroups.length ? (
          <div className={MARKET_SHARE_CARD_GRID_CLASS}>
            {activeGroups.map((group) => {
              const listing = listingForRentalShare(listings, group) as ShareMarketListing | undefined;
              const subscription = group.subscription;
              const actions = rowActions(subscription);
              const occupancy = listing
                ? undefined
                : t("shareMarket.catalog.seatPosition", { position: subscription.seatPosition });
              const footer = listing ? (
                <div className="grid content-start">
                  <CatalogSeatPreviewList
                    listing={listing}
                    seats={listing.seats}
                    preferredSeatIds={[subscription.seatId]}
                    onOpen={() => setSelectedShareId(group.shareId)}
                  />
                  <div data-no-card-open>
                    <RentalActions subscription={subscription} t={t} busy={busyId === subscription.id} onRelease={actions.onRelease} />
                  </div>
                </div>
              ) : (
                <div className="grid content-start gap-1.5 border-t border-slate-100 pt-1.5">
                  <p className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5 px-1.5 text-xs text-slate-500">
                    <strong className="font-medium tabular-nums text-slate-800">{rentalPrice(subscription, locale, t)}</strong>
                    <span>{rentalServiceTerm(subscription, t)}</span>
                    <span className="min-w-0 truncate">{rentalQuota(subscription, locale, t)}</span>
                  </p>
                  <div data-no-card-open>
                    <RentalActions subscription={subscription} t={t} busy={busyId === subscription.id} onRelease={actions.onRelease} />
                  </div>
                </div>
              );
              if (listing) {
                return (
                  <MarketShareCard
                    key={group.shareId}
                    listing={listing}
                    attention={group.attention}
                    cardId={listingCardId("rental", group.shareId)}
                    occupancy={occupancy}
                    onOpen={() => setSelectedShareId(group.shareId)}
                    footer={footer}
                  />
                );
              }
              return (
                <article
                  key={group.shareId}
                  id={listingCardId("rental", group.shareId)}
                  className={cn(
                    "grid min-h-[15rem] min-w-0 cursor-pointer scroll-mt-20 grid-rows-[auto_1fr] gap-2.5 rounded-xl border bg-white p-3 shadow-sm",
                    group.attention ? "border-amber-300 ring-1 ring-amber-200" : "border-slate-200",
                  )}
                  onClick={(event) => {
                    if (!shouldOpenMarketShareCard(event, null)) return;
                    setSelectedShareId(group.shareId);
                  }}
                >
                  <header className="flex min-w-0 items-center justify-between gap-2">
                    <strong className="min-w-0 truncate font-mono text-xs font-semibold">{subscription.shareName}</strong>
                    <span className="shrink-0 text-[11px] font-semibold tabular-nums text-slate-700">{occupancy}</span>
                  </header>
                  {footer}
                </article>
              );
            })}
          </div>
        ) : (
          <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
            <span>{t("account.share.userEmpty")}</span>
            <Link href={shareMarketHref()} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link>
          </div>
        )}
      </section>

      <section className="grid gap-2 border-t border-slate-200 pt-5" aria-labelledby="share-rentals-history">
        <h3 id="share-rentals-history" className="text-xs font-semibold uppercase tracking-wide text-slate-500">
          {t("account.share.history")}
          {history.length ? <span className="ml-1.5 tabular-nums text-slate-400">{history.length}</span> : null}
        </h3>
        {history.length ? (
          <>
            <HistoryTable subscriptions={history} locale={locale} t={t} />
            {nextCursor && onLoadMore ? (
              <Button variant="outline" className="justify-self-center" isDisabled={loadingMore} onClick={() => void onLoadMore()}>
                {loadingMore ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("account.share.loadMore")}
              </Button>
            ) : null}
          </>
        ) : <p className="py-1 text-sm text-slate-400">{t("account.share.historyEmpty")}</p>}
      </section>

      <Drawer.Backdrop isOpen={!!selectedGroup} onOpenChange={(open) => !open && setSelectedShareId(null)}>
        <Drawer.Content placement="right">
          <Drawer.Dialog className={drawerDialogClassName}>
            <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" />
            <Drawer.Header>
              <div className="min-w-0 pr-10">
                <Drawer.Heading className="truncate text-base">{selectedGroup?.subscription.shareName}</Drawer.Heading>
              </div>
            </Drawer.Header>
            <Drawer.Body className="overflow-y-auto pb-28">
              {selectedGroup ? (
                <div className="grid gap-4">
                  <ActiveRentalRow
                    subscription={selectedGroup.subscription}
                    busy={busyId === selectedGroup.subscription.id}
                    attention={selectedGroup.attention}
                    {...rowActions(selectedGroup.subscription)}
                  />
                  {selectedListing ? (
                    <p className="text-xs text-slate-500">
                      {t("shareMarket.catalog.occupancy", {
                        idle: selectedListing.seats.filter((seat) => seat.status === "available" && !seat.readOnly).length,
                        total: selectedListing.seats.length,
                      })}
                    </p>
                  ) : null}
                </div>
              ) : null}
            </Drawer.Body>
          </Drawer.Dialog>
        </Drawer.Content>
      </Drawer.Backdrop>

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
    </div>
  );
}
