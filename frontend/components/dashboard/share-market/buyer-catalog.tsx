"use client";

import * as React from "react";
import { Button, Drawer, Modal } from "@heroui/react";
import {
  CalendarClock,
  Check,
  ChevronDown,
  Clock3,
  Filter,
  Info,
  Loader2,
  LogIn,
  MessageCircle,
  RefreshCw,
  Search,
  Send,
  ShoppingCart,
  UserRound,
  X,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientChat } from "@/components/chat/client-chat";
import { CompactSelect } from "@/components/common/compact-select";
import { SegmentedControl } from "@/components/common/segmented-control";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { ProviderContactsList } from "@/components/common/provider-contacts";
import {
  MarketAccessDialog,
  marketEligibilityFromError,
} from "@/components/common/seller-approval-dialog";
import {
  resolveShareProviderLogo,
  ShareProviderLogo,
} from "@/components/dashboard/share-provider-logo";
import { ShareProviderStatusPanel } from "@/components/dashboard/share-provider-status-panel";
import { SubdomainCopyButton } from "@/components/dashboard/subdomain-copy-button";
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError, quoteShareMarketSeat, rentShareMarketSeat } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type {
  MarketEligibility,
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketProviderFamily,
  ShareMarketRentQuote,
  ShareMarketSeat,
  ShareMarketSubscription,
} from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";
import { cn } from "@/lib/utils";
import {
  PROVIDER_FAMILY_KEYS,
  PROVIDER_FAMILY_ORDER,
  CORE_SHARE_APPS,
  activeSubscriptionForShare,
  formatSeatPrice,
  formatTokenLimit,
  isCoreShareApp,
  isSeatIdle,
  listingIdleCount,
  listingLowestDailyRate,
  marketProviderStatusView,
  shareMarketMutationError,
} from "@/components/dashboard/share-market/market-utils";
import {
  DEFAULT_CATALOG_AVAILABILITY,
  catalogSeatPreview,
  initialCatalogSeat,
  preserveCatalogSeat,
} from "@/components/dashboard/share-market/buyer-catalog-utils";

type SeatCard = { listing: ShareMarketListing; seat: ShareMarketSeat };
type SelectedListing = { listing: ShareMarketListing; seat?: ShareMarketSeat; requiredApp?: string };
type AvailabilityFilter = "idle" | "all";
type CatalogSort = "recommended" | "price" | "uptime";
type SeatAction = "rent" | "approval" | "login" | "rented" | "granting" | "selling" | "unavailable";
type RentTarget = SeatCard & { quote: ShareMarketRentQuote; idempotencyKey: string };

function rentalApps(listing: ShareMarketListing) {
  return CORE_SHARE_APPS.filter((app) => listing.supportedApps.includes(app));
}

function defaultRentalApp(listing: ShareMarketListing) {
  const apps = rentalApps(listing);
  return apps.includes(listing.appType as (typeof CORE_SHARE_APPS)[number])
    ? listing.appType
    : apps[0];
}

function compareReliability(left: ShareMarketListing, right: ShareMarketListing) {
  const coverage = Number(right.reliability.sufficientCoverage) - Number(left.reliability.sufficientCoverage);
  if (coverage) return coverage;
  const rate = (right.reliability.onlineRate24h ?? -1) - (left.reliability.onlineRate24h ?? -1);
  if (rate) return rate;
  return right.reliability.observationCoverage24h - left.reliability.observationCoverage24h;
}

function uptimeValue(listing: ShareMarketListing) {
  return listing.reliability.onlineRate24h == null
    ? "-"
    : `${listing.reliability.onlineRate24h.toFixed(1)}%`;
}

const RENT_BLOCK_REASON_KEYS: Record<string, MessageKey> = {
  owner: "shareMarket.blockReason.owner",
  already_renting: "shareMarket.blockReason.already_renting",
  direct_access: "shareMarket.blockReason.direct_access",
  login_required: "shareMarket.blockReason.login_required",
  share_unavailable: "shareMarket.blockReason.share_unavailable",
  share_offline: "shareMarket.blockReason.share_offline",
  seat_unavailable: "shareMarket.blockReason.seat_unavailable",
  approval_required: "shareMarket.blockReason.approval_required",
  access_required: "shareMarket.blockReason.access_required",
  credit_required: "shareMarket.blockReason.credit_required",
  buyer_restricted: "shareMarket.blockReason.buyer_restricted",
  settlement_required: "shareMarket.blockReason.settlement_required",
  credit_limit_reached: "shareMarket.blockReason.credit_limit_reached",
  relationship_closed: "shareMarket.blockReason.relationship_closed",
  unavailable: "shareMarket.blockReason.unavailable",
};

