"use client";

/* Hallmark · component: account Share rentals table · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · expanded · attention · busy · empty
 */

import * as React from "react";
import Link from "next/link";
import { Button } from "@heroui/react";
import { ArrowUpRight, ExternalLink } from "lucide-react";
import { ExpandableRecordTable, RecordDetailList, type RecordTableColumn } from "@/components/common/expandable-record-table";
import { AccountSectionError } from "@/components/dashboard/account-section";
import { MarketServiceDisputeControl } from "@/components/dashboard/market-service-dispute-control";
import { MarketWalletRenewalCard } from "@/components/dashboard/market-wallet-renewal-card";
import { MarketPagination } from "@/components/dashboard/share-market/market-pagination";
import { paginateListings } from "@/components/dashboard/share-market/buyer-catalog-utils";
import { subdomainTunnelUrl } from "@/components/dashboard/share-dashboard-utils";
import {
  formatTokenLimit,
  grantFailureMessageKey,
  integrityReasonText,
  integrityStatusKey,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";
import {
  RentalActions,
  RentalApps,
  rentalPrice,
  rentalServiceTerm,
  useShareMarketRentalActions,
} from "@/components/dashboard/share-market/rental-controls";
import { needsRentalAttention, partitionShareMarketSubscriptions } from "@/components/dashboard/share-market/subscription-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { trialHours, walletLifecycleKey } from "@/lib/client-checkout";
import { shareMarketHref } from "@/lib/dashboard-nav";
import { shareTrialSummary } from "@/lib/share-trial";
import { compareAccountRecords } from "@/lib/account-market-records";
import type { ShareMarketSubscription } from "@/lib/types";
import { cn } from "@/lib/utils";

type Translate = ReturnType<typeof useLocaleText>["t"];

const LINK_CLASS = "inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

export function formatShortDate(value: string | null | undefined, locale: string) {
  if (!value) return "—";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(new Date(timestamp))
    : value;
}

function formatDateTime(value: string | null | undefined, locale: string) {
  if (!value) return "—";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(timestamp))
    : value;
}

/** One short line for "renews on / ends on / trial", shared by both markets. */
export function rentalTermText(
  rental: Pick<ShareMarketSubscription, "walletContract" | "expiresAt" | "serviceStartedAt">,
  locale: string,
  t: Translate,
) {
  const contract = rental.walletContract;
  if (contract) {
    return t(walletLifecycleKey(contract), {
      date: formatShortDate(contract.periodEnd, locale),
      hours: trialHours(contract.trialSecondsRemaining, locale),
    });
  }
  if (!rental.serviceStartedAt) return t("account.share.activationPending");
  return rental.expiresAt
    ? t("clientCheckout.lifecycle.expires", { date: formatShortDate(rental.expiresAt, locale) })
    : t("shareMarket.serviceDuration.permanent");
}

function statusText(subscription: ShareMarketSubscription, t: Translate) {
  const key = subscriptionStatusKey(subscription.status);
  return key ? t(key) : subscription.status;
}

function OnlineDot({ online }: { online?: boolean }) {
  const { t } = useLocaleText();
  if (online == null) return null;
  const label = online ? t("shareMarket.online") : t("shareMarket.offline");
  return <span role="img" aria-label={label} title={label} className={cn("inline-block size-2 shrink-0 rounded-full", online ? "bg-emerald-500" : "bg-rose-400")} />;
}

function StatusDetails({ subscription, t }: { subscription: ShareMarketSubscription; t: Translate }) {
  const grantFailed = subscription.status === "grant_failed";
  const grantContractViolation = subscription.failureCode === "share_market_grant_contract_violation";
  const hasStatusDetail = grantFailed
    || !!subscription.releaseReason
    || !!subscription.failureCode
    || subscription.grantAttempts != null
    || subscription.integrityState !== "compatible";
  if (!hasStatusDetail) return null;
  return (
    <div className="grid gap-0.5 border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-900">
      {grantFailed || grantContractViolation ? <p className="font-medium">{t(grantFailureMessageKey(subscription.failureCode))}</p> : null}
      {subscription.failureCode ? <p className="break-all font-mono text-[10px] text-rose-800/70">{t("shareMarket.authorizationFailure.code", { code: subscription.failureCode })}</p> : null}
      {subscription.grantAttempts != null ? <p>{t("shareMarket.authorizationFailure.attempts", { count: subscription.grantAttempts })}</p> : null}
      {subscription.releaseReason ? <p className="break-words text-rose-800/80">{t(grantFailed ? "shareMarket.authorizationFailure.reason" : "shareMarket.subscription.statusDetail", { reason: subscription.releaseReason })}</p> : null}
      {subscription.integrityState !== "compatible" ? <p>{t(integrityStatusKey(subscription.integrityState))}{subscription.integrityReason ? ` · ${integrityReasonText(subscription.integrityReason, t)}` : ""}</p> : null}
    </div>
  );
}

