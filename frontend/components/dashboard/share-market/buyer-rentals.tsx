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
import { subdomainTunnelUrl } from "@/components/dashboard/share-dashboard-utils";
import { filterMarketListings } from "@/components/dashboard/share-market/buyer-catalog-utils";
import { MarketListingFilters } from "@/components/dashboard/share-market/market-listing-filters";
import {
  CatalogSeatPreviewList,
  MARKET_SHARE_CARD_GRID_CLASS,
  MarketShareCard,
  listingCardId,
} from "@/components/dashboard/share-market/market-share-card";
import {
  RentalActions,
  RentalApps,
  rentalPrice,
  ShareMarketRentalHistory,
  useShareMarketRentalActions,
} from "@/components/dashboard/share-market/rental-controls";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  shareMarketHref,
} from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import type { ShareMarketListing, ShareMarketProviderFamily, ShareMarketSubscription } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  formatTokenLimit,
  grantFailureMessageKey,
  integrityReasonText,
  integrityStatusKey,
  refundStatusKey,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";
import {
  groupActiveRentalsByShare,
  listingForRentalShare,
  partitionShareMarketSubscriptions,
  sortShareMarketSubscriptions,
} from "@/components/dashboard/share-market/subscription-utils";

export { sortShareMarketSubscriptions };

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
  const [family, setFamily] = React.useState<ShareMarketProviderFamily | "all">("all");
  const [query, setQuery] = React.useState("");
  const rentals = useShareMarketRentalActions(onChanged);
  const interactionActive = rentals.interactionActive || loadingMore;

  React.useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);

  const { history } = React.useMemo(
    () => partitionShareMarketSubscriptions(subscriptions),
    [subscriptions],
  );
  const activeGroups = React.useMemo(
    () => {
      const groups = groupActiveRentalsByShare(subscriptions);
      if (family === "all" && !query.trim()) return groups;
      return groups.filter((group) => {
        const listing = listingForRentalShare(listings, group) as ShareMarketListing | undefined;
        if (!listing) {
          if (family !== "all") return false;
          const needle = query.trim().toLocaleLowerCase();
          if (!needle) return true;
          return [
            group.subscription.shareName,
            group.subscription.subdomain,
            group.subscription.ownerEmail,
            ...(group.subscription.apps || []),
          ].filter(Boolean).join(" ").toLocaleLowerCase().includes(needle);
        }
        return filterMarketListings([listing], family, query).length > 0;
      });
    },
    [family, listings, query, subscriptions],
  );

  if (loading && !subscriptions.length && !listings.length) {
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
      {rentals.error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{rentals.error}</p> : null}
      {listings.length || query || family !== "all" ? (
        <MarketListingFilters
          listings={listings}
          family={family}
          query={query}
          onFamilyChange={setFamily}
          onQueryChange={setQuery}
        />
      ) : null}

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
              const actions = rentals.rowActions(subscription);
              const mySeat = listing?.seats.find((seat) => seat.id === subscription.seatId);
              const occupancy = listing
                ? undefined
                : t("shareMarket.catalog.seatPosition", { position: subscription.seatPosition });
              const footer = listing && mySeat ? (
                <div className="grid content-start">
                  <CatalogSeatPreviewList
                    listing={listing}
                    seats={[mySeat]}
                    preferredSeatIds={[subscription.seatId]}
                    mineSeatIds={[subscription.seatId]}
                    showHint={false}
                  />
                  <div data-no-card-open>
                    <RentalActions subscription={subscription} t={t} busy={rentals.busyId === subscription.id} onRelease={actions.onRelease} onAcceptPrice={actions.onAcceptPrice} onRejectPrice={actions.onRejectPrice} />
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
                    <RentalActions subscription={subscription} t={t} busy={rentals.busyId === subscription.id} onRelease={actions.onRelease} onAcceptPrice={actions.onAcceptPrice} onRejectPrice={actions.onRejectPrice} />
                  </div>
                </div>
              );
              if (listing) {
                return (
                  <MarketShareCard
                    key={group.shareId}
                    listing={listing}
                    attention={group.attention}
                    rented
                    cardId={listingCardId("rental", group.shareId)}
                    occupancy={occupancy}
                    footer={footer}
                  />
                );
              }
              return (
                <article
                  key={group.shareId}
                  id={listingCardId("rental", group.shareId)}
                  className={cn(
                    "grid min-h-[15rem] min-w-0 scroll-mt-20 grid-rows-[auto_1fr] gap-2.5 rounded-xl border bg-white p-3 shadow-sm",
                    group.attention ? "border-amber-300 ring-1 ring-amber-200" : "border-slate-200",
                  )}
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

      <ShareMarketRentalHistory
        subscriptions={history}
        nextCursor={nextCursor}
        loadingMore={loadingMore}
        onLoadMore={onLoadMore}
      />
      {rentals.dialog}
    </div>
  );
}
