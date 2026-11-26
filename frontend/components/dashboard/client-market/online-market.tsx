"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useRouter, useSearchParams } from "next/navigation";
import { ClientListingAddDialog } from "./client-listing-add-dialog";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useAuth } from "@/components/auth/auth-provider";
import {
  getOnlineListings,
  onlineRequest,
  onlineMarketError,
  type OnlineListing,
  type OnlineQuote,
  type OnlineSellerItem,
} from "@/lib/online-client-market";
import { clientMarketTenantHref } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { InPlaceTopup } from "@/components/market/in-place-topup";
import { usePersistentState } from "@/lib/use-persistent-state";
import { currentPageHref } from "@/lib/url-search";
import { ApiError, createMarketAccessRequest } from "@/lib/api";
import {
  type ClientTerminationHostSummary,
} from "@/lib/client-termination";
import { ClientOnlineHeatmap } from "@/components/dashboard/client-online-heatmap";
import { ClientTerminationHostAction } from "@/components/market/client-termination-host-action";
import { formatWalletUnits, suggestedTopupAmount } from "@/lib/market-wallet-money";
import { validPaidWalletQuoteFunding } from "@/lib/market-wallet-quote";
import { trialDuration } from "@/lib/client-checkout";
import {
  ROUTER_OPEN_LOGIN_EVENT,
  SHOP_BROWSE_KEY,
  SHOP_UNKNOWN_COUNTRY,
  aggregateOnlineShopCountries,
  clientMarketWorkspaceFromQuery,
  onlineListingRentable,
  onlineStatusKey,
  shopContinentForCountry,
  onlineShopSpecLine,
  shopCountryKey,
  sortShopCountries,
  type ShopContinent,
} from "./host-utils";
import {
  ONLINE_CLIENT_CARD_GRID_CLASS,
  ClientOfferCard,
  ClientSpecs,
  OnlineClientCard,
  OnlineSellingToolbar,
  OnlineShopRegionPicker,
} from "./online-shop";

const field =
  "h-12 rounded-lg border bg-background px-3 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";
const focusRing =
  "focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";
const button = `min-h-11 ${focusRing}`;
const link = `inline-flex min-h-11 items-center rounded underline outline-none ${focusRing}`;

