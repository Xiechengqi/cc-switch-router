"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { CircleDot, Loader2, RefreshCw } from "lucide-react";
import { useSearchParams } from "next/navigation";
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
import { replaceCurrentSearch } from "@/lib/url-search";
import { cn } from "@/lib/utils";
import { formatUsdMoney } from "@/lib/market-money";
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
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { validPaidWalletQuoteFunding } from "@/lib/market-wallet-quote";
import { trialDuration } from "@/lib/client-checkout";
import {
  ROUTER_OPEN_LOGIN_EVENT,
  SHOP_UNKNOWN_COUNTRY,
  aggregateOnlineShopCountries,
  clientMarketTenantFromQuery,
  clientMarketWorkspaceFromQuery,
  onlineListingRentable,
  shopContinentForCountry,
  shopCountryKey,
  sortShopCountries,
  type ShopContinent,
} from "./host-utils";
import { OnlineShopConfirm, OnlineShopListingButton, OnlineShopRegionPicker } from "./online-shop";

const field =
  "h-12 rounded-lg border bg-background px-3 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";
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
function ListingOfferDialog({
  id,
  label,
  open,
  onOpenChange,
  onListed,
}: {
  id: string;
  label: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onListed: () => void;
}) {
  const { t } = useLocaleText();
  const [price, setPrice] = React.useState("0");
  const [days, setDays] = React.useState("");
  const [ack, setAck] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const paid = Number(price) > 0;
  const valid =
    /^\d+(\.\d{1,2})?$/.test(price) &&
    Number(price) <= 1_000_000 &&
    (!days ||
      (/^\d+$/.test(days) &&
        Number(days) >= (paid ? 0 : 1) &&
        Number(days) <= (paid ? 30 : 365)));
  async function submit() {
    if (!ack || !valid || busy) return;
    setBusy(true);
    setError("");
    try {
      await onlineRequest(
        `installations/${encodeURIComponent(id)}/listing`,
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
  return (
    <Modal.Backdrop
        isOpen={open}
        onOpenChange={(next) => {
          if (!busy) onOpenChange(next);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(440px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>
                {t("online.offer")} · {label}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-4">
              <label className="grid gap-1">
                {t("online.price")}
                <input
                  className={field}
                  inputMode="decimal"
                  value={price}
                  onChange={(e) => setPrice(e.target.value)}
                />
              </label>
              <label className="grid gap-1">
                {t(paid ? "online.trialDays" : "online.freeDays")}
                <input
                  className={field}
                  inputMode="numeric"
                  value={days}
                  onChange={(e) => setDays(e.target.value)}
                />
              </label>
              <label className="flex min-h-11 items-start gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={ack}
                  onChange={(e) => setAck(e.target.checked)}
                />
                {t("online.sellerAck")}
              </label>
              {error && (
                <p role="alert" className="text-danger">
                  {error}
                </p>
              )}
            </Modal.Body>
            <Modal.Footer>
              <Button
                variant="ghost"
                className={button}
                onPress={() => onOpenChange(false)}
                isDisabled={busy}
              >
                {t("common.cancel")}
              </Button>
              <Button
                className={button}
                onPress={() => void submit()}
                isDisabled={!valid || !ack || busy}
              >
                {t("online.list")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
  );
}

type MarketProps = {
  focusId: string | null;
  providerId?: string;
  rentalId?: string | null;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
  onChanged: () => void;
  tenantRecords?: React.ReactNode;
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
  rentalId,
  terminations,
  onTerminationOpen,
  onChanged,
  tenantRecords,
  providerNotice,
}: MarketProps & { authenticated: boolean }) {
  const searchParams = useSearchParams();
  const { session } = useAuth();
  const sessionEmail = session?.user?.email;
  const { locale, t } = useLocaleText();
  const { openConsole } = useClientConsole();
  const [list, setList] = React.useState<OnlineListing[]>([]);
  const [leases, setLeases] = React.useState<OnlineLease[]>([]);
  const [country, setCountry] = React.useState(() => {
    const code = (searchParams.get("country") || "").trim().toUpperCase();
    return /^[A-Z]{2}$/.test(code) ? code : "";
  });
  const [continent, setContinent] = React.useState<ShopContinent>(() => (
    country ? shopContinentForCountry(country) : "all"
  ));
  const [rentableOnly, setRentableOnly] = React.useState(true);
  const [tenantOpen, setTenantOpen] = React.useState(() => (
    clientMarketTenantFromQuery(searchParams.get("view"), rentalId || null)
  ));
  const [workspace, setWorkspace] = React.useState<"catalog" | "selling">(() => (
    clientMarketWorkspaceFromQuery(searchParams.get("view"), searchParams.get("tab"))
  ));
  const [sellerItems, setSellerItems] = React.useState<OnlineSellerItem[]>([]);
  const [listable, setListable] = React.useState<Array<{ id: string; label: string }>>([]);
  const [listingTarget, setListingTarget] = React.useState<{ id: string; label: string } | null>(null);
  const [version, setVersion] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
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
          if (authenticated && workspace === "selling") {
            const dashboard = await getDashboard(control.signal);
            const email = (sessionEmail || "").trim().toLowerCase();
            const mine = dashboard.clients.filter((client) => {
              const owner = client.installation.ownerEmail?.trim().toLowerCase();
              return !!email && owner === email
                && client.installation.marketLeaseV1
                && client.clientTunnel?.online
                && !client.installation.marketListed;
            }).map((client) => ({
              id: client.installation.id,
              label: client.clientTunnel?.subdomain || client.installation.id,
            }));
            setListable(mine);
          } else {
            setListable([]);
          }
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
  }, [authenticated, sessionEmail, version, providerId, workspace]);
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
    setTenantOpen(next);
    replaceCurrentSearch((params) => {
      if (next) params.set("view", "rentals");
      else if (params.get("view") === "rentals") params.delete("view");
    });
  };
  const selectCountry = (code: string) => {
    const next = code === SHOP_UNKNOWN_COUNTRY || /^[A-Z]{2}$/.test(code) ? code : "";
    setCountry(next);
    setPickedId(null);
    setSelection(null);
    if (next) setContinent(shopContinentForCountry(next));
    replaceCurrentSearch((params) => {
      if (/^[A-Z]{2}$/.test(next)) params.set("country", next);
      else params.delete("country");
    });
  };
  React.useEffect(() => {
    if (!authenticated && workspace === "selling") selectWorkspace("catalog");
  }, [authenticated, workspace]);
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
  const filterButton = (pressed: boolean, label: string, onClick: () => void) => (
    <button
      type="button"
      aria-pressed={pressed}
      className={cn(
        "inline-flex h-9 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
        pressed ? "bg-sky-100 font-semibold text-sky-800" : "bg-slate-100 font-medium text-slate-600 hover:bg-slate-200",
      )}
      onClick={onClick}
    >
      <CircleDot className={cn("h-3.5 w-3.5", pressed ? "text-sky-600" : "text-slate-400")} />
      <span>{label}</span>
    </button>
  );
  return (
    <section className="grid min-w-0 gap-4" aria-label={t("online.supply")}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-slate-200 pb-3">
        {authenticated ? (
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <SegmentedControl
              value={tenantOpen ? "catalog" : workspace}
              onChange={(next) => {
                if (tenantOpen) selectTenant(false);
                selectWorkspace(next);
              }}
              ariaLabel={t("clientMarket.shop.workspaceLabel")}
              size="sm"
              items={[
                { id: "catalog", label: t("clientMarket.shop.catalog") },
                { id: "selling", label: t("clientMarket.shop.selling") },
              ]}
            />
            <button
              type="button"
              aria-pressed={tenantOpen}
              className={cn(
                "inline-flex h-8 items-center rounded-lg px-2.5 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
                tenantOpen ? "bg-sky-100 text-sky-800" : "text-slate-600 hover:bg-slate-100",
              )}
              onClick={() => selectTenant(!tenantOpen)}
            >
              {t("clientMarket.shop.tenant")}
            </button>
          </div>
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
        <div className="grid gap-2">
          <ListingOfferDialog
            id={listingTarget?.id || ""}
            label={listingTarget?.label || ""}
            open={!!listingTarget}
            onOpenChange={(next) => { if (!next) setListingTarget(null); }}
            onListed={() => setVersion((value) => value + 1)}
          />
          {listable.map((client) => (
            <article key={client.id} className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-border bg-background p-4">
              <div>
                <h3 className="font-semibold">{client.label}</h3>
                <p className="text-sm text-muted-foreground">{t("online.list")}</p>
              </div>
              <Button className={button} variant="outline" onPress={() => setListingTarget(client)}>{t("online.list")}</Button>
            </article>
          ))}
          {sellerItems.length === 0 && listable.length === 0 && loaded ? <p className="text-sm text-muted-foreground">{t("account.client.providerEmpty")}</p> : null}
          {sellerItems.map((item) => (
            <article key={item.id} id={`market-client-${item.listingId || item.id}`} tabIndex={-1} className="grid gap-2 rounded-xl border border-border bg-background p-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <h3 className="font-semibold">{item.label}</h3>
                  <p className="text-sm text-muted-foreground">{t(onlineStatusKey(item.state))} · {item.cyclePriceMinor ? `${formatUsdMoney(item.cyclePriceMinor, locale)}${t("online.perMonth")}` : `${formatUsdMoney(0, locale)}${t("online.perMonth")}`}</p>
                </div>
                {item.listingId && !["occupied", "unlocking"].includes(item.state) ? (
                  <Button className={button} variant="outline" isDisabled={busy} onPress={() => {
                    const listing = list.find((entry) => entry.installationId === item.listingId);
                    if (!listing) return;
                    close();
                    setSelection(listing);
                  }}>{t("online.unlist")}</Button>
                ) : null}
              </div>
              <p className="text-sm">{item.tenantEmail || t("account.market.notRented")}{item.expiresAt ? ` · ${date(item.expiresAt)}` : ""}</p>
              {item.listingId && item.id === item.listingId ? (
                <details className="min-w-0">
                  <summary className="cursor-pointer text-sm">{t("dashboard.clientOnlineCalendar.title")}</summary>
                  <div className="mt-2">
                    <ClientOnlineHeatmap installationId={item.listingId} />
                  </div>
                </details>
              ) : null}
              <ClientTerminationHostAction summary={terminations.get(item.id)} onOpen={onTerminationOpen} />
            </article>
          ))}
        </div>
      ) : !loaded ? (
        <p role="status">{t("common.loading")}</p>
      ) : tenantOpen ? (
        <div className="grid gap-4">
          {tenantRecords}
          {currentLeases.length > 0 ? (
            <section className="grid gap-2">
              <h2 className="text-sm font-semibold">{t("online.myLeases")}</h2>
              {currentLeases.map((l) => (
                <article key={l.leaseId} id={`market-client-${l.leaseId}`} tabIndex={-1} className="rounded-xl border border-border bg-background p-4">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <strong id={`market-client-${l.leaseId}-title`}>{l.alias}</strong>
                      <p className="text-sm">{t(onlineStatusKey(l.state))} · {t("online.grantUntil")} {date(l.expiresAt)}</p>
                    </div>
                    <div className="flex gap-2">
                      {l.state === "active" ? (
                        <Button className={button} aria-describedby={`market-client-${l.leaseId}-title`} onPress={() => openConsole({ clientId: l.leaseId, kind: "console", url: l.clientUrl, title: l.alias })}>{t("online.console")}</Button>
                      ) : null}
                      {["active", "starting"].includes(l.state) ? (
                        <Button variant="outline" className={button} aria-describedby={`market-client-${l.leaseId}-title`} onPress={() => { close(); setEnding(l); }}>{t("online.end")}</Button>
                      ) : null}
                    </div>
                  </div>
                  {l.terminationId ? <a className={link} href={clientTerminationHref(l.terminationId)}>{t("online.exitRecord")}</a> : null}
                  {l.events.length > 0 ? (
                    <details className="mt-2 text-sm">
                      <summary className="cursor-pointer">{t("online.ownerActivity")}</summary>
                      <ul>{l.events.map((event, index) => <li key={index}>{date(event.at)} · {t(onlineStatusKey(event.kind))}</li>)}</ul>
                    </details>
                  ) : null}
                </article>
              ))}
            </section>
          ) : null}
        </div>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2">
            {filterButton(rentableOnly, t("clientMarket.shop.onlyRentable"), () => { setRentableOnly((value) => !value); setPickedId(null); })}
          </div>
          {providerNotice}
          <OnlineShopRegionPicker
            cards={shopCards}
            continent={continent}
            country={country}
            onContinent={(next) => {
              setContinent(next);
              if (country && next !== "all" && shopContinentForCountry(country) !== next) selectCountry("");
            }}
            onCountry={selectCountry}
            emptyAction={rentableOnly && list.some((item) => !item.owned && (!providerId || item.providerId === providerId))
              ? { label: t("clientMarket.shop.showAll"), onClick: () => { setRentableOnly(false); setPickedId(null); } }
              : undefined}
          />
          {country ? (
            <div className="grid min-w-0 items-start gap-3 xl:grid-cols-[minmax(0,1fr)_minmax(16rem,22rem)]">
              <section className="grid min-w-0 content-start gap-1.5" aria-label={t("clientMarket.shop.hostListLabel", { name: countryName })}>
                <h2 className="text-sm font-semibold">{t("clientMarket.shop.stepHost")}</h2>
                {shopListings.length === 0 ? <p className="text-sm text-muted-foreground">{t("clientMarket.shop.noHosts")}</p> : shopListings.map((listing) => (
                  <OnlineShopListingButton
                    key={listing.installationId}
                    listing={listing}
                    selected={pickedId === listing.installationId}
                    onSelect={(next) => setPickedId(next.installationId)}
                  />
                ))}
              </section>
              <OnlineShopConfirm
                listing={shopListings.find((item) => item.installationId === pickedId) || null}
                countryName={countryName}
                authed={authenticated}
                onContinue={(listing) => {
                  if (!authenticated) {
                    window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
                    return;
                  }
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
                }}
                onLogin={() => window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT))}
                busy={busy}
                error={!quote ? dialogError : ""}
              />
            </div>
          ) : null}
        </>
      )}

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