function RentalDetail({
  subscription,
  busy,
  onRelease,
  onChanged,
}: {
  subscription: ShareMarketSubscription;
  busy: boolean;
  onRelease?: () => void;
  onChanged: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const trial = shareTrialSummary(subscription, locale, t);
  const started = subscription.serviceStartedAt
    ? `${t("shareMarket.serviceDuration.started")}: ${formatDateTime(subscription.serviceStartedAt, locale)}`
    : t("account.share.activationPending");
  return (
    <div className="grid min-w-0 gap-3">
      <RecordDetailList items={[
        { label: t("account.market.col.status"), value: statusText(subscription, t) },
        { label: t("account.share.seat"), value: `#${subscription.seatPosition}` },
        { label: t("account.share.offer"), value: `${rentalPrice(subscription, locale, t)} · ${rentalServiceTerm(subscription, t)}` },
        { label: t("account.market.detail.limits"), value: [
          t("shareMarket.parallelShort", { value: subscription.parallelLimit == null ? "∞" : subscription.parallelLimit }),
          formatTokenLimit(subscription, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`)),
        ].join(" · ") },
        { label: t("account.market.detail.service"), value: `${started} · ${rentalTermText(subscription, locale, t)}` },
        ...(subscription.releasedAt ? [{ label: t("account.market.detail.expires"), value: formatDateTime(subscription.releasedAt, locale) }] : []),
        ...(trial ? [{ label: t("account.market.detail.trial"), value: trial }] : []),
        { label: t("account.market.col.provider"), value: <span className="break-all">{subscription.ownerEmail}</span> },
        { label: t("account.share.updated"), value: formatDateTime(subscription.updatedAt, locale) },
      ]} />
      <StatusDetails subscription={subscription} t={t} />
      {subscription.status !== "released" && subscription.walletContract?.id && subscription.cyclePriceMinor != null ? (
        <MarketWalletRenewalCard contractId={subscription.walletContract.id} refreshKey={subscription.updatedAt} compact onChanged={onChanged} />
      ) : null}
      {subscription.status !== "released" || subscription.openWalletDispute ? <MarketServiceDisputeControl contractId={subscription.walletContractId} initiallyOpen={subscription.openWalletDispute} onChanged={onChanged} /> : null}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <RentalActions subscription={subscription} t={t} busy={busy} onRelease={onRelease} />
        <Link href={shareMarketHref({ shareId: subscription.shareId })} className={LINK_CLASS}>
          {t("account.share.manageInMarket")}<ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
        </Link>
      </div>
    </div>
  );
}

export function AccountShareRentals({
  subscriptions,
  onChanged,
  onInteractionChange,
  nextCursor,
  historyTotal,
  loadingMore = false,
  onLoadMore,
  focusId,
  historyOnly = false,
  historyPageSize,
}: {
  subscriptions: ShareMarketSubscription[];
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
  nextCursor?: string | null;
  historyTotal?: number;
  loadingMore?: boolean;
  onLoadMore?: () => Promise<void> | void;
  focusId?: string | null;
  /** The public market reuses the same expandable history, without current rows. */
  historyOnly?: boolean;
  /** Preserve the Market's numbered history pages while Account can load more. */
  historyPageSize?: number;
}) {
  const { locale, t } = useLocaleText();
  const rentals = useShareMarketRentalActions(onChanged);
  const interactionActive = rentals.interactionActive || loadingMore;
  const [historyPage, setHistoryPage] = React.useState(1);
  const pendingHistoryPage = React.useRef<number | null>(null);

  React.useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);

  const { current, history } = React.useMemo(() => {
    const parts = partitionShareMarketSubscriptions(subscriptions);
    // Attention first, then the rest in the market's own order.
    const order = (item: ShareMarketSubscription) => ({ id: item.id, attention: needsRentalAttention(item), ended: item.status === "released", expiresAt: item.walletContract?.periodEnd || item.expiresAt, updatedAt: item.updatedAt });
    return { current: [...parts.attention, ...parts.active].sort((a, b) => compareAccountRecords(order(a), order(b))), history: parts.history };
  }, [subscriptions]);

  const historyCount = Math.max(historyTotal ?? 0, history.length);
  const pagedHistory = historyPageSize
    ? paginateListings(history, historyPage, historyPageSize, historyCount)
    : null;
  React.useEffect(() => {
    if (!historyPageSize) return;
    const pending = pendingHistoryPage.current;
    if (pending != null && (pending - 1) * historyPageSize < history.length) {
      setHistoryPage(pending);
      pendingHistoryPage.current = null;
    } else if (pending == null) {
      setHistoryPage(current => Math.min(current, Math.max(1, Math.ceil(history.length / historyPageSize))));
    }
  }, [history.length, historyPageSize]);
  const changeHistoryPage = (next: number) => {
    if (!pagedHistory || !historyPageSize || next < 1 || next > pagedHistory.pageCount || loadingMore) return;
    if ((next - 1) * historyPageSize >= history.length && nextCursor && onLoadMore) {
      pendingHistoryPage.current = next;
      void onLoadMore();
    } else {
      pendingHistoryPage.current = null;
      setHistoryPage(next);
    }
  };

  const columns: RecordTableColumn<ShareMarketSubscription>[] = [
    {
      id: "share",
      header: t("account.market.col.share"),
      cell: (subscription) => (
        <div className="grid min-w-0 gap-0.5">
          <span className="flex min-w-0 items-center gap-2">
            <span className="hidden sm:inline-flex"><RentalApps subscription={subscription} size={14} /></span>
            <span className="min-w-0 truncate font-medium">{subscription.shareName}</span>
            <OnlineDot online={subscription.shareOnline} />
          </span>
          <span className="truncate text-xs text-muted-foreground md:hidden">{subscription.ownerEmail}</span>
        </div>
      ),
    },
    {
      id: "provider",
      header: t("account.market.col.provider"),
      minWidth: "md",
      cell: (subscription) => <span className="block truncate text-muted-foreground" title={subscription.ownerEmail}>{subscription.ownerEmail}</span>,
    },
    {
      id: "status",
      header: t("account.market.col.status"),
      className: "w-16 sm:w-32",
      cell: (subscription) => (
        <span className={cn("block truncate", needsRentalAttention(subscription) ? "font-medium text-amber-700" : null)}>
          {statusText(subscription, t)}
        </span>
      ),
    },
    {
      id: "price",
      header: t("account.market.col.price"),
      className: "w-20 sm:w-28",
      cell: (subscription) => <span className="block truncate tabular-nums">{rentalPrice(subscription, locale, t)}</span>,
    },
    {
      id: "term",
      header: t("account.market.col.term"),
      minWidth: "lg",
      className: "w-48",
      cell: (subscription) => <span className="block truncate text-muted-foreground">{rentalTermText(subscription, locale, t)}</span>,
    },
    {
      id: "open",
      minWidth: "sm",
      header: <span className="sr-only">{t("account.market.col.actions")}</span>,
      className: "w-12",
      cell: (subscription) => {
        const url = subdomainTunnelUrl(subscription.subdomain);
        return url ? (
          <a href={url} target="_blank" rel="noopener noreferrer" aria-label={`${t("account.share.openShare")} · ${subscription.shareName}`} title={t("account.share.openShare")}
            className="inline-flex size-9 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
            <ExternalLink className="h-4 w-4" aria-hidden />
          </a>
        ) : null;
      },
    },
  ];

  return (
    <div className="grid min-w-0 gap-5">
      {rentals.error ? <AccountSectionError message={rentals.error} /> : null}
      {!historyOnly ? <section className="grid min-w-0 gap-2" aria-labelledby="account-share-rentals-current">
        <h4 id="account-share-rentals-current" className="text-xs font-semibold uppercase tracking-wide text-slate-500">
          {t("account.share.active")}
          {current.length ? <span className="ml-1.5 tabular-nums text-slate-400">{current.length}</span> : null}
        </h4>
        <ExpandableRecordTable
          rows={current}
          rowKey={(subscription) => subscription.id}
          rowLabel={(subscription) => subscription.shareName}
          ariaLabel={t("account.share.active")}
          columns={columns}
          rowTone={(subscription) => needsRentalAttention(subscription) ? "attention" : undefined}
          focusKey={focusId}
          renderDetail={(subscription) => {
            const actions = rentals.rowActions(subscription);
            return <RentalDetail subscription={subscription} busy={rentals.busyId === subscription.id} onRelease={actions.onRelease} onChanged={onChanged} />;
          }}
          empty={(
            <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
              <span>{t("account.share.userEmpty")}</span>
              <Link href={shareMarketHref()} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" aria-hidden /></Link>
            </div>
          )}
        />
      </section> : null}
      <section className="grid min-w-0 gap-2" aria-labelledby="account-share-rentals-history">
        <h4 id="account-share-rentals-history" className="text-sm font-semibold text-muted-foreground">{t("account.share.history")} · {historyCount}</h4>
        <ExpandableRecordTable rows={pagedHistory?.items ?? history} rowKey={(item) => item.id} rowLabel={(item) => item.shareName}
          columns={columns.filter((column) => column.id !== "open")}
          ariaLabel={t("account.share.history")} focusKey={focusId}
          renderDetail={(subscription) => <RentalDetail subscription={subscription} busy={false} onChanged={onChanged} />}
          empty={<p className="text-sm text-muted-foreground">{t("account.share.historyEmpty")}</p>} />
        {pagedHistory ? <MarketPagination page={pagedHistory.page} pageCount={pagedHistory.pageCount} onPageChange={changeHistoryPage} />
          : nextCursor && onLoadMore ? <Button variant="ghost" className="min-h-11 justify-self-start" isDisabled={loadingMore} onPress={() => void onLoadMore()}>{t(loadingMore ? "common.loading" : "marketWallet.more")}</Button> : null}
      </section>
      {rentals.dialog}
    </div>
  );
}
