"use client";

import * as React from "react";
import { Check } from "lucide-react";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatUsdMoney } from "@/lib/market-money";
import type { MessageKey } from "@/lib/i18n";
import {
  SHOP_CONTINENTS,
  type ShopContinent,
  type ShopCountryCard,
  type ShopCountrySort,
} from "./host-utils";

export function ShopRegionPicker({
  cards,
  continent,
  country,
  sort,
  arch,
  minimumCpu,
  minimumMemory,
  onContinent,
  onCountry,
  onSort,
  onSpecs,
}: {
  cards: ShopCountryCard[];
  continent: ShopContinent;
  country: string;
  sort: ShopCountrySort;
  arch: string;
  minimumCpu: string;
  minimumMemory: string;
  onContinent: (next: ShopContinent) => void;
  onCountry: (code: string) => void;
  onSort: (next: ShopCountrySort) => void;
  onSpecs: (next: { arch: string; minimumCpu: string; minimumMemory: string }) => void;
}) {
  const { locale, t } = useLocaleText();
  const names = React.useMemo(() => new Intl.DisplayNames([locale], { type: "region" }), [locale]);
  const visible = continent === "all" ? cards : cards.filter((card) => card.continent === continent);
  const counts = React.useMemo(() => {
    const next = new Map<ShopContinent, number>(SHOP_CONTINENTS.map((item) => [item, 0]));
    for (const card of cards) {
      next.set(card.continent, (next.get(card.continent) || 0) + 1);
      next.set("all", (next.get("all") || 0) + 1);
    }
    return next;
  }, [cards]);

  const priceLabel = (card: ShopCountryCard) => {
    if (card.hasFreeIdle || card.minMonthlyMinor == null) return t("clientMarket.shop.freeOffer");
    return t("clientMarket.shop.priceFrom", { price: formatUsdMoney(card.minMonthlyMinor, locale) });
  };

  return (
    <section className="client-market-shop-region" aria-labelledby="client-market-shop-region-title">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 id="client-market-shop-region-title" className="text-sm font-semibold text-foreground">
          {t("clientMarket.shop.stepRegion")}
        </h2>
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <label className="flex items-center gap-1">
            <span>{t("hostHealth.arch")}</span>
            <select aria-label={t("hostHealth.arch")} value={arch} className="h-9 rounded-lg border border-border bg-card px-2 text-sm text-foreground" onChange={(event) => onSpecs({ arch: event.target.value, minimumCpu, minimumMemory })}>
              <option value="">{t("hostHealth.allArch")}</option>
              <option value="aarch64">ARM64</option>
              <option value="x86_64">x86_64</option>
            </select>
          </label>
          <label className="flex items-center gap-1">
            <span>{t("hostHealth.minCpu")}</span>
            <input aria-label={t("hostHealth.minCpu")} type="number" min="0" step="0.5" value={minimumCpu} className="h-9 w-16 rounded-lg border border-border bg-card px-2 text-sm text-foreground" onChange={(event) => onSpecs({ arch, minimumCpu: event.target.value, minimumMemory })} />
          </label>
          <label className="flex items-center gap-1">
            <span>{t("hostHealth.minMemory")}</span>
            <input aria-label={t("hostHealth.minMemory")} type="number" min="0" step="0.5" value={minimumMemory} className="h-9 w-16 rounded-lg border border-border bg-card px-2 text-sm text-foreground" onChange={(event) => onSpecs({ arch, minimumCpu, minimumMemory: event.target.value })} />
          </label>
          <label className="flex items-center gap-2">
          <span>{t("clientMarket.shop.sortLabel")}</span>
          <select
            value={sort}
            aria-label={t("clientMarket.shop.sortLabel")}
            className="h-9 rounded-lg border border-border bg-card px-2 text-sm text-foreground"
            onChange={(event) => onSort(event.target.value as ShopCountrySort)}
          >
            {(["name", "idle", "price"] as const).map((item) => (
              <option key={item} value={item}>{t(`clientMarket.shop.sort.${item}` as MessageKey)}</option>
            ))}
          </select>
        </label>
        </div>
      </div>
      <div className="client-market-shop-continents" role="radiogroup" aria-label={t("clientMarket.shop.continentLabel")}>
        {SHOP_CONTINENTS.map((item) => {
          const selected = item === continent;
          return (
            <button
              key={item}
              type="button"
              role="radio"
              aria-checked={selected}
              className={selected ? "client-market-shop-continent is-selected" : "client-market-shop-continent"}
              onClick={() => onContinent(item)}
            >
              {t(`clientMarket.shop.continent.${item}` as MessageKey)}
              <span>{counts.get(item) || 0}</span>
            </button>
          );
        })}
      </div>
      {visible.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("clientMarket.shop.noRegions")}</p>
      ) : (
        <div className="client-market-shop-countries">
          {visible.map((card) => {
            const known = card.code !== "unknown";
            const name = known ? names.of(card.code) || card.code : t("clientMarket.shop.unknownRegion");
            const selected = card.code === country;
            return (
              <button
                key={card.code || "unknown"}
                type="button"
                aria-pressed={selected}
                aria-label={selected ? t("clientMarket.shop.clearCountry") : t("clientMarket.shop.selectCountry", { name })}
                className={selected ? "client-market-shop-country is-selected" : "client-market-shop-country"}
                onClick={() => onCountry(selected ? "" : card.code)}
              >
                <span className="flex min-w-0 items-center gap-2">
                  {known ? <CountryFlag code={card.code} title={name} /> : null}
                  <span className="min-w-0">
                    <span className="block truncate text-sm font-medium text-foreground">{name}</span>
                    <span className="block text-[11px] leading-4 text-muted-foreground">{known ? card.code : "—"}</span>
                  </span>
                </span>
                {selected ? <Check className="h-4 w-4 shrink-0 text-primary" aria-hidden /> : null}
                <span className="mt-3 flex items-end justify-between gap-2 text-xs">
                  <span className="text-muted-foreground">
                    {t("clientMarket.shop.idleCount", { count: card.idle })}
                    <span className="mt-0.5 block">{t("clientMarket.shop.totalCount", { count: card.total })}</span>
                  </span>
                  <strong className="shrink-0 tabular-nums text-foreground">{priceLabel(card)}</strong>
                </span>
              </button>
            );
          })}
        </div>
      )}
    </section>
  );
}
