"use client";

import * as React from "react";
import Link from "next/link";
import { ArrowUpRight } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { AccountInlineLoading, AccountSectionError } from "@/components/dashboard/account-section";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import { AccountShareRentals, rentalTermText } from "@/components/dashboard/account-share-rentals";
import { ExpandableRecordTable, RecordDetailList, type RecordTableColumn } from "@/components/common/expandable-record-table";
import { mergeShareMarketSubscriptionPage } from "@/components/dashboard/share-market/subscription-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getShareMarketOwnedListings,
  getShareMarketSubscriptions,
} from "@/lib/api";
import { rememberView, useRememberedView } from "@/lib/cached-resource";
import { shareMarketHref } from "@/lib/dashboard-nav";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { ShareMarketListing, ShareMarketSubscription } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  formatSeatPrice,
  isCoreShareApp,
  listingIdleCount,
  shareMarketMutationError,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";
import { isHistorySubscription } from "@/components/dashboard/share-market/subscription-utils";
import {
  listingAttentionSeats,
  listingLiveSeats,
  partitionOwnedListings,
} from "@/components/dashboard/share-market/owner-workspace-utils";

export type AccountMarketRole = "user" | "provider";

function ListingApps({ listing }: { listing: ShareMarketListing }) {
  const apps = listing.supportedApps.filter(isCoreShareApp);
  if (!apps.length) return null;
  return (
    <span className="inline-flex shrink-0 items-center gap-1" title={apps.map((app) => SHARE_APP_LABELS[app]).join(" / ")}>
      {apps.map((app) => <ShareAppLogo key={app} app={app} size={16} />)}
    </span>
  );
}

function seatsByPosition(listing: ShareMarketListing) {
  return [...listing.seats].filter((seat) => !seat.retiredAt).sort((left, right) => left.position - right.position);
}

function rentedSeatCount(listing: ShareMarketListing) {
  return listing.seats.filter((seat) => seat.subscription && !isHistorySubscription(seat.subscription.status)).length;
}

