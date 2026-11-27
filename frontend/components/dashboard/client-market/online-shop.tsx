"use client";

import * as React from "react";
import { Globe2, Plus, Search, X } from "lucide-react";
import { formatAgeDaysOrHours } from "@/components/dashboard/share-dashboard-utils";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatUsdMoney } from "@/lib/market-money";
import { clientDisplayName, type OnlineListing } from "@/lib/online-client-market";
import { cn } from "@/lib/utils";
import {
  onlineListingAvailability,
  onlineStatusKey,
  SHOP_UNKNOWN_COUNTRY,
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

/**
 * Filter bar: one row of country chips with how many Clients can be rented in
 * each, the "rentable only" switch and a search box. Chips scroll sideways on
 * narrow screens instead of wrapping into a wall.
 */
export function ClientShopFilters({
  cards,
  country,
  onCountry,
  query,
  onQuery,
  rentable,
}: {
  cards: ShopCountryCard[];
  country: string;
  onCountry: (code: string) => void;
  query: string;
  onQuery: (next: string) => void;
  rentable: { pressed: boolean; label: string; onClick: () => void };
}) {
  const { locale, t } = useLocaleText();
  const regionName = useRegionName();
  const total = cards.reduce((sum, card) => sum + card.idle, 0);
  const priceOf = (card: ShopCountryCard) =>
    card.hasFreeIdle || card.minMonthlyMinor == null ? t("online.free") : formatUsdMoney(card.minMonthlyMinor, locale);
  const chip = (selected: boolean) =>
    cn(
      "inline-flex min-h-11 shrink-0 items-center gap-1.5 rounded-full border px-3 text-xs transition-colors duration-200",
      focusRing,
      selected
        ? "border-accent/40 bg-accent/10 font-semibold text-accent"
        : "border-border bg-card text-foreground hover:border-accent/30",
    );
  return (
    <div className="grid min-w-0 gap-2.5">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <label className="flex h-11 min-w-0 flex-1 items-center gap-2 rounded-lg border border-border bg-card px-3 text-sm shadow-sm focus-within:ring-2 focus-within:ring-accent sm:max-w-xs">
          <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          <input
            type="search"
            aria-label={t("clientMarket.shop.search")}
            placeholder={t("clientMarket.shop.search")}
            value={query}
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground/60"
            onChange={(event) => onQuery(event.target.value)}
          />
          {query ? (
            <button type="button" aria-label={t("common.reset")} className={cn("rounded", focusRing)} onClick={() => onQuery("")}>
              <X className="size-3.5 text-muted-foreground" aria-hidden />
            </button>
          ) : null}
        </label>
        <button
          type="button"
          aria-pressed={rentable.pressed}
          className={cn(
            "ml-auto inline-flex h-11 shrink-0 items-center gap-2 rounded-lg border px-3 text-xs font-medium transition-colors duration-200",
            focusRing,
            rentable.pressed ? "border-accent/40 bg-accent/10 text-accent" : "border-border bg-card text-muted-foreground hover:text-foreground",
          )}
          onClick={rentable.onClick}
        >
          <span className={cn("size-2 rounded-full", rentable.pressed ? "bg-accent" : "bg-slate-300")} aria-hidden />
          {rentable.label}
        </button>
      </div>
      {cards.length > 0 ? (
        <div
          className="-mx-1 flex min-w-0 gap-1.5 overflow-x-auto px-1 pb-1 [scrollbar-width:thin]"
          role="group"
          aria-label={t("clientMarket.shop.countryLabel")}
        >
          <button type="button" aria-pressed={!country} className={chip(!country)} onClick={() => onCountry("")}>
            {t("clientMarket.shop.allCountries")}
            <span className="tabular-nums text-muted-foreground">{total}</span>
          </button>
          {cards.map((card) => {
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
                className={chip(selected)}
                onClick={() => onCountry(selected ? "" : card.code)}
              >
                {known ? <CountryFlag code={card.code} size="compact" title={name} /> : <Globe2 className="size-3.5" aria-hidden />}
                <span className="max-w-32 truncate">{name}</span>
                <span className="tabular-nums text-muted-foreground">{card.idle}</span>
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
  return (listing: Pick<OnlineListing, "owned" | "cyclePriceMinor" | "trialDays" | "freeDays">) => ({
    price: listing.owned
      ? t("clientMarket.shop.selfFree")
      : listing.cyclePriceMinor > 0
        ? formatUsdMoney(listing.cyclePriceMinor, locale)
        : t("online.free"),
    monthly: !listing.owned && listing.cyclePriceMinor > 0,
    term: listing.owned
      ? ""
      : listing.trialDays > 0
        ? t("lease.trialDays", { duration: days.format(listing.trialDays) })
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
              "min-h-11 rounded-xl bg-gradient-to-r from-accent to-[#4D7CFF] px-4 text-sm font-semibold text-white shadow-sm transition-all duration-200 hover:-translate-y-0.5 hover:brightness-110 active:scale-[0.98] disabled:opacity-60 motion-reduce:transition-none",
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
      <h3 className="shrink-0 whitespace-nowrap font-mono text-xs uppercase tracking-[0.15em] text-muted-foreground">
        {t("clientMarket.shop.filter.listed")}
        <span className="ml-1.5 tabular-nums">{listedCount}</span>
      </h3>
      <label className="flex h-11 w-44 shrink-0 grow items-center gap-1.5 rounded-lg border border-border bg-card px-2.5 text-sm shadow-sm focus-within:ring-2 focus-within:ring-accent sm:ml-auto sm:grow-0">
        <Search className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <input
          aria-label={t("clientMarket.shop.sellingSearch")}
          value={query}
          placeholder={t("clientMarket.shop.sellingSearch")}
          className="min-w-0 flex-1 bg-transparent text-xs outline-none"
          onChange={(event) => onQuery(event.target.value)}
        />
        {query ? (
          <button type="button" aria-label={t("common.reset")} className={cn("-mr-2 inline-flex size-10 items-center justify-center rounded", focusRing)} onClick={() => onQuery("")}>
            <X className="size-3.5 text-muted-foreground" aria-hidden />
          </button>
        ) : null}
      </label>
      <button
        type="button"
        className={cn("inline-flex h-11 shrink-0 items-center gap-1 rounded-xl bg-gradient-to-r from-accent to-[#4D7CFF] px-3 text-xs font-semibold text-white shadow-sm transition-all duration-200 hover:brightness-110 active:scale-[0.98]", focusRing)}
        onClick={onAdd}
      >
        <Plus className="size-4" aria-hidden />
        {t("clientMarket.shop.listAdd")}
      </button>
    </div>
  );
}
