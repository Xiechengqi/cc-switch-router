"use client";

import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ClientMarketHost } from "@/lib/types";
import { formatHostOffer, hostDisplayLabel, hostShopSpecLine } from "./host-utils";

export function ShopHostCard({
  host,
  selected,
  onSelect,
}: {
  host: ClientMarketHost;
  selected: boolean;
  onSelect: (host: ClientMarketHost) => void;
}) {
  const { locale, t } = useLocaleText();
  const label = hostDisplayLabel(host);
  const specs = hostShopSpecLine(host);
  return (
    <button
      type="button"
      className={selected ? "client-market-shop-host client-market-shop-host-select is-selected" : "client-market-shop-host client-market-shop-host-select"}
      aria-pressed={selected}
      aria-label={t("clientMarket.shop.selectHost", { name: label })}
      onClick={() => onSelect(host)}
    >
      <span className="flex min-w-0 items-center gap-1.5">
        <CountryFlag code={host.countryCode} />
        <strong className="min-w-0 flex-1 truncate text-xs">{label}</strong>
        <strong className="shrink-0 text-xs tabular-nums">{formatHostOffer(host.dailyRateMinor, locale, host.freeDurationDays, host.cyclePriceMinor)}</strong>
      </span>
      <span className="mt-0.5 flex min-w-0 items-baseline gap-2 text-[11px] leading-4 text-muted-foreground">
        <span className="min-w-0 flex-1 truncate" title={host.hostOwnerEmail}>{host.hostOwnerEmail}</span>
        {specs ? <span className="shrink-0 tabular-nums">{specs}</span> : null}
      </span>
    </button>
  );
}
