"use client";
/*
 * The Client market page body. It reads the listings (and, for sellers, the
 * seller inventory) and composes the pieces:
 *   - ShopCatalog + useClientCheckout: browse and rent. Its "Mine" tab swaps
 *     the shop for the buyer's rentals (MyClientRentals) and their history;
 *   - SellerWorkspace: listed, rented out and past rentals;
 *   - ClientRentalAuditPanel: the Router owner's audit of every rental.
 * After an order the buyer lands on "Mine", with the new rental highlighted.
 *
 * Tabs follow the Share market: "My listings" appears only for users who sell
 * (the server's seller summary says so), "Rental audit" only for the Router
 * owner. A link to a tab the viewer may not see falls back to the catalog.
 */
import * as React from "react";
import { Button, toast } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientRentals } from "@/lib/client-rentals";
import { getClientSellerSummary, type ClientSellerSummary } from "@/lib/client-seller-summary";
import type { ClientTerminationHostSummary } from "@/lib/client-termination";
import {
  getOnlineListings,
  onlineRequest,
  type OnlineListing,
  type OnlineSellerItem,
} from "@/lib/online-client-market";
import { currentPageHref } from "@/lib/url-search";
import { usePersistentState } from "@/lib/use-persistent-state";
import { useClientCheckout } from "./client-checkout";
import { clientMarketWorkspaceFromQuery, SHOP_MINE_KEY, type ClientMarketWorkspace } from "./host-utils";
import { MyClientRentals } from "./my-rentals";
import { ClientRentalRecords } from "./rental-records";
import { ClientRentalAuditPanel } from "./rental-audit-panel";
import { SellerWorkspace } from "./seller-workspace";
import { ShopCatalog } from "./shop-catalog";

const LISTING_POLL_MS = 15_000;

/**
 * How many Clients the buyer rents now and has rented before. It decides
 * whether "Mine" can be picked; null for guests and until the first answer.
 */
function useClientRentalCounts(authenticated: boolean, revision: number) {
  const [counts, setCounts] = React.useState<{ current: number; history: number } | null>(null);
  React.useEffect(() => {
    if (!authenticated) {
      setCounts(null);
      return;
    }
    const control = new AbortController();
    void Promise.all([
      getClientRentals("current", 1, null, control.signal),
      getClientRentals("history", 1, null, control.signal),
    ])
      .then(([current, history]) => {
        if (!control.signal.aborted) setCounts({ current: current.total, history: history.total });
      })
      // A failed count keeps the last answer; it only gates the tab.
      .catch(() => undefined);
    return () => control.abort();
  }, [authenticated, revision]);
  return counts;
}

// Keep the workspace and seller views shareable.
function updateMarketSearch(update: (params: URLSearchParams) => void) {
  window.history.replaceState(null, "", currentPageHref(window.location.href, update));
}

type MarketProps = {
  focusId: string | null;
  providerId?: string;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
  /** Bumps when a termination changes, so an open audit detail rereads. */
  auditRevision?: number;
  providerNotice?: React.ReactNode;
};

export function OnlineClientMarket(props: MarketProps) {
  const { session, loading } = useAuth();
  if (loading) return null;
  return (
    <OnlineMarketView
      key={session?.user?.id || "guest"}
      authenticated={!!session?.authenticated}
      canAudit={!!session?.authenticated && session.canAuditClientMarket === true}
      {...props}
    />
  );
}

