"use client";

import * as React from "react";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatUsdMoney } from "@/lib/market-money";
import type { MessageKey } from "@/lib/i18n";
import {
  SHOP_CONTINENTS,
  type ShopContinent,
  type ShopCountryCard,
} from "./host-utils";

export function ShopRegionPicker({
  cards,
  continent,
  country,
  onContinent,
  onCountry,
}: {
  cards: ShopCountryCard[];
  continent: ShopContinent;
  country: string;
  onContinent: (next: ShopContinent) => void;
  onCountry: (code: string) => void;
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
    const minor = card.hasFreeIdle || card.minMonthlyMinor == null ? 0 : card.minMonthlyMinor;
    return t("clientMarket.shop.priceFrom", { price: formatUsdMoney(minor, locale) });
  };

  return (
    <section className="client-market-shop-region" aria-labelledby="client-market-shop-region-title">
      <h2 id="client-market-shop-region-title" className="text-sm font-semibold text-foreground">
        {t("clientMarket.shop.stepRegion")}
      </h2>
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
                <span className="flex min-w-0 items-center gap-1">
                  {known ? <CountryFlag code={card.code} size="compact" title={name} /> : null}
                  <span className="min-w-0 truncate text-xs font-medium leading-4 text-foreground">{name}</span>
                  {known ? <span className="shrink-0 text-[11px] leading-4 text-muted-foreground">{card.code}</span> : null}
                </span>
                <span className="mt-1.5 flex items-baseline justify-between gap-2 text-[11px] leading-4">
                  <span className="truncate text-muted-foreground">{t("clientMarket.shop.idleCount", { count: card.idle })}</span>
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
