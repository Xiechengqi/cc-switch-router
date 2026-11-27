"use client";

import * as React from "react";
import Link from "next/link";
import { Drawer } from "@heroui/react";
import { ShieldAlert, UserRound } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientOnlineHeatmap } from "@/components/dashboard/client-online-heatmap";
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { MarketPagination } from "@/components/dashboard/share-market/market-pagination";
import { ClientTerminationHostAction } from "@/components/market/client-termination-host-action";
import type { ClientTerminationHostSummary } from "@/lib/client-termination";
import { providerRankingHref } from "@/lib/dashboard-nav";
import { clientDisplayName, type OnlineListing } from "@/lib/online-client-market";
import { currentPageHref } from "@/lib/url-search";
import { usePersistentState } from "@/lib/use-persistent-state";
import { cn } from "@/lib/utils";
import {
  aggregateOnlineShopCountries,
  matchesShopQuery,
  onlineListingAvailability,
  onlineListingRentable,
  onlineStatusKey,
  SHOP_BROWSE_KEY,
  SHOP_CONTINENTS,
  SHOP_PAGE_SIZE,
  SHOP_UNKNOWN_COUNTRY,
  shopContinentForCountry,
  shopCountryKey,
  type ShopContinent,
  sortShopCountries,
  sortShopListings,
} from "./host-utils";
import {
  ClientEgressLine,
  ClientHardware,
  ClientOfferCard,
  ClientReliability,
  ClientShopFilters,
  ONLINE_CLIENT_CARD_GRID_CLASS,
  useListingPrice,
} from "./online-shop";

const RISK_NOTICE_KEY = "cc_switch_router_client_market_risk_notice_v1";

function updateMarketSearch(update: (params: URLSearchParams) => void) {
  window.history.replaceState(null, "", currentPageHref(window.location.href, update));
}

/** One-time, dismissible reminder that the seller's machine runs the Client. */
function RiskNotice() {
  const { t } = useLocaleText();
  const [dismissed, setDismissed, hydrated] = usePersistentState<boolean>(RISK_NOTICE_KEY, false);
  if (!hydrated || dismissed) return null;
  return (
    <div role="note" className="flex min-w-0 items-start gap-3 rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-950">
      <ShieldAlert className="mt-0.5 size-4 shrink-0 text-amber-600" aria-hidden />
      <p className="min-w-0 flex-1 leading-6">{t("clientMarket.risk.notice")}</p>
      <button
        type="button"
        className="h-9 shrink-0 rounded-lg px-2 text-xs font-medium text-amber-900 hover:bg-amber-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
        onClick={() => setDismissed(true)}
      >
        {t("clientMarket.risk.dismiss")}
      </button>
    </div>
  );
}