function rentBlockReasonKey(reason: string): MessageKey {
  return RENT_BLOCK_REASON_KEYS[reason] || "shareMarket.blockReason.unavailable";
}

function seatAction(
  listing: ShareMarketListing,
  seat: ShareMarketSeat,
  subscriptions: ShareMarketSubscription[],
  authed: boolean,
): SeatAction {
  if (listing.isOwner) return "selling";
  const mine = activeSubscriptionForShare(subscriptions, listing.shareId);
  if (mine) return mine.status === "grant_pending" ? "granting" : "rented";
  if (!authed) return "login";
  if (seat.canRent) return "rent";
  if (seat.rentPrerequisitesMet && !seat.eligibility.allowed) return "approval";
  return "unavailable";
}

function Metric({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="min-w-0">
      <dt className="truncate text-[11px] text-muted-foreground">{label}</dt>
      <dd className="truncate text-[11px] font-semibold tabular-nums text-foreground" title={title}>{value}</dd>
    </div>
  );
}

function ListingProviderLogos({ listing }: { listing: ShareMarketListing }) {
  const enabledApps = new Set(listing.supportedApps);
  const capabilities = listing.appCapabilities.filter(
    (item) => isCoreShareApp(item.app) && enabledApps.has(item.app),
  );
  const entries = capabilities.reduce<typeof capabilities>((result, capability) => {
    const logo = resolveShareProviderLogo(capability);
    const key = logo?.key || capability.providerType || capability.providerName || capability.app;
    if (!result.some((item) => {
      const itemLogo = resolveShareProviderLogo(item);
      return (itemLogo?.key || item.providerType || item.providerName || item.app) === key;
    })) result.push(capability);
    return result;
  }, []);
  return (
    <span className="inline-flex shrink-0 items-center gap-1">
      {entries.slice(0, 3).map((capability) => (
        <ShareProviderLogo
          key={`${capability.app}:${capability.providerType || capability.providerName || "provider"}`}
          provider={capability}
          fallbackApp={capability.app as "claude" | "codex" | "gemini"}
          size={16}
        />
      ))}
    </span>
  );
}

function compactSeatTerms(seat: ShareMarketSeat, locale: string, unlimited: string, t: ReturnType<typeof useLocaleText>["t"]) {
  const parallel = seat.parallelLimit == null ? "P∞" : `P${seat.parallelLimit}`;
  const tokens = seat.tokenLimit == null
    ? unlimited
    : `${formatTokenMillions(seat.tokenLimit, locale)}/${t(`shareMarket.period.${seat.tokenPeriod}`)}`;
  return `${parallel} · ${tokens}`;
}

function SeatPreview({ seat, onSelect }: { seat: ShareMarketSeat; onSelect: () => void }) {
  const { locale, t } = useLocaleText();
  return (
    <button
      type="button"
      className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded px-1.5 py-1 text-left text-[11px] hover:bg-slate-50 active:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
      onClick={onSelect}
    >
      <span className="font-semibold text-slate-700">#{seat.position}</span>
      <span className="truncate text-slate-500">{compactSeatTerms(seat, locale, t("common.unlimited"), t)}</span>
      <strong className="shrink-0 tabular-nums text-slate-800">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</strong>
    </button>
  );
}