function OnlineMarketView({
  authenticated,
  canAudit,
  providerId,
  focusId,
  terminations,
  onTerminationOpen,
  auditRevision = 0,
  providerNotice,
}: MarketProps & { authenticated: boolean; canAudit: boolean }) {
  const searchParams = useSearchParams();
  const { t } = useLocaleText();
  const [list, setList] = React.useState<OnlineListing[]>([]);
  const [sellerItems, setSellerItems] = React.useState<OnlineSellerItem[]>([]);
  const [sellerSummary, setSellerSummary] = React.useState<ClientSellerSummary | null>(null);
  const [workspace, setWorkspace] = React.useState<ClientMarketWorkspace>(() =>
    clientMarketWorkspaceFromQuery(searchParams.get("view"), searchParams.get("tab")),
  );
  const [addListingOpen, setAddListingOpen] = React.useState(searchParams.get("add") === "1");
  const [version, setVersion] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  // The seller inventory has its own first load, so Selling never flashes empty.
  const [sellerLoaded, setSellerLoaded] = React.useState(false);
  const [error, setError] = React.useState("");
  const [rentalRevision, setRentalRevision] = React.useState(0);
  const [newLease, setNewLease] = React.useState<string | null>(null);
  const reload = React.useCallback(() => setVersion((value) => value + 1), []);
  // Remembered across reloads, like the shop's region and "rentable only".
  const [mineSelected, setMineSelected] = usePersistentState(SHOP_MINE_KEY, false);
  const rentalCounts = useClientRentalCounts(authenticated, rentalRevision + version);
  const mineEnabled = authenticated && !!rentalCounts && rentalCounts.current + rentalCounts.history > 0;
  // Until the counts arrive, a chosen "Mine" stays; a known-empty one falls back.
  const mine = mineSelected && authenticated && (rentalCounts === null || mineEnabled);

  // Until the summary arrives, a seller's link to "My listings" keeps the tab,
  // so the page neither flickers nor drops the seller back to the catalog.
  const sellingKnown = !authenticated || sellerSummary !== null;
  const canSell = authenticated && sellerSummary?.canSell === true;
  const showSelling = canSell || (!sellingKnown && workspace === "selling");
  const view: ClientMarketWorkspace =
    (workspace === "selling" && !showSelling) || (workspace === "audit" && !canAudit) ? "catalog" : workspace;

  React.useEffect(() => {
    const control = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const selling = authenticated && workspace === "selling";
    // The audit tab has its own reads; the shop and inventory pause meanwhile.
    const shopping = workspace !== "audit";
    if (!selling) setSellerLoaded(false);
    const read = async () => {
      // Each source fails on its own: an inventory error never freezes the shop.
      const [shop, seller, summary] = await Promise.allSettled([
        shopping ? getOnlineListings(control.signal) : Promise.resolve(null),
        selling
          ? onlineRequest<{ items: OnlineSellerItem[] }>("seller-inventory", "GET", undefined, control.signal)
          : Promise.resolve({ items: [] as OnlineSellerItem[] }),
        authenticated ? getClientSellerSummary(control.signal) : Promise.resolve(null),
      ]);
      if (control.signal.aborted) return;
      if (shop.status === "fulfilled" && shop.value) {
        setList(shop.value.listings.filter((listing) => !providerId || listing.providerId === providerId));
      }
      if (seller.status === "fulfilled") {
        setSellerItems(seller.value.items);
        if (selling) setSellerLoaded(true);
      }
      // A failed summary keeps the last answer; it only decides tab visibility.
      if (summary.status === "fulfilled" && summary.value) setSellerSummary(summary.value);
      const failed = [shop, seller].find((result) => result.status === "rejected");
      setError(failed?.status === "rejected" ? (failed.reason instanceof Error ? failed.reason.message : String(failed.reason)) : "");
      if (shopping) setLoaded(true);
      if (shopping) timer = setTimeout(() => void read(), LISTING_POLL_MS);
    };
    void read();
    return () => {
      control.abort();
      clearTimeout(timer);
    };
  }, [authenticated, version, providerId, workspace]);

  const focused = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!focusId) {
      focused.current = null;
      return;
    }
    if (focusId === focused.current) return;
    const card = document.getElementById(`market-client-${focusId}`);
    if (card) {
      focused.current = focusId;
      card.focus();
    }
  }, [focusId, list, sellerItems, workspace]);

  function showReceipt(id: string) {
    setNewLease(id);
    // Rentals live only under "Mine": show the new one there.
    setMineSelected(true);
    setRentalRevision((value) => value + 1);
    reload();
    toast.success(t("clientMarket.mine.ordered"));
  }
  const checkout = useClientCheckout({ listings: list, authenticated, onReceipt: showReceipt });

  const selectWorkspace = React.useCallback((next: ClientMarketWorkspace) => {
    setWorkspace(next);
    updateMarketSearch((params) => {
      params.delete("tab");
      if (next === "catalog") params.delete("view");
      else params.set("view", next);
    });
  }, []);
  // A tab the viewer may not see falls back to the catalog, in state and URL.
  // A deep link to add a listing is dropped with it.
  React.useEffect(() => {
    if (view === workspace) return;
    selectWorkspace("catalog");
    if (workspace === "selling") {
      setAddListingOpen(false);
      updateMarketSearch((params) => {
        params.delete("add");
        params.delete("installation");
      });
    }
  }, [view, workspace, selectWorkspace]);

  const workspaceItems: Array<{ id: ClientMarketWorkspace; label: string }> = [
    { id: "catalog", label: t("market.tab.catalog") },
    ...(showSelling ? [{ id: "selling" as const, label: t("market.tab.selling") }] : []),
    ...(canAudit ? [{ id: "audit" as const, label: t("market.tab.audit") }] : []),
  ];

  return (
    <section className="grid min-w-0 gap-5" aria-label={t("online.supply")}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
        {authenticated ? (
          <SegmentedControl
            value={view}
            onChange={selectWorkspace}
            ariaLabel={t("clientMarket.shop.workspaceLabel")}
            size="sm"
            items={workspaceItems}
          />
        ) : (
          <h2 className="text-sm font-semibold text-foreground">{t("clientMarket.shop.catalog")}</h2>
        )}
        {view !== "audit" ? (
          <Button isIconOnly variant="ghost" aria-label={t("common.reload")} isDisabled={!loaded} onPress={reload}>
            {!loaded ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" /> : <RefreshCw className="size-4" />}
          </Button>
        ) : null}
      </div>
      {view !== "audit" ? checkout.pendingBanner : null}
      {error && view !== "audit" ? (
        <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">
          {error}
          <Button size="sm" variant="ghost" className="h-9" onPress={reload}>
            {t("common.reload")}
          </Button>
        </div>
      ) : null}
      {view === "audit" ? (
        <ClientRentalAuditPanel onTerminationOpen={onTerminationOpen} revision={auditRevision} />
      ) : view === "selling" ? (
        <SellerWorkspace
          listings={list}
          items={sellerItems}
          loaded={loaded && sellerLoaded}
          addOpen={addListingOpen}
          initialInstallation={searchParams.get("installation")}
          onAddOpen={() => setAddListingOpen(true)}
          onAddClose={() => {
            setAddListingOpen(false);
            updateMarketSearch((params) => {
              params.delete("add");
              params.delete("installation");
            });
          }}
          onChanged={reload}
          terminations={terminations}
          onTerminationOpen={onTerminationOpen}
        />
      ) : (
        <ShopCatalog
          listings={list}
          loaded={loaded}
          authenticated={authenticated}
          busy={checkout.busy}
          onRent={checkout.open}
          terminations={terminations}
          onTerminationOpen={onTerminationOpen}
          providerNotice={providerNotice}
          mine={{
            selected: mine,
            enabled: mineEnabled,
            count: rentalCounts?.current ?? 0,
            onChange: setMineSelected,
            view: (
              <div className="grid min-w-0 gap-5">
                {/* Counts trail a fresh order by one read; the new card wins. */}
                {rentalCounts?.current === 0 && !newLease ? (
                  <p className="rounded-md border border-dashed border-slate-200 px-4 py-10 text-center text-sm text-slate-500">
                    {t("clientMarket.mine.none")}
                  </p>
                ) : (
                  <MyClientRentals revision={rentalRevision} highlightId={newLease} onTerminationOpen={onTerminationOpen} />
                )}
                {/* Rental history shows only under "Mine", like the Share catalog. */}
                <ClientRentalRecords historyOnly revision={rentalRevision} onTerminationOpen={onTerminationOpen} />
              </div>
            ),
          }}
        />
      )}
      {checkout.dialog}
    </section>
  );
}
