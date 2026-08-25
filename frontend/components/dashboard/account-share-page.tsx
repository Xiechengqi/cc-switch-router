"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Button } from "@heroui/react";
import {
  ArrowUpRight,
  Loader2,
  RefreshCw,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import { ShareMarketBuyerRentals } from "@/components/dashboard/share-market/buyer-rentals";
import { mergeShareMarketSubscriptionPage } from "@/components/dashboard/share-market/subscription-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getShareMarketOwnedListings,
  getShareMarketSubscriptions,
} from "@/lib/api";
import {
  DASHBOARD_ACCOUNT_SHARE_PATH,
  shareMarketHref,
} from "@/lib/dashboard-nav";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { ShareMarketListing, ShareMarketSubscription } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  isCoreShareApp,
  listingIdleCount,
  shareMarketMutationError,
} from "@/components/dashboard/share-market/market-utils";
import {
  listingAttentionSeats,
  listingClosedRentalSeats,
  listingLiveSeats,
  partitionOwnedListings,
} from "@/components/dashboard/share-market/owner-workspace-utils";

type ShareMonitorTab = "user" | "provider";

function ListingApps({ listing }: { listing: ShareMarketListing }) {
  const apps = listing.supportedApps.filter(isCoreShareApp);
  if (!apps.length) return null;
  return (
    <span className="inline-flex shrink-0 items-center gap-1" title={apps.map((app) => SHARE_APP_LABELS[app]).join(" / ")}>
      {apps.map((app) => <ShareAppLogo key={app} app={app} size={16} />)}
    </span>
  );
}

