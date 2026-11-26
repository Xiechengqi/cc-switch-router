"use client";

import * as React from "react";
import { CircleDot, Globe2, Plus, Search, X } from "lucide-react";
import { formatAgeDaysOrHours } from "@/components/dashboard/share-dashboard-utils";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatUsdMoney } from "@/lib/market-money";
import type { OnlineListing } from "@/lib/online-client-market";
import { cn } from "@/lib/utils";
import {
  onlineListingAvailability,
  onlineStatusKey,
  SHOP_CONTINENTS,
  SHOP_UNKNOWN_COUNTRY,
  type ShopContinent,
  type ShopCountryCard,
} from "./host-utils";

const focusRing = "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary";
const selectedFill = "border-transparent bg-primary/10";

/** Same breakpoints as Share cards: one column until the card can keep its price and spec readable. */
export const ONLINE_CLIENT_CARD_GRID_CLASS =
  "grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2 md:grid-cols-3 xl:grid-cols-4";

export function OnlineShopRegionPicker({
  cards,
  continent,
  country,
  onContinent,
  onCountry,
  emptyAction,
  rentable,
}: {
  cards: ShopCountryCard[];
  continent: ShopContinent;
  country: string;
  onContinent: (next: ShopContinent) => void;
  onCountry: (code: string) => void;
  emptyAction?: { label: string; onClick: () => void };
  rentable?: { pressed: boolean; label: string; onClick: () => void };
}) {
  const { locale, t } = useLocaleText();
  const names = React.useMemo(() => new Intl.DisplayNames([locale], { type: "region" }), [locale]);
  const present = new Set(cards.map((card) => card.continent));
  const continents = SHOP_CONTINENTS.filter((item) => item === "all" || present.has(item));
  const visible = cards.filter((card) => continent === "all" || card.continent === continent);
  const nameOf = (card: ShopCountryCard) => (
    card.code === SHOP_UNKNOWN_COUNTRY ? t("clientMarket.shop.unknownRegion") : names.of(card.code) || card.code
  );
  const priceOf = (card: ShopCountryCard) => {
    if (card.hasFreeIdle || card.minMonthlyMinor == null) return t("online.free");
    return formatUsdMoney(card.minMonthlyMinor, locale);
  };
  const pills = (
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
    <div className="flex min-w-0 flex-1 flex-wrap gap-1.5" role="listbox" aria-label={t("clientMarket.shop.continentLabel")}>
      {continents.map((item) => {
        const selected = item === continent;
        return (
          <button
            key={item}
            type="button"
            role="option"
            aria-selected={selected}
            className={cn(
              "inline-flex min-h-8 items-center rounded-full border border-border bg-background px-3 text-xs text-foreground",
              focusRing,
              selected && selectedFill,
            )}
            onClick={() => onContinent(item)}
          >
            {t(`clientMarket.shop.continent.${item}`)}
          </button>
        );
      })}
    </div>
    {rentable ? (
      <button
        type="button"
        aria-pressed={rentable.pressed}
        className={cn(
          "ml-auto inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
          rentable.pressed ? "bg-sky-100 font-semibold text-sky-800" : "bg-slate-100 font-medium text-slate-600 hover:bg-slate-200",
        )}
        onClick={rentable.onClick}
      >
        <CircleDot className={cn("h-3.5 w-3.5", rentable.pressed ? "text-sky-600" : "text-slate-400")} />
        <span>{rentable.label}</span>
      </button>
    ) : null}
    </div>
  );
  if (cards.length === 0) {
    return (
      <div className="grid gap-2">
        {pills}
        <p className="text-sm text-muted-foreground">
          {t(emptyAction ? "clientMarket.shop.rentableEmpty" : "clientMarket.shop.noRegions")}
        </p>
        {emptyAction ? (
          <button
            type="button"
            className={cn("text-sm font-medium text-primary hover:underline", focusRing)}
            onClick={emptyAction.onClick}
          >
            {emptyAction.label}
          </button>
        ) : null}
      </div>
    );
  }
  return (
    <div className="grid gap-3">
      {pills}
      {visible.length === 0 ? <p className="text-sm text-muted-foreground">{t("clientMarket.shop.noRegions")}</p> : (
        <div className="flex min-w-0 flex-wrap gap-1.5">
          {visible.map((card) => {
            const known = card.code !== SHOP_UNKNOWN_COUNTRY;
            const selected = card.code === country;
            const name = nameOf(card);
            return (
              <button
                key={card.code}
                type="button"
                aria-pressed={selected}
                aria-label={t(
                  selected ? "clientMarket.shop.clearCountry" : "clientMarket.shop.selectCountry",
                  { name },
                )}
                className={cn(
                  "grid w-fit min-w-28 max-w-full content-between gap-1 rounded-xl border border-border bg-background px-2 py-1.5 text-left",
                  focusRing,
                  selected && selectedFill,
                )}
                onClick={() => onCountry(selected ? "" : card.code)}
              >
                <span className="flex min-w-0 items-center gap-1">
                  {known ? <CountryFlag code={card.code} size="compact" title={name} /> : null}
                  <span className="min-w-0 truncate text-xs font-medium leading-4 text-foreground">
                    {name}
                  </span>
                  {known ? (
                    <span className="shrink-0 text-[11px] leading-4 text-muted-foreground">{card.code}</span>
                  ) : null}
                </span>
                <span className="flex flex-wrap items-baseline justify-between gap-x-3 text-[11px] leading-4 text-muted-foreground">
                  <span className="whitespace-nowrap">
                    {t("clientMarket.shop.idleCount", { count: card.idle })}
                  </span>
                  {card.idle > 0 && (
                    <span className="whitespace-nowrap">
                      {t("clientMarket.shop.priceFrom", { price: priceOf(card) })}
                    </span>
                  )}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function ClientSpecs({ listing }: { listing: Pick<OnlineListing, "country" | "hardware"> }) {
  const { locale, t } = useLocaleText();
  const code = listing.country?.toUpperCase();
  const name =
    code && /^[A-Z]{2}$/.test(code) && new Intl.DisplayNames([locale], { type: "region" }).of(code);
  const hw = listing.hardware;
  const parts = [
    hw?.cpu ? t(hw.cpu === 1 ? "clientMarket.shop.core" : "clientMarket.shop.cores", { count: hw.cpu }) : "",
    hw?.memoryBytes
      ? new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(hw.memoryBytes / 2 ** 30) + " GiB"
      : "",
    hw?.arch,
  ].filter(Boolean);
  return (
    <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5 text-muted-foreground">
      {name ? (
        <CountryFlag code={code} title={name} />
      ) : (
        <Globe2
          role="img"
          aria-label={t("clientMarket.shop.unknownRegion")}
          className="size-3.5 text-muted-foreground"
        />
      )}
      <span>{parts.join(" · ") || t("clientMarket.shop.noSpecs")}</span>
    </p>
  );
}

type ClientReliabilityData = Pick<OnlineListing,
  "onlineRate24h" | "observedMinutes24h" | "observationCoverage24h" | "registeredAt"
>;

export function ClientReliability({ listing, inline = false }: { listing: ClientReliabilityData; inline?: boolean }) {
  const { t, locale } = useLocaleText();
  const rate = listing.observedMinutes24h > 0 ? listing.onlineRate24h : null;
  const pair = inline
    ? "grid grid-cols-[auto_auto] items-baseline justify-start gap-x-2"
    : "grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-2";
  return (
    <dl className={inline ? "flex flex-wrap gap-x-6 gap-y-2 text-xs leading-5" : "grid gap-1 text-xs"}>
      <div className={pair}>
        <dt className="text-muted-foreground">{t("clientMarket.shop.uptime")}</dt>
        <dd className="font-semibold tabular-nums text-foreground">
          {rate == null
            ? t("clientMarket.shop.noSamples")
            : new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 }).format(rate / 100)}
        </dd>
        {rate != null && listing.observationCoverage24h < 50 && (
          <dd className={cn("col-span-2 text-[11px] text-muted-foreground", !inline && "mt-1 text-right")}>
            {t("clientMarket.shop.samples", { count: listing.observedMinutes24h })}
          </dd>
        )}
      </div>
      <div className={pair} title={t("clientMarket.shop.registeredSince")}>
        <dt className="text-muted-foreground">{t("dashboard.runningDuration")}</dt>
        <dd className="font-semibold tabular-nums text-foreground">{formatAgeDaysOrHours(listing.registeredAt, locale)}</dd>
      </div>
    </dl>
  );
}

export function ClientOfferCard({
  listing,
  authenticated,
  busy,
  onRent,
  extra,
}: {
  listing: OnlineListing;
  authenticated: boolean;
  busy: boolean;
  onRent: () => void;
  extra?: React.ReactNode;
}) {
  const { locale, t } = useLocaleText();
  const availability = onlineListingAvailability(listing);
  const price = listing.owned
    ? t("clientMarket.shop.selfFree")
    : listing.cyclePriceMinor > 0
      ? formatUsdMoney(listing.cyclePriceMinor, locale)
      : t("online.free");
  const days = new Intl.NumberFormat(locale, { style: "unit", unit: "day", unitDisplay: "long" });
  const term =
    !listing.owned &&
    (listing.trialDays > 0
      ? t("lease.trialDays", { duration: days.format(listing.trialDays) })
      : listing.freeDays
        ? days.format(listing.freeDays)
        : "");
  return (
    <article
      id={`market-client-${listing.installationId}`}
      tabIndex={-1}
      className="flex min-w-0 scroll-mt-20 flex-col gap-3 rounded-xl border border-border bg-background p-4 shadow-sm"
    >
      <header className="grid min-w-0 gap-1">
        <h3 id={`client-offer-name-${listing.installationId}`} className="break-all font-mono text-sm font-semibold tracking-tight text-foreground">
          {listing.label}
        </h3>
        <ClientSpecs listing={listing} />
      </header>
      <ClientReliability listing={listing} />
      {listing.egressIp && (
        <details className="text-xs text-muted-foreground">
          <summary className={cn("w-fit cursor-pointer rounded", focusRing)}>
            {t("clientMarket.shop.network")}
          </summary>
          <p className="mt-1 break-all font-mono">IP {listing.egressIp}</p>
        </details>
      )}
      <footer className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-border pt-3">
        <div>
          <strong className="text-base tabular-nums text-foreground">{price}</strong>
          {!listing.owned && listing.cyclePriceMinor > 0 && (
            <span className="text-xs text-muted-foreground">{t("online.perMonth")}</span>
          )}
          {term && <p className="text-xs text-muted-foreground">{term}</p>}
        </div>
        {availability === "available" ? (
          <button
            type="button"
            disabled={busy}
            aria-describedby={`client-offer-name-${listing.installationId}`}
            onClick={onRent}
            className={cn(
              "min-h-10 rounded-lg bg-primary px-4 text-sm font-semibold text-primary-foreground disabled:opacity-60",
              focusRing,
            )}
          >
            {t(authenticated ? "clientMarket.shop.rent" : "clientMarket.shop.guestRent")}
          </button>
        ) : (
          <span className="text-xs text-muted-foreground">{t(onlineStatusKey(availability))}</span>
        )}
      </footer>
      {extra}
    </article>
  );
}

export function OnlineClientCard({
  name,
  price,
  status,
  detail,
  spec,
  specLabel,
  listing,
  selected = false,
  muted = false,
  cardId,
  onSelect,
  actions,
  extra,
}: {
  name: string;
  price?: string;
  status: string;
  detail?: string;
  spec?: string;
  specLabel?: string;
  listing?: OnlineListing;
  selected?: boolean;
  muted?: boolean;
  cardId?: string;
  onSelect?: () => void;
  actions?: React.ReactNode;
  extra?: React.ReactNode;
}) {
  const offline = listing?.availability === "offline";
  const dot = offline ? "bg-rose-500" : listing?.healthy ? "bg-emerald-500" : "bg-slate-300";
  return (
    <article
      id={cardId}
      tabIndex={cardId ? -1 : undefined}
      className={cn(
        "grid min-h-60 min-w-0 scroll-mt-20 content-start gap-2.5 rounded-xl border p-3 shadow-sm",
        muted ? "bg-slate-50" : "bg-white",
        selected ? "border-primary ring-2 ring-primary/20" : offline ? "border-rose-200" : "border-slate-200",
      )}
    >
      <header className="flex min-w-0 items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className={cn("h-2 w-2 shrink-0 rounded-full", dot)} />
          {onSelect ? (
            <button
              type="button"
              className={cn("min-w-0 truncate text-left font-mono text-xs font-semibold text-foreground hover:underline", focusRing)}
              onClick={onSelect}
            >
              {name}
            </button>
          ) : (
            <strong className="min-w-0 truncate font-mono text-xs font-semibold text-foreground" title={name}>{name}</strong>
          )}
        </div>
        {price ? <strong className="shrink-0 whitespace-nowrap text-[13px] font-semibold tabular-nums text-slate-900">{price}</strong> : null}
      </header>
      <p className="truncate text-[11px] font-semibold text-slate-700">{status}</p>
      {detail ? <p className="break-words text-[11px] text-slate-500 [overflow-wrap:anywhere]">{detail}</p> : null}
      {spec && <p className="break-words text-xs text-muted-foreground" title={specLabel}>{spec}</p>}
      {listing && <ClientReliability listing={listing}/>}
      {actions ? <div className="mt-auto flex flex-wrap gap-2">{actions}</div> : null}
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
      <h3 className="shrink-0 whitespace-nowrap text-xs font-semibold uppercase tracking-wide text-slate-500">
        {t("clientMarket.shop.filter.listed")}
        <span className="ml-1.5 tabular-nums text-slate-400">{listedCount}</span>
      </h3>
      <label className="flex h-9 w-44 shrink-0 grow items-center gap-1.5 rounded-md border border-slate-200 bg-white px-2.5 text-sm shadow-sm sm:ml-auto sm:grow-0">
        <Search className="h-3.5 w-3.5 shrink-0 text-slate-400" />
        <input
          aria-label={t("clientMarket.shop.sellingSearch")}
          value={query}
          placeholder={t("clientMarket.shop.sellingSearch")}
          className="min-w-0 flex-1 bg-transparent text-xs outline-none"
          onChange={(event) => onQuery(event.target.value)}
        />
        {query ? (
          <button type="button" aria-label={t("common.reset")} className={cn("rounded", focusRing)} onClick={() => onQuery("")}>
            <X className="h-3.5 w-3.5 text-slate-400" />
          </button>
        ) : null}
      </label>
      <button
        type="button"
        className={cn("inline-flex h-9 shrink-0 items-center gap-1 rounded-lg border border-slate-200 bg-white px-2.5 text-xs font-medium", focusRing)}
        onClick={onAdd}
      >
        <Plus className="h-4 w-4" />
        {t("clientMarket.shop.listAdd")}
      </button>
    </div>
  );
}
