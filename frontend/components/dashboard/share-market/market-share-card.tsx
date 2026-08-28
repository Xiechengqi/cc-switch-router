"use client";

import * as React from "react";
import { UserRound } from "lucide-react";
import { ShareProviderStatusPanel } from "@/components/dashboard/share-provider-status-panel";
import { MarketProviderLogos } from "@/components/dashboard/share-market/market-share-identity";
import { SubdomainCopyButton } from "@/components/dashboard/subdomain-copy-button";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ShareMarketListing, ShareMarketSeat } from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";
import { cn } from "@/lib/utils";
import {
  formatSeatPrice,
  isSeatIdle,
  listingIdleCount,
  marketProviderStatusView,
} from "@/components/dashboard/share-market/market-utils";
import { catalogSeatPreview } from "@/components/dashboard/share-market/buyer-catalog-utils";

export const MARKET_SHARE_CARD_GRID_CLASS =
  "grid min-w-0 grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4";

export function listingUptimeValue(listing: Pick<ShareMarketListing, "reliability">) {
  return listing.reliability.onlineRate24h == null
    ? "-"
    : `${listing.reliability.onlineRate24h.toFixed(1)}%`;
}

export function listingPerformanceValue(listing: Pick<ShareMarketListing, "performance">) {
  const ttft = listing.performance.averageTtftMs == null
    ? "-"
    : `${(listing.performance.averageTtftMs / 1_000).toFixed(2)}s`;
  const tps = listing.performance.averageTps == null
    ? "-"
    : listing.performance.averageTps.toFixed(1);
  return { ttft, tps, label: `${ttft} / ${tps === "-" ? "-" : `${tps} tok/s`}` };
}

export function listingCardId(prefix: string, shareId: string) {
  return `share-market-${prefix}-${shareId}`;
}

export function shouldOpenMarketShareCard(
  event: React.MouseEvent<HTMLElement>,
  pointerDown: { x: number; y: number } | null,
) {
  if (pointerDown) {
    const deltaX = Math.abs(event.clientX - pointerDown.x);
    const deltaY = Math.abs(event.clientY - pointerDown.y);
    if (deltaX > 4 || deltaY > 4) return false;
  }
  const selection = window.getSelection();
  if (selection && !selection.isCollapsed && selection.toString().trim()) return false;
  const target = event.target as HTMLElement | null;
  if (target?.closest("button,a,input,textarea,label,[data-no-card-open],[data-no-row-drawer],[data-no-row-toggle]")) return false;
  return true;
}

export function MarketShareCardMetric({
  label,
  value,
  title,
}: {
  label: string;
  value: string;
  title?: string;
}) {
  return (
    <div className="min-w-0">
      <dt className="truncate text-[11px] text-muted-foreground">{label}</dt>
      <dd className="truncate text-[11px] font-semibold tabular-nums text-foreground" title={title}>{value}</dd>
    </div>
  );
}

function compactSeatTerms(
  seat: Pick<ShareMarketSeat, "parallelLimit" | "tokenLimit" | "tokenPeriod">,
  locale: string,
  unlimited: string,
  periodLabel: string,
) {
  const parallel = seat.parallelLimit == null ? "P∞" : `P${seat.parallelLimit}`;
  const tokens = seat.tokenLimit == null
    ? unlimited
    : `${formatTokenMillions(seat.tokenLimit, locale)}/${periodLabel}`;
  return `${parallel} · ${tokens}`;
}

export function CatalogSeatPreview({
  seat,
  onSelect,
}: {
  seat: ShareMarketSeat;
  onSelect: () => void;
}) {
  const { locale, t } = useLocaleText();
  return (
    <button
      type="button"
      className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded px-1.5 py-1 text-left text-[11px] hover:bg-slate-50 active:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
      onClick={onSelect}
    >
      <span className="font-semibold text-slate-700">#{seat.position}</span>
      <span className="truncate text-slate-500">
        {compactSeatTerms(seat, locale, t("common.unlimited"), t(`shareMarket.period.${seat.tokenPeriod}`))}
      </span>
      <strong className="shrink-0 tabular-nums text-slate-800">
        {formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}
      </strong>
    </button>
  );
}