function probeKey(code: string) {
  switch (code) {
    case "tunnel_offline":
      return "online.offline";
    case "upgrading":
      return "online.upgrading";
    case "switching":
      return "lease.switching";
    case "awaiting_receipt":
      return "lease.awaiting";
    case "status_mismatch":
      return "lease.mismatch";
    default:
      return "lease.controlUnavailable";
  }
}
// Keep region and seller views shareable.
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
  return (
    <OnlineMarketView
      key={session?.user?.id || "guest"}
      authenticated={!!session?.authenticated}
      {...props}
    />
  );
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
  const { locale, t } = useLocaleText();
  const [list, setList] = React.useState<OnlineListing[]>([]);
  const router = useRouter();
  const queryCountry = (searchParams.get("country") || "").trim().toUpperCase();
  const [browse, setBrowse] = usePersistentState<{
    scope: "mine" | ShopContinent;
    country: string;
    rentableOnly: boolean;
  }>(SHOP_BROWSE_KEY, { scope: "all", country: "", rentableOnly: true });
  const country = /^[A-Z]{2}$/.test(queryCountry)
    ? queryCountry
    : /^[A-Z]{2}$/.test(browse.country) || browse.country === SHOP_UNKNOWN_COUNTRY
      ? browse.country
      : "";
  const continent: ShopContinent = /^[A-Z]{2}$/.test(country)
    ? shopContinentForCountry(country)
    : browse.scope === "mine" ? "all" : browse.scope;
  const rentableOnly = browse.rentableOnly;
  React.useEffect(() => {
    if (browse.scope === "mine") setBrowse((value) => ({ ...value, scope: "all" }));
  }, [browse.scope, setBrowse]);
  const [workspace, setWorkspace] = React.useState<"catalog" | "selling">(() => (
    clientMarketWorkspaceFromQuery(searchParams.get("view"), searchParams.get("tab"))
  ));
  const [sellerItems, setSellerItems] = React.useState<OnlineSellerItem[]>([]);
  const [addListingOpen, setAddListingOpen] = React.useState(searchParams.get("add") === "1");
  const [sellingQuery, setSellingQuery] = React.useState("");
  const [version, setVersion] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [unlisting, setUnlisting] = React.useState<OnlineListing | null>(null);
  const [topupOpen, setTopupOpen] = React.useState(false);
  const [selection, setSelection] = React.useState<OnlineListing | null>(null);
  const [quote, setQuote] = React.useState<OnlineQuote | null>(null);
  const selected = selection
    ? (list.find((listing) =>
        listing.installationId === selection.installationId && listing.owned === selection.owned,
      ) ?? selection)
    : null;
  const [accessRequired, setAccessRequired] = React.useState(false);
  const [uncertain, setUncertain] = React.useState(false);
  const { session } = useAuth();
  const recoveryKey = session?.user?.id ? "cc-client-order:" + session.user.id : "";
  const [pendingOrder, setPendingOrder] = React.useState<string | null>(null);
  const mounted = React.useRef(true);
  React.useEffect(() => {
    mounted.current = true;
    try {
      setPendingOrder(recoveryKey ? sessionStorage.getItem(recoveryKey) : null);
    } catch {
      /* tab storage unavailable */
    }
    return () => {
      mounted.current = false;
    };
  }, [recoveryKey]);
  function rememberOrder(id: string | null) {
    if (mounted.current) setPendingOrder(id);
    try {
      if (!recoveryKey) return;
      if (id) sessionStorage.setItem(recoveryKey, id);
      else sessionStorage.removeItem(recoveryKey);
    } catch {
      /* In-memory recovery remains available. */
    }
  }
  const [topupBusy, setTopupBusy] = React.useState(false);
  const attempted = React.useRef<string | null>(null);
  const quoteVersion = React.useRef(0);
  const [ack, setAck] = React.useState(false);
  const [auto, setAuto] = React.useState(false);
  const [dialogError, setDialogError] = React.useState("");
  React.useEffect(() => {
    const control = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const read = async () => {
      try {
        const [s, seller] = await Promise.all([
          getOnlineListings(control.signal),
          authenticated && workspace === "selling"
            ? onlineRequest<{ items: OnlineSellerItem[] }>(
                "seller-inventory",
                "GET",
                undefined,
                control.signal,
              )
            : Promise.resolve({ items: [] as OnlineSellerItem[] }),
        ]);
        if (!control.signal.aborted) {
          setList(s.listings.filter((l) => !providerId || l.providerId === providerId));
          setSellerItems(seller.items);
          setError("");
        }
      } catch (e) {
        if (!control.signal.aborted) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!control.signal.aborted) {
          setLoaded(true);
          timer = setTimeout(() => void read(), 15_000);
        }
      }
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
  }, [focusId, list, workspace]);
  const money = (minor: number) =>
    minor
      ? new Intl.NumberFormat(locale, {
          style: "currency",
          currency: "USD",
        }).format(minor / 100) + t("online.perMonth")
      : t("online.free");
  const date = (sec: number) => new Date(sec * 1000).toLocaleString(locale);
  const region = (code: string | null) =>
    code && /^[A-Z]{2}$/.test(code)
      ? new Intl.DisplayNames([locale], { type: "region" }).of(code) || code
      : t("online.unknown");
  async function action(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setDialogError("");
    try {
      await fn();
      setVersion((v) => v + 1);
    } catch (e) {
      setAccessRequired(e instanceof ApiError && e.code === "MARKET_ACCESS_REQUIRED");
      setDialogError(onlineMarketError(e, t));
    } finally {
      setBusy(false);
    }
  }
  const close = () => {
    quoteVersion.current++;
    attempted.current = null;
    setUncertain(false);
    setSelection(null);
    setUnlisting(null);
    setTopupOpen(false);
    setAccessRequired(false);
    setQuote(null);
    setAck(false);
    setAuto(false);
    setDialogError("");
  };
  const funding = quote?.walletFunding;
  const paidQuote = !!quote && quote.terms.cyclePriceMinor > 0;
  const selfRental = quote?.terms.selfRental === true;
  const funded =
    !paidQuote ||
    (validPaidWalletQuoteFunding(
      funding ?? undefined,
      quote.terms.cyclePriceMinor,
    ) &&
      funding?.shortfallUnits === "0");
  const selectWorkspace = (next: "catalog" | "selling") => {
    setWorkspace(next);
    updateMarketSearch((params) => {
      params.delete("tab");
      if (next === "selling") params.set("view", "selling");
      else if (params.get("view") === "selling") params.delete("view");
    });
  };
  const selectCountry = (code: string) => {
    const next = code === SHOP_UNKNOWN_COUNTRY || /^[A-Z]{2}$/.test(code) ? code : "";
    setBrowse((current) => ({
      ...current,
      country: next,
      scope: /^[A-Z]{2}$/.test(next) ? shopContinentForCountry(next) : current.scope === "mine" ? "all" : current.scope,
    }));
    setSelection(null);
    updateMarketSearch((params) => {
      if (/^[A-Z]{2}$/.test(next)) params.set("country", next);
      else params.delete("country");
    });
  };
  React.useEffect(() => {
    if (!authenticated && workspace === "selling") selectWorkspace("catalog");
  }, [authenticated, workspace]);
  const shopSource = list.filter(
    (item) => item.state === "listed" && (!rentableOnly || onlineListingRentable(item)),
  );
  const shopCards = sortShopCountries(
    aggregateOnlineShopCountries(shopSource),
    "idle",
    (code) => (/^[A-Z]{2}$/.test(code) ? region(code) : "\uffff"),
  );
  const shopListings = country
    ? shopSource.filter((item) => shopCountryKey(item.country) === country)
    : shopSource.filter((item) => continent === "all" || shopContinentForCountry(item.country) === continent);
  const countryName = !/^[A-Z]{2}$/.test(country) ? t("clientMarket.shop.unknownRegion") : region(country);
  const listingById = new Map(list.map((item) => [item.installationId, item]));
  const priceOf = money;
  async function requote(listing: OnlineListing, renew: boolean, latest = false) {
    const version = ++quoteVersion.current;
    const current = latest
      ? (await getOnlineListings()).listings.find(
          (l) => l.installationId === listing.installationId,
        )
      : listing;
    if (!current || !onlineListingRentable(current)) throw new Error(t("lease.unavailable"));
    const next = await onlineRequest<OnlineQuote>(
      "installations/" + current.installationId + "/quote",
      "POST",
      { revision: current.revision, autoRenew: renew },
    );
    if (version !== quoteVersion.current) return;
    setSelection(current);
    setAccessRequired(false);
    setQuote(next);
    setAuto(next.terms.autoRenew);
    setAck(false);
  }
  React.useEffect(() => {
    if (!quote || !selected || uncertain || busy || topupOpen || topupBusy) return;
    const timer = setTimeout(
      () => {
        if (attempted.current === quote.id) return;
        void action(async () => {
          setQuote(null);
          await requote(selected, auto, true);
          setDialogError(t("lease.requoted"));
        });
      },
      Math.max(0, quote.expiresAt * 1000 - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [quote, selected, uncertain, busy, topupOpen, topupBusy, auto]);
  const quoteListing = (listing: OnlineListing) => {
    if (!authenticated) {
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    if (pendingOrder) {
      setError(t("lease.receipt"));
      return;
    }
    close();
    setSelection(listing);
    void action(() => requote(listing, false));
  };
  function showReceipt(id: string) {
    rememberOrder(null);
    if (!mounted.current) return;
    close();
    router.push(clientMarketTenantHref(id));
  }
  async function confirmRental() {
    if (!selected || !quote || !funded || (!selfRental && !ack)) return;
    attempted.current = quote.id;
    rememberOrder(quote.id);
    try {
      const receipt = await onlineRequest<{ leaseId: string }>("quotes/" + quote.id + "/commit", "POST", {
        acknowledged: selfRental || ack,
      });
      if (!receipt?.leaseId) throw new Error(t("lease.receipt"));
      showReceipt(receipt.leaseId);
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        rememberOrder(null);
        attempted.current = null;
        setUncertain(false);
        setQuote(null);
        setAck(false);
        await requote(selected, auto, true);
        setDialogError(t("lease.requoted"));
        return;
      }
      if (!(e instanceof ApiError) || e.status >= 500) {
        setUncertain(true);
        throw new Error(t("lease.receipt"));
      }
      if (e.status !== 401) rememberOrder(null);
      attempted.current = null;
      setUncertain(false);
      throw e;
    }
  }
  const sellingQueryText = sellingQuery.trim().toLowerCase();
  const matchesSelling = (...parts: Array<string | null | undefined>) =>
    !sellingQueryText || parts.some((part) => (part || "").toLowerCase().includes(sellingQueryText));
  const history = sellerItems.filter(
    (item) => item.state === "released" && matchesSelling(item.label, item.tenantEmail),
  );
  const rentedSellerItems = sellerItems.filter((item) => item.state !== "released" && !!item.tenantEmail);
  const listedSellerItems = sellerItems.filter(
    (item) => item.state !== "released" && !rentedSellerItems.includes(item),
  );
  const visibleListed = listedSellerItems.filter((item) => matchesSelling(item.label, item.tenantEmail));
  const visibleRented = rentedSellerItems.filter((item) => matchesSelling(item.label, item.tenantEmail));
  return (
    <section className="grid min-w-0 gap-4" aria-label={t("online.supply")}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-slate-200 pb-3">
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
          <h2 className="text-sm font-semibold text-slate-900">{t("clientMarket.shop.catalog")}</h2>
        )}
        <Button
          isIconOnly
          variant="ghost"
          aria-label={t("common.reload")}
          isDisabled={!loaded}
          onPress={() => setVersion((v) => v + 1)}
        >
          {!loaded ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
        </Button>
      </div>
      {pendingOrder && !selection && (
        <div role="status" className="flex flex-wrap items-center gap-2">
          {t("lease.receipt")}
          <Button
            isDisabled={busy}
            onPress={() =>
              void action(async () => {
                try {
                  const receipt = await onlineRequest<{ leaseId: string }>(
                    "quotes/" + encodeURIComponent(pendingOrder) + "/commit",
                    "POST",
                    { acknowledged: true },
                  );
                  if (!receipt?.leaseId) throw new Error(t("lease.receipt"));
                  showReceipt(receipt.leaseId);
                } catch (e) {
                  if (e instanceof ApiError && e.status === 409) rememberOrder(null);
                  setError(e instanceof Error ? e.message : String(e));
                }
              })
            }
          >
            {t("common.retry")}
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="text-danger">
          {error}{" "}
          <Button variant="ghost" className={button} onPress={() => setVersion((v) => v + 1)}>
            {t("common.reload")}
          </Button>
        </p>
      )}
      {workspace === "selling" && authenticated ? (
        <div className="grid gap-3">
          {addListingOpen && (
            <ClientListingAddDialog
              initialId={searchParams.get("installation")}
              onClose={() => {
                setAddListingOpen(false);
                updateMarketSearch((p) => {
                  p.delete("add");
                  p.delete("installation");
                });
              }}
              onListed={() => setVersion((value) => value + 1)}
            />
          )}
          {!loaded ? (
            <p role="status">{t("common.loading")}</p>
          ) : (
            <>
              <OnlineSellingToolbar
                listedCount={listedSellerItems.length + rentedSellerItems.length}
                query={sellingQuery}
                onQuery={setSellingQuery}
                onAdd={() => setAddListingOpen(true)}
              />
              {visibleListed.length === 0 && visibleRented.length === 0 && !history.length ? (
                <p className="rounded-md border border-dashed border-slate-200 px-4 py-10 text-center text-sm text-slate-500">
                  {t(
                    sellingQueryText
                      ? "clientMarket.shop.sellingSearchEmpty"
                      : "clientMarket.shop.sellingEmpty",
                  )}
                </p>
              ) : null}
              {visibleListed.length ? (
                <div className={ONLINE_CLIENT_CARD_GRID_CLASS} aria-label={t("clientMarket.shop.cardGrid")}>
                  {visibleListed.map((item) => {
                    const listing = item.listingId ? listingById.get(item.listingId) : undefined;
                    return (
                      <OnlineClientCard
                        key={item.id}
                        cardId={`market-client-${item.listingId || item.id}`}
                        name={item.label}
                        price={priceOf(item.cyclePriceMinor)}
                        status={t(onlineStatusKey(item.state))}
                        detail={
                          item.tenantEmail
                            ? t("clientMarket.shop.tenantLine", { email: item.tenantEmail })
                            : t("clientMarket.shop.notRented")
                        }
                        spec={onlineShopSpecLine(listing?.hardware)}
                        specLabel={t("clientMarket.shop.summarySpecs")}
                        listing={listing}
                        actions={
                          item.listingId && !["occupied", "unlocking"].includes(item.state) ? (
                            <Button
                              className={button}
                              variant="outline"
                              isDisabled={busy}
                              onPress={() => {
                                if (!listing) return;
                                close();
                                setUnlisting(listing);
                              }}
                            >
                              {t("online.unlist")}
                            </Button>
                          ) : null
                        }
                        extra={
                          item.listingId && item.id === item.listingId ? (
                            <div className="grid gap-2">
                              {listing?.lastError && listing.observedAt != null && ["locking", "unlocking"].includes(item.state) && (
                                <p role="status">
                                  {t("online.probe")} · {date(listing.observedAt)} · {t(probeKey(listing.lastError))}
                                  {listing.attempts != null && ` · ${t("metrics.shareControl.attempts", { count: listing.attempts })}`}
                                </p>
                              )}
                              {listing && ["locking", "unlocking"].includes(item.state) && (
                                <Button
                                  size="sm"
                                  variant="outline"
                                  isDisabled={busy}
                                  onPress={() =>
                                    void action(async () => {
                                      await onlineRequest(
                                        "installations/" + listing.installationId + "/retry",
                                        "POST",
                                      );
                                    })
                                  }
                                >
                                  {t("common.retry")}
                                </Button>
                              )}
                              <details className="min-w-0">
                                <summary className="cursor-pointer text-sm">
                                  {t("dashboard.clientOnlineCalendar.title")}
                                </summary>
                                <div className="mt-2">
                                  <ClientOnlineHeatmap installationId={item.listingId} />
                                </div>
                              </details>
                              <ClientTerminationHostAction
                                summary={terminations.get(item.id)}
                                onOpen={onTerminationOpen}
                              />
                            </div>
                          ) : (
                            <ClientTerminationHostAction
                              summary={terminations.get(item.id)}
                              onOpen={onTerminationOpen}
                            />
                          )
                        }
                      />
                    );
                  })}
                </div>
              ) : null}
              {visibleRented.length ? (
                <section className="grid gap-2 border-t border-slate-200 pt-5">
                  <h3 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
                    {t("clientMarket.shop.filter.rented")}
                    <span className="ml-1.5 tabular-nums text-slate-400">{rentedSellerItems.length}</span>
                  </h3>
                  <div className={ONLINE_CLIENT_CARD_GRID_CLASS}>
                    {visibleRented.map((item) => {
                      const listing = item.listingId ? listingById.get(item.listingId) : undefined;
                      return (
                        <OnlineClientCard
                          key={item.id}
                          cardId={`market-client-${item.listingId || item.id}`}
                          name={item.label}
                          price={priceOf(item.cyclePriceMinor)}
                          muted
                          status={t(onlineStatusKey(item.state))}
                          detail={[
                            item.tenantEmail
                              ? t("clientMarket.shop.tenantLine", { email: item.tenantEmail })
                              : t("clientMarket.shop.notRented"),
                            item.expiresAt
                              ? t("clientMarket.shop.expires", { date: date(item.expiresAt) })
                              : "",
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                          spec={onlineShopSpecLine(listing?.hardware)}
                          specLabel={t("clientMarket.shop.summarySpecs")}
                          listing={listing}
                          extra={
                            <>
                              <ClientTerminationHostAction
                                summary={item.listingId ? terminations.get(item.listingId) : undefined}
                                onOpen={onTerminationOpen}
                              />
                              {item.terminationId && (
                                <Button size="sm" onPress={() => onTerminationOpen(item.terminationId!)}>
                                  {t("online.exitRecord")}
                                </Button>
                              )}
                            </>
                          }
                        />
                      );
                    })}
                  </div>
                </section>
              ) : null}
              {history.length > 0 && (
                <details>
                  <summary>
                    {t("lease.history")} · {history.length}
                  </summary>
                  <ul>
                    {history.map((item) => (
                      <li key={item.id} className="flex flex-wrap items-center gap-2">
                        <span>
                          {item.label} · {item.tenantEmail}
                        </span>
                        {item.terminationId && (
                          <Button variant="ghost" onPress={() => onTerminationOpen(item.terminationId!)}>
                            {t("online.exitRecord")}
                          </Button>
                        )}
                      </li>
                    ))}
                  </ul>
                </details>
              )}
              {dialogError && !selection && !unlisting && <p role="alert">{dialogError}</p>}
            </>
          )}
        </div>
      ) : !loaded ? (
        <p role="status">{t("common.loading")}</p>
      ) : (
        <>
          {providerNotice}
          <OnlineShopRegionPicker
            cards={shopCards}
            continent={continent}
            country={country}
            onContinent={(next) => {
              setBrowse((current) => ({ ...current, scope: next }));
              if (country && (next === "all" || shopContinentForCountry(country) !== next)) selectCountry("");
            }}
            onCountry={selectCountry}
            emptyAction={
              rentableOnly && list.some((item) => item.state === "listed")
                ? {
                    label: t("clientMarket.shop.showAll"),
                    onClick: () => setBrowse((v) => ({ ...v, rentableOnly: false })),
                  }
                : undefined
            }
            rentable={{
              pressed: rentableOnly,
              label: t("clientMarket.shop.onlyRentable"),
              onClick: () => setBrowse((v) => ({ ...v, rentableOnly: !v.rentableOnly })),
            }}
          />
          {shopListings.length > 0 || (country && shopSource.length > 0) ? (
            <section
              className="grid gap-2"
              aria-label={
                country
                  ? t("clientMarket.shop.hostListLabel", { name: countryName })
                  : t("clientMarket.shop.cardGrid")
              }
            >
              <h2 className="text-sm font-semibold">{t("clientMarket.shop.stepHost")}</h2>
              {shopListings.length === 0 ? (
                <p className="text-sm text-muted-foreground">{t("clientMarket.shop.noHosts")}</p>
              ) : (
                <div className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
                  {shopListings.map((listing) => (
                    <ClientOfferCard
                      key={listing.installationId}
                      listing={listing}
                      authenticated={authenticated}
                      busy={busy}
                      onRent={() => quoteListing(listing)}
                      extra={
                        <ClientTerminationHostAction
                          summary={terminations.get(listing.installationId)}
                          onOpen={onTerminationOpen}
                        />
                      }
                    />
                  ))}
                </div>
              )}
            </section>
          ) : null}
        </>
      )}

      <Modal.Backdrop
        isOpen={!!unlisting}
        onOpenChange={(open) => {
          if (!open && !busy) close();
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(420px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>
                {t("online.unlist")} · {unlisting?.label}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body>
              <p>{t("online.unlistNotice")}</p>
              {dialogError && <p role="alert">{dialogError}</p>}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={busy} onPress={close}>
                {t("common.cancel")}
              </Button>
              <Button
                isDisabled={busy}
                onPress={() =>
                  void action(async () => {
                    if (!unlisting) return;
                    await onlineRequest(
                      "installations/" + encodeURIComponent(unlisting.installationId) + "/listing",
                      "DELETE",
                    );
                    close();
                  })
                }
              >
                {t("online.unlist")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <Modal.Backdrop
        isOpen={!!selected}
        onOpenChange={(open) => {
          if (!open && !busy && !topupBusy) close();
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="flex max-h-[90dvh] w-[min(460px,calc(100vw-1.5rem))] flex-col">
            <Modal.Header className="grid shrink-0 gap-1">
              <Modal.Heading className="break-all">{selected?.label}</Modal.Heading>
              {selected && <ClientSpecs listing={selected} />}
            </Modal.Header>
            <Modal.Body className="grid min-h-0 gap-4 overflow-y-auto text-sm">
              {quote && (
                <>
                  <div className="grid gap-3 rounded-xl bg-muted/60 p-4">
                    <div className="flex flex-wrap items-baseline justify-between gap-2">
                      <span>
                        {t(
                          paidQuote && !quote.terms.trialSeconds
                            ? "clientMarket.shop.priceAtActivation"
                            : "clientMarket.shop.due",
                        )}
                      </span>
                      <strong className="text-2xl font-semibold tabular-nums tracking-tight">
                        {formatUsdMoney(quote.terms.trialSeconds ? 0 : quote.terms.cyclePriceMinor, locale)}
                      </strong>
                    </div>
                    <div className="flex flex-wrap items-baseline justify-between gap-2 border-t border-border pt-2 text-xs">
                      <span className="text-muted-foreground">
                        {t(quote.terms.trialSeconds ? "online.trialHours" : "clientMarket.shop.term")}
                      </span>
                      <span>
                        {quote.terms.trialSeconds
                          ? trialDuration(quote.terms.trialSeconds, locale)
                          : paidQuote
                            ? t("clientMarket.shop.monthly")
                            : quote.terms.freeDays
                              ? new Intl.NumberFormat(locale, {
                                  style: "unit",
                                  unit: "day",
                                  unitDisplay: "long",
                                }).format(quote.terms.freeDays)
                              : t("clientMarket.shop.ongoing")}
                      </span>
                    </div>
                    {quote.terms.trialSeconds > 0 && (
                      <p className="text-right text-xs">
                        {t("clientMarket.shop.trialAfter", { price: money(quote.terms.cyclePriceMinor) })}
                      </p>
                    )}
                  </div>
                  {selfRental && <p className="text-muted-foreground">{t("clientMarket.shop.selfNote")}</p>}
                  {paidQuote && (
                    <>
                      {funding && validPaidWalletQuoteFunding(funding, quote.terms.cyclePriceMinor) ? (
                        <dl className="grid gap-1.5 text-sm tabular-nums">
                          <div className="flex justify-between gap-3">
                            <dt>{t("clientMarket.shop.firstPeriod")}</dt>
                            <dd>{formatWalletUnits(funding.requiredUnits, "$", locale)}</dd>
                          </div>
                          <div className="flex justify-between gap-3">
                            <dt>{t("marketWallet.available")}</dt>
                            <dd>{formatWalletUnits(funding.availableUnits, "$", locale)}</dd>
                          </div>
                          {funding.committedUnits !== "0" && (
                            <div className="flex justify-between gap-3">
                              <dt>{t("clientMarket.shop.otherPayments")}</dt>
                              <dd>{formatWalletUnits(funding.committedUnits, "$", locale)}</dd>
                            </div>
                          )}
                          {!funded && (
                            <div className="flex justify-between gap-3 font-semibold">
                              <dt>{t("clientMarket.shop.balanceGap")}</dt>
                              <dd>{formatWalletUnits(funding.shortfallUnits, "$", locale)}</dd>
                            </div>
                          )}
                        </dl>
                      ) : (
                        <p role="alert">{t("marketWallet.shortcutUnavailable")}</p>
                      )}
                      <label className="flex items-center gap-2">
                        <input
                          type="checkbox"
                          checked={auto}
                          disabled={busy || uncertain || topupOpen || topupBusy}
                          onChange={(e) => {
                            const renew = e.target.checked;
                            setAuto(renew);
                            setQuote(null);
                            setAck(false);
                            void action(() => requote(selected!, renew));
                          }}
                        />
                        {t("online.renew")}
                      </label>
                      <details className="text-xs text-muted-foreground">
                        <summary className="cursor-pointer">{t("clientMarket.shop.billingDetails")}</summary>
                        <p className="mt-2">
                          {t("clientTermination.quote", {
                            rate: quote.terms.terminationPolicy.rateBps / 100,
                          })}
                        </p>
                      </details>
                    </>
                  )}
                  {!selfRental && funded && (
                    <label className="flex items-start gap-2 leading-5">
                      <input
                        className="mt-1"
                        type="checkbox"
                        checked={ack}
                        disabled={uncertain || busy}
                        onChange={(e) => setAck(e.target.checked)}
                      />
                      {t("clientMarket.shop.rootConsent")}
                    </label>
                  )}
                  {topupOpen &&
                    !funded &&
                    funding &&
                    selected &&
                    validPaidWalletQuoteFunding(funding, quote.terms.cyclePriceMinor) && (
                      <InPlaceTopup
                        active
                        purpose={t("marketWallet.shortfall", {
                          amount: formatWalletUnits(funding.shortfallUnits, "$", locale),
                        })}
                        requiredUnits={(
                          BigInt(funding.requiredUnits) + BigInt(funding.committedUnits)
                        ).toString()}
                        suggestedAmount={suggestedTopupAmount(funding.shortfallUnits)}
                        idPrefix="client-rent-topup"
                        onBusyChange={setTopupBusy}
                        onFunded={() =>
                          void action(async () => {
                            setTopupOpen(false);
                            setQuote(null);
                            await requote(selected, auto, true);
                          })
                        }
                      />
                    )}
                </>
              )}
              {busy && !quote && <p role="status">{t("common.loading")}</p>}
              {dialogError && (
                <p role="alert" className="text-sm text-danger">
                  {dialogError}
                </p>
              )}
            </Modal.Body>
            <Modal.Footer className="shrink-0 border-t border-border pt-3">
              <Button variant="ghost" isDisabled={busy || topupBusy} onPress={close}>
                {t("common.cancel")}
              </Button>
              {accessRequired && selected ? (
                <Button
                  isDisabled={busy}
                  onPress={() =>
                    void action(async () => {
                      await createMarketAccessRequest({
                        targetKind: "client_host",
                        targetId: selected.installationId,
                      });
                      toast.success(t("marketApproval.requested"));
                      close();
                    })
                  }
                >
                  {t("marketApproval.apply")}
                </Button>
              ) : !quote ||
                (paidQuote &&
                  !validPaidWalletQuoteFunding(funding ?? undefined, quote.terms.cyclePriceMinor)) ? (
                <Button
                  isDisabled={busy}
                  onPress={() => selected && void action(() => requote(selected, auto, true))}
                >
                  {t("online.quote")}
                </Button>
              ) : !funded ? (
                <Button isDisabled={busy || topupBusy} onPress={() => setTopupOpen(!topupOpen)}>
                  {t(topupOpen ? "clientMarket.shop.backToQuote" : "clientMarket.shop.topup")}
                </Button>
              ) : (
                <Button
                  isDisabled={busy || topupBusy || (!selfRental && !ack)}
                  onPress={() => void action(confirmRental)}
                >
                  {t(
                    uncertain
                      ? "common.retry"
                      : paidQuote
                        ? "online.confirm"
                        : "clientMarket.shop.confirmFree",
                  )}
                </Button>
              )}
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </section>
  );
}
