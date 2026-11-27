"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { CircleDot, Globe2, Plus, Search, X } from "lucide-react";
import { formatAgeDaysOrHours } from "@/components/dashboard/share-dashboard-utils";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatUsdMoney } from "@/lib/market-money";
import { clientDisplayName, type OnlineListing } from "@/lib/online-client-market";
import { cn } from "@/lib/utils";
import {
  onlineListingAvailability,
  onlineStatusKey,
  SHOP_CONTINENTS,
  SHOP_UNKNOWN_COUNTRY,
  type ShopContinent,
  type ShopCountryCard,
} from "./host-utils";

const focusRing = "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";

/** Same breakpoints as Share cards: one column until the card can keep its price and spec readable. */
export const ONLINE_CLIENT_CARD_GRID_CLASS =
  "grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2 md:grid-cols-3 xl:grid-cols-4";

function useRegionName() {
  const { locale, t } = useLocaleText();
  const names = React.useMemo(() => new Intl.DisplayNames([locale], { type: "region" }), [locale]);
  return (code: string | null | undefined) => {
    const normalized = (code || "").toUpperCase();
    return /^[A-Z]{2}$/.test(normalized) ? names.of(normalized) || normalized : t("clientMarket.shop.unknownRegion");
  };
}

/** Share Market filter chip (`market-listing-filters.tsx`): gray, sky when selected. */
const SHOP_CHIP =
  "inline-flex h-9 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:cursor-not-allowed disabled:opacity-50";
const shopChip = (selected: boolean) =>
  cn(
    SHOP_CHIP,
    selected
      ? "bg-sky-100 font-semibold text-sky-800"
      : "bg-slate-100 font-medium text-slate-600 enabled:hover:bg-slate-200 enabled:hover:text-slate-900",
  );
const shopChipCount = (selected: boolean) => cn("tabular-nums", selected ? "text-sky-600" : "text-slate-400");

/**
 * Filter bar in the Share Market toolbar layout: continent tabs on the left
 * (every continent always shown, disabled when it has no Client), then the
 * "rentable only" toggle and the search box at the far right. Countries of the
 * chosen continent follow on a second, sideways-scrolling row.
 */
