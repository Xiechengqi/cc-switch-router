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
  const [catalog, setCatalog] = React.useState<ShareMarketCatalog | null>(null);
  const [ownedListings, setOwnedListings] = React.useState<ShareMarketListing[]>([]);
  const [subscriptions, setSubscriptions] = React.useState<ShareMarketSubscription[]>([]);
  const [pausePolling, setPausePolling] = React.useState(false);
  const [workspace, setWorkspaceState] = React.useState<Workspace>(() =>
    workspaceFromQuery(searchParams.get("view") || searchParams.get("tab")),
  );
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const requestRef = React.useRef<AbortController | null>(null);
  const focusedShareId = searchParams.get("focus") || undefined;

  const load = React.useCallback(async ({
    scope = "all",
    silent = false,
    skipIfBusy = false,
  }: { scope?: LoadScope; silent?: boolean; skipIfBusy?: boolean } = {}) => {
    if (skipIfBusy && requestRef.current) return;
    requestRef.current?.abort();
    const controller = new AbortController();
    requestRef.current = controller;
    if (!silent) setLoading(true);
    if (!skipIfBusy) setError("");

    try {
      if (!authed) {
        const nextCatalog = await getShareMarketCatalog(controller.signal);
        if (controller.signal.aborted) return;
        setCatalog(nextCatalog);
        setOwnedListings([]);
        setSubscriptions([]);
        return;
      }

      if (scope === "all") {
        const [nextCatalog, nextOwned, nextSubscriptions] = await Promise.all([
          getShareMarketCatalog(controller.signal),
          getShareMarketOwnedListings(controller.signal),
          getShareMarketSubscriptions(controller.signal),
        ]);
        if (controller.signal.aborted) return;
        setCatalog(nextCatalog);
        setOwnedListings(nextOwned.listings);
        setSubscriptions(nextSubscriptions.subscriptions);
      } else if (scope === "catalog") {
        const [nextCatalog, nextSubscriptions] = await Promise.all([
          getShareMarketCatalog(controller.signal),
          getShareMarketSubscriptions(controller.signal),
        ]);
        if (controller.signal.aborted) return;
        setCatalog(nextCatalog);
        setSubscriptions(nextSubscriptions.subscriptions);
      } else if (scope === "rentals") {
        const nextSubscriptions = await getShareMarketSubscriptions(controller.signal);
        if (controller.signal.aborted) return;
        setSubscriptions(nextSubscriptions.subscriptions);
      } else {
        const nextOwned = await getShareMarketOwnedListings(controller.signal);
        if (controller.signal.aborted) return;
        setOwnedListings(nextOwned.listings);
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
    if (!authed) {
      setWorkspaceState((current) => {
        if (current !== "catalog") replaceWorkspaceQuery("catalog");
        return "catalog";
      });
    }
    void load({ scope: "all" });
    return () => requestRef.current?.abort();
  }, [authed, authLoading, load]);

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

      {loading && !catalog && workspace === "catalog" ? (
        <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-slate-500">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("shareMarket.loading")}
        </div>
      ) : null}
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}

      {catalog && workspace === "catalog" ? (
        <ShareMarketBuyerCatalog
          catalog={catalog}
          subscriptions={subscriptions}
          authed={authed}
          focusedShareId={focusedShareId}
          onChanged={() => load({ scope: "catalog", silent: true })}
          onInteractionChange={setPausePolling}
          onSwitchSelling={authed ? () => setWorkspace("selling") : undefined}
        />
      ) : null}
      {authed && workspace === "rentals" ? (
        <ShareMarketBuyerRentals
          subscriptions={subscriptions}
          loading={loading}
          onChanged={() => load({ scope: "rentals", silent: true })}
          onInteractionChange={setPausePolling}
        />
      ) : null}
      {authed && workspace === "selling" ? (
        <ShareMarketOwnerWorkspace
          listings={ownedListings}
          loading={loading}
          focusedShareId={focusedShareId}
          onChanged={() => load({ scope: "selling", silent: true })}
          onInteractionChange={setPausePolling}
        />
      ) : null}
    </main>
  );
}
