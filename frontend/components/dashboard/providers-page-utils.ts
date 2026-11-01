import { ApiError } from "@/lib/api";
import type { MarketProvider, MarketProviderRankGeneration } from "@/lib/types";

export const PROVIDER_MARKET_FILTERS = ["all", "share", "client"] as const;
export type ProviderMarketFilter = (typeof PROVIDER_MARKET_FILTERS)[number];

/** The market filter from `?market=`; anything else shows every market. */
export function providerMarketFilter(value: string | null | undefined): ProviderMarketFilter {
  return PROVIDER_MARKET_FILTERS.includes(value as ProviderMarketFilter) ? value as ProviderMarketFilter : "all";
}

export function actorBoundValue<T>(
  snapshot: { actorKey: string; value: T } | null,
  actorKey: string,
) {
  return snapshot?.actorKey === actorKey ? snapshot.value : null;
}

/** Whether the Provider has something a buyer can take in the given market. */
export function hasAvailableSupply(provider: MarketProvider, market: ProviderMarketFilter = "all") {
  const share = provider.inventory.hasShareMarket && provider.inventory.availableShareSeats > 0;
  const client = provider.inventory.hasClientMarket && provider.inventory.idleHostTotal > 0;
  return market === "share" ? share : market === "client" ? client : share || client;
}

export function filterMarketProviders(
  providers: MarketProvider[],
  filters: {
    query?: string;
    market?: ProviderMarketFilter;
    availableOnly?: boolean;
    /** The name the page shows, when it differs from `displayName`. */
    nameOf?: (provider: MarketProvider) => string;
  },
) {
  const query = (filters.query || "").trim().toLocaleLowerCase();
  const market = filters.market || "all";
  return providers.filter((provider) => {
    if (query) {
      const names = [provider.displayName, filters.nameOf?.(provider) ?? ""];
      if (!names.some((name) => name.toLocaleLowerCase().includes(query))) return false;
    }
    if (market === "share" && !provider.inventory.hasShareMarket) return false;
    if (market === "client" && !provider.inventory.hasClientMarket) return false;
    if (filters.availableOnly && !hasAvailableSupply(provider, market)) return false;
    return true;
  });
}

export type RankProgress = {
  buyers: { count: number; min: number; met: boolean };
  days: { count: number; min: number; met: boolean };
};

/** How far a collecting Provider is from the formal-rank thresholds. */
export function rankProgress(
  provider: Pick<MarketProvider, "independentBuyerCount" | "observationDays">,
  generation: Pick<MarketProviderRankGeneration, "minimumIndependentBuyers" | "minimumObservationDays">,
): RankProgress {
  return {
    buyers: {
      count: provider.independentBuyerCount,
      min: generation.minimumIndependentBuyers,
      met: provider.independentBuyerCount >= generation.minimumIndependentBuyers,
    },
    days: {
      count: provider.observationDays,
      min: generation.minimumObservationDays,
      met: provider.observationDays >= generation.minimumObservationDays,
    },
  };
}

export type ProviderLoadError = "notReady" | "notFound" | "generic";

/** Coded Router errors become page states instead of raw server text. */
export function providerLoadError(reason: unknown): ProviderLoadError {
  if (!(reason instanceof ApiError)) return "generic";
  if (reason.code === "provider_rank_not_ready" || reason.status === 503) return "notReady";
  if (reason.code === "provider_not_found" || reason.status === 404) return "notFound";
  return "generic";
}

export function basisPointsScore(value?: number) {
  if (value == null || !Number.isFinite(value)) return undefined;
  return Math.max(0, Math.min(10_000, value)) / 100;
}

export function formatBasisPointScore(value: number | undefined, locale: string) {
  const score = basisPointsScore(value);
  if (score == null) return "—";
  const formatted = new Intl.NumberFormat(locale, {
    maximumFractionDigits: 1,
    minimumFractionDigits: score > 0 && score < 10 ? 1 : 0,
  }).format(score);
  return `${formatted} / 100`;
}

/** A formal rank is only safe to show when all public rank fields agree. */
export function isFormallyRanked(
  provider: Pick<MarketProvider, "rankState" | "rankPosition" | "scoreBps">,
) {
  return provider.rankState === "ranked"
    && Number.isInteger(provider.rankPosition)
    && (provider.rankPosition ?? 0) > 0
    && provider.scoreBps != null
    && Number.isFinite(provider.scoreBps)
    && provider.scoreBps >= 0
    && provider.scoreBps <= 10_000;
}
