"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareMarketBuyerCatalog } from "@/components/dashboard/share-market/buyer-catalog";
import { shareMarketMutationError } from "@/components/dashboard/share-market/market-utils";
import { mergeShareMarketSubscriptionPage, subscriptionsNeedGrantPolling } from "@/components/dashboard/share-market/subscription-utils";
import { ShareMarketOwnerWorkspace } from "@/components/dashboard/share-market/owner-workspace";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getShareMarketCatalog,
  getShareMarketOwnedListings,
  getShareMarketOwnedShares,
  getShareMarketRentedListings,
  getShareMarketSubscriptions,
} from "@/lib/api";
import type {
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketSubscription,
} from "@/lib/types";

type Workspace = "catalog" | "selling";
type LoadScope = Workspace | "all";
const SHARE_MARKET_POLL_MS = 15_000;
const SHARE_MARKET_GRANT_POLL_MS = 2_000;

function workspaceFromQuery(value: string | null): Workspace {
  if (value === "selling" || value === "mine") return "selling";
  return "catalog";
}

function mineFromQuery(value: string | null) {
  return value === "rentals" || value === "rented";
}

function replaceWorkspaceQuery(workspace: Workspace) {
  const url = new URL(window.location.href);
  if (workspace === "catalog") url.searchParams.delete("view");
  else url.searchParams.set("view", workspace);
  url.searchParams.delete("tab");
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

function consumeRentalsQuery() {
  const url = new URL(window.location.href);
  const view = url.searchParams.get("view") || url.searchParams.get("tab");
  if (view !== "rentals" && view !== "rented") return;
  url.searchParams.delete("view");
  url.searchParams.delete("tab");
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

export function ShareMarketWorkspace() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const actorKey = authed
    ? session?.user?.id || session?.user?.email?.toLowerCase() || "authenticated"
    : "anonymous";
  const [catalog, setCatalog] = React.useState<ShareMarketCatalog | null>(null);
  const [ownedListings, setOwnedListings] = React.useState<ShareMarketListing[]>([]);
  const [rentedListings, setRentedListings] = React.useState<ShareMarketListing[]>([]);
  const [ownedShareCount, setOwnedShareCount] = React.useState<number | null>(null);
  const [subscriptions, setSubscriptions] = React.useState<ShareMarketSubscription[]>([]);
  const [subscriptionCursor, setSubscriptionCursor] = React.useState<string | null>(null);
  const [loadingMoreSubscriptions, setLoadingMoreSubscriptions] = React.useState(false);
  const [pausePolling, setPausePolling] = React.useState(false);
  const [workspace, setWorkspaceState] = React.useState<Workspace>(() =>
    workspaceFromQuery(searchParams.get("view") || searchParams.get("tab")),
  );
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [loadedActorKey, setLoadedActorKey] = React.useState(actorKey);
  const requestRef = React.useRef<AbortController | null>(null);
  const loadMoreRequestRef = React.useRef<AbortController | null>(null);
  const actorKeyRef = React.useRef(actorKey);
  actorKeyRef.current = actorKey;
  const expandedSubscriptionHistoryRef = React.useRef(false);
  const subscriptionHistoryGenerationRef = React.useRef(0);
  const focusedShareId = searchParams.get("focus") || undefined;
  const initialMine = mineFromQuery(searchParams.get("view") || searchParams.get("tab"));

  React.useEffect(() => {
    consumeRentalsQuery();
  }, []);

  const load = React.useCallback(async ({
    scope = "all",
    silent = false,
    skipIfBusy = false,
  }: { scope?: LoadScope; silent?: boolean; skipIfBusy?: boolean } = {}) => {
    const requestedActorKey = actorKey;
    if (actorKeyRef.current !== requestedActorKey) return;
    if (skipIfBusy && requestRef.current) return;
    const resetSubscriptionHistory = !silent
      && (scope === "all" || scope === "catalog");
    if (resetSubscriptionHistory) {
      subscriptionHistoryGenerationRef.current += 1;
      loadMoreRequestRef.current?.abort();
      loadMoreRequestRef.current = null;
      setLoadingMoreSubscriptions(false);
    }
    requestRef.current?.abort();
    const controller = new AbortController();
    requestRef.current = controller;
    if (!silent) setLoading(true);
    if (!skipIfBusy) setError("");

    try {
      if (resetSubscriptionHistory) expandedSubscriptionHistoryRef.current = false;
      const applySubscriptionPage = (page: Awaited<ReturnType<typeof getShareMarketSubscriptions>>) => {
        setSubscriptions((current) => resetSubscriptionHistory
          ? page.subscriptions
          : mergeShareMarketSubscriptionPage(current, page.subscriptions, false));
        if (resetSubscriptionHistory || !expandedSubscriptionHistoryRef.current) {
          setSubscriptionCursor(page.nextCursor || null);
        }
      };

      if (!authed) {
        const nextCatalog = await getShareMarketCatalog(controller.signal);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        setCatalog(nextCatalog);
        setOwnedListings([]);
        setRentedListings([]);
        setOwnedShareCount(0);
        setSubscriptions([]);
        setSubscriptionCursor(null);
        setLoadedActorKey(actorKey);
        return;
      }

      if (scope === "all") {
        const [nextCatalog, nextOwned, nextRented, nextShares, nextSubscriptions] = await Promise.all([
          getShareMarketCatalog(controller.signal),
          getShareMarketOwnedListings(controller.signal),
          getShareMarketRentedListings(controller.signal),
          getShareMarketOwnedShares(controller.signal),
          getShareMarketSubscriptions(controller.signal),
        ]);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        setCatalog(nextCatalog);
        setOwnedListings(nextOwned.listings);
        setRentedListings(nextRented.listings);
        setOwnedShareCount(nextShares.length);
        applySubscriptionPage(nextSubscriptions);
        setLoadedActorKey(actorKey);
      } else if (scope === "catalog") {
        const [nextCatalog, nextRented, nextShares, nextSubscriptions] = await Promise.all([
          getShareMarketCatalog(controller.signal),
          getShareMarketRentedListings(controller.signal),
          getShareMarketOwnedShares(controller.signal),
          getShareMarketSubscriptions(controller.signal),
        ]);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        setCatalog(nextCatalog);
        setRentedListings(nextRented.listings);
        setOwnedShareCount(nextShares.length);
        applySubscriptionPage(nextSubscriptions);
        setLoadedActorKey(actorKey);
      } else {
        const [nextOwned, nextShares] = await Promise.all([
          getShareMarketOwnedListings(controller.signal),
          getShareMarketOwnedShares(controller.signal),
        ]);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        setOwnedListings(nextOwned.listings);
        setOwnedShareCount(nextShares.length);
        setLoadedActorKey(actorKey);
      }
    } catch (reason) {
      if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
      if (!skipIfBusy) setError(shareMarketMutationError(reason, t));
    } finally {
      if (requestRef.current === controller) {
        requestRef.current = null;
        setLoading(false);
      }
    }
  }, [actorKey, authed, t]);

  const loadMoreSubscriptions = React.useCallback(async () => {
    if (!subscriptionCursor || loadingMoreSubscriptions) return;
    const requestedActorKey = actorKey;
    const requestedGeneration = subscriptionHistoryGenerationRef.current;
    const controller = new AbortController();
    loadMoreRequestRef.current?.abort();
    loadMoreRequestRef.current = controller;
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
      if (loadMoreRequestRef.current === controller) {
        loadMoreRequestRef.current = null;
        setLoadingMoreSubscriptions(false);
      }
    }
  }, [actorKey, loadingMoreSubscriptions, subscriptionCursor, t]);

  React.useEffect(() => {
    if (authLoading) return;
    subscriptionHistoryGenerationRef.current += 1;
    loadMoreRequestRef.current?.abort();
    loadMoreRequestRef.current = null;
    setLoadingMoreSubscriptions(false);
    expandedSubscriptionHistoryRef.current = false;
    if (!authed) {
      setWorkspaceState((current) => {
        if (current !== "catalog") replaceWorkspaceQuery("catalog");
        return "catalog";
      });
    }
    void load({ scope: "all" });
    return () => {
      requestRef.current?.abort();
      loadMoreRequestRef.current?.abort();
    };
  }, [actorKey, authed, authLoading, load]);

  const grantPolling = subscriptionsNeedGrantPolling(subscriptions);
  React.useEffect(() => {
    if (authLoading) return;
    const tick = () => {
      if (document.visibilityState !== "visible" || pausePolling) return;
      void load({ scope: workspace, silent: true, skipIfBusy: true });
    };
    if (grantPolling) tick();
    const timer = window.setInterval(tick, grantPolling ? SHARE_MARKET_GRANT_POLL_MS : SHARE_MARKET_POLL_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") tick();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [authLoading, grantPolling, load, pausePolling, workspace]);

  const setWorkspace = (next: Workspace) => {
    if (!authed && next !== "catalog") return;
    setWorkspaceState(next);
    replaceWorkspaceQuery(next);
    void load({ scope: next });
  };

  const actorDataCurrent = loadedActorKey === actorKey;
  const visibleCatalog = actorDataCurrent ? catalog : null;
  const visibleOwnedListings = actorDataCurrent ? ownedListings : [];
  const visibleRentedListings = actorDataCurrent ? rentedListings : [];
  const visibleOwnedShareCount = actorDataCurrent ? ownedShareCount : null;
  const visibleSubscriptions = actorDataCurrent ? subscriptions : [];
  const visibleSubscriptionCursor = actorDataCurrent ? subscriptionCursor : null;
  const hasOwnedShares = (visibleOwnedShareCount ?? 0) > 0;
  const showSellingTab = authed && hasOwnedShares;

  React.useEffect(() => {
    if (workspace !== "selling" || visibleOwnedShareCount == null || hasOwnedShares) return;
    setWorkspaceState("catalog");
    replaceWorkspaceQuery("catalog");
  }, [hasOwnedShares, visibleOwnedShareCount, workspace]);

  const workspaceItems = [
    { id: "catalog" as const, label: t("shareMarket.workspace.catalog") },
    ...(showSellingTab
      ? [{ id: "selling" as const, label: t("shareMarket.workspace.selling") }]
      : []),
  ];
  const workspaceValue = workspace === "selling" && !showSellingTab ? "catalog" : workspace;

  return (
    <main className="mx-auto grid w-full max-w-7xl gap-5 px-1 pb-10">
      <h1 className="sr-only">{t("shareMarket.title")}</h1>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-slate-200 pb-3">
        {authed ? (
          <SegmentedControl
            value={workspaceValue}
            onChange={setWorkspace}
            ariaLabel={t("shareMarket.workspace.label")}
            size="sm"
            items={workspaceItems}
          />
        ) : (
          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-slate-900">{t("shareMarket.workspace.catalog")}</h2>
            <p className="mt-0.5 text-xs text-slate-500">{t("shareMarket.catalog.subtitle")}</p>
          </div>
        )}
        <div className="flex items-center justify-end gap-2">
          <Button isIconOnly variant="ghost" aria-label={t("common.reload")} isDisabled={loading} onClick={() => void load({ scope: workspace })}>
            <RefreshCw className={loading ? "h-4 w-4 animate-spin" : "h-4 w-4"} />
          </Button>
        </div>
      </div>

      {loading && !visibleCatalog && workspace === "catalog" ? (
        <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-slate-500">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("shareMarket.loading")}
        </div>
      ) : null}
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}

      {visibleCatalog && workspace === "catalog" ? (
        <ShareMarketBuyerCatalog
          key={`catalog:${actorKey}:${initialMine ? "mine" : "all"}`}
          catalog={visibleCatalog}
          subscriptions={visibleSubscriptions}
          rentedListings={visibleRentedListings}
          authed={authed}
          focusedShareId={focusedShareId}
          initialMine={authed && initialMine}
          onChanged={() => load({ scope: "catalog", silent: true })}
          onInteractionChange={setPausePolling}
          onSwitchSelling={authed ? () => setWorkspace("selling") : undefined}
          nextCursor={visibleSubscriptionCursor}
          loadingMore={loadingMoreSubscriptions}
          onLoadMore={loadMoreSubscriptions}
        />
      ) : null}
      {authed && workspace === "selling" ? (
        <ShareMarketOwnerWorkspace
          key={`selling:${actorKey}`}
          listings={visibleOwnedListings}
          loading={loading}
          focusedShareId={focusedShareId}
          onChanged={() => load({ scope: "selling", silent: true })}
          onInteractionChange={setPausePolling}
          showHeading={false}
        />
      ) : null}
    </main>
  );
}