function ListingCard({ listing, focused, onOpen }: { listing: ShareMarketListing; focused: boolean; onOpen: (seat?: ShareMarketSeat) => void }) {
  const { locale, t } = useLocaleText();
  const idle = listing.seats.filter(isSeatIdle);
  const preview = catalogSeatPreview(listing.seats);
  const ttft = listing.performance.averageTtftMs == null ? "-" : `${(listing.performance.averageTtftMs / 1_000).toFixed(2)}s`;
  const tps = listing.performance.averageTps == null ? "-" : listing.performance.averageTps.toFixed(1);
  const providerView = marketProviderStatusView(listing, locale, {
    unknown: t("shareMarket.catalog.providerUnknown"),
    passthrough: t("shareMarket.modelPassthrough"),
  });
  const subdomain = listing.subdomain?.trim() || listing.shareName;
  return (
    <article
      id={`share-market-catalog-${listing.shareId}`}
      className={cn(
        "grid min-h-[15rem] min-w-0 scroll-mt-20 grid-rows-[auto_auto_auto_1fr] gap-2.5 rounded-xl border bg-white p-3 shadow-sm transition-[border-color,box-shadow] hover:border-primary/35",
        focused
          ? "border-primary ring-2 ring-primary/20"
          : listing.shareOnline
            ? "border-slate-200"
            : "border-rose-200",
      )}
    >
      <header className="flex min-w-0 items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <span
            className={cn("h-2 w-2 shrink-0 rounded-full", listing.shareOnline ? "bg-emerald-500" : "bg-rose-500")}
            title={listing.shareOnline ? listing.shareStatus : t("shareMarket.blockReason.share_offline")}
          />
          <ListingProviderLogos listing={listing} />
          <button type="button" className="min-w-0 rounded text-left active:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" onClick={() => onOpen()}>
            <strong className="block truncate font-mono text-xs font-semibold text-foreground" title={subdomain}>{subdomain}</strong>
          </button>
          {listing.subdomain ? <SubdomainCopyButton subdomain={listing.subdomain} /> : null}
        </div>
        <button type="button" className="shrink-0 rounded text-right active:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" onClick={() => onOpen()}>
          <strong className="whitespace-nowrap text-[11px] font-semibold tabular-nums text-slate-700">
            {t("shareMarket.catalog.occupancy", { idle: idle.length, total: listing.seats.length })}
          </strong>
        </button>
      </header>
      <ShareProviderStatusPanel view={providerView} wrapPrimaryLine />
      <div className="min-w-0">
        <dl className="grid grid-cols-2 gap-2">
          <Metric
            label={t("shareMarket.catalog.uptime24h")}
            value={uptimeValue(listing)}
            title={`${t("shareMarket.catalog.coverage24hValue", { value: listing.reliability.observationCoverage24h.toFixed(1) })} · ${t("shareMarket.catalog.observedMinutesValue", { count: listing.reliability.observedMinutes24h })}`}
          />
          <Metric
            label="P50 TTFT/TPS"
            value={`${ttft} / ${tps === "-" ? "-" : `${tps} tok/s`}`}
            title={`${t("shareMarket.catalog.samplesValue", { count: listing.performance.ttftSampleCount })} · ${t("shareMarket.catalog.samplesValue", { count: listing.performance.tpsSampleCount })}`}
          />
        </dl>
        <p className="mt-1 flex min-w-0 items-start gap-1 text-[10px] leading-4 text-slate-500">
          <UserRound className="mt-0.5 h-3 w-3 shrink-0" aria-hidden />
          <span className="shrink-0">{t("shareMarket.owner")}:</span>
          <span className="min-w-0 break-all" title={listing.ownerEmail}>{listing.ownerEmail}</span>
        </p>
      </div>
      <div className="grid content-start gap-0.5 border-t border-slate-100 pt-1.5">
        {preview.map((seat) => <SeatPreview key={seat.id} seat={seat} onSelect={() => onOpen(seat)} />)}
        {idle.length > preview.length ? (
          <button type="button" className="rounded px-1.5 pt-1 text-left text-[10px] font-medium text-accent hover:underline active:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" onClick={() => onOpen()}>
            {t("shareMarket.catalog.moreSeats", { count: idle.length - preview.length })}
          </button>
        ) : null}
        {!idle.length ? <button type="button" className="rounded px-1.5 py-2 text-left text-xs text-slate-500 active:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" onClick={() => onOpen()}>{t("shareMarket.catalog.full")}</button> : null}
      </div>
    </article>
  );
}

