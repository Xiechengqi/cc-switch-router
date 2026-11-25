"use client";

import * as React from "react";
import { CircleDot } from "lucide-react";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatUsdMoney } from "@/lib/market-money";
import type { OnlineListing } from "@/lib/online-client-market";
import { cn } from "@/lib/utils";
import {
  SHOP_CONTINENTS,
  SHOP_UNKNOWN_COUNTRY,
  type ShopContinent,
  type ShopCountryCard,
  onlineListingRentable,
  onlineShopSpecLine,
} from "./host-utils";
import { onlineStatusKey } from "./online-market";

const focusRing = "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary";
const selectedFill = "border-transparent bg-primary/10";

export function OnlineShopRegionPicker({
  cards,
  continent,
  country,
  onContinent,
  onCountry,
  emptyAction,
  mine,
  rentable,
}: {
  cards: ShopCountryCard[];
  continent: ShopContinent;
  country: string;
  onContinent: (next: ShopContinent) => void;
  onCountry: (code: string) => void;
  emptyAction?: { label: string; onClick: () => void };
  mine?: { selected: boolean; onSelect: () => void };
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
    if (card.hasFreeIdle || card.minMonthlyMinor == null) return formatUsdMoney(0, locale);
    return formatUsdMoney(card.minMonthlyMinor, locale);
  };
  const pills = (
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
    <div className="flex min-w-0 flex-wrap gap-1.5" role="listbox" aria-label={t("clientMarket.shop.continentLabel")}>
      {mine ? (
        <button
          type="button"
          role="option"
          aria-selected={mine.selected}
          className={cn(
            "inline-flex min-h-8 items-center rounded-full border border-border bg-background px-3 text-xs text-foreground",
            focusRing,
            mine.selected && selectedFill,
          )}
          onClick={mine.onSelect}
        >
          {t("clientMarket.shop.mine")}
        </button>
      ) : null}
      {continents.map((item) => {
        const selected = item === continent && !mine?.selected;
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
          "inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
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
  if (mine?.selected) return pills;
  if (cards.length === 0) {
    return (
      <div className="grid justify-items-start gap-2">
        {mine ? pills : null}
        <p className="text-sm text-muted-foreground">{t(emptyAction ? "clientMarket.shop.rentableEmpty" : "clientMarket.shop.noRegions")}</p>
        {emptyAction ? (
          <button type="button" className={cn("text-sm font-medium text-primary hover:underline", focusRing)} onClick={emptyAction.onClick}>
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
                aria-label={t(selected ? "clientMarket.shop.clearCountry" : "clientMarket.shop.selectCountry", { name })}
                className={cn(
                  "grid w-[7.25rem] content-between gap-1 rounded-xl border border-border bg-background px-2 py-1.5 text-left",
                  focusRing,
                  selected && selectedFill,
                )}
                onClick={() => onCountry(selected ? "" : card.code)}
              >
                <span className="flex min-w-0 items-center gap-1">
                  {known ? <CountryFlag code={card.code} size="compact" title={name} /> : null}
                  <span className="min-w-0 truncate text-xs font-medium leading-4 text-foreground">{name}</span>
                  {known ? <span className="shrink-0 text-[11px] leading-4 text-muted-foreground">{card.code}</span> : null}
                </span>
                <span className="flex items-baseline justify-between gap-1 text-[11px] leading-4 text-muted-foreground">
                  <span>{t("clientMarket.shop.idleCount", { count: card.idle })}</span>
                  <span className="truncate">{t("clientMarket.shop.priceFrom", { price: priceOf(card) })}</span>
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function OnlineShopListingButton({
  listing,
  selected,
  onSelect,
}: {
  listing: OnlineListing;
  selected: boolean;
  onSelect: (listing: OnlineListing) => void;
}) {
  const { locale, t } = useLocaleText();
  const spec = onlineShopSpecLine(listing.hardware);
  return (
    <button
      type="button"
      id={`market-client-${listing.installationId}`}
      aria-pressed={selected}
      aria-label={t("clientMarket.shop.selectHost", { name: listing.label })}
      className={cn(
        "grid gap-0.5 rounded-xl border border-border bg-background px-2.5 py-2 text-left",
        focusRing,
        selected && selectedFill,
      )}
      onClick={() => onSelect(listing)}
    >
      <span className="flex items-baseline justify-between gap-2">
        <span className="min-w-0 truncate text-sm font-medium">{listing.label}</span>
        <span className="shrink-0 text-sm tabular-nums">{listing.cyclePriceMinor ? `${formatUsdMoney(listing.cyclePriceMinor, locale)}${t("online.perMonth")}` : `${formatUsdMoney(0, locale)}${t("online.perMonth")}`}</span>
      </span>
      {spec ? <span className="truncate text-xs text-muted-foreground">{spec}</span> : null}
    </button>
  );
}

export function OnlineShopConfirm({
  listing,
  countryName,
  authed,
  onContinue,
  onLogin,
  busy = false,
  error = "",
}: {
  listing: OnlineListing | null;
  countryName: string;
  authed: boolean;
  onContinue: (listing: OnlineListing) => void;
  onLogin: () => void;
  busy?: boolean;
  error?: string;
}) {
  const { locale, t } = useLocaleText();
  if (!listing) {
    return (
      <aside className="grid content-start gap-2 rounded-xl border border-border bg-background p-4">
        <h2 className="text-sm font-semibold">{t("clientMarket.shop.confirmTitle")}</h2>
        <p className="text-sm text-muted-foreground">{t("clientMarket.shop.confirmEmpty")}</p>
      </aside>
    );
  }
  const hardware = listing.hardware;
  const rentable = onlineListingRentable(listing);
  const status = listing.state !== "listed" ? listing.state : listing.occupied ? "occupied" : listing.healthy ? "available" : "offline";
  const memory = hardware?.memoryBytes != null
    ? t("online.memory", { size: new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(hardware.memoryBytes / 2 ** 30) })
    : null;
  const disk = hardware?.diskBytes != null
    ? t("online.diskSize", { size: new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(hardware.diskBytes / 1000 ** 3) })
    : null;
  return (
    <aside className="grid content-start gap-3 rounded-xl border border-border bg-background p-4" aria-label={t("clientMarket.shop.confirmTitle")}>
      <div>
        <h2 className="text-sm font-semibold">{listing.label}</h2>
        <p className="text-xs text-muted-foreground">{countryName}</p>
      </div>
      <div className="rounded-lg bg-muted px-3 py-2 text-sm">
        <p className="text-xs text-muted-foreground">{t("clientMarket.shop.summarySpecs")}</p>
        <p>{[hardware?.arch, hardware?.cpu != null ? t("online.cpu", { count: hardware.cpu }) : "", memory].filter(Boolean).join(" · ") || t("online.unknown")}</p>
        {disk ? <p className="text-muted-foreground">{disk}</p> : null}
      </div>
      <dl className="grid gap-1 text-xs text-muted-foreground">
        <div className="flex justify-between gap-2"><dt>{t("online.region")}</dt><dd>{countryName}</dd></div>
        <div className="flex justify-between gap-2"><dt>{t("online.probe")}</dt><dd>{listing.observedAt ? new Date(listing.observedAt * 1000).toLocaleString(locale) : "—"}</dd></div>
        <div className="flex justify-between gap-2"><dt>{t("online.hours")}</dt><dd className="tabular-nums">{t("online.hoursValue", { hours: listing.availableHours24 })}</dd></div>
        {listing.egressIp ? <div className="flex justify-between gap-2"><dt>IP</dt><dd>{listing.egressIp}</dd></div> : null}
      </dl>
      {listing.egressDiffers ? <p className="text-xs text-muted-foreground">{t("online.egressDiffers")}</p> : null}
      <p className="text-sm font-semibold">{listing.cyclePriceMinor ? `${formatUsdMoney(listing.cyclePriceMinor, locale)}${t("online.perMonth")}` : `${formatUsdMoney(0, locale)}${t("online.perMonth")}`}</p>
      {listing.trialDays > 0 ? <p className="text-xs text-muted-foreground">{t("online.trialDays")} · {listing.trialDays}</p> : null}
      {listing.freeDays ? <p className="text-xs text-muted-foreground">{t("online.freeDays")} · {listing.freeDays}</p> : null}
      <p className="text-xs text-muted-foreground">{t("online.rootNotice")}</p>
      <p className="text-xs text-muted-foreground">{t("clientMarket.shop.purchaseTerms")}</p>
      {!rentable ? <p role="status" className="rounded-lg bg-primary/10 px-2.5 py-1.5 text-xs leading-5 text-foreground">{t("clientMarket.shop.notIdle", { status: t(onlineStatusKey(status)) })}</p> : null}
      {error ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
      {!authed || rentable ? (
        <button
          type="button"
          disabled={busy}
          className={cn("inline-flex min-h-11 items-center justify-center rounded-lg bg-primary px-3 text-sm font-medium text-primary-foreground disabled:opacity-50", focusRing)}
          onClick={() => (authed ? onContinue(listing) : onLogin())}
        >
          {t(authed ? "clientMarket.shop.continue" : "nav.login")}
        </button>
      ) : null}
    </aside>
  );
}