export function ShopCatalog({
  listings,
  loaded,
  authenticated,
  busy,
  onRent,
  terminations,
  onTerminationOpen,
  providerNotice,
}: {
  listings: OnlineListing[];
  loaded: boolean;
  authenticated: boolean;
  busy: boolean;
  onRent: (listing: OnlineListing) => void;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
  providerNotice?: React.ReactNode;
}) {
  const { locale, t } = useLocaleText();
  const names = React.useMemo(() => new Intl.DisplayNames([locale], { type: "region" }), [locale]);
  const countryName = React.useCallback(
    (code: string) => (/^[A-Z]{2}$/.test(code) ? names.of(code) || code : t("clientMarket.shop.unknownRegion")),
    [names, t],
  );
  const [browse, setBrowse] = usePersistentState<{ country: string; continent?: string; rentableOnly: boolean }>(SHOP_BROWSE_KEY, {
    country: "",
    continent: "all",
    rentableOnly: true,
  });
  // A shared link's country wins over the remembered one.
  const [linkCountry, setLinkCountry] = React.useState<string | null>(() => {
    const code = (typeof window === "undefined" ? "" : new URLSearchParams(window.location.search).get("country") || "")
      .trim()
      .toUpperCase();
    return /^[A-Z]{2}$/.test(code) ? code : null;
  });
  const remembered = /^[A-Z]{2}$/.test(browse.country) || browse.country === SHOP_UNKNOWN_COUNTRY ? browse.country : "";
  const country = linkCountry ?? remembered;
  // A chosen country always implies its continent, so the two never disagree.
  const continentOf = (code: string) => shopContinentForCountry(code === SHOP_UNKNOWN_COUNTRY ? "" : code);
  const rememberedContinent = (SHOP_CONTINENTS as readonly string[]).includes(browse.continent || "")
    ? (browse.continent as ShopContinent)
    : "all";
  const continent: ShopContinent = country ? continentOf(country) : rememberedContinent;
  const [query, setQuery] = React.useState(() =>
    typeof window === "undefined" ? "" : new URLSearchParams(window.location.search).get("q") || "",
  );
  const [page, setPage] = React.useState(1);
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const rentableOnly = browse.rentableOnly !== false;
  const listed = listings.filter((item) => item.state === "listed");
  const pool = listed.filter((item) => !rentableOnly || onlineListingRentable(item));
  const cards = sortShopCountries(aggregateOnlineShopCountries(pool), "idle", (code) =>
    /^[A-Z]{2}$/.test(code) ? countryName(code) : "￿",
  );
  const visible = sortShopListings(
    pool.filter(
      (item) =>
        (continent === "all" || continentOf(shopCountryKey(item.country)) === continent) &&
        (!country || shopCountryKey(item.country) === country) &&
        matchesShopQuery(item, query, countryName),
    ),
  );
  const pages = Math.max(1, Math.ceil(visible.length / SHOP_PAGE_SIZE));
  const current = Math.min(page, pages);
  const shown = visible.slice((current - 1) * SHOP_PAGE_SIZE, current * SHOP_PAGE_SIZE);
  const selected = selectedId ? listings.find((item) => item.installationId === selectedId) ?? null : null;
  React.useEffect(() => setPage(1), [continent, country, query, rentableOnly]);

  const selectCountry = (code: string) => {
    const next = code === SHOP_UNKNOWN_COUNTRY || /^[A-Z]{2}$/.test(code) ? code : "";
    setLinkCountry(null);
    setBrowse((value) => ({ ...value, country: next, continent: next ? continentOf(next) : value.continent }));
    updateMarketSearch((params) => {
      if (/^[A-Z]{2}$/.test(next)) params.set("country", next);
      else params.delete("country");
    });
  };
  const selectContinent = (next: ShopContinent) => {
    setLinkCountry(null);
    setBrowse((value) => ({ ...value, country: "", continent: next }));
    updateMarketSearch((params) => params.delete("country"));
  };
  const changeQuery = (next: string) => {
    setQuery(next);
    updateMarketSearch((params) => {
      if (next.trim()) params.set("q", next);
      else params.delete("q");
    });
  };
  if (!loaded) return <p role="status" className="py-10 text-center text-sm text-muted-foreground">{t("common.loading")}</p>;
  return (
    <div className="grid min-w-0 gap-4">
      <RiskNotice />
      {providerNotice}
      <ClientShopFilters
        cards={cards}
        continent={continent}
        onContinent={selectContinent}
        country={country}
        onCountry={selectCountry}
        query={query}
        onQuery={changeQuery}
        rentable={{
          pressed: rentableOnly,
          label: t("clientMarket.shop.onlyRentable"),
          onClick: () => setBrowse((value) => ({ ...value, rentableOnly: !rentableOnly })),
        }}
      />
      {shown.length === 0 ? (
        <div className="grid justify-items-center gap-2 rounded-xl border border-dashed border-border px-4 py-12 text-center text-sm text-muted-foreground">
          <p>
            {t(
              query.trim() || country || continent !== "all"
                ? "clientMarket.shop.noHosts"
                : rentableOnly && listed.length
                  ? "clientMarket.shop.rentableEmpty"
                  : "clientMarket.shop.noRegions",
            )}
          </p>
          {rentableOnly && listed.length > pool.length ? (
            <button
              type="button"
              className="h-9 rounded-lg px-2 text-sm font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
              onClick={() => setBrowse((value) => ({ ...value, rentableOnly: false }))}
            >
              {t("clientMarket.shop.showAll")}
            </button>
          ) : null}
        </div>
      ) : (
        <section
          className="grid gap-3"
          aria-label={country ? t("clientMarket.shop.hostListLabel", { name: countryName(country) }) : t("clientMarket.shop.cardGrid")}
        >
          <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">
            {t("clientMarket.shop.resultCount", { count: visible.length })}
          </p>
          <div className={ONLINE_CLIENT_CARD_GRID_CLASS}>
            {shown.map((listing) => (
              <ClientOfferCard
                key={listing.installationId}
                listing={listing}
                authenticated={authenticated}
                busy={busy}
                onRent={() => onRent(listing)}
                onOpen={() => setSelectedId(listing.installationId)}
                extra={<ClientTerminationHostAction summary={terminations.get(listing.installationId)} onOpen={onTerminationOpen} />}
              />
            ))}
          </div>
          <MarketPagination page={current} pageCount={pages} onPageChange={setPage} />
        </section>
      )}
      <ClientDetailDrawer
        listing={selected}
        authenticated={authenticated}
        busy={busy}
        onClose={() => setSelectedId(null)}
        onRent={(listing) => {
          setSelectedId(null);
          onRent(listing);
        }}
      />
    </div>
  );
}

