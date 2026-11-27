"use client";
/*
 * The Client market page body. It reads the listings (and, for sellers, the
 * seller inventory) and composes the pieces:
 *   - MyClientRentals: the buyer's running rentals, on top;
 *   - ShopCatalog + useClientCheckout: browse and rent;
 *   - SellerWorkspace: listed, rented out and past rentals.
 * After an order the buyer stays here and the new rental card is highlighted.
 */
import * as React from "react";
import { Button, toast } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ClientTerminationHostSummary } from "@/lib/client-termination";
import {
  getOnlineListings,
  onlineRequest,
  type OnlineListing,
  type OnlineSellerItem,
} from "@/lib/online-client-market";
import { currentPageHref } from "@/lib/url-search";
import { useClientCheckout } from "./client-checkout";
import { clientMarketWorkspaceFromQuery } from "./host-utils";
import { MyClientRentals } from "./my-rentals";
import { SellerWorkspace } from "./seller-workspace";
import { ShopCatalog } from "./shop-catalog";

const LISTING_POLL_MS = 15_000;

// Keep the workspace and seller views shareable.
function updateMarketSearch(update: (params: URLSearchParams) => void) {
  window.history.replaceState(null, "", currentPageHref(window.location.href, update));
}

type MarketProps = {
  focusId: string | null;
  providerId?: string;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
  providerNotice?: React.ReactNode;
};

export function OnlineClientMarket(props: MarketProps) {
  const { session, loading } = useAuth();
  if (loading) return null;
  return <OnlineMarketView key={session?.user?.id || "guest"} authenticated={!!session?.authenticated} {...props} />;
}

function OnlineMarketView({
  authenticated,
  providerId,
  focusId,
  terminations,
  onTerminationOpen,
  providerNotice,
}: MarketProps & { authenticated: boolean }) {
  const searchParams = useSearchParams();
  const { t } = useLocaleText();
  const [list, setList] = React.useState<OnlineListing[]>([]);
  const [sellerItems, setSellerItems] = React.useState<OnlineSellerItem[]>([]);
  const [workspace, setWorkspace] = React.useState<"catalog" | "selling">(() =>
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

  React.useEffect(() => {
    const control = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const selling = authenticated && workspace === "selling";
    if (!selling) setSellerLoaded(false);
    const read = async () => {
      // Each source fails on its own: an inventory error never freezes the shop.
      const [shop, seller] = await Promise.allSettled([
        getOnlineListings(control.signal),
        selling
          ? onlineRequest<{ items: OnlineSellerItem[] }>("seller-inventory", "GET", undefined, control.signal)
          : Promise.resolve({ items: [] as OnlineSellerItem[] }),
      ]);
      if (control.signal.aborted) return;
      if (shop.status === "fulfilled") setList(shop.value.listings.filter((listing) => !providerId || listing.providerId === providerId));
      if (seller.status === "fulfilled") {
        setSellerItems(seller.value.items);
        if (selling) setSellerLoaded(true);
      }
      const failed = [shop, seller].find((result) => result.status === "rejected");
      setError(failed?.status === "rejected" ? (failed.reason instanceof Error ? failed.reason.message : String(failed.reason)) : "");
      setLoaded(true);
      timer = setTimeout(() => void read(), LISTING_POLL_MS);
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
    setRentalRevision((value) => value + 1);
    reload();
    toast.success(t("clientMarket.mine.ordered"));
  }
  const checkout = useClientCheckout({ listings: list, authenticated, onReceipt: showReceipt });

  const selectWorkspace = (next: "catalog" | "selling") => {
    setWorkspace(next);
    updateMarketSearch((params) => {
      params.delete("tab");
      if (next === "selling") params.set("view", "selling");
      else if (params.get("view") === "selling") params.delete("view");
    });
  };
  React.useEffect(() => {
    if (!authenticated && workspace === "selling") selectWorkspace("catalog");
  }, [authenticated, workspace]);

  return (
    <section className="grid min-w-0 gap-5" aria-label={t("online.supply")}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
        {authenticated ? (
          <SegmentedControl
            value={workspace}
            onChange={selectWorkspace}
            ariaLabel={t("clientMarket.shop.workspaceLabel")}
            size="sm"
            items={[
              { id: "catalog", label: t("clientMarket.shop.catalog") },
              { id: "selling", label: t("clientMarket.shop.selling") },
            ]}
          />
        ) : (
          <h2 className="text-sm font-semibold text-foreground">{t("clientMarket.shop.catalog")}</h2>
        )}
        <Button isIconOnly variant="ghost" aria-label={t("common.reload")} isDisabled={!loaded} onPress={reload}>
          {!loaded ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" /> : <RefreshCw className="size-4" />}
        </Button>
      </div>
      {checkout.pendingBanner}
      {error ? (
        <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">
          {error}
          <Button size="sm" variant="ghost" className="min-h-11" onPress={reload}>
            {t("common.reload")}
          </Button>
        </div>
      ) : null}
      {workspace === "selling" && authenticated ? (
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
        <>
          {authenticated ? (
            <MyClientRentals revision={rentalRevision} highlightId={newLease} onTerminationOpen={onTerminationOpen} />
          ) : null}
          <ShopCatalog
            listings={list}
            loaded={loaded}
            authenticated={authenticated}
            busy={checkout.busy}
            onRent={checkout.open}
            terminations={terminations}
            onTerminationOpen={onTerminationOpen}
            providerNotice={providerNotice}
          />
        </>
      )}
      {checkout.dialog}
    </section>
  );
}
