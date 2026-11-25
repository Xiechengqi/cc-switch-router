"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2, Plus, RefreshCw } from "lucide-react";
import { useSearchParams } from "next/navigation";
import { CompactSelect } from "@/components/common/compact-select";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientConsole } from "@/components/dashboard/client-console";
import {
  onlineRequest,
  type OnlineListing,
  type OnlineLease,
  type OnlineQuote,
  type OnlineSellerItem,
} from "@/lib/online-client-market";
import { usePersistentState } from "@/lib/use-persistent-state";
import { replaceCurrentSearch } from "@/lib/url-search";
import { cn } from "@/lib/utils";
import { ApiError, createMarketAccessRequest } from "@/lib/api";
import {
  clientTerminationHref,
  previewClientTermination,
  terminationExplanationValid,
  terminationPreviewExpired,
  type ClientTerminationPreviewRead,
  type ClientTerminationRequest,
  type ClientTerminationHostSummary,
} from "@/lib/client-termination";
import { ClientOnlineHeatmap } from "@/components/dashboard/client-online-heatmap";
import { ClientTerminationHostAction } from "@/components/market/client-termination-host-action";
import { getDashboard } from "@/lib/api";
import type { DashboardClient } from "@/lib/types";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { validPaidWalletQuoteFunding } from "@/lib/market-wallet-quote";
import { trialDuration } from "@/lib/client-checkout";
import {
  ROUTER_OPEN_LOGIN_EVENT,
  SHOP_BROWSE_KEY,
  SHOP_UNKNOWN_COUNTRY,
  aggregateOnlineShopCountries,
  clientMarketTenantFromQuery,
  clientMarketWorkspaceFromQuery,
  onlineListingRentable,
  shopContinentForCountry,
  onlineShopSpecLine,
  shopCountryKey,
  sortShopCountries,
  type ShopContinent,
} from "./host-utils";
import {
  ONLINE_CLIENT_CARD_GRID_CLASS,
  OnlineClientCard,
  OnlineSellingToolbar,
  OnlineShopRegionPicker,
  onlineClientMonthlyPrice,
} from "./online-shop";

const field =
  "h-12 rounded-lg border bg-background px-3 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";
const offerField =
  "h-10 min-w-0 rounded-md border border-slate-200 bg-white px-3 text-sm text-slate-900 outline-none focus:border-slate-400 disabled:cursor-not-allowed disabled:bg-slate-50 disabled:text-slate-500";
const focusRing =
  "focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";
const button = `min-h-11 ${focusRing}`;
const link = `inline-flex min-h-11 items-center rounded underline outline-none ${focusRing}`;

const statuses = [
  "available",
  "offline",
  "occupied",
  "locking",
  "unlocking",
  "starting",
  "active",
  "ending",
  "cleaning",
  "released",
  "assistance",
  "terminal",
  "upgrade",
  "restart",
  "rollback",
] as const;
export function onlineStatusKey(value: string) {
  const known = statuses.find((s) => s === value);
  return known ? (`online.${known}` as const) : "online.unknown";
}
type ListingCandidate = {
  id: string;
  label: string;
  platform: string;
  countryCode?: string;
  region?: string;
  online: boolean;
  reason: "listed" | "offline" | "capability" | null;
};

function listingCandidate(client: DashboardClient, email: string): ListingCandidate | null {
  const owner = client.installation.ownerEmail?.trim().toLowerCase();
  if (!email || owner !== email) return null;
  const online = !!client.clientTunnel?.online;
  const reason = client.installation.marketListed
    ? "listed"
    : !client.installation.marketLeaseV1
      ? "capability"
      : !online
        ? "offline"
        : null;
  return {
    id: client.installation.id,
    label: client.clientTunnel?.subdomain || client.installation.id,
    platform: client.installation.platform,
    countryCode: client.installation.countryCode,
    region: client.installation.region,
    online,
    reason,
  };
}

function listingBlockedKey(reason: "listed" | "offline" | "capability") {
  if (reason === "listed") return "clientMarket.shop.dialog.blocked.listed" as const;
  if (reason === "offline") return "clientMarket.shop.dialog.blocked.offline" as const;
  return "clientMarket.shop.dialog.blocked.capability" as const;
}

