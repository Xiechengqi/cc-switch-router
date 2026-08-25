"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { CircleDollarSign, Loader2, RefreshCw } from "lucide-react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareMarketBuyerCatalog } from "@/components/dashboard/share-market/buyer-catalog";
import { ShareMarketBuyerRentals } from "@/components/dashboard/share-market/buyer-rentals";
import { shareMarketMutationError } from "@/components/dashboard/share-market/market-utils";
import { mergeShareMarketSubscriptionPage } from "@/components/dashboard/share-market/subscription-utils";
import { ShareMarketOwnerWorkspace } from "@/components/dashboard/share-market/owner-workspace";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getShareMarketCatalog,
  getShareMarketOwnedListings,
  getShareMarketSubscriptions,
} from "@/lib/api";
import { DASHBOARD_ACCOUNT_BILLING_PATH } from "@/lib/dashboard-nav";
import type {
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketSubscription,
} from "@/lib/types";

type Workspace = "catalog" | "rentals" | "selling";
type LoadScope = Workspace | "all";
const SHARE_MARKET_POLL_MS = 15_000;

function workspaceFromQuery(value: string | null): Workspace {
  if (value === "selling" || value === "mine") return "selling";
  if (value === "rentals" || value === "rented") return "rentals";
  return "catalog";
}

function replaceWorkspaceQuery(workspace: Workspace) {
  const url = new URL(window.location.href);
  if (workspace === "catalog") url.searchParams.delete("view");
  else url.searchParams.set("view", workspace);
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

  const load = React.useCallback(async ({
    scope = "all",
    silent = false,
    skipIfBusy = false,
  }: { scope?: LoadScope; silent?: boolean; skipIfBusy?: boolean } = {}) => {
    const requestedActorKey = actorKey;
    if (actorKeyRef.current !== requestedActorKey) return;
    if (skipIfBusy && requestRef.current) return;
    const resetSubscriptionHistory = !silent
      && (scope === "all" || scope === "catalog" || scope === "rentals");
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
        setSubscriptions([]);
        setSubscriptionCursor(null);
        setLoadedActorKey(actorKey);
        return;
      }

      if (scope === "all") {
        const [nextCatalog, nextOwned, nextSubscriptions] = await Promise.all([
          getShareMarketCatalog(controller.signal),
          getShareMarketOwnedListings(controller.signal),
          getShareMarketSubscriptions(controller.signal),
        ]);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        setCatalog(nextCatalog);
        setOwnedListings(nextOwned.listings);
        applySubscriptionPage(nextSubscriptions);
        setLoadedActorKey(actorKey);
      } else if (scope === "catalog") {
        const [nextCatalog, nextSubscriptions] = await Promise.all([
          getShareMarketCatalog(controller.signal),
          getShareMarketSubscriptions(controller.signal),
        ]);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        setCatalog(nextCatalog);
        applySubscriptionPage(nextSubscriptions);
        setLoadedActorKey(actorKey);
      } else if (scope === "rentals") {
        const nextSubscriptions = await getShareMarketSubscriptions(controller.signal);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        applySubscriptionPage(nextSubscriptions);
        setLoadedActorKey(actorKey);
      } else {
        const nextOwned = await getShareMarketOwnedListings(controller.signal);
        if (controller.signal.aborted || actorKeyRef.current !== requestedActorKey) return;
        setOwnedListings(nextOwned.listings);
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

  React.useEffect(() => {
    if (authLoading) return;
    const tick = () => {
      if (document.visibilityState !== "visible" || pausePolling) return;
      void load({ scope: workspace, silent: true, skipIfBusy: true });
    };
    const timer = window.setInterval(tick, SHARE_MARKET_POLL_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") tick();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [authLoading, load, pausePolling, workspace]);

  const setWorkspace = (next: Workspace) => {
    if (!authed && next !== "catalog") return;
    setWorkspaceState(next);
    replaceWorkspaceQuery(next);
    void load({ scope: next });
  };

  const actorDataCurrent = loadedActorKey === actorKey;
  const visibleCatalog = actorDataCurrent ? catalog : null;
  const visibleOwnedListings = actorDataCurrent ? ownedListings : [];
  const visibleSubscriptions = actorDataCurrent ? subscriptions : [];
  const visibleSubscriptionCursor = actorDataCurrent ? subscriptionCursor : null;

  return (
    <main className="mx-auto grid w-full max-w-7xl gap-5 px-1 pb-10">
      <h1 className="sr-only">{t("shareMarket.title")}</h1>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-slate-200 pb-3">
        {authed ? (
          <SegmentedControl
            value={workspace}
            onChange={setWorkspace}
            ariaLabel={t("shareMarket.workspace.label")}
            size="sm"
            items={[
              { id: "catalog", label: t("shareMarket.workspace.catalog") },
              { id: "rentals", label: t("shareMarket.workspace.rentals") },
              { id: "selling", label: t("shareMarket.workspace.selling") },
            ]}
          />
        ) : (
          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-slate-900">{t("shareMarket.workspace.catalog")}</h2>
            <p className="mt-0.5 text-xs text-slate-500">{t("shareMarket.catalog.subtitle")}</p>
          </div>
        )}
        <div className="flex items-center justify-end gap-2">
          {authed ? (
            <Link
              href={DASHBOARD_ACCOUNT_BILLING_PATH}
              className="inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-md border border-slate-200 bg-white px-3 text-sm font-medium text-slate-700 hover:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
            >
              <CircleDollarSign className="h-4 w-4" />
              {t("marketBilling.open")}
            </Link>
          ) : null}
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
          key={`catalog:${actorKey}`}
          catalog={visibleCatalog}
          subscriptions={visibleSubscriptions}
          authed={authed}
          focusedShareId={focusedShareId}
          onChanged={() => load({ scope: "catalog", silent: true })}
          onInteractionChange={setPausePolling}
          onSwitchSelling={authed ? () => setWorkspace("selling") : undefined}
        />
      ) : null}
      {authed && workspace === "rentals" ? (
        <ShareMarketBuyerRentals
          key={`rentals:${actorKey}`}
          subscriptions={visibleSubscriptions}
          loading={loading}
          onChanged={() => load({ scope: "rentals", silent: true })}
          onInteractionChange={setPausePolling}
          nextCursor={visibleSubscriptionCursor}
          loadingMore={loadingMoreSubscriptions}
          onLoadMore={loadMoreSubscriptions}
          showHeading={false}
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
        />
      ) : null}
    </main>
  );
}
