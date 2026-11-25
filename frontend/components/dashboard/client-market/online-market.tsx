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
  onlineMarketError,
  type OnlineListing,
  type OnlineLease,
  type OnlineQuote,
  type OnlineSellerItem,
} from "@/lib/online-client-market";
import { LeaseActions } from "./lease-actions";
import { InPlaceTopup } from "@/components/market/in-place-topup";
import { usePersistentState } from "@/lib/use-persistent-state";
import { currentPageHref } from "@/lib/url-search";
import { clientWebTerminalUrl } from "@/lib/client-web-view";
import { ApiError, createMarketAccessRequest } from "@/lib/api";
import {
  clientTerminationHref,
  type ClientTerminationHostSummary,
} from "@/lib/client-termination";
import { ClientOnlineHeatmap } from "@/components/dashboard/client-online-heatmap";
import { ClientTerminationHostAction } from "@/components/market/client-termination-host-action";
import { getDashboard } from "@/lib/api";
import type { DashboardClient } from "@/lib/types";
import { formatWalletUnits, suggestedTopupAmount } from "@/lib/market-wallet-money";
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
  "host_settings",
] as const;
export function onlineStatusKey(value: string) {
  const known = statuses.find((s) => s === value);
  return known ? (`online.${known}` as const) : "online.unknown";
}
function probeKey(code: string) {
  switch (code) {
    case "tunnel_offline":
      return "online.offline";
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

// These query changes also drive the lease card focus and the selected shop view.
function updateMarketSearch(update: (params: URLSearchParams) => void) {
  window.history.replaceState(null, "", currentPageHref(window.location.href, update));
}

function ClientListingAddDialog({
  open,
  onOpenChange,
  onListed,
  initialId,
}: {
  initialId:string|null;
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
        setClientId(
          mine.find((item) => item.id === initialId)?.id ||
            mine.find((item) => !item.reason)?.id ||
            mine[0]?.id ||
            "",
        );
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
  }, [email, loadRevision, open, initialId]);
  const selected = candidates.find((item) => item.id === clientId);
  const paid = Number(price) > 0;
  type Check = {
    ready: boolean;
    checks: Record<"ownerVerified" | "activated" | "capable" | "shareClear" | "unlisted", boolean>;
    online: boolean;
    paidReady: boolean;
    workspace: boolean;
    hardware: boolean;
    egress: boolean;
    egressIp?: string;
    resource?: OnlineListing["hardware"];
  };
  const [check, setCheck] = React.useState<Check | null>(null);
  const [probeRevision, reprobe] = React.useReducer((n) => n + 1, 0);
  const [probeError, setProbeError] = React.useState("");
  const selectedId = selected?.id;
  React.useEffect(() => {
    setCheck(null);
    setProbeError("");
    setAck(false);
    if (!open || !selectedId) return;
    const ctl = new AbortController();
    void onlineRequest<Check>(
      "installations/" + encodeURIComponent(selectedId) + "/preflight",
      "POST",
      { paid },
      ctl.signal,
    )
      .then((v) => {
        if (!ctl.signal.aborted) setCheck(v);
      })
      .catch((e) => {
        if (!ctl.signal.aborted) setProbeError(onlineMarketError(e, t));
      });
    return () => ctl.abort();
  }, [selectedId, open, paid, probeRevision]);
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
    if (!selected || !check?.ready || !ack || !valid || busy) return;
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
      setError(onlineMarketError(e, t));
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
        { key: "tunnel", value: t(selected.online ? "clientMarket.shop.dialog.online" : "online.offline") },
      ].filter((item) => item.value)
    : [];
  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(720px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header>
            <Modal.Heading>{t("clientMarket.shop.dialog.title")}</Modal.Heading>
          </Modal.Header>
          <Modal.Body className="grid max-h-[75vh] gap-4 overflow-y-auto">
            {loading ? (
              <div className="flex items-center gap-2 py-6 text-sm text-slate-500">
                <Loader2 className="h-4 w-4 animate-spin" />
                {t("common.loading")}
              </div>
            ) : null}
            {!loading && loadError ? (
              <div
                role="alert"
                className="flex flex-wrap items-center gap-3 rounded-md border border-rose-200 bg-rose-50 px-3 py-2.5 text-sm text-rose-800"
              >
                <span className="min-w-[12rem] flex-1">{loadError}</span>
                <Button
                  className="whitespace-nowrap"
                  size="sm"
                  variant="outline"
                  onPress={() => setLoadRevision((current) => current + 1)}
                >
                  <RefreshCw className="h-4 w-4" />
                  {t("common.retry")}
                </Button>
              </div>
            ) : null}
            {!loading && !loadError && candidates.length === 0 ? (
              <p className="text-sm text-slate-500">{t("clientMarket.shop.dialog.none")}</p>
            ) : null}
            {!loading && selected ? (
              <>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t("clientMarket.shop.dialog.select")}
                  <CompactSelect
                    value={clientId}
                    options={candidates.map((item) => ({
                      value: item.id,
                      label: item.label,
                      content: (
                        <span className="inline-flex min-w-0 items-center gap-1.5">
                          <strong
                            className="min-w-0 truncate font-mono text-xs font-semibold text-slate-800"
                            title={item.label}
                          >
                            {item.label}
                          </strong>
                          <span className="inline-flex shrink-0 items-center gap-1 text-[11px] text-slate-500">
                            <span
                              className={
                                item.reason
                                  ? "h-1.5 w-1.5 rounded-full bg-slate-400"
                                  : "h-1.5 w-1.5 rounded-full bg-emerald-500"
                              }
                            />
                            {t(
                              item.reason
                                ? listingBlockedKey(item.reason)
                                : "clientMarket.shop.dialog.online",
                            )}
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
                    {summary.map((item) => (
                      <span key={item.key}>{item.value}</span>
                    ))}
                  </div>
                ) : null}
                <div aria-live="polite" className="grid gap-1 text-xs">
                  {check ? (
                    <ul className="grid gap-1">
                      {Object.entries({
                        ...check.checks,
                        online: check.online,
                        workspace: check.workspace,
                        egress: check.egress,
                        hardware: check.hardware,
                        ...(paid ? { paidReady: check.paidReady } : {}),
                      }).map(([key, pass]) => (
                        <li key={key}>
                          {pass ? "✓" : "○"}{" "}
                          {t(("lease.check." + key) as keyof typeof import("@/lib/i18n").messages.en)}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <span>{probeError || t("common.loading")}</span>
                  )}
                  {check?.egressIp && (
                    <span>
                      IP {check.egressIp} · {onlineShopSpecLine(check.resource)}
                    </span>
                  )}
                  <div className="flex items-center gap-3">
                    <Button size="sm" variant="ghost" onPress={reprobe}>
                      {t("common.retry")}
                    </Button>
                    <a className="underline" href="/?tab=mine">
                      {t("nav.clients")}
                    </a>
                    {selected.reason === "listed" && (
                      <a className="underline" href="/client-market/?view=selling">
                        {t("clientMarket.shop.selling")}
                      </a>
                    )}
                    {check && !check.checks.shareClear && (
                      <a href="/share-market/?view=selling">Share Market</a>
                    )}
                    {paid && check && !check.paidReady && (
                      <a href="/account/provider-settings/">{t("lease.provider")}</a>
                    )}
                  </div>
                </div>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t("online.price")}
                  <input
                    className={offerField}
                    inputMode="decimal"
                    value={price}
                    disabled={busy}
                    onChange={(event) => setPrice(event.target.value)}
                  />
                </label>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t(paid ? "online.trialDays" : "online.freeDays")}
                  <input
                    className={offerField}
                    inputMode="numeric"
                    value={days}
                    disabled={busy}
                    onChange={(event) => setDays(event.target.value)}
                  />
                </label>
                <label className="flex min-h-11 items-start gap-2 text-sm text-slate-700">
                  <input
                    type="checkbox"
                    checked={ack}
                    disabled={busy || !check?.ready}
                    onChange={(event) => setAck(event.target.checked)}
                  />
                  {t("online.sellerAck")}
                </label>
              </>
            ) : null}
            {error ? (
              <p role="alert" className="text-sm text-rose-700">
                {error}
              </p>
            ) : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onPress={() => onOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              isDisabled={busy || loading || !selected || !valid || !ack || !check?.ready}
              onPress={() => void submit()}
            >
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
  focusId: string | null;
  providerId?: string;
  rentalId?: string | null;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
  onChanged: () => void;
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
  providerNotice,
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
  const [addListingOpen, setAddListingOpen] = React.useState(searchParams.get("add") === "1");
  const [sellingQuery, setSellingQuery] = React.useState("");
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
        const [s, r, seller] = await Promise.all([
          onlineRequest<{ listings: OnlineListing[] }>(
            "listings",
            "GET",
            undefined,
            control.signal,
          ),
          authenticated
            ? onlineRequest<{ leases: OnlineLease[] }>(
                focusId ? "leases?current=true&focus=" + encodeURIComponent(focusId) : "leases?current=true",
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
  }, [authenticated, version, providerId, workspace, focusId]);
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
    setPickedId(null);
    setSelection(null);
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
    updateMarketSearch((params) => {
      params.delete("tab");
      if (next === "selling") params.set("view", "selling");
      else if (params.get("view") === "selling") params.delete("view");
    });
  };
  const selectTenant = (next: boolean) => {
    setBrowse((current) => ({ ...current, scope: next ? "mine" : current.scope === "mine" ? "all" : current.scope }));
    updateMarketSearch((params) => {
      if (next) params.set("view", "rentals");
      else {
        if (params.get("view") === "rentals") params.delete("view");
        params.delete("rentalId");
      }
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
    updateMarketSearch((params) => {
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
  const currentLeases = leases.filter((lease) => lease.state !== "released" || lease.leaseId === focusId);
  const countryName = !/^[A-Z]{2}$/.test(country) ? t("clientMarket.shop.unknownRegion") : region(country);
  const listingById = new Map(list.map((item) => [item.installationId, item]));
  const priceOf = money;
  const hoursOf = (listing: OnlineListing | undefined) =>
    listing ? t("online.hoursValue", { hours: listing.availableHours24 }) : undefined;
  async function requote(listing: OnlineListing, renew: boolean, latest = false) {
    const version = ++quoteVersion.current;
    const current = latest
      ? (await onlineRequest<{ listings: OnlineListing[] }>("listings")).listings.find(
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
    setQuote(next);
    setAuto(next.terms.autoRenew);
    setAck(false);
  }
  React.useEffect(() => {
    if (!quote || !selected || uncertain || busy || topupBusy) return;
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
  }, [quote, selected, uncertain, busy, topupBusy, auto]);
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
    setPickedId(listing.installationId);
    setSelection(listing);
    void action(() => requote(listing, false));
  };
  function showReceipt(id: string) {
    rememberOrder(null);
    if (!mounted.current) return;
    close();
    setWorkspace("catalog");
    setBrowse((v) => ({ ...v, scope: "mine" }));
    updateMarketSearch((params) => {
      params.set("view", "rentals");
      params.set("focus", id);
      for (const k of ["tab", "rentalId", "country", "provider", "add", "installation"]) params.delete(k);
    });
    onChanged();
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
          <ClientListingAddDialog
            open={addListingOpen}
            initialId={searchParams.get("installation")}
            onOpenChange={(open) => {
              setAddListingOpen(open);
              if (!open)
                updateMarketSearch((p) => {
                  p.delete("add");
                  p.delete("installation");
                });
            }}
            onListed={() => setVersion((value) => value + 1)}
          />
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
                        healthy={listing ? listing.healthy : null}
                        status={t(onlineStatusKey(item.state))}
                        detail={
                          item.tenantEmail
                            ? t("clientMarket.shop.tenantLine", { email: item.tenantEmail })
                            : t("clientMarket.shop.notRented")
                        }
                        spec={onlineShopSpecLine(listing?.hardware)}
                        specLabel={t("clientMarket.shop.summarySpecs")}
                        hours={hoursOf(listing)}
                        hoursLabel={t("online.hours")}
                        actions={
                          item.listingId && !["occupied", "unlocking"].includes(item.state) ? (
                            <Button
                              className={button}
                              variant="outline"
                              isDisabled={busy}
                              onPress={() => {
                                if (!listing) return;
                                close();
                                setSelection(listing);
                              }}
                            >
                              {t("online.unlist")}
                            </Button>
                          ) : null
                        }
                        extra={
                          item.listingId && item.id === item.listingId ? (
                            <div className="grid gap-2">
                              {listing?.lastError && (
                                <p role="status">
                                  {t(probeKey(listing.lastError))} · {listing.attempts || 0}
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
                          healthy={listing ? listing.healthy : null}
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
                          hours={hoursOf(listing)}
                          hoursLabel={t("online.hours")}
                          actions={
                            listing?.clientUrl && (
                              <>
                                {(["console", "terminal"] as const).map((kind) => (
                                  <Button
                                    size="sm"
                                    key={kind}
                                    variant="outline"
                                    onPress={() =>
                                      openConsole({
                                        clientId: listing.installationId,
                                        kind,
                                        url:
                                          kind === "terminal"
                                            ? clientWebTerminalUrl(listing.clientUrl!)
                                            : listing.clientUrl!,
                                        title: listing.label,
                                      })
                                    }
                                  >
                                    {t(kind === "console" ? "lease.assistance" : "dashboard.clientTerminal")}
                                  </Button>
                                ))}
                              </>
                            )
                          }
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
              {dialogError && !selection && <p role="alert">{dialogError}</p>}
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
            emptyAction={
              !tenantOpen &&
              rentableOnly &&
              list.some((item) => !item.owned && (!providerId || item.providerId === providerId))
                ? {
                    label: t("clientMarket.shop.showAll"),
                    onClick: () => {
                      setBrowse((current) => ({ ...current, rentableOnly: false }));
                      setPickedId(null);
                    },
                  }
                : undefined
            }
            mine={authenticated ? { selected: tenantOpen, onSelect: () => selectTenant(true) } : undefined}
            rentable={
              tenantOpen
                ? undefined
                : {
                    pressed: rentableOnly,
                    label: t("clientMarket.shop.onlyRentable"),
                    onClick: () => {
                      setBrowse((current) => ({ ...current, rentableOnly: !current.rentableOnly }));
                      setPickedId(null);
                    },
                  }
            }
          />
          {tenantOpen ? (
            <div className="grid gap-4">
              {currentLeases.length === 0 && <p>{t("lease.empty")}</p>}
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
                        price={priceOf(l.cyclePriceMinor)}
                        detail={[region(l.country), t("online.grantUntil") + " " + date(l.expiresAt)].join(
                          " · ",
                        )}
                        actions={
                          <LeaseActions
                            id={l.leaseId}
                            active={l.state === "active"}
                            canEnd={["starting", "active"].includes(l.state)}
                            address={l.clientUrl}
                            onChanged={() => {
                              onChanged();
                              setVersion((n) => n + 1);
                            }}
                          />
                        }
                        extra={
                          <>
                            {l.state === "active" && (
                              <>
                                <p className="break-all text-xs">{l.clientUrl}</p>
                                <p className="break-all text-xs">
                                  {t("lease.usage", { host: "<slug>--" + new URL(l.clientUrl).host })}
                                </p>
                              </>
                            )}
                            {l.state === "starting" && (
                              <p role="status">
                                {t("lease.provisioning", { time: date(l.createdAt + 600) })}
                              </p>
                            )}
                            {l.state !== "starting" && l.activatedAt === null && (
                              <p role="status">
                                {t("lease.activationFailed")}{" "}
                                <button className="underline" onClick={() => selectTenant(false)}>
                                  {t("clientMarket.shop.catalog")}
                                </button>
                              </p>
                            )}
                            {l.holdStatus && <p>{t(`lease.hold.${l.holdStatus}`)}</p>}
                            {l.cyclePriceMinor > 0 && (
                              <p>
                                {t(l.autoRenew ? "lease.renewOn" : "lease.renewOff")}
                                {l.periodEnd ? " · " + t("lease.periodEnd", { time: date(l.periodEnd) }) : ""}
                              </p>
                            )}
                            {l.terminationId ? (
                              <a className={link} href={clientTerminationHref(l.terminationId)}>
                                {t("online.exitRecord")}
                              </a>
                            ) : null}
                            {l.events.length > 0 ? (
                              <details className="text-sm">
                                <summary className="cursor-pointer">{t("online.ownerActivity")}</summary>
                                <ul>
                                  {l.events.map((event, index) => (
                                    <li key={index}>
                                      {date(event.at)} · {t(onlineStatusKey(event.kind))}
                                      {(event.count || 0) > 1 ? " ×" + event.count : ""}
                                    </li>
                                  ))}
                                </ul>
                              </details>
                            ) : null}
                          </>
                        }
                      />
                    ))}
                  </div>
                </section>
              ) : null}
            </div>
          ) : country ? (
            <section
              className="grid gap-2"
              aria-label={t("clientMarket.shop.hostListLabel", { name: countryName })}
            >
              <h2 className="text-sm font-semibold">{t("clientMarket.shop.stepHost")}</h2>
              {shopListings.length === 0 ? (
                <p className="text-sm text-muted-foreground">{t("clientMarket.shop.noHosts")}</p>
              ) : (
                <div className={ONLINE_CLIENT_CARD_GRID_CLASS}>
                  {shopListings.map((listing) => {
                    const rentable = onlineListingRentable(listing);
                    const status =
                      listing.state !== "listed"
                        ? listing.state
                        : listing.occupied
                          ? "occupied"
                          : listing.healthy
                            ? "available"
                            : "offline";
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
                        actions={
                          !authenticated || rentable ? (
                            <Button
                              className={button}
                              isDisabled={busy}
                              aria-label={t("clientMarket.shop.cardContinue", { name: listing.label })}
                              onPress={() => quoteListing(listing)}
                            >
                              {t(authenticated ? "clientMarket.shop.continue" : "nav.login")}
                            </Button>
                          ) : null
                        }
                        extra={
                          <>
                            {listing.egressIp ? (
                              <p className="truncate text-[11px] text-muted-foreground">
                                IP {listing.egressIp}
                              </p>
                            ) : null}
                            {listing.trialDays > 0 ? (
                              <p className="text-[11px] text-muted-foreground">
                                {t("lease.trialDays", { count: listing.trialDays })}
                              </p>
                            ) : null}
                            {listing.freeDays ? (
                              <p className="text-[11px] text-muted-foreground">
                                {t("online.freeDuration")} · {listing.freeDays}
                              </p>
                            ) : null}
                            <p className="text-[11px] text-muted-foreground">{t("online.rootNotice")}</p>
                            {!rentable ? (
                              <p
                                role="status"
                                className="rounded-lg bg-primary/10 px-2.5 py-1.5 text-xs leading-5"
                              >
                                {t("clientMarket.shop.notIdle", { status: t(onlineStatusKey(status)) })}
                              </p>
                            ) : null}
                            {pickedId === listing.installationId && dialogError && !quote ? (
                              <p role="alert" className="text-xs text-danger">
                                {dialogError}
                              </p>
                            ) : null}
                            <ClientTerminationHostAction
                              summary={terminations.get(listing.installationId)}
                              onOpen={onTerminationOpen}
                            />
                          </>
                        }
                      />
                    );
                  })}
                </div>
              )}
            </section>
          ) : null}
        </>
      )}

      <Modal.Backdrop
        isOpen={!!selection}
        onOpenChange={(v) => {
          if (!v && !busy && !topupBusy) close();
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(460px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>
                {t(selected?.owned ? "online.unlist" : "online.rent")} · {selected?.label}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[75dvh] gap-3 overflow-auto">
              {selected?.owned ? (
                <p>{t("online.unlistNotice")}</p>
              ) : (
                selected && (
                  <>
                    <strong>{money(quote?.terms.cyclePriceMinor ?? selected.cyclePriceMinor)}</strong>
                    {!!quote?.terms.trialSeconds && (
                      <p>
                        {t("online.trialHours")} · {trialDuration(quote.terms.trialSeconds, locale)}
                      </p>
                    )}
                    {freeDays && (
                      <p>
                        {t("online.freeDuration")} · {freeDays}
                      </p>
                    )}
                    {paidQuote && (
                      <p>
                        {t("clientTermination.quote", { rate: quote.terms.terminationPolicy.rateBps / 100 })}
                      </p>
                    )}
                    {(paidQuote || selected.cyclePriceMinor > 0) && (
                      <label>
                        <input
                          type="checkbox"
                          checked={auto}
                          disabled={busy || uncertain || topupBusy}
                          onChange={(e) => {
                            const renew = e.target.checked;
                            setAuto(renew);
                            setQuote(null);
                            setAck(false);
                            void action(() => requote(selected, renew));
                          }}
                        />{" "}
                        {t("online.renew")}
                      </label>
                    )}
                    {paidQuote &&
                      (funding && validPaidWalletQuoteFunding(funding, quote.terms.cyclePriceMinor) ? (
                        <>
                          <p>
                            {t("marketWallet.available")} ·{" "}
                            {formatWalletUnits(funding.availableUnits, "$", locale)}
                          </p>
                          {!funded && (
                            <InPlaceTopup
                              active
                              purpose={t("marketWallet.shortfall", {
                                amount: formatWalletUnits(funding.shortfallUnits, "$", locale),
                              })}
                              requiredUnits={funding.requiredUnits}
                              suggestedAmount={suggestedTopupAmount(funding.shortfallUnits)}
                              idPrefix="client-rent-topup"
                              onBusyChange={setTopupBusy}
                              onFunded={() => void action(() => requote(selected, auto, true))}
                            />
                          )}
                        </>
                      ) : (
                        <p role="alert">{t("marketWallet.shortcutUnavailable")}</p>
                      ))}
                    {quote && (
                      <p>
                        {t("online.quoteUntil")} {date(quote.expiresAt)}
                      </p>
                    )}
                    <label>
                      <input
                        type="checkbox"
                        checked={ack}
                        disabled={!quote || uncertain || busy}
                        onChange={(e) => setAck(e.target.checked)}
                      />{" "}
                      {t("online.rootNotice")}
                    </label>
                    {accessRequired && (
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
                    )}
                  </>
                )
              )}
              {dialogError && (
                <p role="alert" className="text-danger">
                  {dialogError}
                </p>
              )}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={busy || topupBusy} onPress={close}>
                {t("common.cancel")}
              </Button>
              <Button
                isDisabled={
                  busy ||
                  topupBusy ||
                  (!selected?.owned && ((!!quote && funded && !ack) || !selected || accessRequired))
                }
                onPress={() =>
                  void action(async () => {
                    if (!selected) return;
                    if (selected.owned) {
                      await onlineRequest("installations/" + selected.installationId + "/listing", "DELETE");
                      close();
                      return;
                    }
                    if (!quote || !funded) {
                      await requote(selected, auto, true);
                      return;
                    }
                    attempted.current = quote.id;
                    rememberOrder(quote.id);
                    try {
                      const receipt = await onlineRequest<{ leaseId: string }>(
                        "quotes/" + quote.id + "/commit",
                        "POST",
                        { acknowledged: ack },
                      );
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
                  })
                }
              >
                {t(
                  selected?.owned
                    ? "online.unlist"
                    : uncertain
                      ? "common.retry"
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