function SeatChoice({ seat, selected, onSelect }: { seat: ShareMarketSeat; selected: boolean; onSelect: () => void }) {
  const { locale, t } = useLocaleText();
  const idle = isSeatIdle(seat);
  return (
    <button
      type="button"
      disabled={!idle}
      className={cn(
        "grid w-full grid-cols-[auto_minmax(0,1fr)_auto] gap-3 rounded-lg border p-3 text-left transition-colors active:bg-primary/10 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
        selected ? "border-primary bg-primary/5 ring-1 ring-primary/20" : "border-slate-200",
        idle ? "hover:border-primary/40" : "cursor-not-allowed bg-slate-50 opacity-60",
      )}
      onClick={onSelect}
    >
      <span className="grid h-7 w-7 place-items-center rounded-full bg-white text-xs font-bold text-slate-700 ring-1 ring-slate-200">{seat.position}</span>
      <span className="min-w-0 text-xs">
        <strong className={idle ? "text-emerald-700" : "text-slate-500"}>{idle ? t("shareMarket.available") : t("shareMarket.occupied")}</strong>
        <span className="mt-1 block text-slate-600">
          {t("shareMarket.parallelShort", { value: seat.parallelLimit == null ? "∞" : seat.parallelLimit })}
          {" · "}
          {formatTokenLimit(seat, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`))}
        </span>
        <span className="mt-0.5 block text-slate-500">
          {seat.serviceDurationDays == null ? t("shareMarket.serviceDuration.permanent") : t("shareMarket.serviceDuration.daysValue", { count: seat.serviceDurationDays })}
        </span>
      </span>
      <strong className="shrink-0 text-sm tabular-nums">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</strong>
    </button>
  );
}

function actionLabel(action: SeatAction, t: ReturnType<typeof useLocaleText>["t"]) {
  if (action === "rent") return t("shareMarket.rentSelected");
  if (action === "approval") return t("marketApproval.apply");
  if (action === "login") return t("nav.login");
  if (action === "rented") return t("shareMarket.catalog.rented");
  if (action === "granting") return t("shareMarket.catalog.granting");
  if (action === "selling") return t("shareMarket.workspace.selling");
  return t("shareMarket.unavailable");
}

export function ShareMarketBuyerCatalog({
  catalog,
  subscriptions,
  authed,
  focusedShareId,
  onChanged,
  onInteractionChange,
  onSwitchSelling,
}: {
  catalog: ShareMarketCatalog;
  subscriptions: ShareMarketSubscription[];
  authed: boolean;
  focusedShareId?: string;
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
  onSwitchSelling?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const chat = useClientChat();
  const [query, setQuery] = React.useState("");
  const [family, setFamily] = React.useState<ShareMarketProviderFamily | "all">("all");
  const [availability, setAvailability] = React.useState<AvailabilityFilter>(DEFAULT_CATALOG_AVAILABILITY);
  const [sort, setSort] = React.useState<CatalogSort>("recommended");
  const [selected, setSelected] = React.useState<SelectedListing | null>(null);
  const [rentTarget, setRentTarget] = React.useState<RentTarget | null>(null);
  const [accessTarget, setAccessTarget] = React.useState<(SeatCard & { eligibility: MarketEligibility }) | null>(null);
  const [busySeatId, setBusySeatId] = React.useState("");
  const [error, setError] = React.useState("");
  const [quoteNowMs, setQuoteNowMs] = React.useState(() => Date.now());
  const focusedRef = React.useRef("");
  const blocking = !!rentTarget || !!accessTarget || !!busySeatId;

  React.useEffect(() => {
    onInteractionChange?.(blocking);
    return () => onInteractionChange?.(false);
  }, [blocking, onInteractionChange]);

  React.useEffect(() => {
    if (!rentTarget) return;
    setQuoteNowMs(Date.now());
    const timer = window.setInterval(() => setQuoteNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [rentTarget]);

  const listings = React.useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return catalog.listings
      .filter((listing) => family === "all" || listing.providerFamily === family || listing.providerFamilies.includes(family))
      .filter((listing) => availability === "all" || listingIdleCount(listing) > 0)
      .filter((listing) => {
        if (!needle) return true;
        return [
          listing.shareName,
          listing.subdomain,
          listing.ownerEmail,
          ...listing.supportedApps,
          ...listing.appCapabilities.flatMap((item) => [item.providerName, item.providerType, item.subscriptionLevel, item.upstreamModel, ...item.models]),
        ].filter(Boolean).join(" ").toLocaleLowerCase().includes(needle);
      })
      .sort((left, right) => {
        const online = Number(right.shareOnline) - Number(left.shareOnline);
        if (online) return online;
        if (sort === "price") return listingLowestDailyRate(left) - listingLowestDailyRate(right) || left.shareName.localeCompare(right.shareName);
        if (sort === "uptime") return compareReliability(left, right) || left.shareName.localeCompare(right.shareName);
        return listingIdleCount(right) - listingIdleCount(left) || compareReliability(left, right) || listingLowestDailyRate(left) - listingLowestDailyRate(right);
      });
  }, [availability, catalog.listings, family, query, sort]);

  React.useEffect(() => {
    if (!selected) return;
    const listing = catalog.listings.find((item) => item.id === selected.listing.id);
    if (!listing) return setSelected(null);
    const seat = selected.seat ? preserveCatalogSeat(listing.seats, selected.seat.id) : undefined;
    const apps = rentalApps(listing);
    const requiredApp = selected.requiredApp && apps.includes(selected.requiredApp as (typeof CORE_SHARE_APPS)[number])
      ? selected.requiredApp
      : defaultRentalApp(listing);
    if (listing !== selected.listing || seat !== selected.seat || requiredApp !== selected.requiredApp) {
      setSelected({ listing, seat, requiredApp });
    }
  }, [catalog.listings, selected]);

  React.useEffect(() => {
    if (!focusedShareId || focusedRef.current === focusedShareId) return;
    const listing = catalog.listings.find((item) => item.shareId === focusedShareId);
    if (!listing) return;
    focusedRef.current = focusedShareId;
    setAvailability("all");
    setSelected({ listing, requiredApp: defaultRentalApp(listing) });
    window.requestAnimationFrame(() => document.getElementById(`share-market-catalog-${focusedShareId}`)?.scrollIntoView({ block: "start" }));
  }, [catalog.listings, focusedShareId]);

  const openListing = (listing: ShareMarketListing, seat?: ShareMarketSeat) => {
    setError("");
    setSelected({
      listing,
      seat: initialCatalogSeat(listing.seats, seat),
      requiredApp: defaultRentalApp(listing),
    });
  };

  const triggerSeat = async (item: SeatCard, requiredApp?: string) => {
    const action = seatAction(item.listing, item.seat, subscriptions, authed);
    if (action === "login") return window.dispatchEvent(new Event("router-open-login"));
    if (action === "selling") return onSwitchSelling?.();
    if (action === "rented" || action === "granting") return;
    if (action === "approval") return setAccessTarget({ ...item, eligibility: item.seat.eligibility });
    if (action !== "rent" || busySeatId) return;
    setBusySeatId(item.seat.id);
    setError("");
    try {
      const quote = await quoteShareMarketSeat(item.seat.id, requiredApp);
      setRentTarget({ ...item, quote, idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}` });
    } catch (reason) {
      const eligibility = marketEligibilityFromError(reason);
      if (eligibility) setAccessTarget({ ...item, eligibility });
      else setError(shareMarketMutationError(reason, t));
    } finally {
      setBusySeatId("");
    }
  };

  const refreshQuote = async () => {
    if (!rentTarget || busySeatId) return;
    setBusySeatId(rentTarget.seat.id);
    setError("");
    try {
      const quote = await quoteShareMarketSeat(
        rentTarget.seat.id,
        rentTarget.quote.offer.service.requiredApp,
      );
      setRentTarget({
        listing: rentTarget.listing,
        seat: rentTarget.seat,
        quote,
        idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}`,
      });
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusySeatId("");
    }
  };

  const confirmRent = async () => {
    if (!rentTarget || busySeatId) return;
    if (Date.parse(rentTarget.quote.expiresAt) <= Date.now()) {
      await refreshQuote();
      return;
    }
    setBusySeatId(rentTarget.seat.id);
    setError("");
    try {
      await rentShareMarketSeat(rentTarget.seat.id, rentTarget.quote.id, rentTarget.idempotencyKey);
      setRentTarget(null);
      await onChanged();
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 410) {
        setQuoteNowMs(Date.parse(rentTarget.quote.expiresAt));
        setError(t("shareMarket.rentConfirm.expired"));
      } else {
        setError(shareMarketMutationError(reason, t));
      }
    } finally {
      setBusySeatId("");
    }
  };

  const selectedAction = selected?.seat
    ? seatAction(selected.listing, selected.seat, subscriptions, authed)
    : "unavailable";
  const activeFilterCount = Number(family !== "all") + Number(availability !== "idle") + Number(sort !== "recommended");
  const selectedApps = selected ? rentalApps(selected.listing) : [];
  const quoteRemainingSeconds = rentTarget
    ? Math.max(0, Math.ceil((Date.parse(rentTarget.quote.expiresAt) - quoteNowMs) / 1_000))
    : 0;
  const quoteExpired = !!rentTarget && quoteRemainingSeconds <= 0;

  return (
    <div className="grid min-w-0 gap-4">
      <div className="flex min-w-0 items-center gap-2">
        <label className="flex h-10 min-w-0 flex-1 items-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm shadow-sm hover:border-slate-300 focus-within:outline focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-primary">
          <Search className="h-4 w-4 shrink-0 text-slate-400" />
          <input aria-label={t("shareMarket.catalog.searchCompact")} value={query} onChange={(event) => setQuery(event.target.value)} className="min-w-0 flex-1 bg-transparent outline-none" placeholder={t("shareMarket.catalog.searchCompact")} />
          {query ? <button type="button" aria-label={t("common.reset")} className="rounded active:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" onClick={() => setQuery("")}><X className="h-4 w-4 text-slate-400" /></button> : null}
        </label>
        <details className="group relative">
          <summary className="flex h-10 cursor-pointer list-none items-center gap-1.5 whitespace-nowrap rounded-md border border-slate-200 bg-white px-3 text-sm font-medium text-slate-700 shadow-sm active:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary">
            <Filter className="h-4 w-4" />{t("shareMarket.catalog.filters")}{activeFilterCount ? ` ${activeFilterCount}` : ""}<ChevronDown className="h-3.5 w-3.5 group-open:rotate-180" />
          </summary>
          <div className="absolute right-0 z-30 mt-2 grid w-72 gap-3 rounded-lg border border-slate-200 bg-white p-3 shadow-xl">
            <CompactSelect value={family} options={[{ value: "all", label: t("shareMarket.catalog.allFamilies") }, ...PROVIDER_FAMILY_ORDER.map((value) => ({ value, label: t(PROVIDER_FAMILY_KEYS[value]) }))]} onChange={(value) => setFamily(value as ShareMarketProviderFamily | "all")} ariaLabel={t("shareMarket.catalog.familyFilter")} className="w-full" />
            <label className="flex items-center justify-between text-sm text-slate-700">
              {t("shareMarket.catalog.onlyRentable")}
              <input type="checkbox" checked={availability === "idle"} onChange={(event) => setAvailability(event.target.checked ? "idle" : "all")} />
            </label>
            <div className="border-t border-slate-100 pt-3">
              <CompactSelect
                value={sort}
                options={[
                  { value: "recommended", label: t("shareMarket.catalog.sort.recommended") },
                  { value: "price", label: t("shareMarket.catalog.sort.price") },
                  { value: "uptime", label: t("shareMarket.catalog.sort.uptime") },
                ]}
                onChange={(value) => setSort(value as CatalogSort)}
                ariaLabel={t("shareMarket.catalog.sort")}
                className="w-full"
              />
            </div>
          </div>
        </details>
      </div>

      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}
      <div className="grid min-w-0 grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
        {listings.map((listing) => <ListingCard key={listing.id} listing={listing} focused={focusedShareId === listing.shareId} onOpen={(seat) => openListing(listing, seat)} />)}
      </div>
      {!listings.length ? <div className="grid min-h-48 place-items-center border-y border-dashed border-slate-200 text-sm text-slate-500">{t("shareMarket.catalog.empty")}</div> : null}

      <Drawer.Backdrop isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Drawer.Content placement="right">
          <Drawer.Dialog className={drawerDialogClassName}>
            <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" />
            <Drawer.Header>
              <div className="min-w-0 pr-10">
                <Drawer.Heading className="truncate text-base">{selected?.listing.shareName}</Drawer.Heading>
                <p className="mt-1 truncate font-mono text-xs text-slate-500">{selected?.listing.subdomain}</p>
              </div>
            </Drawer.Header>
            <Drawer.Body className="overflow-y-auto pb-28">
              {selected ? (
                <div className="grid gap-5">
                  {selectedApps.length > 1 ? (
                    <section className="grid gap-2">
                      <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.chooseApp")}</h3>
                      <SegmentedControl
                        value={selected.requiredApp || selectedApps[0]}
                        onChange={(requiredApp) => setSelected({ ...selected, requiredApp })}
                        items={selectedApps.map((app) => ({ id: app, label: SHARE_APP_LABELS[app] }))}
                        ariaLabel={t("shareMarket.catalog.chooseApp")}
                        fullWidth
                      />
                    </section>
                  ) : null}
                  <section className="grid gap-3">
                    <div className="flex items-center justify-between">
                      <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.chooseSeat")}</h3>
                      <span className="text-xs text-slate-400">{t("shareMarket.catalog.idleSeats", { count: listingIdleCount(selected.listing) })}</span>
                    </div>
                    {selected.listing.seats.map((seat) => <SeatChoice key={seat.id} seat={seat} selected={selected.seat?.id === seat.id} onSelect={() => setSelected({ ...selected, seat })} />)}
                  </section>
                  <section className="grid gap-3 border-t border-slate-200 pt-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.providerStatus")}</h3>
                    <ShareProviderStatusPanel
                      view={marketProviderStatusView(selected.listing, locale, {
                        unknown: t("shareMarket.catalog.providerUnknown"),
                        passthrough: t("shareMarket.modelPassthrough"),
                      })}
                    />
                  </section>
                  <section className="grid gap-3 border-t border-slate-200 pt-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.serviceQuality")}</h3>
                    <dl className="grid grid-cols-3 gap-3">
                      <Metric label="TTFT" value={selected.listing.performance.averageTtftMs == null ? "-" : `${(selected.listing.performance.averageTtftMs / 1_000).toFixed(2)}s`} />
                      <Metric label="TPS" value={selected.listing.performance.averageTps == null ? "-" : selected.listing.performance.averageTps.toFixed(1)} />
                      <Metric label={t("shareMarket.catalog.uptime24h")} value={uptimeValue(selected.listing)} title={t("shareMarket.catalog.coverage24hValue", { value: selected.listing.reliability.observationCoverage24h.toFixed(1) })} />
                    </dl>
                    <p className={cn("text-xs", selected.listing.reliability.sufficientCoverage ? "text-slate-500" : "text-amber-700")}>
                      {selected.listing.reliability.sufficientCoverage
                        ? t("shareMarket.catalog.observedMinutesValue", { count: selected.listing.reliability.observedMinutes24h })
                        : t("shareMarket.catalog.coverageInsufficient", { count: selected.listing.reliability.observedMinutes24h })}
                    </p>
                  </section>
                  <section className="grid gap-3 border-t border-slate-200 pt-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.seller")}</h3>
                    <div className="flex min-w-0 items-center gap-2 text-sm"><UserRound className="h-4 w-4 text-slate-400" /><span className="min-w-0 break-all font-medium">{selected.listing.ownerEmail}</span><PaymentMethodIcons kinds={selected.listing.paymentMethodKinds} /></div>
                    <ProviderContactsList contacts={selected.listing.contacts} />
                    <Button variant="outline" onClick={() => void chat.openClientChat(selected.listing.installationId)}><MessageCircle className="h-4 w-4" />{t("marketApproval.openChat")}</Button>
                  </section>
                  {error ? <p className="text-sm text-rose-700">{error}</p> : null}
                </div>
              ) : null}
            </Drawer.Body>
            {selected ? (
              <div className="absolute inset-x-0 bottom-0 flex items-center justify-between gap-3 border-t border-slate-200 bg-white px-5 py-4">
                <div className="min-w-0">
                  <strong className="block truncate text-sm">{selected.seat ? `${t("shareMarket.catalog.seatPosition", { position: selected.seat.position })} · ${formatSeatPrice(selected.seat, locale, t("shareMarket.free"), t("marketBilling.day"))}` : t("shareMarket.catalog.selectSeatFirst")}</strong>
                  <span className="text-xs text-slate-500">{selected.seat?.rentBlockReason ? t(rentBlockReasonKey(selected.seat.rentBlockReason)) : t("shareMarket.catalog.frozenTermsHint")}</span>
                </div>
                <Button
                  variant="primary"
                  isDisabled={!selected.seat || selectedAction === "unavailable" || selectedAction === "rented" || selectedAction === "granting" || !!busySeatId}
                  onClick={() => selected.seat && void triggerSeat(
                    { listing: selected.listing, seat: selected.seat },
                    selected.requiredApp,
                  )}
                >
                  {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : selectedAction === "approval" ? <Send className="h-4 w-4" /> : selectedAction === "login" ? <LogIn className="h-4 w-4" /> : selectedAction === "rented" ? <Check className="h-4 w-4" /> : <ShoppingCart className="h-4 w-4" />}
                  {selected.seat ? actionLabel(selectedAction, t) : t("shareMarket.catalog.chooseSeat")}
                </Button>
              </div>
            ) : null}
          </Drawer.Dialog>
        </Drawer.Content>
      </Drawer.Backdrop>

      <Modal.Backdrop isOpen={!!rentTarget} onOpenChange={(open) => !open && !busySeatId && setRentTarget(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(580px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("shareMarket.rentConfirm.title")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid gap-3">
              {rentTarget ? (
                <>
                  <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 border-y border-slate-200 py-3 text-sm">
                    <dt className="text-slate-500">{t("shareMarket.col.share")}</dt><dd className="font-medium">{rentTarget.quote.offer.shareName} · #{rentTarget.quote.offer.seatPosition}</dd>
                    <dt className="text-slate-500">{t("shareMarket.catalog.requiredApp")}</dt><dd className="font-medium">{isCoreShareApp(rentTarget.quote.offer.service.requiredApp) ? SHARE_APP_LABELS[rentTarget.quote.offer.service.requiredApp] : rentTarget.quote.offer.service.requiredApp}</dd>
                    <dt className="text-slate-500">{t("shareMarket.catalog.provider")}</dt><dd>{t(PROVIDER_FAMILY_KEYS[rentTarget.quote.offer.service.providerFamily])}{rentTarget.quote.offer.service.providerType ? ` · ${rentTarget.quote.offer.service.providerType}` : ""}</dd>
                    <dt className="text-slate-500">{t("shareMarket.catalog.modelPolicy")}</dt><dd>{rentTarget.quote.offer.service.modelMode === "passthrough" ? t("shareMarket.modelPassthrough") : rentTarget.quote.offer.service.upstreamModel || rentTarget.quote.offer.service.models.join(" / ") || t("shareMarket.catalog.modelUnknown")}</dd>
                    <dt className="text-slate-500">{t("shareMarket.parallel")}</dt><dd>{rentTarget.quote.offer.parallelLimit == null ? t("common.unlimited") : rentTarget.quote.offer.parallelLimit}</dd>
                    <dt className="text-slate-500">{t("shareMarket.tokens")}</dt><dd>{formatTokenLimit(rentTarget.quote.offer, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`))}</dd>
                    <dt className="text-slate-500">{t("shareMarket.catalog.shareCapacity")}</dt><dd>{rentTarget.quote.offer.service.shareParallelLimit == null ? t("common.unlimited") : t("shareMarket.parallelShort", { value: rentTarget.quote.offer.service.shareParallelLimit })}</dd>
                    <dt className="text-slate-500">{t("shareMarket.catalog.shareTokens")}</dt><dd>{rentTarget.quote.offer.service.shareTokenLimit == null ? t("common.unlimited") : `${formatTokenMillions(rentTarget.quote.offer.service.shareTokensUsed, locale)} / ${formatTokenMillions(rentTarget.quote.offer.service.shareTokenLimit, locale)}`}</dd>
                    <dt className="text-slate-500">{t("shareMarket.catalog.serviceTerm")}</dt><dd>{rentTarget.quote.offer.serviceDurationDays == null ? t("shareMarket.serviceDuration.permanent") : t("shareMarket.serviceDuration.daysValue", { count: rentTarget.quote.offer.serviceDurationDays })}</dd>
                    <dt className="text-slate-500">{t("shareMarket.dialog.amount")}</dt><dd className="font-medium">{formatSeatPrice({ isFree: rentTarget.quote.offer.dailyRateMinor == null, dailyRateMinor: rentTarget.quote.offer.dailyRateMinor }, locale, t("shareMarket.free"), t("marketBilling.day"))}</dd>
                  </dl>
                  <div className="flex gap-3 border-l-2 border-sky-400 bg-sky-50 px-3 py-2 text-sm leading-6 text-sky-950"><Clock3 className="mt-1 h-4 w-4 shrink-0" /><div><p>{rentTarget.quote.offer.dailyRateMinor == null ? t("shareMarket.rentConfirm.freeBilling") : t("shareMarket.rentConfirm.postpaid", { hours: catalog.trialHours })}</p>{rentTarget.quote.offer.dailyRateMinor != null ? <strong className="mt-1 block text-xs">{rentTarget.quote.trialSecondsRemaining > 0 ? t("shareMarket.rentConfirm.remainingTrial", { hours: (rentTarget.quote.trialSecondsRemaining / 3600).toFixed(1) }) : t("shareMarket.rentConfirm.noTrial")}</strong> : null}</div></div>
                  <div className="flex gap-3 border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-sm leading-6 text-amber-950"><CalendarClock className="mt-1 h-4 w-4 shrink-0" /><p>{rentTarget.quote.offer.serviceDurationDays == null ? t("shareMarket.rentConfirm.servicePermanent") : t("shareMarket.rentConfirm.serviceFixed", { days: rentTarget.quote.offer.serviceDurationDays })}</p></div>
                  <p className={cn("flex items-start gap-2 text-xs leading-5", quoteExpired ? "text-rose-700" : "text-slate-500")} title={t("shareMarket.rentConfirm.quoteExpiry", { time: new Intl.DateTimeFormat(locale, { timeStyle: "medium" }).format(new Date(rentTarget.quote.expiresAt)) })}><Info className="mt-0.5 h-3.5 w-3.5 shrink-0" />{quoteExpired ? t("shareMarket.rentConfirm.expired") : t("shareMarket.rentConfirm.expiresIn", { seconds: quoteRemainingSeconds })}</p>
                  {error ? <p className="text-sm text-rose-700">{error}</p> : null}
                </>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={!!busySeatId} onClick={() => setRentTarget(null)}>{t("common.cancel")}</Button>
              <Button variant="primary" isDisabled={!!busySeatId} onClick={() => void (quoteExpired ? refreshQuote() : confirmRent())}>{busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : quoteExpired ? <RefreshCw className="h-4 w-4" /> : <ShoppingCart className="h-4 w-4" />}{quoteExpired ? t("shareMarket.rentConfirm.requote") : t("shareMarket.rentConfirm.confirm")}</Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <MarketAccessDialog
        open={!!accessTarget}
        product="share"
        ownerEmail={accessTarget?.listing.ownerEmail || ""}
        buyerEmail={session?.user?.email || ""}
        contacts={accessTarget?.listing.contacts}
        targetKind="share_seat"
        targetId={accessTarget?.seat.id || ""}
        currency={accessTarget?.seat.currency}
        eligibility={accessTarget?.eligibility || { allowed: false, status: "access_required" }}
        onOpenChange={(open) => !open && setAccessTarget(null)}
        onOpenChat={() => accessTarget ? chat.openClientChat(accessTarget.listing.installationId) : undefined}
        onRequestChanged={onChanged}
      />
    </div>
  );
}