function ListingSeatsDetail({ listing }: { listing: ShareMarketListing }) {
  const { locale, t } = useLocaleText();
  const seats = seatsByPosition(listing);
  return (
    <div className="grid min-w-0 gap-3">
      <RecordDetailList items={[
        { label: t("account.market.col.seats"), value: `${listingIdleCount({ seats: listingLiveSeats(listing) })} / ${listingLiveSeats(listing).length}` },
        { label: t("account.market.col.rented"), value: rentedSeatCount(listing) },
      ]} />
      {seats.length ? (
        <div className="min-w-0 overflow-hidden rounded-md border border-border/70 bg-background">
          <table className="w-full table-fixed text-left text-xs" aria-label={`${listing.shareName} · ${t("account.market.col.seat")}`}>
            <thead className="bg-muted/50 text-muted-foreground">
              <tr>
                <th scope="col" className="w-14 px-3 py-1.5 font-medium">{t("account.market.col.seat")}</th>
                <th scope="col" className="px-3 py-1.5 font-medium">{t("account.market.col.renter")}</th>
                <th scope="col" className="hidden w-28 px-3 py-1.5 font-medium sm:table-cell">{t("account.market.col.price")}</th>
                <th scope="col" className="w-28 px-3 py-1.5 font-medium">{t("account.market.col.status")}</th>
                <th scope="col" className="hidden w-44 px-3 py-1.5 font-medium md:table-cell">{t("account.market.col.term")}</th>
              </tr>
            </thead>
            <tbody>
              {seats.map((seat) => {
                const subscription = seat.subscription && !isHistorySubscription(seat.subscription.status) ? seat.subscription : undefined;
                const statusKey = subscription ? subscriptionStatusKey(subscription.status) : null;
                return (
                  <tr key={seat.id} className="border-t border-border/60">
                    <td className="px-3 py-1.5 tabular-nums text-muted-foreground">#{seat.position}</td>
                    <td className="min-w-0 break-all px-3 py-1.5">
                      {subscription?.renterEmail || <span className="text-muted-foreground">{t("account.market.vacant")}</span>}
                      <span className="mt-1 block text-muted-foreground sm:hidden">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("market.day"), t("market.month"))}</span>
                      {subscription ? <span className="mt-1 block text-muted-foreground md:hidden">{rentalTermText(subscription, locale, t)}</span> : null}
                    </td>
                    <td className="hidden truncate px-3 py-1.5 tabular-nums sm:table-cell">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("market.day"), t("market.month"))}</td>
                    <td className="truncate px-3 py-1.5">{subscription ? (statusKey ? t(statusKey) : subscription.status) : seat.status === "available" ? t("account.market.vacant") : seat.status}</td>
                    <td className="hidden truncate px-3 py-1.5 text-muted-foreground md:table-cell">{subscription ? rentalTermText(subscription, locale, t) : "—"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : <p className="text-sm text-muted-foreground">{t("account.market.detail.seatsEmpty")}</p>}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {listing.subdomain ? <span className="mr-auto min-w-0 truncate font-mono text-[11px] text-muted-foreground">{listing.subdomain}</span> : null}
        <Link
          href={shareMarketHref({ workspace: "selling", shareId: listing.shareId })}
          className="inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
        >
          {t("account.share.manageInMarket")}<ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
        </Link>
      </div>
    </div>
  );
}

function AccountShareListings({ listings, focusId }: { listings: ShareMarketListing[]; focusId?: string | null }) {
  const { t } = useLocaleText();
  const columns: RecordTableColumn<ShareMarketListing>[] = [
    {
      id: "share",
      header: t("account.market.col.share"),
      cell: (listing) => (
        <span className="flex min-w-0 items-center gap-2">
          <ListingApps listing={listing} />
          <span className="min-w-0 truncate font-medium">{listing.shareName}</span>
          <span role="img" aria-label={listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
            title={listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
            className={cn("inline-block size-2 shrink-0 rounded-full", listing.shareOnline ? "bg-emerald-500" : "bg-rose-400")} />
        </span>
      ),
    },
    {
      id: "status",
      header: t("account.market.col.status"),
      className: "w-24 sm:w-28",
      cell: (listing) => {
        const attention = listingAttentionSeats(listing).length;
        return (
          <div className="grid min-w-0 gap-0.5">
            <span className="truncate">{listing.status === "closed" ? t("shareMarket.closed") : t("account.market.listed")}</span>
            {attention ? <span className="truncate text-xs font-medium text-amber-700">{t("account.market.attention", { count: attention })}</span> : null}
          </div>
        );
      },
    },
    {
      id: "seats",
      header: t("account.market.col.seats"),
      minWidth: "sm",
      className: "w-28",
      cell: (listing) => {
        const live = listingLiveSeats(listing);
        return <span className="tabular-nums">{listingIdleCount({ seats: live })} / {live.length}</span>;
      },
    },
    {
      id: "rented",
      header: t("account.market.col.rented"),
      minWidth: "md",
      className: "w-20",
      cell: (listing) => <span className="tabular-nums">{rentedSeatCount(listing)}</span>,
    },
    {
      id: "manage", header: <span className="sr-only">{t("account.market.col.actions")}</span>, minWidth: "sm", className: "w-12",
      cell: (listing) => <Link href={shareMarketHref({ workspace: "selling", shareId: listing.shareId })} aria-label={`${t("account.share.manageInMarket")} · ${listing.shareName}`} className="inline-flex size-11 items-center justify-center rounded-md hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring"><ArrowUpRight className="size-4" aria-hidden /></Link>,
    },
  ];
  return (
    <ExpandableRecordTable
      rows={listings}
      rowKey={(listing) => listing.id}
      rowLabel={(listing) => listing.shareName}
      ariaLabel={t("account.listings.shareSection")}
      columns={columns}
      rowTone={(listing) => listingAttentionSeats(listing).length ? "attention" : undefined}
      focusKey={focusId}
      renderDetail={(listing) => <ListingSeatsDetail listing={listing} />}
    />
  );
}

type AccountShareView = {
  subscriptions: ShareMarketSubscription[];
  subscriptionCursor: string | null;
  subscriptionHistoryTotal: number;
  ownedListings: ShareMarketListing[];
};

const accountShareViewKey = (role: AccountMarketRole, actorKey: string) => `account-share:${role}:${actorKey}`;

/**
 * Share Market seats of this account: what it rents (`user`) or what it lists
 * (`provider`). Each role loads only its own list, so one endpoint failing
 * never hides the other, and remembers it for an instant return.
 */
export function AccountShareSection({ role, focusId }: { role: AccountMarketRole; focusId?: string | null }) {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const actorKey = authed
    ? session?.user?.id || session?.user?.email?.toLowerCase() || "authenticated"
    : "anonymous";
  const { remembered, takeSeed } = useRememberedView<AccountShareView>(authed ? accountShareViewKey(role, actorKey) : null);
  const [subscriptions, setSubscriptions] = React.useState<ShareMarketSubscription[]>(remembered?.subscriptions ?? []);
  const [subscriptionCursor, setSubscriptionCursor] = React.useState<string | null>(remembered?.subscriptionCursor ?? null);
  const [subscriptionHistoryTotal, setSubscriptionHistoryTotal] = React.useState(remembered?.subscriptionHistoryTotal ?? 0);
  const [loadingMoreSubscriptions, setLoadingMoreSubscriptions] = React.useState(false);
  const [ownedListings, setOwnedListings] = React.useState<ShareMarketListing[]>(remembered?.ownedListings ?? []);
  const [loading, setLoading] = React.useState(!remembered);
  const [error, setError] = React.useState("");
  const [failedActorKey, setFailedActorKey] = React.useState<string | null>(null);
  const [pausePolling, setPausePolling] = React.useState(false);
  const [loadedActorKey, setLoadedActorKey] = React.useState(actorKey);
  const abortRef = React.useRef<AbortController | null>(null);
  const loadMoreAbortRef = React.useRef<AbortController | null>(null);
  const actorKeyRef = React.useRef(actorKey);
  actorKeyRef.current = actorKey;
  const expandedSubscriptionHistoryRef = React.useRef(false);
  const subscriptionHistoryGenerationRef = React.useRef(0);

  const load = React.useCallback(async ({
    silent = false,
    skipIfBusy = false,
  }: { silent?: boolean; skipIfBusy?: boolean } = {}) => {
    const requestedActorKey = actorKey;
    if (!authed || actorKeyRef.current !== requestedActorKey) return;
    if (skipIfBusy && abortRef.current) return;
    if (!silent) {
      subscriptionHistoryGenerationRef.current += 1;
      loadMoreAbortRef.current?.abort();
      loadMoreAbortRef.current = null;
      setLoadingMoreSubscriptions(false);
    }
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    if (!silent) {
      setLoading(true);
      setFailedActorKey(null);
    }
    if (!skipIfBusy) setError("");
    try {
      const [nextSubscriptions, nextListings] = await Promise.all([
        role === "user" ? getShareMarketSubscriptions(controller.signal) : null,
        role === "provider" ? getShareMarketOwnedListings(controller.signal) : null,
      ]);
      if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
      // A partial response must not take the section down with it.
      const subscriptionPage = nextSubscriptions?.subscriptions ?? [];
      const listings = nextListings?.listings ?? [];
      if (nextSubscriptions) {
        setSubscriptions((current) => silent
          ? mergeShareMarketSubscriptionPage(current, subscriptionPage, false)
          : subscriptionPage);
        setSubscriptionHistoryTotal(nextSubscriptions.historyTotal ?? 0);
        if (!silent || !expandedSubscriptionHistoryRef.current) {
          if (!silent) expandedSubscriptionHistoryRef.current = false;
          setSubscriptionCursor(nextSubscriptions.nextCursor || null);
        }
      }
      if (nextListings) setOwnedListings(listings);
      rememberView<AccountShareView>(accountShareViewKey(role, requestedActorKey), {
        subscriptions: subscriptionPage,
        subscriptionCursor: nextSubscriptions?.nextCursor || null,
        subscriptionHistoryTotal: nextSubscriptions?.historyTotal ?? 0,
        ownedListings: listings,
      });
      setLoadedActorKey(requestedActorKey);
      setFailedActorKey(null);
    } catch (reason) {
      if (!controller.signal.aborted
        && actorKeyRef.current === requestedActorKey
        && !skipIfBusy) {
        setError(shareMarketMutationError(reason, t));
        if (!silent) setFailedActorKey(requestedActorKey);
      }
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null;
        setLoading(false);
      }
    }
  }, [actorKey, authed, role, t]);
  const loadMoreSubscriptions = React.useCallback(async () => {
    if (!subscriptionCursor || loadingMoreSubscriptions) return;
    const requestedActorKey = actorKey;
    const requestedGeneration = subscriptionHistoryGenerationRef.current;
    const controller = new AbortController();
    loadMoreAbortRef.current?.abort();
    loadMoreAbortRef.current = controller;
    setLoadingMoreSubscriptions(true);
    setError("");
    try {
      const page = await getShareMarketSubscriptions(controller.signal, subscriptionCursor);
      if (controller.signal.aborted
        || actorKeyRef.current !== requestedActorKey
        || subscriptionHistoryGenerationRef.current !== requestedGeneration) return;
      expandedSubscriptionHistoryRef.current = true;
      setSubscriptions((current) => mergeShareMarketSubscriptionPage(current, page.subscriptions ?? [], true));
      setSubscriptionCursor(page.nextCursor || null);
      setSubscriptionHistoryTotal(page.historyTotal ?? 0);
    } catch (reason) {
      if (controller.signal.aborted
        || actorKeyRef.current !== requestedActorKey
        || subscriptionHistoryGenerationRef.current !== requestedGeneration) return;
      setError(shareMarketMutationError(reason, t));
    } finally {
      if (loadMoreAbortRef.current === controller) {
        loadMoreAbortRef.current = null;
        setLoadingMoreSubscriptions(false);
      }
    }
  }, [actorKey, loadingMoreSubscriptions, subscriptionCursor, t]);
  React.useEffect(() => {
    subscriptionHistoryGenerationRef.current += 1;
    loadMoreAbortRef.current?.abort();
    loadMoreAbortRef.current = null;
    setLoadingMoreSubscriptions(false);
    expandedSubscriptionHistoryRef.current = false;
    // A remembered view is already on screen; refresh it without a spinner.
    if (authed) void load({ silent: takeSeed() });
    else setLoading(false);
    return () => {
      abortRef.current?.abort();
      loadMoreAbortRef.current?.abort();
    };
  }, [actorKey, authed, load, takeSeed]);
  React.useEffect(() => {
    if (!authed || pausePolling) return;
    const timer = window.setInterval(
      () => void load({ silent: true, skipIfBusy: true }),
      CLIENT_MARKET_POLL_MS,
    );
    return () => window.clearInterval(timer);
  }, [authed, load, pausePolling]);

  // Follow a rental link even when it points beyond the first history page.
  const focusCursors = React.useRef(new Set<string>());
  React.useEffect(() => { focusCursors.current.clear(); }, [actorKey, focusId]);
  React.useEffect(() => {
    if (role !== "user" || !focusId || loading || loadingMoreSubscriptions || !subscriptionCursor
      || loadedActorKey !== actorKey || subscriptions.some((item) => item.id === focusId)
      || focusCursors.current.has(subscriptionCursor)) return;
    focusCursors.current.add(subscriptionCursor);
    void loadMoreSubscriptions();
  }, [actorKey, focusId, role, loading, loadedActorKey, subscriptions, subscriptionCursor, loadingMoreSubscriptions, loadMoreSubscriptions]);

  const { attentionListings, active, closed } = React.useMemo(
    () => partitionOwnedListings(ownedListings),
    [ownedListings],
  );
  const providerListings = [...attentionListings, ...active, ...closed];

  if (!authed) return null;
  if (loading || (loadedActorKey !== actorKey && failedActorKey !== actorKey)) return <AccountInlineLoading />;
  if (loadedActorKey !== actorKey) return <AccountSectionError message={error} onRetry={() => void load()} />;
  return (
    <div className="grid min-w-0 gap-4">
      {error ? <AccountSectionError message={error} /> : null}
      {role === "user" ? (
        <AccountShareRentals
          key={`user:${actorKey}`}
          subscriptions={subscriptions}
          focusId={focusId}
          onChanged={() => load({ silent: true })}
          onInteractionChange={setPausePolling}
          nextCursor={subscriptionCursor}
          historyTotal={subscriptionHistoryTotal}
          loadingMore={loadingMoreSubscriptions}
          onLoadMore={loadMoreSubscriptions}
        />
      ) : providerListings.length ? (
        <AccountShareListings key={`provider:${actorKey}`} listings={providerListings} focusId={focusId} />
      ) : <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-12 text-center text-sm text-muted-foreground"><span>{t("account.share.providerEmpty")}</span><Link href={shareMarketHref({ workspace: "selling" })} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link></div>}
    </div>
  );
}
