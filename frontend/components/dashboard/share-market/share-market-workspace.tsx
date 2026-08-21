"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { CircleDollarSign, Loader2, Plus, RefreshCw } from "lucide-react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareMarketBuyerCatalog } from "@/components/dashboard/share-market/buyer-catalog";
import {
  ShareMarketAddListingDialog,
  ShareMarketOwnerWorkspace,
} from "@/components/dashboard/share-market/owner-workspace";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getShareMarketCatalog,
  getShareMarketOwnedListings,
  getShareMarketOwnedShares,
  getShareMarketSubscriptions,
} from "@/lib/api";
import { DASHBOARD_ACCOUNT_BILLING_PATH } from "@/lib/dashboard-nav";
import type {
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketSubscription,
} from "@/lib/types";

type Workspace = "catalog" | "selling";
const SHARE_MARKET_POLL_MS = 5_000;

function workspaceFromQuery(value: string | null): Workspace {
  return value === "selling" || value === "mine" ? "selling" : "catalog";
}

function replaceWorkspaceQuery(workspace: Workspace) {
  const url = new URL(window.location.href);
  if (workspace === "selling") url.searchParams.set("view", "selling");
  else url.searchParams.delete("view");
  url.searchParams.delete("tab");
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

export function ShareMarketWorkspace() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const [catalog, setCatalog] = React.useState<ShareMarketCatalog | null>(null);
  const [ownedListings, setOwnedListings] = React.useState<ShareMarketListing[]>([]);
  const [subscriptions, setSubscriptions] = React.useState<ShareMarketSubscription[]>([]);
  const [canCreateListing, setCanCreateListing] = React.useState(false);
  const [addListingOpen, setAddListingOpen] = React.useState(false);
  const [pausePolling, setPausePolling] = React.useState(false);
  const [workspace, setWorkspaceState] = React.useState<Workspace>(() =>
    workspaceFromQuery(searchParams.get("view") || searchParams.get("tab")),
  );
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const requestRef = React.useRef<AbortController | null>(null);
  const focusedShareId = searchParams.get("focus") || undefined;

  const load = React.useCallback(async ({
    silent = false,
    skipIfBusy = false,
  }: { silent?: boolean; skipIfBusy?: boolean } = {}) => {
    if (skipIfBusy && requestRef.current) return;
    requestRef.current?.abort();
    const controller = new AbortController();
    requestRef.current = controller;
    if (!silent) {
      setLoading(true);
    }
    if (!skipIfBusy) setError("");
    try {
      const publicRequest = getShareMarketCatalog(controller.signal);
      if (authed) {
        const [nextCatalog, nextOwned, nextSubscriptions, nextOwnedShares] = await Promise.all([
          publicRequest,
          getShareMarketOwnedListings(controller.signal),
          getShareMarketSubscriptions(controller.signal),
          getShareMarketOwnedShares(controller.signal),
        ]);
        if (controller.signal.aborted) return;
        setCatalog(nextCatalog);
        setOwnedListings(nextOwned.listings);
        setSubscriptions(nextSubscriptions.subscriptions);
        setCanCreateListing(nextOwnedShares.some(
          (share) => !share.alreadyListed && !share.freeAccess && share.shareStatus === "active",
        ));
      } else {
        const nextCatalog = await publicRequest;
        if (controller.signal.aborted) return;
        setCatalog(nextCatalog);
        setOwnedListings([]);
        setSubscriptions([]);
        setCanCreateListing(false);
      }
    } catch (reason) {
      if (controller.signal.aborted) return;
      if (!skipIfBusy) setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (requestRef.current === controller) {
        requestRef.current = null;
        setLoading(false);
      }
    }
  }, [authed]);

  React.useEffect(() => {
    if (authLoading) return;
    void load();
    return () => requestRef.current?.abort();
  }, [authLoading, load]);

  React.useEffect(() => {
    if (authLoading) return;
    const tick = () => {
      if (document.visibilityState !== "visible" || addListingOpen || pausePolling) return;
      void load({ silent: true, skipIfBusy: true });
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
  }, [addListingOpen, authLoading, load, pausePolling]);

  const hasSellingWorkspace = authed && ownedListings.some((listing) => listing.status === "active");
  const canOpenRequestedListing = authed
    && workspace === "selling"
    && !!focusedShareId
    && ownedListings.some((listing) => listing.shareId === focusedShareId);
  const effectiveWorkspace = hasSellingWorkspace || canOpenRequestedListing ? workspace : "catalog";
  React.useEffect(() => {
    if (!loading && workspace === "selling" && !hasSellingWorkspace && !canOpenRequestedListing) {
      setWorkspaceState("catalog");
      replaceWorkspaceQuery("catalog");
    }
  }, [canOpenRequestedListing, hasSellingWorkspace, loading, workspace]);

  const setWorkspace = (next: Workspace) => {
    setWorkspaceState(next);
    replaceWorkspaceQuery(next);
  };

  return (
    <main className="mx-auto grid w-full max-w-7xl gap-5 px-1 pb-10">
      <h1 className="sr-only">{t("shareMarket.title")}</h1>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-slate-200 pb-3">
        {hasSellingWorkspace ? (
          <SegmentedControl
            value={effectiveWorkspace}
            onChange={setWorkspace}
            ariaLabel={t("shareMarket.workspace.label")}
            size="sm"
            items={[
              { id: "catalog", label: t("shareMarket.workspace.catalog") },
              { id: "selling", label: t("shareMarket.workspace.selling") },
            ]}
          />
        ) : effectiveWorkspace === "selling" ? (
          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-slate-900">{t("shareMarket.workspace.selling")}</h2>
            <p className="mt-0.5 text-xs text-slate-500">{t("shareMarket.workspace.sellingHint")}</p>
          </div>
        ) : (
          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-slate-900">{t("shareMarket.workspace.catalog")}</h2>
            <p className="mt-0.5 text-xs text-slate-500">{t("shareMarket.catalog.subtitle")}</p>
          </div>
        )}
        <div className="flex flex-wrap items-center justify-end gap-2">
          {authed && canCreateListing && !hasSellingWorkspace ? (
            <Button size="sm" variant="primary" onClick={() => setAddListingOpen(true)}>
              <Plus className="h-4 w-4" />
              {t("shareMarket.addShare")}
            </Button>
          ) : null}
          {authed ? (
            <Link
              href={DASHBOARD_ACCOUNT_BILLING_PATH}
              className="inline-flex h-9 items-center gap-1.5 rounded-md border border-slate-200 bg-white px-3 text-sm font-medium text-slate-700 hover:bg-slate-50"
            >
              <CircleDollarSign className="h-4 w-4" />
              {t("marketBilling.open")}
            </Link>
          ) : null}
          <Button isIconOnly variant="ghost" aria-label={t("common.reload")} isDisabled={loading} onClick={() => void load()}>
            <RefreshCw className={loading ? "h-4 w-4 animate-spin" : "h-4 w-4"} />
          </Button>
        </div>
      </div>

      {loading && !catalog ? (
        <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-slate-500">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("shareMarket.loading")}
        </div>
      ) : null}
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}

      {catalog && effectiveWorkspace === "catalog" ? (
        <ShareMarketBuyerCatalog
          catalog={catalog}
          subscriptions={subscriptions}
          authed={authed}
          focusedShareId={workspace === "catalog" ? focusedShareId : undefined}
          onChanged={() => load({ silent: true })}
          onInteractionChange={setPausePolling}
          onSwitchSelling={hasSellingWorkspace ? () => setWorkspace("selling") : undefined}
        />
      ) : null}
      {effectiveWorkspace === "selling" ? (
        <ShareMarketOwnerWorkspace
          listings={ownedListings}
          loading={loading}
          focusedShareId={focusedShareId}
          onChanged={() => load({ silent: true })}
          onInteractionChange={setPausePolling}
        />
      ) : null}
      <ShareMarketAddListingDialog
        open={addListingOpen}
        onOpenChange={setAddListingOpen}
        onSaved={() => void load()}
      />
    </main>
  );
}
