import type {
  MarketFundingSummary,
  MarketProvider,
  MyMarketProviderFunding,
} from "@/lib/types";
import type { MarketTopupFunding } from "@/lib/market-funding";

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

function fundingDemand(funding: MarketTopupFunding) {
  if ("totalRequiredHoldMinor" in funding) {
    return [
      funding.requiredTopupMinor,
      funding.totalRequiredHoldMinor,
      funding.cyclePriceMinor,
      0,
    ] as const;
  }
  return [
    funding.requiredTopupMinor,
    funding.recommendedTopupMinor,
    funding.projectedDailyRateMinor,
    funding.activeDailyRateMinor,
  ] as const;
}

/**
 * Share and Client summaries point at the same Provider-scoped prepaid account.
 * Pick the relationship with the higher immediate demand so the shared top-up
 * dialog starts from the safer amount without double-counting the balance.
 */
export function preferredProviderTopupFunding(provider: MyMarketProviderFunding) {
  const candidates: MarketTopupFunding[] = [
    provider.shareFunding,
    provider.clientFunding,
    ...(provider.recurring.topupFunding ? [provider.recurring.topupFunding] : []),
  ];
  return candidates.reduce((preferred, candidate) => {
    const current = fundingDemand(preferred);
    const next = fundingDemand(candidate);
    for (let index = 0; index < current.length; index += 1) {
      if (current[index] !== next[index]) {
        return next[index] > current[index] ? candidate : preferred;
      }
    }
    return preferred;
  });
}

/** Credit exposure is shared by Provider, while grants can differ by product. */
export function providerCreditSummary(
  provider: Pick<MyMarketProviderFunding, "shareFunding" | "clientFunding">,
) {
  const fundings = [provider.shareFunding, provider.clientFunding];
  if (fundings.some((funding) => funding.creditKind === "unlimited")) {
    return { kind: "unlimited" as const };
  }
  const availableMinor = fundings
    .filter((funding) => funding.creditKind === "limited")
    .reduce(
      (available, funding) => Math.max(available, funding.creditAvailableMinor ?? 0),
      0,
    );
  return fundings.some((funding) => funding.creditKind === "limited")
    ? { kind: "limited" as const, availableMinor }
    : { kind: "none" as const };
}