function ClientDetailDrawer({
  listing,
  authenticated,
  busy,
  onClose,
  onRent,
}: {
  listing: OnlineListing | null;
  authenticated: boolean;
  busy: boolean;
  onClose: () => void;
  onRent: (listing: OnlineListing) => void;
}) {
  const { t } = useLocaleText();
  const priceOf = useListingPrice();
  const terms = listing ? priceOf(listing) : null;
  const availability = listing ? onlineListingAvailability(listing) : "unavailable";
  const section = "grid gap-2";
  // Same as the Share seat drawer headings (buyer-catalog.tsx).
  const label = "text-xs font-semibold uppercase text-slate-500";
  return (
    <Drawer.Backdrop isOpen={!!listing} onOpenChange={(open) => !open && onClose()}>
      <Drawer.Content placement="right">
        <Drawer.Dialog className={drawerDialogClassName}>
          <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" />
          <Drawer.Header>
            <div className="min-w-0 pr-10">
              <Drawer.Heading className="truncate text-base">{listing ? clientDisplayName(listing) : ""}</Drawer.Heading>
            </div>
          </Drawer.Header>
          <Drawer.Body className="min-h-0 overflow-y-auto">
            {listing ? (
              <div className="grid gap-6">
                <section className={section}>
                  <h3 className={label}>{t("clientMarket.detail.egress")}</h3>
                  <ClientEgressLine listing={listing} emphasize />
                  <p className="text-xs leading-5 text-muted-foreground">{t("clientMarket.detail.egressHint")}</p>
                </section>
                <section className={section}>
                  <h3 className={label}>{t("clientMarket.detail.reliability")}</h3>
                  <ClientReliability listing={listing} inline />
                  <ClientOnlineHeatmap installationId={listing.installationId} />
                </section>
                <section className={section}>
                  <h3 className={label}>{t("clientMarket.shop.summarySpecs")}</h3>
                  <ClientHardware listing={listing} />
                </section>
                {listing.sellerEmail ? (
                  <section className={section}>
                    <h3 className={label}>{t("clientMarket.detail.seller")}</h3>
                    <p className="flex min-w-0 items-center gap-2 text-sm">
                      <UserRound className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                      <span className="min-w-0 break-all font-medium">{listing.sellerEmail}</span>
                    </p>
                    {listing.providerId ? (
                      <Link className="w-fit text-sm text-accent hover:underline" href={providerRankingHref(listing.providerId)}>
                        {t("clientMarket.detail.sellerRank")}
                      </Link>
                    ) : null}
                  </section>
                ) : null}
                <p className="rounded-lg bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-950">{t("clientMarket.risk.notice")}</p>
              </div>
            ) : null}
          </Drawer.Body>
          {listing && terms ? (
            <div className="flex shrink-0 flex-wrap items-end justify-between gap-3 border-t border-border bg-card pt-4">
              <div className="min-w-0">
                <strong className="text-lg tabular-nums">{terms.price}</strong>
                {terms.monthly ? <span className="text-xs text-muted-foreground">{t("online.perMonth")}</span> : null}
                {terms.term ? <p className="text-xs text-muted-foreground">{terms.term}</p> : null}
              </div>
              {availability === "available" ? (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => onRent(listing)}
                  className={cn(
                    "inline-flex h-9 items-center rounded-lg px-4 text-sm font-semibold transition-colors disabled:opacity-60 bg-slate-900 text-white hover:bg-slate-800 active:bg-slate-700",
                    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2",
                  )}
                >
                  {t(authenticated ? "clientMarket.shop.rent" : "clientMarket.shop.guestRent")}
                </button>
              ) : (
                <span className="rounded-full bg-muted px-3 py-1 text-xs text-muted-foreground">{t(onlineStatusKey(availability))}</span>
              )}
            </div>
          ) : null}
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}
