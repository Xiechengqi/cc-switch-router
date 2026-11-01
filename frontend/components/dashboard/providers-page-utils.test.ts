import assert from "node:assert/strict";
import test from "node:test";
import { ApiError } from "@/lib/api";
import type { MarketProvider } from "@/lib/types";
import {
  actorBoundValue,
  basisPointsScore,
  filterMarketProviders,
  formatBasisPointScore,
  hasAvailableSupply,
  isFormallyRanked,
  providerLoadError,
  providerMarketFilter,
  rankProgress,
} from "./providers-page-utils";

test("private snapshots are hidden synchronously when the authenticated actor changes", () => {
  const snapshot = { actorKey: "buyer-a", value: { balance: 100 } };
  assert.deepEqual(actorBoundValue(snapshot, "buyer-a"), { balance: 100 });
  assert.equal(actorBoundValue(snapshot, "buyer-b"), null);
  assert.equal(actorBoundValue(null, "buyer-a"), null);
});

function provider(
  id: string,
  overrides: Partial<MarketProvider> = {},
): MarketProvider {
  return {
    id,
    displayName: id,
    official: false,
    status: "active",
    rankState: "collecting",
    components: {
      serviceQualityBps: 0,
      effectiveChoiceBps: 0,
      fulfillmentBps: 0,
      supplyBreadthBps: 0,
    },
    independentBuyerCount: 0,
    observationDays: 0,
    inventory: {
      activeShareCount: 0,
      availableShareSeats: 0,
      hostTotal: 0,
      idleHostTotal: 0,
      countryCount: 0,
      appCount: 0,
      hasShareMarket: false,
      hasClientMarket: false,
    },
    quality: {
      shareProbeSuccesses: 0,
      shareProbeTotal: 0,
      hostOnlineSamples: 0,
      hostObservedSamples: 0,
      fulfillmentSuccesses: 0,
      fulfillmentTotal: 0,
    },
    performance: { displayOnly: true },

    exploration: false,
    joinedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

test("Provider name and market filters stay orthogonal and preserve server order", () => {
  const rankedShare = provider("Ranked Share", {
    rankState: "ranked",
    rankPosition: 1,
    inventory: {
      ...provider("base").inventory,
      activeShareCount: 1,
      hasShareMarket: true,
    },
  });
  const collectingClient = provider("Collecting Client", {
    inventory: {
      ...provider("base").inventory,
      hostTotal: 2,
      hasClientMarket: true,
    },
  });
  const providers = [rankedShare, collectingClient];

  assert.deepEqual(
    filterMarketProviders(providers, { market: "share" }).map((item) => item.id),
    ["Ranked Share"],
  );
  assert.deepEqual(
    filterMarketProviders(providers, { market: "client", query: "client" }).map((item) => item.id),
    ["Collecting Client"],
  );
  assert.deepEqual(filterMarketProviders(providers, {}), providers);
});

test("basis point scores are clamped and formatted as scores, not percentages", () => {
  assert.equal(basisPointsScore(9_876), 98.76);
  assert.equal(basisPointsScore(11_000), 100);
  assert.equal(basisPointsScore(-1), 0);
  assert.equal(basisPointsScore(undefined), undefined);
  assert.equal(formatBasisPointScore(8_640, "en"), "86.4 / 100");
  assert.equal(formatBasisPointScore(undefined, "en"), "—");
  assert.equal(formatBasisPointScore(8_640, "en").includes("%"), false);
});

test("formal rank presentation fails closed when rank fields disagree", () => {
  assert.equal(isFormallyRanked(provider("ranked", {
    rankState: "ranked",
    rankPosition: 1,
    scoreBps: 8_640,
  })), true);
  assert.equal(isFormallyRanked(provider("collecting", {
    rankState: "collecting",
    rankPosition: 1,
    scoreBps: 8_640,
  })), false);
  assert.equal(isFormallyRanked(provider("missing score", {
    rankState: "ranked",
    rankPosition: 1,
  })), false);
  assert.equal(isFormallyRanked(provider("invalid score", {
    rankState: "ranked",
    rankPosition: 1,
    scoreBps: 10_001,
  })), false);
});

test("available-only follows the selected market and search matches the shown name", () => {
  const base = provider("base").inventory;
  const fullShareIdleHost = provider("Official Provider", {
    official: true,
    inventory: { ...base, activeShareCount: 1, hasShareMarket: true, hostTotal: 1, idleHostTotal: 1, hasClientMarket: true },
  });
  const openShare = provider("Open Share", {
    inventory: { ...base, activeShareCount: 1, availableShareSeats: 2, hasShareMarket: true },
  });
  assert.equal(hasAvailableSupply(fullShareIdleHost, "share"), false);
  assert.equal(hasAvailableSupply(fullShareIdleHost, "client"), true);
  assert.equal(hasAvailableSupply(fullShareIdleHost), true);
  const providers = [fullShareIdleHost, openShare];
  assert.deepEqual(
    filterMarketProviders(providers, { market: "share", availableOnly: true }).map((item) => item.id),
    ["Open Share"],
  );
  assert.deepEqual(
    filterMarketProviders(providers, {
      query: "官方",
      nameOf: (item) => (item.official ? "官方供应商" : item.displayName),
    }).map((item) => item.id),
    ["Official Provider"],
  );
});

test("the market filter from the address falls back to every market", () => {
  assert.equal(providerMarketFilter("share"), "share");
  assert.equal(providerMarketFilter("client"), "client");
  assert.equal(providerMarketFilter("hosts"), "all");
  assert.equal(providerMarketFilter(null), "all");
});

test("rank progress reports each threshold separately", () => {
  const progress = rankProgress(
    provider("collecting", { independentBuyerCount: 1, observationDays: 27 }),
    { minimumIndependentBuyers: 3, minimumObservationDays: 7 },
  );
  assert.deepEqual(progress.buyers, { count: 1, min: 3, met: false });
  assert.deepEqual(progress.days, { count: 27, min: 7, met: true });
});

test("coded Router errors map to page states", () => {
  assert.equal(providerLoadError(new ApiError(503, "x", "provider_rank_not_ready")), "notReady");
  assert.equal(providerLoadError(new ApiError(404, "x", "provider_not_found")), "notFound");
  assert.equal(providerLoadError(new ApiError(500, "x")), "generic");
  assert.equal(providerLoadError(new Error("offline")), "generic");
});
