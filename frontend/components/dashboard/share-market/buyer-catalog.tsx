"use client";

import * as React from "react";
import Link from "next/link";
import { Button, Drawer, Modal } from "@heroui/react";
import {
  CalendarClock,
  Check,
  Clock3,
  Gauge,
  Info,
  Loader2,
  LogIn,
  MessageCircle,
  PanelRightOpen,
  Search,
  Send,
  ShieldCheck,
  ShoppingCart,
  UserRound,
  Users,
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
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { rentShareMarketSeat } from "@/lib/api";
import { DASHBOARD_ACCOUNT_SHARE_PATH } from "@/lib/dashboard-nav";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type {
  MarketEligibility,
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketProviderFamily,
  ShareMarketSeat,
  ShareMarketSubscription,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  PROVIDER_FAMILY_KEYS,
  PROVIDER_FAMILY_ORDER,
  activeSubscriptionForShare,
  capabilityModelLabel,
  formatSeatPrice,
  formatTokenLimit,
  isCoreShareApp,
  isSeatIdle,
  listingIdleCount,
  listingLowestDailyRate,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";

type SeatCard = { listing: ShareMarketListing; seat: ShareMarketSeat };
type SeatAction = "rent" | "approval" | "login" | "rented" | "granting" | "selling" | "occupied" | "unavailable";
type AvailabilityFilter = "all" | "idle";
type CatalogSort = "idle" | "price" | "uptime";

function familyRank(family: ShareMarketProviderFamily | "all") {
  if (family === "all") return -1;
  const rank = PROVIDER_FAMILY_ORDER.indexOf(family);
  return rank < 0 ? PROVIDER_FAMILY_ORDER.length : rank;
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
  if (!isSeatIdle(seat)) return seat.subscription ? "occupied" : "unavailable";
  if (!listing.shareOnline) return "unavailable";
  if (!authed) return "login";
  if (seat.canRent) return "rent";
  if (seat.rentPrerequisitesMet && !seat.eligibility.allowed) return "approval";
  return "unavailable";
}

function listingAction(
  listing: ShareMarketListing,
  subscriptions: ShareMarketSubscription[],
  authed: boolean,
): SeatAction {
  if (listing.isOwner) return "selling";
  const mine = activeSubscriptionForShare(subscriptions, listing.shareId);
  if (mine) return mine.status === "grant_pending" ? "granting" : "rented";
  const idle = listing.seats.filter(isSeatIdle);
  if (!listing.shareOnline) return "unavailable";
  if (!idle.length) return "occupied";
  if (!authed) return "login";
  if (idle.some((seat) => seat.canRent)) return "rent";
  if (idle.some((seat) => seat.rentPrerequisitesMet && !seat.eligibility.allowed)) return "approval";
  return "unavailable";
}

function firstActionableSeat(
  listing: ShareMarketListing,
  subscriptions: ShareMarketSubscription[],
  authed: boolean,
) {
  const mine = activeSubscriptionForShare(subscriptions, listing.shareId);
  if (mine) {
    return listing.seats.find((seat) => seat.id === mine.seatId) || listing.seats[0];
  }
  const preferred = listing.seats
    .filter(isSeatIdle)
    .sort((left, right) => (left.dailyRateMinor ?? 0) - (right.dailyRateMinor ?? 0) || left.position - right.position);
  return preferred.find((seat) => {
    const action = seatAction(listing, seat, subscriptions, authed);
    return action === "rent" || action === "approval" || action === "login";
  }) || preferred[0] || listing.seats[0];
}

function AppLogos({ apps }: { apps: string[] }) {
  return (
    <span className="inline-flex items-center gap-1">
      {apps.filter(isCoreShareApp).map((app) => (
        <ShareAppLogo key={app} app={app} size={17} />
      ))}
    </span>
  );
}

function SeatActionButton({
  action,
  busy,
  onAction,
}: {
  action: SeatAction;
  busy: boolean;
  onAction: () => void;
}) {
  const { t } = useLocaleText();
  const config = {
    rent: { label: t("shareMarket.rent"), icon: ShoppingCart, variant: "primary" as const },
    approval: { label: t("marketApproval.apply"), icon: Send, variant: "primary" as const },
    login: { label: t("nav.login"), icon: LogIn, variant: "primary" as const },
    rented: { label: t("shareMarket.catalog.rented"), icon: Check, variant: "outline" as const },
    granting: { label: t("shareMarket.catalog.granting"), icon: Loader2, variant: "outline" as const },
    selling: { label: t("shareMarket.workspace.selling"), icon: Gauge, variant: "outline" as const },
    occupied: { label: t("shareMarket.occupied"), icon: Users, variant: "outline" as const },
    unavailable: { label: t("shareMarket.unavailable"), icon: ShieldCheck, variant: "outline" as const },
  }[action];
  const Icon = config.icon;
  return (
    <Button
      size="sm"
      variant={config.variant}
      isDisabled={busy || action === "unavailable" || action === "occupied"}
      onClick={onAction}
    >
      {busy || action === "granting" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Icon className="h-4 w-4" />}
      {config.label}
    </Button>
  );
}

function AppPolicies({ listing }: { listing: ShareMarketListing }) {
  const { t } = useLocaleText();
  return (
    <div className="grid gap-1.5 text-xs">
      {listing.appCapabilities.map((capability) => (
        <div key={capability.app} className="flex min-w-0 items-center gap-2">
          {isCoreShareApp(capability.app) ? <ShareAppLogo app={capability.app} size={14} /> : null}
          <span className="shrink-0 font-medium text-slate-700">
            {isCoreShareApp(capability.app) ? SHARE_APP_LABELS[capability.app] : capability.app}:
          </span>
          <span className="min-w-0 truncate text-slate-500" title={capabilityModelLabel(
            capability,
            t("shareMarket.modelPassthrough"),
            t("shareMarket.catalog.modelUnknown"),
          )}>
            {capabilityModelLabel(
              capability,
              t("shareMarket.modelPassthrough"),
              t("shareMarket.catalog.modelUnknown"),
            )}
          </span>
        </div>
      ))}
    </div>
  );
}

function Metric({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="min-w-0">
      <dt className="truncate text-[11px] text-slate-400">{label}</dt>
      <dd className="mt-0.5 truncate text-sm font-semibold tabular-nums text-slate-800" title={title}>{value}</dd>
    </div>
  );
}

function lowestIdlePrice(
  listing: ShareMarketListing,
  locale: string,
  freeLabel: string,
  dayLabel: string,
) {
  const idle = listing.seats.filter(isSeatIdle);
  const seats = idle.length ? idle : listing.seats;
  const cheapest = [...seats].sort((left, right) =>
    (left.dailyRateMinor ?? 0) - (right.dailyRateMinor ?? 0) || left.position - right.position,
  )[0];
  return cheapest ? formatSeatPrice(cheapest, locale, freeLabel, dayLabel) : freeLabel;
}

function riderLabel(seat: ShareMarketSeat, t: ReturnType<typeof useLocaleText>["t"]) {
  const email = seat.subscription?.renterEmail?.trim();
  const statusKey = subscriptionStatusKey(seat.subscription?.status || "");
  const status = statusKey ? t(statusKey) : "";
  if (email && status && seat.subscription?.status !== "active_free" && seat.subscription?.status !== "active_postpaid") {
    return `${email} · ${status}`;
  }
  if (email) return email;
  if (status) return status;
  if (seat.status === "occupied" || seat.status === "reserved" || seat.status === "revoking") {
    return t("shareMarket.catalog.anonymousRider");
  }
  return t("shareMarket.available");
}

function ListingCardView({
  listing,
  action,
  busy,
  trialHours,
  focused,
  onDetails,
  onAction,
}: {
  listing: ShareMarketListing;
  action: SeatAction;
  busy: boolean;
  trialHours: number;
  focused?: boolean;
  onDetails: () => void;
  onAction: () => void;
}) {
  const { locale, t } = useLocaleText();
  const performance = listing.performance;
  const reliability = listing.reliability;
  const idleCount = listingIdleCount(listing);
  const totalSeats = listing.seats.length;
  const occupancy = idleCount
    ? t("shareMarket.catalog.occupancy", { idle: idleCount, total: totalSeats })
    : t("shareMarket.catalog.full");
  const subscriptionLevels = [...new Set(
    listing.appCapabilities
      .map((capability) => capability.subscriptionLevel?.trim())
      .filter((value): value is string => !!value),
  )];
  const familyNames = [...new Set([listing.providerFamily, ...listing.providerFamilies])]
    .map((family) => t(PROVIDER_FAMILY_KEYS[family]));
  const multiProvider = familyNames.length > 1;
  const price = lowestIdlePrice(listing, locale, t("shareMarket.free"), t("marketBilling.day"));
  const paidIdle = listing.seats.some((seat) => isSeatIdle(seat) && !seat.isFree);
  const ttft = performance.averageTtftMs == null ? "-" : `${(performance.averageTtftMs / 1_000).toFixed(2)}s`;
  const tps = performance.averageTps == null ? "-" : performance.averageTps.toFixed(1);
  const footerHint = action === "approval"
    ? t("shareMarket.catalog.approvalRequired")
    : action === "rented" || action === "granting"
      ? t("shareMarket.catalog.alreadyRenting")
      : !listing.shareOnline
        ? t("shareMarket.catalog.offlineHint")
        : paidIdle
          ? t("shareMarket.catalog.postpaidHint")
          : t("shareMarket.free");
  return (
    <article
      id={`share-market-catalog-${listing.shareId}`}
      className={cn(
        "grid min-h-[23rem] min-w-0 scroll-mt-20 grid-rows-[auto_auto_auto_1fr_auto] gap-3 rounded-md border bg-white p-4 shadow-sm transition-colors hover:border-slate-300",
        focused ? "border-accent ring-1 ring-accent/30" : "border-slate-200",
        !listing.shareOnline && "opacity-80",
      )}
    >
      <header className="flex min-w-0 items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <strong className="truncate text-sm text-slate-900">
              {listing.shareName}
            </strong>
            {subscriptionLevels.length ? (
              <span className="truncate text-xs text-slate-500" title={subscriptionLevels.join(" / ")}>
                {subscriptionLevels.join(" / ")}
              </span>
            ) : null}
            {multiProvider ? (
              <span className="shrink-0 rounded-full bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium text-slate-600">
                {t("shareMarket.catalog.multiProviders")}
              </span>
            ) : null}
          </div>
          <p className="mt-1 truncate font-mono text-[11px] text-slate-400" title={listing.subdomain || listing.shareName}>
            {listing.subdomain || listing.shareName}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <span className={cn("text-[11px] font-medium", listing.shareOnline ? "text-emerald-700" : "text-rose-600")}>
            {listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
          </span>
          <AppLogos apps={listing.supportedApps} />
        </div>
      </header>

      <div className="min-w-0">
        <div className="flex items-baseline justify-between gap-3">
          <strong className="text-xl font-semibold text-slate-950">
            {idleCount && listing.seats.length > 1 ? t("shareMarket.catalog.fromPrice", { price }) : price}
          </strong>
          <span className={cn("shrink-0 text-xs font-medium", idleCount ? "text-emerald-700" : "text-slate-500")}>
            {occupancy}
          </span>
        </div>
        <p className="mt-1 text-xs text-slate-500">
          {paidIdle ? t("shareMarket.catalog.trial", { hours: trialHours }) : t("shareMarket.free")}
          {" · "}
          {familyNames.join(" / ")}
        </p>
      </div>

      <AppPolicies listing={listing} />

      <div className="grid content-start gap-3 border-y border-slate-100 py-3">
        <dl className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3">
          <Metric
            label="TTFT / TPS"
            value={`${ttft} / ${tps}`}
            title={t("shareMarket.catalog.performanceSamples", {
              ttft: performance.ttftSampleCount,
              tps: performance.tpsSampleCount,
            })}
          />
          <Metric label={t("shareMarket.catalog.uptime24h")} value={`${reliability.onlineRate24h.toFixed(1)}%`} />
          <Metric label={t("shareMarket.owner")} value={listing.ownerEmail} />
        </dl>
        <div className="grid gap-1.5">
          <p className="text-[11px] font-medium uppercase tracking-wide text-slate-400">
            {t("shareMarket.catalog.seatsAndRiders")}
          </p>
          {listing.seats.length ? listing.seats.map((seat) => {
            const idle = isSeatIdle(seat);
            return (
              <div key={seat.id} className="flex min-w-0 items-center justify-between gap-3 text-xs">
                <span className="shrink-0 font-medium text-slate-700">#{seat.position}</span>
                <span className={cn("min-w-0 truncate", idle ? "text-emerald-700" : "text-slate-500")} title={riderLabel(seat, t)}>
                  {idle ? t("shareMarket.available") : riderLabel(seat, t)}
                </span>
                <strong className="shrink-0 tabular-nums text-slate-800">
                  {formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}
                </strong>
              </div>
            );
          }) : (
            <p className="text-xs text-slate-500">{t("shareMarket.catalog.noRiders")}</p>
          )}
        </div>
      </div>

      <footer className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
        <div className="flex min-w-0 items-center gap-2">
          <PaymentMethodIcons kinds={listing.paymentMethodKinds} />
          <span className="truncate text-[11px] text-slate-500" title={footerHint}>
            {footerHint}
          </span>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button size="sm" variant="ghost" onClick={onDetails}>
            <PanelRightOpen className="h-4 w-4" />
            {t("shareMarket.catalog.details")}
          </Button>
          <SeatActionButton action={action} busy={busy} onAction={onAction} />
        </div>
      </footer>
    </article>
  );
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
  const [availability, setAvailability] = React.useState<AvailabilityFilter>("all");
  const [sort, setSort] = React.useState<CatalogSort>("idle");
  const [selected, setSelected] = React.useState<SeatCard | null>(null);
  const [rentTarget, setRentTarget] = React.useState<SeatCard | null>(null);
  const [accessTarget, setAccessTarget] = React.useState<(SeatCard & { eligibility: MarketEligibility }) | null>(null);
  const [busySeatId, setBusySeatId] = React.useState("");
  const [rentError, setRentError] = React.useState("");
  const focusedRef = React.useRef("");
  const blockingInteraction = !!rentTarget || !!accessTarget || !!busySeatId;

  React.useEffect(() => {
    onInteractionChange?.(blockingInteraction);
    return () => onInteractionChange?.(false);
  }, [blockingInteraction, onInteractionChange]);

  const listings = React.useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return catalog.listings
      .filter((listing) =>
        family === "all"
        || listing.providerFamily === family
        || listing.providerFamilies.includes(family),
      )
      .filter((listing) => availability !== "idle" || listingIdleCount(listing) > 0)
      .filter((listing) => {
        if (!needle) return true;
        const text = [
          listing.shareName,
          listing.subdomain,
          listing.ownerEmail,
          listing.providerFamily,
          ...listing.providerFamilies,
          ...listing.seats.flatMap((seat) => [seat.position, seat.subscription?.renterEmail, seat.subscription?.status]),
          ...listing.supportedApps,
          ...listing.appCapabilities.flatMap((capability) => [
            capability.providerName,
            capability.providerType,
            capability.subscriptionLevel,
            capability.upstreamModel,
            ...capability.models,
          ]),
        ].filter(Boolean).join(" ").toLocaleLowerCase();
        return text.includes(needle);
      })
      .sort((left, right) => {
        const online = Number(right.shareOnline) - Number(left.shareOnline);
        if (online) return online;
        if (sort === "price") {
          return listingLowestDailyRate(left) - listingLowestDailyRate(right)
            || listingIdleCount(right) - listingIdleCount(left)
            || left.shareName.localeCompare(right.shareName);
        }
        if (sort === "uptime") {
          return right.reliability.onlineRate24h - left.reliability.onlineRate24h
            || listingIdleCount(right) - listingIdleCount(left)
            || left.shareName.localeCompare(right.shareName);
        }
        return listingIdleCount(right) - listingIdleCount(left)
          || listingLowestDailyRate(left) - listingLowestDailyRate(right)
          || left.shareName.localeCompare(right.shareName);
      });
  }, [availability, catalog.listings, family, query, sort]);

  const groups = React.useMemo(() => {
    if (family === "all" && sort !== "idle") {
      return [["all", listings] as const];
    }
    const map = new Map<ShareMarketProviderFamily, ShareMarketListing[]>();
    for (const listing of listings) {
      const groupFamily = family === "all" ? listing.providerFamily : family;
      const items = map.get(groupFamily) || [];
      items.push(listing);
      map.set(groupFamily, items);
    }
    return [...map.entries()].sort(([left], [right]) => familyRank(left) - familyRank(right));
  }, [family, listings, sort]);

  const idleByFamily = React.useMemo(() => {
    const counts = new Map<ShareMarketProviderFamily | "all", number>();
    let allIdle = 0;
    for (const listing of catalog.listings) {
      const idle = listingIdleCount(listing);
      allIdle += idle;
      for (const providerFamily of new Set([listing.providerFamily, ...listing.providerFamilies])) {
        counts.set(providerFamily, (counts.get(providerFamily) || 0) + idle);
      }
    }
    counts.set("all", allIdle);
    return counts;
  }, [catalog.listings]);

  React.useEffect(() => {
    if (!selected) return;
    const listing = catalog.listings.find((item) => item.id === selected.listing.id);
    if (!listing) {
      setSelected(null);
      return;
    }
    const seat = listing.seats.find((item) => item.id === selected.seat.id) || listing.seats[0];
    if (!seat) {
      setSelected(null);
      return;
    }
    if (seat === selected.seat && listing === selected.listing) return;
    setSelected({ listing, seat });
  }, [catalog.listings, selected]);

  React.useEffect(() => {
    if (!focusedShareId || focusedRef.current === focusedShareId) return;
    const listing = catalog.listings.find((item) => item.shareId === focusedShareId);
    if (!listing) return;
    focusedRef.current = focusedShareId;
    setFamily("all");
    setAvailability("all");
    setQuery("");
    const seat = firstActionableSeat(listing, subscriptions, authed) || listing.seats[0];
    if (seat) setSelected({ listing, seat });
    window.requestAnimationFrame(() => {
      document.getElementById(`share-market-catalog-${focusedShareId}`)?.scrollIntoView({ block: "start" });
    });
  }, [authed, catalog.listings, focusedShareId, subscriptions]);

  const triggerAction = (item: SeatCard) => {
    const action = seatAction(item.listing, item.seat, subscriptions, authed);
    if (action === "login") {
      window.dispatchEvent(new Event("router-open-login"));
    } else if (action === "rent") {
      setRentError("");
      setRentTarget(item);
    } else if (action === "approval") {
      setAccessTarget({ ...item, eligibility: item.seat.eligibility });
    } else if (action === "rented" || action === "granting") {
      setSelected(item);
    } else if (action === "selling") {
      onSwitchSelling?.();
    }
  };

  const confirmRent = async () => {
    if (!rentTarget || busySeatId) return;
    const rented = rentTarget;
    setBusySeatId(rented.seat.id);
    setRentError("");
    try {
      await rentShareMarketSeat(rentTarget.seat.id, rentTarget.seat.offerRevision);
      setRentTarget(null);
      setSelected(rented);
      await onChanged();
    } catch (reason) {
      const eligibility = marketEligibilityFromError(reason);
      if (eligibility) {
        setRentTarget(null);
        setAccessTarget({ ...rented, eligibility });
      } else {
        setRentError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      setBusySeatId("");
    }
  };

  const selectedAction = selected
    ? seatAction(selected.listing, selected.seat, subscriptions, authed)
    : "unavailable";
  const listingPrimaryAction = selected
    ? listingAction(selected.listing, subscriptions, authed)
    : "unavailable";
  const familyOptions = [
    { value: "all" as const, label: t("shareMarket.catalog.allFamilies") },
    ...PROVIDER_FAMILY_ORDER.map((value) => ({ value, label: t(PROVIDER_FAMILY_KEYS[value]) })),
  ].map((option) => ({
    ...option,
    description: t("shareMarket.catalog.idleSeats", { count: idleByFamily.get(option.value) || 0 }),
  }));
  const sortOptions = [
    { value: "idle", label: t("shareMarket.catalog.sort.idle") },
    { value: "price", label: t("shareMarket.catalog.sort.price") },
    { value: "uptime", label: t("shareMarket.catalog.sort.uptime") },
  ];

  return (
    <div className="grid min-w-0 gap-5">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <label className="flex h-10 min-w-[14rem] flex-1 items-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm shadow-sm">
          <Search className="h-4 w-4 shrink-0 text-slate-400" aria-hidden />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-slate-400"
            placeholder={t("shareMarket.catalog.search")}
            aria-label={t("shareMarket.searchAria")}
          />
          {query ? (
            <button type="button" className="text-slate-400 hover:text-slate-700" aria-label={t("common.close")} onClick={() => setQuery("")}>
              <X className="h-4 w-4" />
            </button>
          ) : null}
        </label>
        <CompactSelect
          value={family}
          options={familyOptions}
          onChange={(value) => setFamily(value as ShareMarketProviderFamily | "all")}
          ariaLabel={t("shareMarket.catalog.familyFilter")}
          className="w-full sm:w-56"
          triggerClassName="min-h-10 h-auto w-full py-1.5 text-sm"
        />
        <SegmentedControl
          value={availability}
          onChange={setAvailability}
          ariaLabel={t("shareMarket.catalog.availabilityFilter")}
          size="sm"
          items={[
            { id: "all", label: t("shareMarket.catalog.availability.all") },
            { id: "idle", label: t("shareMarket.catalog.availability.idle") },
          ]}
        />
        <CompactSelect
          value={sort}
          options={sortOptions}
          onChange={(value) => setSort(value as CatalogSort)}
          ariaLabel={t("shareMarket.catalog.sort")}
          className="w-full sm:w-44"
          triggerClassName="min-h-10 h-auto w-full py-1.5 text-sm"
        />
        {subscriptions.length ? (
          <Link
            href={`${DASHBOARD_ACCOUNT_SHARE_PATH}?tab=user`}
            className="inline-flex h-10 items-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm font-medium text-slate-700 hover:bg-slate-50"
          >
            <UserRound className="h-4 w-4" />
            {t("shareMarket.catalog.myRentals", { count: subscriptions.filter((item) => !["released", "grant_failed"].includes(item.status)).length })}
          </Link>
        ) : null}
      </div>

      {groups.map(([providerFamily, items]) => (
        <section key={providerFamily} className="grid min-w-0 gap-3">
          <div className="flex items-baseline justify-between border-b border-slate-200 pb-2">
            <h2 className="text-sm font-semibold text-slate-900">
              {providerFamily === "all" ? t("shareMarket.catalog.allFamilies") : t(PROVIDER_FAMILY_KEYS[providerFamily])}
            </h2>
            <span className="text-xs tabular-nums text-slate-400">
              {t("shareMarket.catalog.listingCount", { count: items.length })}
              {" · "}
              {t("shareMarket.catalog.idleSeats", { count: items.reduce((sum, listing) => sum + listingIdleCount(listing), 0) })}
            </span>
          </div>
          <div className="grid min-w-0 gap-3 md:grid-cols-2 xl:grid-cols-3">
            {items.map((listing) => {
              const seat = firstActionableSeat(listing, subscriptions, authed);
              return (
                <ListingCardView
                  key={listing.id}
                  listing={listing}
                  action={listingAction(listing, subscriptions, authed)}
                  busy={busySeatId === seat?.id}
                  trialHours={catalog.trialHours}
                  focused={focusedShareId === listing.shareId}
                  onDetails={() => setSelected({ listing, seat: seat || listing.seats[0] })}
                  onAction={() => {
                    const target = firstActionableSeat(listing, subscriptions, authed);
                    if (target) triggerAction({ listing, seat: target });
                  }}
                />
              );
            })}
          </div>
        </section>
      ))}

      {!listings.length ? (
        <div className="grid min-h-48 place-items-center border-y border-dashed border-slate-200 text-sm text-slate-500">
          {t("shareMarket.catalog.empty")}
        </div>
      ) : null}

      <Drawer.Backdrop isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Drawer.Content placement="right">
          <Drawer.Dialog className={drawerDialogClassName}>
            <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" />
            <Drawer.Header>
              <div className="min-w-0 pr-10">
                <div className="flex min-w-0 items-center gap-2">
                  <Drawer.Heading className="truncate text-base">{selected?.listing.shareName}</Drawer.Heading>
                  {selected ? <AppLogos apps={selected.listing.supportedApps} /> : null}
                </div>
                <p className="mt-1 break-all font-mono text-xs text-slate-500">{selected?.listing.subdomain}</p>
              </div>
            </Drawer.Header>
            <Drawer.Body className="overflow-y-auto pb-28">
              {selected ? (
                <div className="grid gap-5">
                  <section className="grid gap-2 border-b border-slate-200 pb-4">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <strong className="text-sm">{t(PROVIDER_FAMILY_KEYS[selected.listing.providerFamily])}</strong>
                      <span className={selected.listing.shareOnline ? "text-sm font-medium text-emerald-700" : "text-sm font-medium text-rose-700"}>
                        {selected.listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
                      </span>
                    </div>
                    <p className="text-sm text-slate-600">
                      {t("shareMarket.catalog.occupancy", {
                        idle: listingIdleCount(selected.listing),
                        total: selected.listing.seats.length,
                      })}
                    </p>
                  </section>

                  <section className="grid gap-3 border-b border-slate-200 pb-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.appCapabilities")}</h3>
                    {selected.listing.appCapabilities.map((capability) => (
                      <div key={capability.app} className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
                        {isCoreShareApp(capability.app) ? <ShareAppLogo app={capability.app} size={18} /> : <Info className="h-5 w-5" />}
                        <div className="min-w-0">
                          <strong>{isCoreShareApp(capability.app) ? SHARE_APP_LABELS[capability.app] : capability.app}</strong>
                          <p className="break-words text-slate-600">
                            {[capability.providerName || capability.providerType, capability.subscriptionLevel].filter(Boolean).join(" · ") || t("shareMarket.catalog.providerUnknown")}
                          </p>
                          <p className="break-words text-slate-500">
                            {capabilityModelLabel(capability, t("shareMarket.modelPassthrough"), t("shareMarket.catalog.modelUnknown"))}
                          </p>
                        </div>
                      </div>
                    ))}
                  </section>

                  <section className="grid gap-3 border-b border-slate-200 pb-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.terms")}</h3>
                    <dl className="grid grid-cols-2 gap-3 text-sm">
                      <Metric label={t("shareMarket.parallel")} value={selected.seat.parallelLimit == null ? t("common.unlimited") : String(selected.seat.parallelLimit)} />
                      <Metric label={t("shareMarket.tokens")} value={formatTokenLimit(selected.seat, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`))} />
                      <Metric label={t("shareMarket.catalog.serviceTerm")} value={selected.seat.serviceDurationDays == null ? t("shareMarket.serviceDuration.permanent") : t("shareMarket.serviceDuration.daysValue", { count: selected.seat.serviceDurationDays })} />
                      <Metric label={t("shareMarket.catalog.billingStart")} value={selected.seat.isFree ? t("shareMarket.free") : t("shareMarket.catalog.afterTrial", { hours: catalog.trialHours })} />
                    </dl>
                    {!selected.seat.isFree ? (
                      <p className="text-xs leading-5 text-slate-500">{t("shareMarket.catalog.postpaidHint")}</p>
                    ) : null}
                  </section>

                  <section className="grid gap-3 border-b border-slate-200 pb-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.serviceQuality")}</h3>
                    <dl className="grid grid-cols-2 gap-3 text-sm sm:grid-cols-3">
                      <Metric label="TTFT" value={selected.listing.performance.averageTtftMs == null ? "-" : `${(selected.listing.performance.averageTtftMs / 1_000).toFixed(2)}s`} />
                      <Metric label="TPS" value={selected.listing.performance.averageTps == null ? "-" : selected.listing.performance.averageTps.toFixed(1)} />
                      <Metric label={t("shareMarket.catalog.samples")} value={String(selected.listing.performance.recentRequestCount)} />
                      <Metric label={t("shareMarket.catalog.uptime24h")} value={`${selected.listing.reliability.onlineRate24h.toFixed(1)}%`} />
                      <Metric label={t("shareMarket.catalog.coverage24h")} value={`${selected.listing.reliability.observationCoverage24h.toFixed(1)}%`} />
                      <Metric label={t("shareMarket.catalog.observedMinutes")} value={String(selected.listing.reliability.observedMinutes24h)} />
                    </dl>
                  </section>

                  <section className="grid gap-3">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.seller")}</h3>
                    <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
                      <UserRound className="h-4 w-4 text-slate-400" />
                      <span className="min-w-0 break-all font-medium">{selected.listing.ownerEmail}</span>
                      <PaymentMethodIcons kinds={selected.listing.paymentMethodKinds} />
                    </div>
                    <ProviderContactsList contacts={selected.listing.contacts} />
                    <Button variant="outline" onClick={() => void chat.openClientChat(selected.listing.installationId)}>
                      <MessageCircle className="h-4 w-4" />
                      {t("marketApproval.openChat")}
                    </Button>
                  </section>

                  <section className="grid gap-2 border-t border-slate-200 pt-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.seatsAndRiders")}</h3>
                    {selected.listing.seats.map((seat) => {
                      const idle = isSeatIdle(seat);
                      return (
                        <button
                          key={seat.id}
                          type="button"
                          className={cn(
                            "grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 border-b border-slate-100 py-2 text-left text-sm last:border-0",
                            selected.seat.id === seat.id && "bg-slate-50",
                          )}
                          onClick={() => setSelected({ listing: selected.listing, seat })}
                        >
                          <span className="font-medium">#{seat.position}</span>
                          <span className={cn("min-w-0 truncate", idle ? "text-emerald-700" : "text-slate-500")} title={idle ? t("shareMarket.available") : riderLabel(seat, t)}>
                            {idle ? t("shareMarket.available") : riderLabel(seat, t)}
                          </span>
                          <strong>{formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</strong>
                        </button>
                      );
                    })}
                  </section>
                </div>
              ) : null}
            </Drawer.Body>
            {selected ? (
              <div className="absolute inset-x-0 bottom-0 flex items-center justify-between gap-3 border-t border-slate-200 bg-white px-5 py-4">
                <div className="min-w-0">
                  <strong className="block truncate text-lg">{formatSeatPrice(selected.seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</strong>
                  <span className="text-xs text-slate-500">
                    {t("shareMarket.catalog.seatPosition", { position: selected.seat.position })}
                    {" · "}
                    {t("shareMarket.catalog.occupancy", {
                      idle: listingIdleCount(selected.listing),
                      total: selected.listing.seats.length,
                    })}
                  </span>
                </div>
                <SeatActionButton
                  action={isSeatIdle(selected.seat) ? selectedAction : listingPrimaryAction}
                  busy={busySeatId === selected.seat.id}
                  onAction={() => {
                    if (isSeatIdle(selected.seat)) {
                      triggerAction(selected);
                      return;
                    }
                    const target = firstActionableSeat(selected.listing, subscriptions, authed);
                    if (target) triggerAction({ listing: selected.listing, seat: target });
                  }}
                />
              </div>
            ) : null}
          </Drawer.Dialog>
        </Drawer.Content>
      </Drawer.Backdrop>

      <Modal.Backdrop isOpen={!!rentTarget} onOpenChange={(open) => !open && !busySeatId && setRentTarget(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("shareMarket.rentConfirm.title")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid gap-3">
              {rentTarget ? (
                <>
                  <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 border-y border-slate-200 py-3 text-sm">
                    <dt className="text-slate-500">{t("shareMarket.col.share")}</dt>
                    <dd className="truncate font-medium">{rentTarget.listing.shareName} · #{rentTarget.seat.position}</dd>
                    <dt className="text-slate-500">{t("shareMarket.owner")}</dt>
                    <dd className="break-all">{rentTarget.listing.ownerEmail}</dd>
                    <dt className="text-slate-500">{t("shareMarket.catalog.seatsAndRiders")}</dt>
                    <dd>{t("shareMarket.catalog.occupancy", { idle: listingIdleCount(rentTarget.listing), total: rentTarget.listing.seats.length })}</dd>
                    <dt className="text-slate-500">{t("shareMarket.dialog.amount")}</dt>
                    <dd className="font-medium">{formatSeatPrice(rentTarget.seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</dd>
                  </dl>
                  <div className="flex gap-3 border-l-2 border-sky-400 bg-sky-50 px-3 py-2 text-sm leading-6 text-sky-950">
                    <Clock3 className="mt-1 h-4 w-4 shrink-0" />
                    <p>{rentTarget.seat.isFree ? t("shareMarket.rentConfirm.freeBilling") : t("shareMarket.rentConfirm.postpaid", { hours: catalog.trialHours })}</p>
                  </div>
                  <div className="flex gap-3 border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-sm leading-6 text-amber-950">
                    <CalendarClock className="mt-1 h-4 w-4 shrink-0" />
                    <p>{rentTarget.seat.serviceDurationDays == null ? t("shareMarket.rentConfirm.servicePermanent") : t("shareMarket.rentConfirm.serviceFixed", { days: rentTarget.seat.serviceDurationDays })}</p>
                  </div>
                  {rentError ? <p className="text-sm text-rose-700">{rentError}</p> : null}
                </>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={!!busySeatId} onClick={() => setRentTarget(null)}>{t("common.cancel")}</Button>
              <Button variant="primary" isDisabled={!!busySeatId} onClick={() => void confirmRent()}>
                {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShoppingCart className="h-4 w-4" />}
                {t("shareMarket.rentConfirm.confirm")}
              </Button>
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
