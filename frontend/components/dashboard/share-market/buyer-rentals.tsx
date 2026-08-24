"use client";

import * as React from "react";
import Link from "next/link";
import { Button, Chip } from "@heroui/react";
import {
  ArrowUpRight,
  Check,
  ExternalLink,
  Loader2,
  RotateCcw,
  X,
} from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { subdomainTunnelUrl } from "@/components/dashboard/share-dashboard-utils";
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
import type { ShareMarketSubscription } from "@/lib/types";
import {
  formatTokenLimit,
  grantFailureMessageKey,
  integrityReasonText,
  integrityStatusKey,
  isCoreShareApp,
  isTerminalSubscription,
  refundStatusKey,
  shareMarketMutationError,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";

type PendingAction = {
  subscriptionId: string;
  title: string;
  description: string;
  label: string;
  run: () => Promise<unknown>;
};

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

function anomalyRank(status: string) {
  return [
    "billing_control_failed",
    "revoke_failed",
    "grant_failed",
    "billing_suspended",
    "billing_suspend_pending",
    "billing_resume_pending",
    "revoke_pending",
    "grant_pending",
  ].indexOf(status);
}

export function sortShareMarketSubscriptions(
  left: ShareMarketSubscription,
  right: ShareMarketSubscription,
) {
  const leftRank = anomalyRank(left.status);
  const rightRank = anomalyRank(right.status);
  return (
    (leftRank < 0 ? 99 : leftRank) - (rightRank < 0 ? 99 : rightRank)
    || Date.parse(right.updatedAt) - Date.parse(left.updatedAt)
    || left.shareName.localeCompare(right.shareName)
  );
}

function formatDate(value: string | undefined, locale: string) {
  if (!value) return "-";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(timestamp))
    : value;
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
  const apps = [...new Set(
    (subscription.apps?.length ? subscription.apps : [subscription.appType]).filter(isCoreShareApp),
  )];
  const priceChange = subscription.priceChange;
  const grantFailed = subscription.status === "grant_failed";
  const grantContractViolation = subscription.failureCode === "share_market_grant_contract_violation";
  const hasStatusDetail = grantFailed
    || !!subscription.releaseReason
    || !!subscription.failureCode
    || subscription.grantAttempts != null
    || subscription.integrityState !== "compatible"
    || !!subscription.terminationAdjustment;
  const serviceTerm = subscription.serviceDurationDays == null
    ? t("shareMarket.serviceDuration.permanent")
    : t("shareMarket.serviceDuration.daysValue", { count: subscription.serviceDurationDays });
  const price = subscription.dailyRateMinor == null
    ? t("shareMarket.free")
    : `${formatUsdMoney(subscription.dailyRateMinor, locale)} / ${t("marketBilling.day")}`;
  const serviceTiming = subscription.serviceStartedAt
    ? subscription.expiresAt
      ? `${t("shareMarket.serviceDuration.started")}: ${formatDate(subscription.serviceStartedAt, locale)} · ${t("shareMarket.serviceDuration.expires")}: ${formatDate(subscription.expiresAt, locale)}`
      : `${t("shareMarket.serviceDuration.started")}: ${formatDate(subscription.serviceStartedAt, locale)} · ${t("shareMarket.serviceDuration.permanent")}`
    : t("account.share.activationPending");

  return (
    <section
      aria-busy={busy}
      className={`grid gap-3 rounded-md border bg-card p-4 shadow-sm sm:p-5 ${anomalous ? "border-rose-200 ring-1 ring-rose-100" : "border-border"}`}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {apps.length ? (
          <span className="inline-flex items-center gap-1" title={apps.map((app) => SHARE_APP_LABELS[app]).join(" / ")}>
            {apps.map((app) => <ShareAppLogo key={app} app={app} size={18} />)}
          </span>
        ) : null}
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

      {hasStatusDetail ? (
        <div className="grid gap-0.5 border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-900">
          {grantFailed || grantContractViolation ? <p className="font-medium">{t(grantFailureMessageKey(subscription.failureCode))}</p> : null}
          {subscription.failureCode ? <p className="break-all font-mono text-[10px] text-rose-800/70">{t("shareMarket.authorizationFailure.code", { code: subscription.failureCode })}</p> : null}
          {subscription.grantAttempts != null ? <p>{t("shareMarket.authorizationFailure.attempts", { count: subscription.grantAttempts })}</p> : null}
          {subscription.releaseReason ? <p className="break-words text-rose-800/80">{t(grantFailed ? "shareMarket.authorizationFailure.reason" : "shareMarket.subscription.statusDetail", { reason: subscription.releaseReason })}</p> : null}
          {subscription.integrityState !== "compatible" ? <p>{t(integrityStatusKey(subscription.integrityState))}{subscription.integrityReason ? ` · ${integrityReasonText(subscription.integrityReason, t)}` : ""}</p> : null}
          {subscription.terminationAdjustment ? <p>{t("shareMarket.refund.summary", { amount: formatUsdMoney(subscription.terminationAdjustment.amountMinor, locale), status: t(refundStatusKey(subscription.terminationAdjustment.status)) })}</p> : null}
        </div>
      ) : null}

      {priceChange ? (
        <div className="grid gap-2 border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-sm text-amber-950">
          <strong>{t(`shareMarket.priceChange.status.${priceChange.status}`)}</strong>
          <span>{t("shareMarket.priceChange.summary", {
            previous: formatUsdMoney(priceChange.previousDailyRateMinor, locale),
            proposed: formatUsdMoney(priceChange.proposedDailyRateMinor, locale),
          })}</span>
          {perspective === "user" && priceChange.status === "pending" ? (
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="primary" isDisabled={busy} onClick={onAcceptPrice}><Check className="h-4 w-4" />{t("shareMarket.priceChange.accept")}</Button>
              <Button size="sm" variant="outline" isDisabled={busy} onClick={onRejectPrice}><X className="h-4 w-4" />{t("shareMarket.priceChange.reject")}</Button>
            </div>
          ) : null}
        </div>
      ) : null}

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

export function ShareMarketBuyerRentals({
  subscriptions,
  loading,
  onChanged,
  onInteractionChange,
  nextCursor,
  loadingMore = false,
  onLoadMore,
}: {
  subscriptions: ShareMarketSubscription[];
  loading: boolean;
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
  nextCursor?: string | null;
  loadingMore?: boolean;
  onLoadMore?: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const [busyId, setBusyId] = React.useState("");
  const [action, setAction] = React.useState<PendingAction | null>(null);
  const [error, setError] = React.useState("");
  const interactionActive = !!busyId || !!action || loadingMore;

  React.useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);

  const sorted = React.useMemo(
    () => [...subscriptions].sort(sortShareMarketSubscriptions),
    [subscriptions],
  );
  const active = sorted.filter((subscription) => !isTerminalSubscription(subscription.status));
  const history = sorted.filter((subscription) => isTerminalSubscription(subscription.status));

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

  if (loading && !subscriptions.length) {
    return <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  }

  return (
    <div className="grid min-w-0 gap-5">
      <div>
        <h2 className="text-sm font-semibold text-foreground">{t("shareMarket.workspace.rentals")}</h2>
        <p className="mt-0.5 text-xs text-muted-foreground">{t("shareMarket.workspace.rentalsHint")}</p>
      </div>
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}

      <section className="grid gap-3" aria-labelledby="share-rentals-active">
        <h3 id="share-rentals-active" className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{t("account.share.active")}</h3>
        {active.length ? active.map((subscription) => (
          <ShareMarketSubscriptionCard
            key={subscription.id}
            subscription={subscription}
            busy={busyId === subscription.id}
            onRelease={() => setAction({
              subscriptionId: subscription.id,
              title: t("shareMarket.confirm.releaseTitle"),
              description: t("shareMarket.confirm.releaseDescription", { share: subscription.shareName }),
              label: t("shareMarket.release"),
              run: () => releaseShareMarketSubscription(subscription.id),
            })}
            onAcceptPrice={() => subscription.priceChange && setAction({
              subscriptionId: subscription.id,
              title: t("shareMarket.priceChange.acceptTitle"),
              description: t("shareMarket.priceChange.acceptDescription", {
                previous: formatUsdMoney(subscription.priceChange.previousDailyRateMinor, locale),
                proposed: formatUsdMoney(subscription.priceChange.proposedDailyRateMinor, locale),
              }),
              label: t("shareMarket.priceChange.accept"),
              run: () => acceptShareMarketPriceChange(subscription.priceChange!.id),
            })}
            onRejectPrice={() => subscription.priceChange && void run(
              subscription.id,
              () => rejectShareMarketPriceChange(subscription.priceChange!.id),
            )}
          />
        )) : (
          <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
            <span>{t("account.share.userEmpty")}</span>
            <Link href={shareMarketHref()} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link>
          </div>
        )}
      </section>

      <section className="grid gap-3 border-t border-border pt-5" aria-labelledby="share-rentals-history">
        <h3 id="share-rentals-history" className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{t("account.share.history")}</h3>
        {history.length ? (
          <>
            {history.map((subscription) => (
              <ShareMarketSubscriptionCard key={subscription.id} subscription={subscription} busy={false} />
            ))}
            {nextCursor && onLoadMore ? (
              <Button variant="outline" className="justify-self-center" isDisabled={loadingMore} onClick={() => void onLoadMore()}>
                {loadingMore ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("account.share.loadMore")}
              </Button>
            ) : null}
          </>
        ) : <p className="py-3 text-sm text-muted-foreground">{t("account.share.historyEmpty")}</p>}
      </section>

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
