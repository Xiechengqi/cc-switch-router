import type { MarketProvider } from "@/lib/types";

export type ProviderMarketFilter = "all" | "share" | "client";

export function actorBoundValue<T>(
  snapshot: { actorKey: string; value: T } | null,
  actorKey: string,
) {
  return snapshot?.actorKey === actorKey ? snapshot.value : null;
}

export function filterMarketProviders(
  providers: MarketProvider[],
  filters: {
    query?: string;
    market?: ProviderMarketFilter;
  },
) {
  const query = (filters.query || "").trim().toLocaleLowerCase();
  const market = filters.market || "all";
  return providers.filter((provider) => {
    if (query && !provider.displayName.toLocaleLowerCase().includes(query)) return false;
    if (market === "share" && !provider.inventory.hasShareMarket) return false;
    if (market === "client" && !provider.inventory.hasClientMarket) return false;
    return true;
  });
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