function ClientListingAddDialog({
  open,
  onOpenChange,
  onListed,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onListed: () => void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const email = (session?.user?.email || "").trim().toLowerCase();
  const [candidates, setCandidates] = React.useState<ListingCandidate[]>([]);
  const [clientId, setClientId] = React.useState("");
  const [loading, setLoading] = React.useState(false);
  const [loadRevision, setLoadRevision] = React.useState(0);
  const [loadError, setLoadError] = React.useState("");
  const [price, setPrice] = React.useState("0");
  const [days, setDays] = React.useState("");
  const [ack, setAck] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    let active = true;
    setCandidates([]);
    setClientId("");
    setPrice("0");
    setDays("");
    setAck(false);
    setLoading(true);
    setLoadError("");
    setError("");
    getDashboard(controller.signal)
      .then((dashboard) => {
        if (!active) return;
        const mine = dashboard.clients.flatMap((client) => {
          const row = listingCandidate(client, email);
          return row ? [row] : [];
        });
        setCandidates(mine);
        setClientId(mine.find((item) => !item.reason)?.id || "");
      })
      .catch((reason) => {
        if (active && !controller.signal.aborted) {
          setLoadError(reason instanceof Error ? reason.message : String(reason));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [email, loadRevision, open]);
  const eligible = candidates.filter((item) => !item.reason);
  const blocked = candidates.filter((item): item is ListingCandidate & { reason: "listed" | "offline" | "capability" } => item.reason !== null);
  const selected = eligible.find((item) => item.id === clientId);
  const paid = Number(price) > 0;
  const valid =
    /^\d+(\.\d{1,2})?$/.test(price) &&
    Number(price) <= 1_000_000 &&
    (!days ||
      (/^\d+$/.test(days) &&
        Number(days) >= (paid ? 0 : 1) &&
        Number(days) <= (paid ? 30 : 365)));
  const countryName = (code?: string) =>
    code && /^[A-Z]{2}$/.test(code)
      ? new Intl.DisplayNames([locale], { type: "region" }).of(code) || code
      : "";
  const selectClient = (next: string) => {
    setClientId(next);
    setPrice("0");
    setDays("");
    setAck(false);
    setError("");
  };
  async function submit() {
    if (!selected || !ack || !valid || busy) return;
    setBusy(true);
    setError("");
    try {
      await onlineRequest(
        `installations/${encodeURIComponent(selected.id)}/listing`,
        "POST",
        {
          cyclePriceMinor: Math.round(Number(price) * 100),
          freeDays: !paid && days ? Number(days) : null,
          trialDays: paid ? Number(days) : 0,
          acknowledged: true,
        },
      );
      toast.success(t("online.listPending"));
      onListed();
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  const countryLabel = countryName(selected?.countryCode);
  const summary = selected
    ? [
        { key: "platform", value: selected.platform },
        { key: "country", value: countryLabel },
        { key: "region", value: selected.region && selected.region !== countryLabel ? selected.region : "" },
        { key: "tunnel", value: t("clientMarket.shop.dialog.online") },
      ].filter((item) => item.value)
    : [];
  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(720px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("clientMarket.shop.dialog.title")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[75vh] gap-4 overflow-y-auto">
            {loading ? <div className="flex items-center gap-2 py-6 text-sm text-slate-500"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div> : null}
            {!loading && loadError ? (
              <div role="alert" className="flex flex-wrap items-center gap-3 rounded-md border border-rose-200 bg-rose-50 px-3 py-2.5 text-sm text-rose-800">
                <span className="min-w-[12rem] flex-1">{loadError}</span>
                <Button className="whitespace-nowrap" size="sm" variant="outline" onPress={() => setLoadRevision((current) => current + 1)}>
                  <RefreshCw className="h-4 w-4" />
                  {t("common.retry")}
                </Button>
              </div>
            ) : null}
            {!loading && !loadError && eligible.length === 0 ? <p className="text-sm text-slate-500">{t("clientMarket.shop.dialog.none")}</p> : null}
            {!loading && selected ? (
              <>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t("clientMarket.shop.dialog.select")}
                  <CompactSelect
                    value={clientId}
                    options={eligible.map((item) => ({
                      value: item.id,
                      label: item.label,
                      content: (
                        <span className="inline-flex min-w-0 items-center gap-1.5">
                          <strong className="min-w-0 truncate font-mono text-xs font-semibold text-slate-800" title={item.label}>{item.label}</strong>
                          <span className="inline-flex shrink-0 items-center gap-1 text-[11px] text-slate-500">
                            <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                            {t("clientMarket.shop.dialog.online")}
                          </span>
                        </span>
                      ),
                    }))}
                    onChange={selectClient}
                    ariaLabel={t("clientMarket.shop.dialog.select")}
                    className="w-full"
                    triggerClassName="min-h-9 w-full py-1.5 text-xs"
                  />
                </label>
                {summary.length ? (
                  <div className="flex min-w-0 flex-wrap gap-x-4 gap-y-1 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600">
                    <strong className="text-slate-700">{t("clientMarket.shop.dialog.summary")}</strong>
                    {summary.map((item) => <span key={item.key}>{item.value}</span>)}
                  </div>
                ) : null}
                <label className="grid gap-1 text-xs text-slate-500">
                  {t("online.price")}
                  <input className={offerField} inputMode="decimal" value={price} disabled={busy} onChange={(event) => setPrice(event.target.value)} />
                </label>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t(paid ? "online.trialDays" : "online.freeDays")}
                  <input className={offerField} inputMode="numeric" value={days} disabled={busy} onChange={(event) => setDays(event.target.value)} />
                </label>
                <label className="flex min-h-11 items-start gap-2 text-sm text-slate-700">
                  <input type="checkbox" checked={ack} disabled={busy} onChange={(event) => setAck(event.target.checked)} />
                  {t("online.sellerAck")}
                </label>
              </>
            ) : null}
            {!loading && blocked.length ? (
              <section className="grid gap-2 border-t border-slate-200 pt-4">
                <strong className="text-xs font-semibold text-slate-700">{t("clientMarket.shop.dialog.blockedTitle")}</strong>
                <div className="grid gap-2">
                  {blocked.map((item) => (
                    <div key={item.id} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-500">
                      <span className="min-w-0 flex-1 truncate font-mono" title={item.label}>{item.label}</span>
                      <span>{t(listingBlockedKey(item.reason))}</span>
                    </div>
                  ))}
                </div>
              </section>
            ) : null}
            {error ? <p role="alert" className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onPress={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || loading || !selected || !valid || !ack} onPress={() => void submit()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
              {t("clientMarket.shop.dialog.publish")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

type MarketProps = {
  rentInstallationId?: string | null;
  focusId: string | null;
  providerId?: string;
  rentalId?: string | null;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
  onChanged: () => void;
  tenantRecords?: React.ReactNode;
  providerNotice?: React.ReactNode;
  quoteOnly?: boolean;
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

/** Quote dialog only. The Clients page uses this so its cards do not render the shop. */
export function OnlineRentDialog({ installationId, onClose }: { installationId: string; onClose: () => void }) {
  return <OnlineClientMarket focusId={null} rentInstallationId={installationId} terminations={new Map()} onTerminationOpen={() => {}} onChanged={onClose} quoteOnly />;
}
function OnlineMarketView({
  authenticated,
  rentInstallationId,
  providerId,
  focusId,
  rentalId,
  terminations,
  onTerminationOpen,
  onChanged,
  tenantRecords,
  providerNotice,
  quoteOnly = false,
}: MarketProps & { authenticated: boolean }) {
  const searchParams = useSearchParams();
  const { locale, t } = useLocaleText();
  const { openConsole } = useClientConsole();
  const [list, setList] = React.useState<OnlineListing[]>([]);
  const [leases, setLeases] = React.useState<OnlineLease[]>([]);
  const queryCountry = (searchParams.get("country") || "").trim().toUpperCase();
  const queryChoosesBrowse = clientMarketTenantFromQuery(searchParams.get("view"), rentalId || null)
    || /^[A-Z]{2}$/.test(queryCountry);
  const [browse, setBrowse] = usePersistentState<{ scope: "mine" | ShopContinent; country: string; rentableOnly: boolean }>(
    SHOP_BROWSE_KEY,
    { scope: "all", country: "", rentableOnly: true },
  );
  const country = queryChoosesBrowse && /^[A-Z]{2}$/.test(queryCountry)
    ? queryCountry
    : /^[A-Z]{2}$/.test(browse.country) || browse.country === SHOP_UNKNOWN_COUNTRY
      ? browse.country
      : "";
  const continent: ShopContinent = /^[A-Z]{2}$/.test(country)
    ? shopContinentForCountry(country)
    : browse.scope === "mine" ? "all" : browse.scope;
  const rentableOnly = browse.rentableOnly;
  const tenantOpen = queryChoosesBrowse
    ? clientMarketTenantFromQuery(searchParams.get("view"), rentalId || null)
    : browse.scope === "mine";
  const [workspace, setWorkspace] = React.useState<"catalog" | "selling">(() => (
    clientMarketWorkspaceFromQuery(searchParams.get("view"), searchParams.get("tab"))
  ));
  const [sellerItems, setSellerItems] = React.useState<OnlineSellerItem[]>([]);
  const [addListingOpen, setAddListingOpen] = React.useState(false);
  const [sellingQuery, setSellingQuery] = React.useState("");
  const [version, setVersion] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const quotedRentId = React.useRef("");
  const [pickedId, setPickedId] = React.useState<string | null>(null);
  const [selection, setSelection] = React.useState<OnlineListing | null>(null);
  const [quote, setQuote] = React.useState<OnlineQuote | null>(null);
  const selected = selection
    ? (list.find((listing) =>
        listing.installationId === selection.installationId && listing.owned === selection.owned,
      ) ?? selection)
    : null;
  const [exitRead, setExitRead] =
    React.useState<ClientTerminationPreviewRead | null>(null);
  const exitPreview = exitRead?.preview ?? null;
  const [exitReason, setExitReason] =
    React.useState<ClientTerminationRequest["reason"]>("voluntary");
  const [claim, setClaim] = React.useState("");
  const [accessRequired, setAccessRequired] = React.useState(false);
  const [ending, setEnding] = React.useState<OnlineLease | null>(null);
  const [ack, setAck] = React.useState(false);
  const [auto, setAuto] = React.useState(false);
  const [dialogError, setDialogError] = React.useState("");
  React.useEffect(() => {
    const control = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const read = async () => {
      try {
        const [s, r, seller] = await Promise.all([
          onlineRequest<{ listings: OnlineListing[] }>(
            "listings",
            "GET",
            undefined,
            control.signal,
          ),
          authenticated
            ? onlineRequest<{ leases: OnlineLease[] }>(
                "leases",
                "GET",
                undefined,
                control.signal,
              )
            : Promise.resolve({ leases: [] as OnlineLease[] }),
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
          setList(
            s.listings.filter(
              (l) => !providerId || l.providerId === providerId,
            ),
          );
          setLeases(r.leases);
          setSellerItems(seller.items);
          setError("");
        }
      } catch (e) {
        if (!control.signal.aborted)
          setError(e instanceof Error ? e.message : String(e));
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
    if (!focusId) { focused.current = null; return; }
    if (focusId === focused.current) return;
    const card = document.getElementById(`market-client-${focusId}`);
    if (card) {
      focused.current = focusId;
      card.focus();
    }
  }, [focusId, list, leases]);
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
      if (
        e instanceof ApiError && e.status === 409 &&
        selected && !selected.owned && !ending
      ) {
        setQuote(null);
        setVersion((v) => v + 1);
      }
      setAccessRequired(e instanceof ApiError && e.code === "MARKET_ACCESS_REQUIRED");
      setDialogError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  const close = () => {
    setPickedId(null);
    setSelection(null);
    setEnding(null);
    setExitRead(null);
    setExitReason("voluntary");
    setClaim("");
    setAccessRequired(false);
    setQuote(null);
    setAck(false);
    setAuto(false);
    setDialogError("");
  };
  const funding = quote?.walletFunding;
  const paidQuote = !!quote && quote.terms.cyclePriceMinor > 0;
  const freeDays = quote ? quote.terms.freeDays : selected?.freeDays;
  const funded =
    !paidQuote ||
    (validPaidWalletQuoteFunding(
      funding ?? undefined,
      quote.terms.cyclePriceMinor,
    ) &&
      funding?.shortfallUnits === "0");
  const selectWorkspace = (next: "catalog" | "selling") => {
    setWorkspace(next);
    replaceCurrentSearch((params) => {
      params.delete("tab");
      if (next === "selling") params.set("view", "selling");
      else if (params.get("view") === "selling") params.delete("view");
    });
  };
  const selectTenant = (next: boolean) => {
    setBrowse((current) => ({ ...current, scope: next ? "mine" : current.scope === "mine" ? "all" : current.scope }));
    replaceCurrentSearch((params) => {
      if (next) params.set("view", "rentals");
      else if (params.get("view") === "rentals") params.delete("view");
    });
  };
  const selectCountry = (code: string) => {
    const next = code === SHOP_UNKNOWN_COUNTRY || /^[A-Z]{2}$/.test(code) ? code : "";
    setBrowse((current) => ({
      ...current,
      country: next,
      scope: /^[A-Z]{2}$/.test(next) ? shopContinentForCountry(next) : current.scope === "mine" ? "all" : current.scope,
    }));
    setPickedId(null);
    setSelection(null);
    replaceCurrentSearch((params) => {
      if (/^[A-Z]{2}$/.test(next)) params.set("country", next);
      else params.delete("country");
    });
  };
  React.useEffect(() => {
    if (!authenticated && workspace === "selling") selectWorkspace("catalog");
  }, [authenticated, workspace]);
  React.useEffect(() => {
    const listing = list.find((item) => item.installationId === rentInstallationId && onlineListingRentable(item) && !item.owned);
    if (!listing || quotedRentId.current === `${listing.installationId}:${listing.revision}`) return;
    quotedRentId.current = `${listing.installationId}:${listing.revision}`;
    if (!authenticated) {
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    setSelection(listing);
    void action(async () => {
      setQuote(await onlineRequest<OnlineQuote>(`installations/${listing.installationId}/quote`, "POST", { revision: listing.revision, autoRenew: false }));
    });
  }, [authenticated, list, rentInstallationId]);
  const shopSource = list.filter((item) => (
    (!providerId || item.providerId === providerId)
    && !item.owned
    && (!rentableOnly || onlineListingRentable(item))
  ));
  const shopCards = sortShopCountries(
    aggregateOnlineShopCountries(shopSource),
    "idle",
    (code) => (/^[A-Z]{2}$/.test(code) ? region(code) : "\uffff"),
  );
  const shopListings = country
    ? shopSource.filter((item) => shopCountryKey(item.country) === country)
    : [];
  const currentLeases = leases.filter((lease) => lease.state !== "released");
  const countryName = !/^[A-Z]{2}$/.test(country)
    ? t("clientMarket.shop.unknownRegion")
    : region(country);
  const listingById = new Map(list.map((item) => [item.installationId, item]));
  const priceOf = (minor: number) => onlineClientMonthlyPrice(minor, locale, t("online.perMonth"));
  const hoursOf = (listing: OnlineListing | undefined) => listing
    ? t("online.hoursValue", { hours: listing.availableHours24 })
    : undefined;
  const quoteListing = (listing: OnlineListing) => {
    if (!authenticated) {
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    setPickedId(listing.installationId);
    setEnding(null);
    setExitRead(null);
    setExitReason("voluntary");
    setClaim("");
    setAck(false);
    setAuto(false);
    setDialogError("");
    setSelection(listing);
    void action(async () => {
      setQuote(await onlineRequest<OnlineQuote>(
        `installations/${listing.installationId}/quote`,
        "POST",
        { revision: listing.revision, autoRenew: false },
      ));
    });
  };
  const sellingQueryText = sellingQuery.trim().toLowerCase();
  const matchesSelling = (...parts: Array<string | null | undefined>) => (
    !sellingQueryText || parts.some((part) => (part || "").toLowerCase().includes(sellingQueryText))
  );
  const rentedSellerItems = sellerItems.filter((item) => !!item.tenantEmail || ["occupied", "unlocking"].includes(item.state));
  const listedSellerItems = sellerItems.filter((item) => !rentedSellerItems.includes(item));
  const visibleListed = listedSellerItems.filter((item) => matchesSelling(item.label, item.tenantEmail));
  const visibleRented = rentedSellerItems.filter((item) => matchesSelling(item.label, item.tenantEmail));
  return (
    <section className={quoteOnly ? "contents" : "grid min-w-0 gap-4"} aria-label={t("online.supply")}>
      {!quoteOnly ? <React.Fragment><div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-slate-200 pb-3">
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
        <Button isIconOnly variant="ghost" aria-label={t("common.reload")} isDisabled={!loaded} onPress={() => setVersion((v) => v + 1)}>
          {!loaded ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
        </Button>
      </div>
      {error && (
        <p role="alert" className="text-danger">
          {error}{" "}
          <Button
            variant="ghost"
            className={button}
            onPress={() => setVersion((v) => v + 1)}
          >
            {t("common.reload")}
          </Button>
        </p>
      )}
      {workspace === "selling" && authenticated ? (
        <div className="grid gap-3">
          <ClientListingAddDialog
            open={addListingOpen}
            onOpenChange={setAddListingOpen}
            onListed={() => setVersion((value) => value + 1)}
          />
          {!loaded ? <p role="status">{t("common.loading")}</p> : (
            <>
              <OnlineSellingToolbar
                listedCount={sellerItems.length}
                query={sellingQuery}
                onQuery={setSellingQuery}
                onAdd={() => setAddListingOpen(true)}
              />
              {visibleListed.length === 0 && visibleRented.length === 0 ? (
                <p className="rounded-md border border-dashed border-slate-200 px-4 py-10 text-center text-sm text-slate-500">
                  {t(sellingQueryText ? "clientMarket.shop.sellingSearchEmpty" : "clientMarket.shop.sellingEmpty")}
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
                        healthy={listing ? listing.healthy : null}
                        status={t(onlineStatusKey(item.state))}
                        detail={item.tenantEmail ? t("clientMarket.shop.tenantLine", { email: item.tenantEmail }) : t("clientMarket.shop.notRented")}
                        spec={onlineShopSpecLine(listing?.hardware)}
                        specLabel={t("clientMarket.shop.summarySpecs")}
                        hours={hoursOf(listing)}
                        hoursLabel={t("online.hours")}
                        actions={item.listingId && !["occupied", "unlocking"].includes(item.state) ? (
                          <Button className={button} variant="outline" isDisabled={busy} onPress={() => {
                            if (!listing) return;
                            close();
                            setSelection(listing);
                          }}>{t("online.unlist")}</Button>
                        ) : null}
                        extra={item.listingId && item.id === item.listingId ? (
                          <div className="grid gap-2">
                            <details className="min-w-0">
                              <summary className="cursor-pointer text-sm">{t("dashboard.clientOnlineCalendar.title")}</summary>
                              <div className="mt-2"><ClientOnlineHeatmap installationId={item.listingId} /></div>
                            </details>
                            <ClientTerminationHostAction summary={terminations.get(item.id)} onOpen={onTerminationOpen} />
                          </div>
                        ) : <ClientTerminationHostAction summary={terminations.get(item.id)} onOpen={onTerminationOpen} />}
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
                          healthy={listing ? listing.healthy : null}
                          muted
                          status={t(onlineStatusKey(item.state))}
                          detail={[item.tenantEmail ? t("clientMarket.shop.tenantLine", { email: item.tenantEmail }) : t("clientMarket.shop.notRented"), item.expiresAt ? t("clientMarket.shop.expires", { date: date(item.expiresAt) }) : ""].filter(Boolean).join(" · ")}
                          spec={onlineShopSpecLine(listing?.hardware)}
                          specLabel={t("clientMarket.shop.summarySpecs")}
                          hours={hoursOf(listing)}
                          hoursLabel={t("online.hours")}
                          extra={<ClientTerminationHostAction summary={terminations.get(item.id)} onOpen={onTerminationOpen} />}
                        />
                      );
                    })}
                  </div>
                </section>
              ) : null}
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
              if (tenantOpen) selectTenant(false);
              setBrowse((current) => ({ ...current, scope: next }));
              if (country && next !== "all" && shopContinentForCountry(country) !== next) selectCountry("");
            }}
            onCountry={selectCountry}
            emptyAction={!tenantOpen && rentableOnly && list.some((item) => !item.owned && (!providerId || item.providerId === providerId))
              ? { label: t("clientMarket.shop.showAll"), onClick: () => { setBrowse((current) => ({ ...current, rentableOnly: false })); setPickedId(null); } }
              : undefined}
            mine={authenticated ? { selected: tenantOpen, onSelect: () => selectTenant(true) } : undefined}
            rentable={tenantOpen ? undefined : {
              pressed: rentableOnly,
              label: t("clientMarket.shop.onlyRentable"),
              onClick: () => { setBrowse((current) => ({ ...current, rentableOnly: !current.rentableOnly })); setPickedId(null); },
            }}
          />
          {tenantOpen ? (
            <div className="grid gap-4">
              {tenantRecords}
          {currentLeases.length > 0 ? (
            <section className="grid gap-2">
              <h2 className="text-sm font-semibold">{t("online.myLeases")}</h2>
              <div className={ONLINE_CLIENT_CARD_GRID_CLASS}>
              {currentLeases.map((l) => (
                <OnlineClientCard
                  key={l.leaseId}
                  cardId={`market-client-${l.leaseId}`}
                  name={l.alias}
                  healthy={l.state === "active"}
                  status={t(onlineStatusKey(l.state))}
                  detail={`${t("online.grantUntil")} ${date(l.expiresAt)}`}
                  actions={(
                    <>
                      {l.state === "active" ? (
                        <Button className={button} onPress={() => openConsole({ clientId: l.leaseId, kind: "console", url: l.clientUrl, title: l.alias })}>{t("online.console")}</Button>
                      ) : null}
                      {["active", "starting"].includes(l.state) ? (
                        <Button variant="outline" className={button} onPress={() => { close(); setEnding(l); }}>{t("online.end")}</Button>
                      ) : null}
                    </>
                  )}
                  extra={(
                    <>
                      {l.terminationId ? <a className={link} href={clientTerminationHref(l.terminationId)}>{t("online.exitRecord")}</a> : null}
                      {l.events.length > 0 ? (
                        <details className="text-sm">
                          <summary className="cursor-pointer">{t("online.ownerActivity")}</summary>
                          <ul>{l.events.map((event, index) => <li key={index}>{date(event.at)} · {t(onlineStatusKey(event.kind))}</li>)}</ul>
                        </details>
                      ) : null}
                    </>
                  )}
                />
              ))}
              </div>
            </section>
          ) : null}
        </div>
          ) : country ? (
            <section className="grid gap-2" aria-label={t("clientMarket.shop.hostListLabel", { name: countryName })}>
              <h2 className="text-sm font-semibold">{t("clientMarket.shop.stepHost")}</h2>
              {shopListings.length === 0 ? <p className="text-sm text-muted-foreground">{t("clientMarket.shop.noHosts")}</p> : (
                <div className={ONLINE_CLIENT_CARD_GRID_CLASS}>
                  {shopListings.map((listing) => {
                    const rentable = onlineListingRentable(listing);
                    const status = listing.state !== "listed" ? listing.state : listing.occupied ? "occupied" : listing.healthy ? "available" : "offline";
                    return (
                      <OnlineClientCard
                        key={listing.installationId}
                        cardId={`market-client-${listing.installationId}`}
                        name={listing.label}
                        price={priceOf(listing.cyclePriceMinor)}
                        healthy={listing.healthy}
                        muted={!rentable}
                        selected={pickedId === listing.installationId}
                        status={t(onlineStatusKey(status))}
                        detail={countryName}
                        spec={onlineShopSpecLine(listing.hardware)}
                        specLabel={t("clientMarket.shop.summarySpecs")}
                        hours={hoursOf(listing)}
                        hoursLabel={t("online.hours")}
                        onSelect={() => setPickedId(listing.installationId)}
                        actions={!authenticated || rentable ? (
                          <Button className={button} isDisabled={busy} aria-label={t("clientMarket.shop.cardContinue", { name: listing.label })} onPress={() => quoteListing(listing)}>
                            {t(authenticated ? "clientMarket.shop.continue" : "nav.login")}
                          </Button>
                        ) : null}
                        extra={(
                          <>
                            {listing.egressIp ? <p className="truncate text-[11px] text-muted-foreground">IP {listing.egressIp}</p> : null}
                            {listing.trialDays > 0 ? <p className="text-[11px] text-muted-foreground">{t("online.trialDays")} · {listing.trialDays}</p> : null}
                            {listing.freeDays ? <p className="text-[11px] text-muted-foreground">{t("online.freeDays")} · {listing.freeDays}</p> : null}
                            <p className="text-[11px] text-muted-foreground">{t("online.rootNotice")}</p>
                            {!rentable ? <p role="status" className="rounded-lg bg-primary/10 px-2.5 py-1.5 text-xs leading-5">{t("clientMarket.shop.notIdle", { status: t(onlineStatusKey(status)) })}</p> : null}
                            {pickedId === listing.installationId && dialogError && !quote ? <p role="alert" className="text-xs text-danger">{dialogError}</p> : null}
                          </>
                        )}
                      />
                    );
                  })}
                </div>
              )}
            </section>
          ) : null}
        </>
      )}
      </React.Fragment> : null}

      <Modal.Backdrop
        isOpen={!!ending || !!quote || !!selected?.owned || accessRequired}
        onOpenChange={(v) => {
          if (!v && !busy) close();
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(460px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>
                {t(
                  ending
                    ? exitPreview
                      ? "online.end"
                      : "online.previewExit"
                    : selected?.owned
                      ? "online.unlist"
                      : "online.rent",
                )}{" "}
                · {ending?.alias || selected?.label}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              {ending ? (
                <>
                  <p>{t("online.endNotice")}</p>
                  {exitPreview && (
                    <>
                      <p>
                        {t("clientTermination.compensation")} ·{" "}
                        {formatWalletUnits(
                          exitPreview.compensationUnits,
                          "$",
                          locale,
                        )}
                      </p>
                      <p>
                        {t("clientTermination.return")} ·{" "}
                        {formatWalletUnits(
                          exitPreview.returnUnits,
                          "$",
                          locale,
                        )}
                      </p>
                      <label className="grid gap-1">
                        {t("clientTermination.reason")}
                        <select
                          className={field}
                          value={exitReason}
                          onChange={(e) =>
                            setExitReason(
                              e.target
                                .value as ClientTerminationRequest["reason"],
                            )
                          }
                        >
                          <option value="voluntary">
                            {t("clientTermination.voluntary")}
                          </option>
                          <option value="provider_issue">
                            {t("clientTermination.providerIssue")}
                          </option>
                          <option value="platform_issue">
                            {t("clientTermination.platformIssue")}
                          </option>
                        </select>
                      </label>
                      {exitReason !== "voluntary" && (
                        <label className="grid gap-1">
                          {t("clientTermination.claim")}
                          <textarea
                            className="min-h-28 resize-y rounded-lg border bg-background px-3 py-2 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
                            value={claim}
                            onChange={(e) => setClaim(e.target.value)}
                            maxLength={2000}
                          />
                        </label>
                      )}
                    </>
                  )}
                </>
              ) : selected?.owned ? (
                <p>{t("online.unlistNotice")}</p>
              ) : (
                selected && (
                  <>
                    <p className="font-semibold">
                      {money(
                        quote?.terms.cyclePriceMinor ??
                          selected.cyclePriceMinor,
                      )}
                    </p>
                    {quote?.terms.trialSeconds ? (
                      <p>
                        {t("online.trialHours")} ·{" "}
                        {trialDuration(quote.terms.trialSeconds, locale)}
                      </p>
                    ) : null}
                    {quote && quote.terms.cyclePriceMinor > 0 && (
                      <p>
                        {t("clientTermination.quote", {
                          rate: quote.terms.terminationPolicy.rateBps / 100,
                        })}
                      </p>
                    )}
                    {paidQuote &&
                      (validPaidWalletQuoteFunding(
                        funding ?? undefined,
                        quote.terms.cyclePriceMinor,
                      ) && funding ? (
                        <div>
                          <p>
                            {t("marketWallet.available")} ·{" "}
                            {formatWalletUnits(
                              funding.availableUnits,
                              "$",
                              locale,
                            )}
                          </p>
                          {!funded && (
                            <p>
                              {t("marketWallet.shortfall", {
                                amount: formatWalletUnits(
                                  funding.shortfallUnits,
                                  "$",
                                  locale,
                                ),
                              })}{" "}
                              ·{" "}
                              <a
                                className={`rounded underline outline-none ${focusRing}`}
                                href="/account/wallet/"
                                target="_blank"
                                rel="noreferrer"
                              >
                                {t("marketWallet.title")}
                              </a>
                            </p>
                          )}
                        </div>
                      ) : (
                        <p role="alert">
                          {t("marketWallet.shortcutUnavailable")}
                        </p>
                      ))}
                    {freeDays && (
                      <p>
                        {t("online.freeDuration")} · {freeDays}
                      </p>
                    )}
                    {(quote ? paidQuote : selected.cyclePriceMinor > 0) && (
                      <label className="flex min-h-11 items-start gap-2 text-sm">
                        <input
                          type="checkbox"
                          checked={auto}
                          disabled={!!quote}
                          onChange={(e) => setAuto(e.target.checked)}
                        />
                        {t("online.renew")}
                      </label>
                    )}
                    {quote && (
                      <p className="text-sm">
                        {t("online.quoteUntil")} {date(quote.expiresAt)}
                      </p>
                    )}
                    <label className="flex min-h-11 items-start gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={ack}
                        onChange={(e) => setAck(e.target.checked)}
                      />
                      {t("online.rootNotice")}
                    </label>
                  </>
                )
              )}
              {accessRequired && selected && (
                <Button
                  className={button}
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
              )}
              {dialogError && (
                <p role="alert" className="text-danger">
                  {dialogError}
                </p>
              )}
            </Modal.Body>
            <Modal.Footer>
              <Button
                variant="ghost"
                className={button}
                isDisabled={busy}
                onPress={close}
              >
                {t("common.cancel")}
              </Button>
              <Button
                className={button}
                isDisabled={
                  busy ||
                  (!ending && !selected?.owned && !ack) ||
                  (!!ending &&
                    exitReason !== "voluntary" &&
                    !terminationExplanationValid(claim))
                }
                onPress={() =>
                  void action(async () => {
                    if (ending) {
                      if (!exitRead || terminationPreviewExpired(exitRead)) {
                        setExitRead(
                          await previewClientTermination(ending.leaseId),
                        );
                        return;
                      }
                      try {
                        await onlineRequest(
                          `leases/${ending.leaseId}/end`,
                          "POST",
                          {
                            previewId: exitRead.preview.previewId,
                            reason: exitReason,
                            claim: exitReason === "voluntary" ? "" : claim,
                          },
                        );
                      } catch (e) {
                        // An expired preview or a changed period: the next press previews again.
                        if (e instanceof ApiError && e.status === 409)
                          setExitRead(null);
                        throw e;
                      }
                      onChanged();
                      close();
                    } else if (selected?.owned) {
                      await onlineRequest(
                        `installations/${selected.installationId}/listing`,
                        "DELETE",
                      );
                      close();
                    } else if (selected && (!quote || !funded)) {
                      setQuote(
                        await onlineRequest<OnlineQuote>(
                          `installations/${selected.installationId}/quote`,
                          "POST",
                          { revision: selected.revision, autoRenew: auto },
                        ),
                      );
                    } else if (quote) {
                      // A committed quote can still be replayed after expiry if its reply was lost.
                      await onlineRequest(`quotes/${quote.id}/commit`, "POST", {
                        acknowledged: ack,
                      });
                      onChanged();
                      close();
                    }
                  })
                }
              >
                {t(
                  ending
                    ? exitPreview
                      ? "online.end"
                      : "online.previewExit"
                    : selected?.owned
                      ? "online.unlist"
                      : quote && !funded
                        ? "common.reload"
                        : quote
                          ? "online.confirm"
                          : "online.quote",
                )}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </section>
  );
}
