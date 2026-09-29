import assert from "node:assert/strict";
import test from "node:test";
import type {
  MarketFundingSummary,
  MarketProvider,
  MarketRecurringFundingSummary,
  MyMarketProviderFunding,
} from "@/lib/types";
import {
  actorBoundValue,
  basisPointsPercent,
  filterMarketProviders,
  preferredProviderTopupFunding,
  providerCreditSummary,
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
    paymentMethodKinds: [],
    exploration: false,
    joinedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function funding(projectedDailyRateMinor: number, requiredTopupMinor = 0): MarketFundingSummary {
  return {
    supplierUserId: "supplier",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    fundingMode: "prepaid",
    prepaidBalanceMinor: 1_000,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 1_000,
    creditKind: "none",
    creditOutstandingMinor: 0,
    creditReservedMinor: 0,
    activeDailyRateMinor: projectedDailyRateMinor,
    additionalDailyRateMinor: 0,
    projectedDailyRateMinor,
    requiredCoverageMinor: 0,
    prepaidCoverageMinor: 0,
    creditCoverageMinor: 0,
    requiredTopupMinor,
    recommendedTopupMinor: projectedDailyRateMinor * 7,
    recommendedCoverageDays: 7,
    topupAvailable: true,
  };
}

test("Provider filters stay orthogonal and preserve server order", () => {
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
    filterMarketProviders(providers, { rank: "collecting", query: "client" }).map((item) => item.id),
    ["Collecting Client"],
  );
  assert.deepEqual(filterMarketProviders(providers, {}), providers);
});

test("basis point display clamps malformed values", () => {
  assert.equal(basisPointsPercent(9_876), 98.76);
  assert.equal(basisPointsPercent(11_000), 100);
  assert.equal(basisPointsPercent(-1), 0);
  assert.equal(basisPointsPercent(undefined), undefined);
});

test("top-up defaults to the higher-demand Provider relationship", () => {
  const shareFunding = funding(100);
  const clientFunding = funding(300);
  const item: MyMarketProviderFunding = {
    marketProviderId: "mp_1",
    displayName: "Provider",
    official: false,
    supplierUserId: "supplier",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    shareFunding,
    clientFunding,
    shareLegacyDailyRateMinor: 100,
    clientLegacyDailyRateMinor: 300,
    recurring: {
      activeContractCount: 0,
      monthlyCommitmentMinor: 0,
      nextRenewalMinor: 0,
    },
  };
  assert.equal(preferredProviderTopupFunding(item), clientFunding);

  const urgentShare = funding(100, 500);
  assert.equal(
    preferredProviderTopupFunding({ ...item, shareFunding: urgentShare }),
    urgentShare,
  );

  const recurring: MarketRecurringFundingSummary = {
    supplierUserId: "supplier",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    pricingModel: "prepaid_calendar_month",
    billingInterval: "calendar_month",
    cyclePriceMinor: 2_000,
    renewalPolicy: "manual",
    prepaidBalanceMinor: 1_000,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 1_000,
    initialHoldMinor: 2_000,
    renewalHoldMinor: 0,
    totalRequiredHoldMinor: 2_000,
    requiredTopupMinor: 1_000,
    topupAvailable: true,
  };
  assert.equal(
    preferredProviderTopupFunding({
      ...item,
      recurring: { ...item.recurring, topupFunding: recurring },
    }),
    recurring,
  );
});

test("Provider credit summary covers both product grants without double-counting", () => {
  const shareFunding = funding(100);
  const clientFunding = {
    ...funding(200),
    creditKind: "limited" as const,
    creditAvailableMinor: 750,
  };
  assert.deepEqual(
    providerCreditSummary({ shareFunding, clientFunding }),
    { kind: "limited", availableMinor: 750 },
  );
  assert.deepEqual(
    providerCreditSummary({
      shareFunding: { ...shareFunding, creditKind: "unlimited" },
      clientFunding,
    }),
    { kind: "unlimited" },
  );
  assert.deepEqual(
    providerCreditSummary({ shareFunding, clientFunding: funding(200) }),
    { kind: "none" },
  );
});
