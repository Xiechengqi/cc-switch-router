"use client";

import * as React from "react";
import { CircleDot, Plus, Search, X } from "lucide-react";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatUsdMoney } from "@/lib/market-money";
import { cn } from "@/lib/utils";
import {
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
    if (card.hasFreeIdle || card.minMonthlyMinor == null) return t("online.free");
    return formatUsdMoney(card.minMonthlyMinor, locale);
  };
  const pills = (
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
    <div className="flex min-w-0 flex-1 flex-wrap gap-1.5" role="listbox" aria-label={t("clientMarket.shop.continentLabel")}>
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

export function OnlineClientCard({
  name,
  price,
  healthy,
  status,
  detail,
  spec,
  specLabel,
  hours,
  hoursLabel,
  selected = false,
  muted = false,
  cardId,
  onSelect,
  actions,
  extra,
}: {
  name: string;
  price?: string;
  healthy?: boolean | null;
  status: string;
  detail?: string;
  spec?: string;
  specLabel?: string;
  hours?: string;
  hoursLabel?: string;
  selected?: boolean;
  muted?: boolean;
  cardId?: string;
  onSelect?: () => void;
  actions?: React.ReactNode;
  extra?: React.ReactNode;
}) {
  const dot = healthy == null ? "bg-slate-300" : healthy ? "bg-emerald-500" : "bg-rose-500";
  return (
    <article
      id={cardId}
      tabIndex={cardId ? -1 : undefined}
      className={cn(
        "grid min-h-60 min-w-0 scroll-mt-20 content-start gap-2.5 rounded-xl border p-3 shadow-sm",
        muted ? "bg-slate-50" : "bg-white",
        selected ? "border-primary ring-2 ring-primary/20" : healthy === false ? "border-rose-200" : "border-slate-200",
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
      {detail ? <p className="truncate text-[11px] text-slate-500" title={detail}>{detail}</p> : null}
      {spec || hours || specLabel || hoursLabel ? (
        <dl className="grid grid-cols-2 gap-2">
          <div className="min-w-0">
            <dt className="truncate text-[11px] text-muted-foreground">{hoursLabel}</dt>
            <dd className="truncate text-[11px] font-semibold tabular-nums text-foreground">{hours || "—"}</dd>
          </div>
          <div className="min-w-0">
            <dt className="truncate text-[11px] text-muted-foreground">{specLabel}</dt>
            <dd className="truncate text-[11px] font-semibold text-foreground" title={spec || undefined}>{spec || "—"}</dd>
          </div>
        </dl>
      ) : null}
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