function AccountListingRow({ listing }: { listing: ShareMarketListing }) {
  const { t } = useLocaleText();
  const closed = listing.status === "closed";
  const liveSeats = listingLiveSeats(listing);
  const idle = listingIdleCount({ seats: liveSeats });
  const remaining = listingClosedRentalSeats(listing).length;
  const attention = listingAttentionSeats(listing).length;
  const statusLabel = closed
    ? t("shareMarket.closed")
    : t("shareMarket.catalog.occupancy", { idle, total: liveSeats.length });
  return (
    <article className="flex min-w-0 flex-col gap-2 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between">
      <div className="min-w-0">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
          <ListingApps listing={listing} />
          <strong className="min-w-0 truncate text-sm text-slate-900">{listing.shareName}</strong>
          <span className={cn("shrink-0 text-[11px] font-medium", listing.shareOnline ? "text-emerald-700" : "text-rose-700")}>
            {listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
          </span>
          <span className="min-w-0 truncate text-[11px] text-slate-500">{statusLabel}</span>
        </div>
        <p className="mt-1 flex min-w-0 flex-wrap gap-x-2 gap-y-0.5 text-xs text-slate-500">
          {attention ? <span className="font-medium text-rose-700">{t("account.share.seatsAttention")} · {attention}</span> : null}
          {closed && remaining ? <span>{t("shareMarket.listings.activeRentals", { count: remaining })}</span> : null}
          {listing.subdomain ? <span className="min-w-0 truncate font-mono text-[11px] text-slate-400">{listing.subdomain}</span> : null}
        </p>
      </div>
      <Link
        href={shareMarketHref({ workspace: "selling", shareId: listing.shareId })}
        className="inline-flex h-7 shrink-0 items-center gap-1 rounded-md px-2 text-xs font-medium text-slate-700 hover:bg-slate-100 hover:text-slate-900"
      >
        {t("account.share.manageInMarket")}
        <ArrowUpRight className="h-3.5 w-3.5" />
      </Link>
    </article>
  );
}

export function AccountSharePage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const router = useRouter();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const actorKey = authed
    ? session?.user?.id || session?.user?.email?.toLowerCase() || "authenticated"
    : "anonymous";
  const tab: ShareMonitorTab = searchParams.get("tab") === "provider" ? "provider" : "user";
  const [subscriptions, setSubscriptions] = React.useState<ShareMarketSubscription[]>([]);
  const [subscriptionCursor, setSubscriptionCursor] = React.useState<string | null>(null);
  const [loadingMoreSubscriptions, setLoadingMoreSubscriptions] = React.useState(false);
  const [ownedListings, setOwnedListings] = React.useState<ShareMarketListing[]>([]);
  const [loading, setLoading] = React.useState(true);
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

  const setTab = (next: ShareMonitorTab) => {
    const params = new URLSearchParams(searchParams.toString());
    params.set("tab", next);
    router.replace(`${DASHBOARD_ACCOUNT_SHARE_PATH}?${params.toString()}`, { scroll: false });
  };
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
        getShareMarketSubscriptions(controller.signal),
        getShareMarketOwnedListings(controller.signal),
      ]);
      if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
      setSubscriptions((current) => silent
        ? mergeShareMarketSubscriptionPage(current, nextSubscriptions.subscriptions, false)
        : nextSubscriptions.subscriptions);
      if (!silent || !expandedSubscriptionHistoryRef.current) {
        if (!silent) expandedSubscriptionHistoryRef.current = false;
        setSubscriptionCursor(nextSubscriptions.nextCursor || null);
      }
      setOwnedListings(nextListings.listings);
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
  }, [actorKey, authed, t]);
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
      setSubscriptions((current) => mergeShareMarketSubscriptionPage(current, page.subscriptions, true));
      setSubscriptionCursor(page.nextCursor || null);
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
    if (authed) void load();
    else setLoading(false);
    return () => {
      abortRef.current?.abort();
      loadMoreAbortRef.current?.abort();
    };
  }, [actorKey, authed, load]);
  React.useEffect(() => {
    if (!authed || pausePolling) return;
    const timer = window.setInterval(
      () => void load({ silent: true, skipIfBusy: true }),
      CLIENT_MARKET_POLL_MS,
    );
    return () => window.clearInterval(timer);
  }, [authed, load, pausePolling]);

  const { attentionListings, active, closed } = React.useMemo(
    () => partitionOwnedListings(ownedListings),
    [ownedListings],
  );
  const providerListings = [...attentionListings, ...active, ...closed];

  if (!authed) return <p className="py-6 text-sm text-muted-foreground">{t("shareMarket.loginRequired")}</p>;
  if (loading || (loadedActorKey !== actorKey && failedActorKey !== actorKey)) return <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  if (loadedActorKey !== actorKey) {
    return (
      <div role="alert" className="flex flex-wrap items-center gap-3 border-l-2 border-rose-400 bg-rose-50 px-3 py-3 text-sm text-rose-700">
        <span className="min-w-0 flex-1 break-words">{error}</span>
        <Button size="sm" variant="outline" onClick={() => void load()}>
          <RefreshCw className="h-4 w-4" />
          {t("common.retry")}
        </Button>
      </div>
    );
  }
  return (
    <div className="grid min-w-0 gap-4">
      <div><h2 className="text-base font-semibold text-foreground">{t("account.nav.share")}</h2><p className="mt-0.5 text-sm text-muted-foreground">{t("account.shareHint")}</p></div>
      <SegmentedControl value={tab} onChange={setTab} ariaLabel={t("account.nav.share")} size="md" className="w-full max-w-sm" fullWidth items={[{ id: "user", label: t("account.share.tab.user") }, { id: "provider", label: t("account.share.tab.provider") }]} />
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}
      {tab === "user" ? (
        <ShareMarketBuyerRentals
          key={`user:${actorKey}`}
          subscriptions={subscriptions}
          loading={false}
          onChanged={() => load({ silent: true })}
          onInteractionChange={setPausePolling}
          nextCursor={subscriptionCursor}
          loadingMore={loadingMoreSubscriptions}
          onLoadMore={loadMoreSubscriptions}
        />
      ) : providerListings.length ? (
        <div className="divide-y divide-slate-100 rounded-md border border-slate-200 bg-white">
          {providerListings.map((listing) => (
            <AccountListingRow key={`${actorKey}:${listing.id}`} listing={listing} />
          ))}
        </div>
      ) : <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-12 text-center text-sm text-muted-foreground"><span>{t("account.share.providerEmpty")}</span><Link href={shareMarketHref({ workspace: "selling" })} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link></div>}
    </div>
  );
}