export function CatalogSeatPreviewList({
  listing,
  onOpen,
}: {
  listing: ShareMarketListing;
  onOpen: (seat?: ShareMarketSeat) => void;
}) {
  const { t } = useLocaleText();
  const idle = listing.seats.filter(isSeatIdle);
  const preview = catalogSeatPreview(listing.seats);
  return (
    <div className="grid content-start gap-0.5 border-t border-slate-100 pt-1.5">
      {preview.map((seat) => (
        <CatalogSeatPreview key={seat.id} seat={seat} onSelect={() => onOpen(seat)} />
      ))}
      {idle.length > preview.length ? (
        <button
          type="button"
          className="rounded px-1.5 pt-1 text-left text-[10px] font-medium text-accent hover:underline active:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
          onClick={() => onOpen()}
        >
          {t("shareMarket.catalog.moreSeats", { count: idle.length - preview.length })}
        </button>
      ) : null}
      {!idle.length ? (
        <button
          type="button"
          className="rounded px-1.5 py-2 text-left text-xs text-slate-500 active:bg-slate-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
          onClick={() => onOpen()}
        >
          {t("shareMarket.catalog.full")}
        </button>
      ) : null}
    </div>
  );
}

export function MarketShareCard({
  listing,
  focused = false,
  muted = false,
  attention = false,
  cardId,
  occupancy,
  footer,
  onOpen,
}: {
  listing: ShareMarketListing;
  focused?: boolean;
  muted?: boolean;
  attention?: boolean;
  cardId?: string;
  occupancy?: React.ReactNode;
  footer?: React.ReactNode;
  onOpen: () => void;
}) {
  const { locale, t } = useLocaleText();
  const pointerDownRef = React.useRef<{ x: number; y: number } | null>(null);
  const idleCount = listingIdleCount(listing);
  const performance = listingPerformanceValue(listing);
  const providerView = marketProviderStatusView(listing, locale, {
    unknown: t("shareMarket.catalog.providerUnknown"),
    passthrough: t("shareMarket.modelPassthrough"),
  });
  const subdomain = listing.subdomain?.trim() || listing.shareName;
  return (
    <article
      id={cardId}
      className={cn(
        "grid min-h-[15rem] min-w-0 cursor-pointer scroll-mt-20 select-text grid-rows-[auto_auto_auto_1fr] gap-2.5 rounded-xl border p-3 shadow-sm transition-[border-color,box-shadow,background-color] hover:border-primary/35",
        muted || !idleCount ? "bg-slate-50" : "bg-white",
        focused
          ? "border-primary ring-2 ring-primary/20"
          : attention
            ? "border-amber-300 ring-1 ring-amber-200"
            : listing.shareOnline
              ? "border-slate-200"
              : "border-rose-200",
      )}
      onMouseDown={(event) => {
        pointerDownRef.current = { x: event.clientX, y: event.clientY };
      }}
      onClick={(event) => {
        if (!shouldOpenMarketShareCard(event, pointerDownRef.current)) return;
        pointerDownRef.current = null;
        onOpen();
      }}
    >
      <header className="flex min-w-0 items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <span
            className={cn("h-2 w-2 shrink-0 rounded-full", listing.shareOnline ? "bg-emerald-500" : "bg-rose-500")}
            title={listing.shareOnline ? listing.shareStatus : t("shareMarket.blockReason.share_offline")}
          />
          <MarketProviderLogos source={listing} />
          <strong className="min-w-0 truncate font-mono text-xs font-semibold text-foreground" title={subdomain}>
            {subdomain}
          </strong>
          {listing.subdomain ? <SubdomainCopyButton subdomain={listing.subdomain} /> : null}
        </div>
        <strong className="shrink-0 whitespace-nowrap text-[11px] font-semibold tabular-nums text-slate-700">
          {occupancy ?? t("shareMarket.catalog.occupancy", { idle: idleCount, total: listing.seats.length })}
        </strong>
      </header>
      <ShareProviderStatusPanel view={providerView} wrapPrimaryLine />
      <div className="min-w-0">
        <dl className="grid grid-cols-2 gap-2">
          <MarketShareCardMetric
            label={t("shareMarket.catalog.uptime24h")}
            value={listingUptimeValue(listing)}
            title={`${t("shareMarket.catalog.coverage24hValue", { value: listing.reliability.observationCoverage24h.toFixed(1) })} · ${t("shareMarket.catalog.observedMinutesValue", { count: listing.reliability.observedMinutes24h })}`}
          />
          <MarketShareCardMetric
            label="P50 TTFT/TPS"
            value={performance.label}
            title={`${t("shareMarket.catalog.samplesValue", { count: listing.performance.ttftSampleCount })} · ${t("shareMarket.catalog.samplesValue", { count: listing.performance.tpsSampleCount })}`}
          />
        </dl>
        <p className="mt-1 flex min-w-0 items-start gap-1 text-[10px] leading-4 text-slate-500">
          <UserRound className="mt-0.5 h-3 w-3 shrink-0" aria-hidden />
          <span className="shrink-0">{t("shareMarket.owner")}:</span>
          <span className="min-w-0 break-all" title={listing.ownerEmail}>{listing.ownerEmail}</span>
        </p>
      </div>
      {footer}
    </article>
  );
}