export function ClientShopFilters({
  cards,
  continent,
  onContinent,
  country,
  onCountry,
  query,
  onQuery,
  rentable,
  mine,
}: {
  cards: ShopCountryCard[];
  continent: ShopContinent;
  onContinent: (next: ShopContinent) => void;
  country: string;
  onCountry: (code: string) => void;
  query: string;
  onQuery: (next: string) => void;
  rentable: { pressed: boolean; label: string; onClick: () => void };
  /**
   * "Mine", left of "All": the buyer's rentals and rental history instead of
   * the shop. Disabled for guests and for buyers who never rented a Client.
   */
  mine?: { selected: boolean; enabled: boolean; count: number; label: string; hint: string; onSelect: () => void };
}) {
  const { locale, t } = useLocaleText();
  const regionName = useRegionName();
  const priceOf = (card: ShopCountryCard) =>
    card.hasFreeIdle || card.minMonthlyMinor == null ? t("online.free") : formatUsdMoney(card.minMonthlyMinor, locale);
  const inContinent = (value: ShopContinent) => (value === "all" ? cards : cards.filter((card) => card.continent === value));
  const idle = (list: ShopCountryCard[]) => list.reduce((sum, card) => sum + card.idle, 0);
  const countries = inContinent(continent);
  return (
    <div className="grid min-w-0 gap-2">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <div
          role="tablist"
          aria-label={t("clientMarket.shop.continentLabel")}
          className="flex min-w-0 flex-1 basis-full items-center gap-1 overflow-x-auto sm:basis-auto"
        >
          {mine ? (
            <button
              type="button"
              role="tab"
              aria-selected={mine.selected}
              title={mine.hint}
              disabled={!mine.selected && !mine.enabled}
              className={shopChip(mine.selected)}
              onClick={() => !mine.selected && mine.onSelect()}
            >
              <span>{mine.label}</span>
              {mine.count ? <span className={shopChipCount(mine.selected)}>{mine.count}</span> : null}
            </button>
          ) : null}
          {SHOP_CONTINENTS.map((value) => {
            const members = inContinent(value);
            const selected = !mine?.selected && value === continent;
            return (
              <button
                key={value}
                type="button"
                role="tab"
                aria-selected={selected}
                // "All" is the way back from "Mine", so it is never disabled.
                disabled={!selected && value !== "all" && members.length === 0}
                className={shopChip(selected)}
                onClick={() => onContinent(selected ? "all" : value)}
              >
                <span>{t(`clientMarket.shop.continent.${value}`)}</span>
                <span className={shopChipCount(selected)}>{idle(members)}</span>
              </button>
            );
          })}
        </div>
        {mine?.selected ? null : (
          <>
            <button
              type="button"
              aria-pressed={rentable.pressed}
              className={shopChip(rentable.pressed)}
              onClick={rentable.onClick}
            >
              <CircleDot className={cn("h-3.5 w-3.5", rentable.pressed ? "text-sky-600" : "text-slate-400")} aria-hidden />
              <span className="whitespace-nowrap">{rentable.label}</span>
            </button>
            <label className="flex h-9 w-44 shrink-0 grow items-center gap-1.5 rounded-md border border-slate-200 bg-white px-2.5 text-sm shadow-sm hover:border-slate-300 focus-within:outline focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-primary sm:grow-0">
              <Search className="h-3.5 w-3.5 shrink-0 text-slate-400" aria-hidden />
              <input
                type="search"
                aria-label={t("clientMarket.shop.search")}
                placeholder={t("clientMarket.shop.search")}
                value={query}
                className="min-w-0 flex-1 bg-transparent text-xs outline-none [&::-webkit-search-cancel-button]:hidden"
                onChange={(event) => onQuery(event.target.value)}
              />
              {query ? (
                <button
                  type="button"
                  aria-label={t("common.reset")}
                  className="rounded active:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  onClick={() => onQuery("")}
                >
                  <X className="h-3.5 w-3.5 text-slate-400" aria-hidden />
                </button>
              ) : null}
            </label>
          </>
        )}
      </div>
      {countries.length > 0 && !mine?.selected ? (
        <div
          className="-mx-1 flex min-w-0 gap-1 overflow-x-auto px-1 pb-1 [scrollbar-width:thin]"
          role="group"
          aria-label={t("clientMarket.shop.countryLabel")}
        >
          {countries.map((card) => {
            const known = card.code !== SHOP_UNKNOWN_COUNTRY;
            const selected = card.code === country;
            const name = regionName(known ? card.code : null);
            return (
              <button
                key={card.code}
                type="button"
                aria-pressed={selected}
                aria-label={t(selected ? "clientMarket.shop.clearCountry" : "clientMarket.shop.selectCountry", { name })}
                title={card.idle > 0 ? t("clientMarket.shop.priceFrom", { price: priceOf(card) }) : undefined}
                className={shopChip(selected)}
                onClick={() => onCountry(selected ? "" : card.code)}
              >
                {known ? <CountryFlag code={card.code} size="compact" title={name} /> : <Globe2 className="size-3.5" aria-hidden />}
                <span className="max-w-32 truncate">{name}</span>
                <span className={shopChipCount(selected)}>{card.idle}</span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/** Flag, country and the public network of the real egress path. */
export function ClientEgressLine({
  listing,
  emphasize = false,
}: {
  listing: Pick<OnlineListing, "country" | "egressIp">;
  emphasize?: boolean;
}) {
  const { t } = useLocaleText();
  const regionName = useRegionName();
  const code = listing.country?.toUpperCase();
  const known = !!code && /^[A-Z]{2}$/.test(code);
  const name = regionName(code);
  return (
    <p className={cn("flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5", emphasize ? "text-sm" : "text-xs")}>
      {known ? (
        <CountryFlag code={code} title={name} />
      ) : (
        <Globe2 role="img" aria-label={t("clientMarket.shop.unknownRegion")} className="size-3.5 text-muted-foreground" />
      )}
      <span className={cn("font-medium text-foreground", emphasize && "font-semibold")}>{name}</span>
      {listing.egressIp ? (
        <span className="font-mono text-muted-foreground" title={t("clientMarket.shop.egressHint")}>
          {listing.egressIp}
        </span>
      ) : null}
    </p>
  );
}

/** Hardware facts. Kept for the listing dialog, where the egress is shown alongside. */
export function ClientSpecs({ listing }: { listing: Pick<OnlineListing, "country" | "hardware"> }) {
  const { locale, t } = useLocaleText();
  const regionName = useRegionName();
  const code = listing.country?.toUpperCase();
  const name = code && /^[A-Z]{2}$/.test(code) ? regionName(code) : "";
  return (
    <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5 text-muted-foreground">
      {name ? (
        <CountryFlag code={code} title={name} />
      ) : (
        <Globe2 role="img" aria-label={t("clientMarket.shop.unknownRegion")} className="size-3.5 text-muted-foreground" />
      )}
      <span>{hardwareLine(listing.hardware, locale, t) || t("clientMarket.shop.noSpecs")}</span>
    </p>
  );
}

function hardwareLine(
  hw: OnlineListing["hardware"],
  locale: string,
  t: ReturnType<typeof useLocaleText>["t"],
) {
  return [
    hw?.cpu ? t(hw.cpu === 1 ? "clientMarket.shop.core" : "clientMarket.shop.cores", { count: hw.cpu }) : "",
    hw?.memoryBytes
      ? new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(hw.memoryBytes / 2 ** 30) + " GiB"
      : "",
    hw?.arch,
  ]
    .filter(Boolean)
    .join(" · ");
}

export function ClientHardware({ listing }: { listing: Pick<OnlineListing, "hardware"> }) {
  const { locale, t } = useLocaleText();
  const line = hardwareLine(listing.hardware, locale, t);
  return <p className="truncate text-[11px] text-muted-foreground" title={t("clientMarket.shop.summarySpecs")}>{line || t("clientMarket.shop.noSpecs")}</p>;
}

type ClientReliabilityData = Pick<
  OnlineListing,
  "onlineRate24h" | "observedMinutes24h" | "observationCoverage24h" | "registeredAt" | "onlineRate30d" | "observedMinutes30d" | "observedDays30d"
>;

/**
 * 30-day online rate (24 hours on Routers that do not report 30 days) and how
 * long this Client has been connected to the Router.
 */
export function ClientReliability({ listing, inline = false }: { listing: ClientReliabilityData; inline?: boolean }) {
  const { t, locale } = useLocaleText();
  const monthly = listing.onlineRate30d !== undefined;
  const observed = monthly ? listing.observedMinutes30d ?? 0 : listing.observedMinutes24h;
  const rate = observed > 0 ? (monthly ? listing.onlineRate30d ?? null : listing.onlineRate24h) : null;
  const thin = monthly
    ? (listing.observedDays30d ?? 0) < 3
    : listing.observationCoverage24h < 50;
  const pair = inline
    ? "grid grid-cols-[auto_auto] items-baseline justify-start gap-x-2"
    : "grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-2";
  return (
    <dl className={inline ? "flex flex-wrap gap-x-6 gap-y-2 text-xs leading-5" : "grid gap-1 text-xs"}>
      <div className={pair}>
        <dt className="text-muted-foreground">{t(monthly ? "clientMarket.shop.uptime30d" : "clientMarket.shop.uptime")}</dt>
        <dd className="font-semibold tabular-nums text-foreground">
          {rate == null
            ? t("clientMarket.shop.noSamples")
            : new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 }).format(rate / 100)}
        </dd>
        {rate != null && thin ? (
          <dd className={cn("col-span-2 text-[11px] text-muted-foreground", !inline && "mt-0.5 text-right")}>
            {monthly
              ? t("clientMarket.shop.observedDays", { count: listing.observedDays30d ?? 0 })
              : t("clientMarket.shop.samples", { count: listing.observedMinutes24h })}
          </dd>
        ) : null}
      </div>
      {listing.registeredAt ? (
        <div className={pair} title={t("clientMarket.shop.registeredSince")}>
          <dt className="text-muted-foreground">{t("dashboard.runningDuration")}</dt>
          <dd className="font-semibold tabular-nums text-foreground">{formatAgeDaysOrHours(listing.registeredAt, locale)}</dd>
        </div>
      ) : null}
    </dl>
  );
}

/**
 * "This period: 12.5 h available, 0.5 h unavailable (not charged)", from the
 * settlement record. Null when nothing is charged (free rentals).
 */
export function useBilledTime() {
  const { locale, t } = useLocaleText();
  const hours = React.useMemo(() => new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }), [locale]);
  const time = React.useMemo(() => new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }), [locale]);
  return (value: { availableSeconds?: number | null; unavailableSeconds?: number | null; settledThrough?: string | null }) => {
    if (value.availableSeconds == null) return null;
    const line = t("clientMarket.billing.hours", {
      available: hours.format(value.availableSeconds / 3600),
      unavailable: hours.format((value.unavailableSeconds ?? 0) / 3600),
    });
    return value.settledThrough
      ? `${line} · ${t("clientMarket.billing.settledThrough", { time: time.format(new Date(value.settledThrough)) })}`
      : line;
  };
}

export function useListingPrice() {
  const { locale, t } = useLocaleText();
  const days = React.useMemo(() => new Intl.NumberFormat(locale, { style: "unit", unit: "day", unitDisplay: "long" }), [locale]);
  const hours = React.useMemo(() => new Intl.NumberFormat(locale, { style: "unit", unit: "hour", unitDisplay: "long" }), [locale]);
  return (listing: Pick<OnlineListing, "owned" | "cyclePriceMinor" | "trialHours" | "freeDays">) => ({
    price: listing.owned
      ? t("clientMarket.shop.selfFree")
      : listing.cyclePriceMinor > 0
        ? formatUsdMoney(listing.cyclePriceMinor, locale)
        : t("online.free"),
    monthly: !listing.owned && listing.cyclePriceMinor > 0,
    term: listing.owned
      ? ""
      : listing.trialHours > 0
        ? t("lease.trial", { duration: hours.format(listing.trialHours) })
        : listing.freeDays
          ? days.format(listing.freeDays)
          : "",
  });
}

export function ClientOfferCard({
  listing,
  authenticated,
  busy,
  onRent,
  onOpen,
  extra,
}: {
  listing: OnlineListing;
  authenticated: boolean;
  busy: boolean;
  onRent: () => void;
  onOpen?: () => void;
  extra?: React.ReactNode;
}) {
  const { t } = useLocaleText();
  const availability = onlineListingAvailability(listing);
  const { price, monthly, term } = useListingPrice()(listing);
  const name = clientDisplayName(listing);
  const nameId = `client-offer-name-${listing.installationId}`;
  return (
    <article
      id={`market-client-${listing.installationId}`}
      tabIndex={-1}
      aria-labelledby={nameId}
      className="group flex min-w-0 scroll-mt-20 flex-col gap-3 rounded-xl border border-border bg-card p-4 shadow-sm transition-all duration-300 hover:-translate-y-0.5 hover:shadow-md motion-reduce:transition-none motion-reduce:hover:translate-y-0"
    >
      <header className="grid min-w-0 gap-1.5">
        <ClientEgressLine listing={listing} emphasize />
        {onOpen ? (
          <button
            type="button"
            id={nameId}
            className={cn("w-fit max-w-full truncate rounded text-left text-sm font-semibold tracking-tight text-foreground hover:text-accent", focusRing)}
            onClick={onOpen}
          >
            {name}
          </button>
        ) : (
          <h3 id={nameId} className="truncate text-sm font-semibold tracking-tight text-foreground">{name}</h3>
        )}
        {listing.sellerEmail ? (
          <p className="truncate text-[11px] text-muted-foreground" title={listing.sellerEmail}>
            {t("clientMarket.shop.seller", { email: listing.sellerEmail })}
          </p>
        ) : null}
      </header>
      <ClientReliability listing={listing} />
      <ClientHardware listing={listing} />
      <footer className="mt-auto flex flex-wrap items-end justify-between gap-2 border-t border-border pt-3">
        <div className="min-w-0">
          <strong className="text-base tabular-nums text-foreground">{price}</strong>
          {monthly ? <span className="text-xs text-muted-foreground">{t("online.perMonth")}</span> : null}
          {term ? <p className="text-xs text-muted-foreground">{term}</p> : null}
        </div>
        {availability === "available" ? (
          <button
            type="button"
            disabled={busy}
            aria-describedby={nameId}
            onClick={onRent}
            className={cn(
              "inline-flex h-9 items-center rounded-lg px-3 text-xs font-semibold transition-colors disabled:opacity-60 bg-slate-900 text-white hover:bg-slate-800 active:bg-slate-700",
              focusRing,
            )}
          >
            {t(authenticated ? "clientMarket.shop.rent" : "clientMarket.shop.guestRent")}
          </button>
        ) : (
          <span className="rounded-full bg-muted px-2.5 py-1 text-xs text-muted-foreground">{t(onlineStatusKey(availability))}</span>
        )}
      </footer>
      {extra}
    </article>
  );
}

/** The seller's own listing or rented Client. */
export function OnlineClientCard({
  name,
  price,
  status,
  tone = "neutral",
  detail,
  listing,
  muted = false,
  cardId,
  actions,
  extra,
}: {
  name: string;
  price?: string;
  status: string;
  tone?: "neutral" | "good" | "attention" | "danger";
  detail?: React.ReactNode;
  listing?: OnlineListing;
  muted?: boolean;
  cardId?: string;
  actions?: React.ReactNode;
  extra?: React.ReactNode;
}) {
  const offline = listing?.availability === "offline";
  const dot = offline || tone === "danger"
    ? "bg-rose-500"
    : tone === "attention"
      ? "bg-amber-500"
      : listing?.healthy || tone === "good"
        ? "bg-emerald-500"
        : "bg-slate-300";
  return (
    <article
      id={cardId}
      tabIndex={cardId ? -1 : undefined}
      className={cn(
        "grid min-w-0 scroll-mt-20 content-start gap-2.5 rounded-xl border p-4 shadow-sm",
        muted ? "bg-muted/40" : "bg-card",
        offline || tone === "danger" ? "border-rose-200" : tone === "attention" ? "border-amber-200" : "border-border",
      )}
    >
      <header className="flex min-w-0 items-start justify-between gap-2">
        <strong className="min-w-0 truncate text-sm font-semibold text-foreground" title={name}>{name}</strong>
        {price ? <strong className="shrink-0 whitespace-nowrap text-sm font-semibold tabular-nums text-foreground">{price}</strong> : null}
      </header>
      {listing ? <ClientEgressLine listing={listing} /> : null}
      <p className="flex min-w-0 items-center gap-1.5 text-xs font-medium text-foreground">
        <span className={cn("size-2 shrink-0 rounded-full", dot)} aria-hidden />
        <span className="truncate">{status}</span>
      </p>
      {detail ? <div className="grid gap-1 break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">{detail}</div> : null}
      {listing ? <ClientReliability listing={listing} /> : null}
      {actions ? <div className="mt-auto flex flex-wrap gap-2 pt-1">{actions}</div> : null}
      {extra}
    </article>
  );
}

/**
 * The "Listed" row on My listings. It copies the Share owner toolbar
 * (`MarketListingFilters` with a leading heading) class for class, so both
 * markets' seller views read as one product.
 */
export function OnlineSellingToolbar({
  listedCount,
  query,
  onQuery,
  onAdd,
}: {
  listedCount: number;
  query: string;
  onQuery: (next: string) => void;
  onAdd: () => void;
}) {
  const { t } = useLocaleText();
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <h3 className="shrink-0 whitespace-nowrap text-xs font-semibold uppercase tracking-wide text-slate-500">
        {t("clientMarket.shop.filter.listed")}
        {listedCount ? <span className="ml-1.5 tabular-nums text-slate-400">{listedCount}</span> : null}
      </h3>
      <label className="ml-auto flex h-9 w-44 shrink-0 grow items-center gap-1.5 rounded-md border border-slate-200 bg-white px-2.5 text-sm shadow-sm hover:border-slate-300 focus-within:outline focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-primary sm:grow-0">
        <Search className="h-3.5 w-3.5 shrink-0 text-slate-400" aria-hidden />
        <input
          aria-label={t("clientMarket.shop.sellingSearch")}
          value={query}
          placeholder={t("clientMarket.shop.sellingSearch")}
          className="min-w-0 flex-1 bg-transparent text-xs outline-none"
          onChange={(event) => onQuery(event.target.value)}
        />
        {query ? (
          <button
            type="button"
            aria-label={t("common.reset")}
            className="rounded active:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
            onClick={() => onQuery("")}
          >
            <X className="h-3.5 w-3.5 text-slate-400" aria-hidden />
          </button>
        ) : null}
      </label>
      <div className="shrink-0">
        <Button size="sm" variant={listedCount ? "outline" : "primary"} className="h-9 shrink-0" onClick={onAdd}>
          <Plus className="h-4 w-4" aria-hidden />
          {t("clientMarket.shop.listAdd")}
        </Button>
      </div>
    </div>
  );
}
