"use client";

import { Check } from "lucide-react";
import { CountryFlag } from "@/components/common/country-flag";
import { ProviderContactButton } from "@/components/common/provider-contacts";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ClientMarketHost } from "@/lib/types";
import { HostHardwareSummary, HostHealthSummary } from "./host-health";
import { formatHostOffer, hostDisplayLabel } from "./host-utils";

export function ShopHostCard({
  host,
  selected,
  onSelect,
  onHealthOpen,
}: {
  host: ClientMarketHost;
  selected: boolean;
  onSelect: (host: ClientMarketHost) => void;
  onHealthOpen: (hostId: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const label = hostDisplayLabel(host);
  const listed = host.createdAt ? new Date(host.createdAt) : null;
  const listedLabel = listed && !Number.isNaN(listed.getTime())
    ? t("clientMarket.shop.listedAt", { date: new Intl.DateTimeFormat(locale, { year: "numeric", month: "short", day: "numeric" }).format(listed) })
    : "";
  return (
    <article className={selected ? "client-market-shop-host is-selected" : "client-market-shop-host"}>
      <button
        type="button"
        className="client-market-shop-host-select"
        aria-pressed={selected}
        aria-label={t("clientMarket.shop.selectHost", { name: label })}
        onClick={() => onSelect(host)}
      >
        <span className="flex min-w-0 items-start justify-between gap-2">
          <span className="min-w-0">
            <span className="flex min-w-0 items-center gap-2">
              <CountryFlag code={host.countryCode} />
              <strong className="truncate text-sm">{label}</strong>
            </span>
            <span className="mt-1 block truncate text-xs text-muted-foreground" title={host.hostOwnerEmail}>{host.hostOwnerEmail}</span>
          </span>
          {selected ? <Check className="h-4 w-4 shrink-0 text-primary" aria-hidden /> : null}
        </span>
        <span className="mt-3 block"><HostHardwareSummary health={host.hostHealth} /></span>
        <span className="mt-3 flex items-end justify-between gap-2">
          <strong className="text-sm tabular-nums">{formatHostOffer(host.dailyRateMinor, locale, host.freeDurationDays, host.cyclePriceMinor)}</strong>
          {listedLabel ? <span className="text-xs text-muted-foreground">{listedLabel}</span> : null}
        </span>
      </button>
      <div className="flex items-center justify-between gap-2 border-t border-border px-3 py-2">
        <HostHealthSummary health={host.hostHealth} hostLabel={label} variant="table" onOpen={() => onHealthOpen(host.id)} />
        <ProviderContactButton contacts={host.contacts} />
      </div>
    </article>
  );
}
