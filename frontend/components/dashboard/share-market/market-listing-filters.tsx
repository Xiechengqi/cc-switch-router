"use client";

import type { ReactNode } from "react";
import { ArrowDownUp, CircleDot, Search, X } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ShareMarketListing, ShareMarketProviderFamily } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  PROVIDER_FAMILY_KEYS,
} from "@/components/dashboard/share-market/market-utils";
import {
  MARKET_CATALOG_SORTS,
  listingFamilyTabs,
  type MarketCatalogSort,
} from "@/components/dashboard/share-market/buyer-catalog-utils";

export function MarketListingFilters({
  listings,
  family,
  query,
  onFamilyChange,
  onQueryChange,
  leading,
  trailing,
  mine = false,
  mineCount = 0,
  mineEnabled = false,
  onMineChange,
  idleOnly = false,
  onIdleOnlyChange,
  sort,
  onSortChange,
}: {
  listings: ShareMarketListing[];
  family: ShareMarketProviderFamily | "all";
  query: string;
  onFamilyChange: (family: ShareMarketProviderFamily | "all") => void;
  onQueryChange: (query: string) => void;
  leading?: ReactNode;
  trailing?: ReactNode;
  mine?: boolean;
  mineCount?: number;
  mineEnabled?: boolean;
  onMineChange?: (mine: boolean) => void;
  idleOnly?: boolean;
  onIdleOnlyChange?: (idleOnly: boolean) => void;
  sort?: MarketCatalogSort;
  onSortChange?: (sort: MarketCatalogSort) => void;
}) {
  const { t } = useLocaleText();
  const familyTabs = listingFamilyTabs(listings);
  const showMine = !!onMineChange;
  const showIdleOnly = !!onIdleOnlyChange;
  const showSort = !!onSortChange && !!sort;
  if (!familyTabs.length && !query && !leading && !trailing && !showMine && !showIdleOnly && !showSort) return null;
  return (
    // Wraps instead of scrolling sideways: the provider tabs, the availability toggle and
    // the sort are three different decisions, and on a narrow screen each deserves a line
    // of its own rather than being pushed off the edge of a single overflowing strip.
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      {leading}
      <div className="flex min-w-0 flex-1 basis-full items-center gap-1 overflow-x-auto sm:basis-auto" role="tablist" aria-label={t("shareMarket.catalog.familyFilter")}>
        {showMine ? (
          <button
            type="button"
            role="tab"
            aria-selected={mine}
            title={t("shareMarket.catalog.mineHint")}
            disabled={!mine && !mineEnabled}
            className={cn(
              "inline-flex h-9 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
              mine
                ? "bg-sky-100 font-semibold text-sky-800"
                : "bg-slate-100 font-medium text-slate-600 hover:bg-slate-200 hover:text-slate-900",
              !mine && !mineEnabled ? "cursor-not-allowed opacity-50 hover:bg-slate-100 hover:text-slate-600" : null,
            )}
            onClick={() => {
              if (mine) onMineChange(false);
              else if (mineEnabled) onMineChange(true);
            }}
          >
            <span>{t("shareMarket.catalog.mine")}</span>
            <span className={cn("tabular-nums", mine ? "text-sky-600" : "text-slate-400")}>{mineCount}</span>
          </button>
        ) : null}
        {familyTabs.map((item) => {
          const selectedFamily = family === item.value;
          return (
            <button
              key={item.value}
              type="button"
              role="tab"
              aria-selected={selectedFamily}
              title={t("shareMarket.catalog.familyIdleHint", { family: t(PROVIDER_FAMILY_KEYS[item.value]), count: item.idle })}
              className={cn(
                "inline-flex h-9 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
                selectedFamily
                  ? "bg-sky-100 font-semibold text-sky-800"
                  : "bg-slate-100 font-medium text-slate-600 hover:bg-slate-200 hover:text-slate-900",
              )}
              onClick={() => onFamilyChange(selectedFamily ? "all" : item.value)}
            >
              <span>{t(PROVIDER_FAMILY_KEYS[item.value])}</span>
              <span className={cn("tabular-nums", selectedFamily ? "text-sky-600" : "text-slate-400")}>{item.idle}</span>
            </button>
          );
        })}
      </div>
      {showIdleOnly ? (
        <button
          type="button"
          aria-pressed={idleOnly}
          title={t("shareMarket.catalog.availabilityFilter")}
          className={cn(
            "inline-flex h-9 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
            idleOnly
              ? "bg-sky-100 font-semibold text-sky-800"
              : "bg-slate-100 font-medium text-slate-600 hover:bg-slate-200 hover:text-slate-900",
          )}
          onClick={() => onIdleOnlyChange(!idleOnly)}
        >
          <CircleDot className={cn("h-3.5 w-3.5", idleOnly ? "text-sky-600" : "text-slate-400")} />
          <span className="whitespace-nowrap">{t("shareMarket.catalog.onlyRentable")}</span>
        </button>
      ) : null}
      {showSort ? (
        <label className="flex h-9 shrink-0 items-center gap-1.5 rounded-md border border-slate-200 bg-white px-2.5 shadow-sm hover:border-slate-300 focus-within:outline focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-primary">
          <ArrowDownUp className="h-3.5 w-3.5 shrink-0 text-slate-400" />
          <select
            aria-label={t("shareMarket.catalog.sort")}
            value={sort}
            onChange={(event) => onSortChange(event.target.value as MarketCatalogSort)}
            className="min-w-0 bg-transparent text-xs outline-none"
          >
            {MARKET_CATALOG_SORTS.map((value) => (
              <option key={value} value={value}>{t(`shareMarket.catalog.sort.${value}`)}</option>
            ))}
          </select>
        </label>
      ) : null}
      <label className="flex h-9 w-44 shrink-0 grow items-center gap-1.5 sm:grow-0 rounded-md border border-slate-200 bg-white px-2.5 text-sm shadow-sm hover:border-slate-300 focus-within:outline focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-primary">
        <Search className="h-3.5 w-3.5 shrink-0 text-slate-400" />
        <input
          aria-label={t("shareMarket.catalog.searchCompact")}
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          className="min-w-0 flex-1 bg-transparent text-xs outline-none"
          placeholder={t("shareMarket.catalog.searchCompact")}
        />
        {query ? (
          <button
            type="button"
            aria-label={t("common.reset")}
            className="rounded active:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
            onClick={() => onQueryChange("")}
          >
            <X className="h-3.5 w-3.5 text-slate-400" />
          </button>
        ) : null}
      </label>
      {trailing ? <div className="shrink-0">{trailing}</div> : null}
    </div>
  );
}
