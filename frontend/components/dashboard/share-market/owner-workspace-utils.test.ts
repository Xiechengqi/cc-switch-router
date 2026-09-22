import assert from "node:assert/strict";
import test from "node:test";
import type {
  MarketFundingSummary,
  MarketRecurringFundingSummary,
  ShareMarketListing,
  ShareMarketOwnedShare,
  ShareMarketSeat,
  ShareMarketSubscription,
} from "@/lib/types";
import {
  activeListingSeatCount,
  canCreateOwnedShareListing,
  isCompletedSeat,
  isPriceOnlySeatAttention,
  listingAttentionSeats,
  listingBlockedFromReopen,
  listingCanExpand,
  listingClosedRentalSeats,
  listingExpandableSeats,
  listingIdleSeats,
  listingLiveSeatCount,
  listingLiveSeats,
  listingLowestIdleSeat,
  listingOccupancyCounts,
  needsOwnedSeatAttention,
  ownedShareBlockedReasonKey,
  ownedShareReopenListingId,
  partitionOwnedListings,
  reopenableListingSeats,
} from "./owner-workspace-utils";
import { shareMarketMutationError } from "./market-utils";
import { ApiError } from "@/lib/api";
import {
  applyMarketFundingConflict,
  applyRecurringFundingConflict,
  defaultMarketFundingTopupMinor,
  formatFundingRunway,
  marketFundingConflictFromError,
  marketFundingAfterRecurringHolds,
  marketFundingDecisionState,
  marketFundingTopupErrorKey,
  marketFundingTopupUnavailableKey,
  marketFundingUsesCredit,
  recurringFundingForRenewal,
} from "@/lib/market-funding";

function seat(
  id: string,
  status: string,
  canRepublish: boolean,
  readOnly = false,
) {
  return { id, status, canRepublish, readOnly } as ShareMarketSeat;
}

test("reopen helpers select only explicitly republishable seats", () => {
  const listing = {
    seats: [
      seat("idle-before-stop", "disabled", true),
      seat("active-rental", "occupied", false),
      seat("retired-history", "retired", false, true),
    ],
  } as ShareMarketListing;

  assert.deepEqual(
    reopenableListingSeats(listing).map((item) => item.id),
    ["idle-before-stop"],
  );
  assert.equal(activeListingSeatCount(listing), 1);
});

test("owned Share capabilities keep create and reopen as separate actions", () => {
  const stopped = {
    canCreateListing: false,
    createBlockedReason: "reopen_required",
    reopenListingId: "listing-stopped",
  } as ShareMarketOwnedShare;
  const available = {
    canCreateListing: true,
  } as ShareMarketOwnedShare;

  assert.equal(canCreateOwnedShareListing(stopped), false);
  assert.equal(ownedShareReopenListingId(stopped), "listing-stopped");
  assert.equal(
    ownedShareBlockedReasonKey(stopped.createBlockedReason),
    "shareMarket.dialog.blocked.reopenRequired",
  );
  assert.equal(canCreateOwnedShareListing(available), true);
  assert.equal(ownedShareReopenListingId(available), null);
});

test("owned Share blocked reasons remain stable UI contracts", () => {
  assert.equal(
    ownedShareBlockedReasonKey("active_rentals"),
    "shareMarket.dialog.blocked.activeRentals",
  );
  assert.equal(
    ownedShareBlockedReasonKey("pending_share_edit"),
    "shareMarket.dialog.blocked.pendingShareEdit",
  );
  assert.equal(
    ownedShareBlockedReasonKey("unexpected"),
    "shareMarket.dialog.blocked.unknown",
  );
});

function listing(
  id: string,
  status: string,
  seats: ShareMarketSeat[],
  extra: Partial<ShareMarketListing> = {},
) {
  return {
    id,
    shareId: id,
    shareName: extra.shareName || id,
    status,
    canReopen: extra.canReopen ?? status === "closed",
    seats,
  } as ShareMarketListing;
}

function rentedSeat(
  id: string,
  status: string,
  extra: Partial<ShareMarketSeat> & { subscriptionStatus?: string } = {},
) {
  const { subscriptionStatus, ...seat } = extra;
  return {
    id,
    position: seat.position || 1,
    status,
    readOnly: seat.readOnly || false,
    canRepublish: seat.canRepublish || false,
    subscription: subscriptionStatus
      ? { status: subscriptionStatus, integrityState: "compatible" } as ShareMarketSubscription
      : seat.subscription,
    ...seat,
  } as ShareMarketSeat;
}

test("completed seats are released or retired only", () => {
  assert.equal(isCompletedSeat(rentedSeat("a", "retired", { readOnly: true })), true);
  assert.equal(isCompletedSeat(rentedSeat("b", "available", { subscriptionStatus: "released" })), true);
  assert.equal(isCompletedSeat(rentedSeat("c", "occupied", { subscriptionStatus: "grant_failed" })), false);
  assert.equal(isCompletedSeat(rentedSeat("d", "available")), false);
});

test("pending price changes without integrity failure are price-only attention", () => {
  assert.equal(isPriceOnlySeatAttention(rentedSeat("price", "occupied", {
    subscription: {
      status: "active_postpaid",
      integrityState: "compatible",
      priceChange: { id: "pc", status: "pending" },
    } as ShareMarketSubscription,
  })), true);
  assert.equal(isPriceOnlySeatAttention(rentedSeat("failed", "occupied", { subscriptionStatus: "grant_failed" })), false);
});

test("grant_failed seats need attention and stay out of history and live lists", () => {
  const failed = rentedSeat("failed", "occupied", { position: 2, subscriptionStatus: "grant_failed" });
  const idle = rentedSeat("idle", "available", { position: 1 });
  const released = rentedSeat("done", "retired", { position: 3, readOnly: true, subscriptionStatus: "released" });
  const current = listing("live", "active", [released, failed, idle]);

  assert.equal(needsOwnedSeatAttention(failed), true);
  assert.deepEqual(listingAttentionSeats(current).map((item) => item.id), ["failed"]);
  assert.deepEqual(listingLiveSeats(current).map((item) => item.id), ["idle"]);
  assert.equal(listingLiveSeatCount(current), 2);
});

test("closed listings keep remaining rentals and hide idle stopped seats", () => {
  const idleStopped = rentedSeat("idle", "available", { position: 1, canRepublish: true });
  const disabled = rentedSeat("disabled", "disabled", { position: 3, canRepublish: true });
  const occupied = rentedSeat("rent", "occupied", { position: 2, subscriptionStatus: "active_free" });
  const stopped = listing("stopped", "closed", [idleStopped, disabled, occupied], { canReopen: true });

  assert.deepEqual(listingClosedRentalSeats(stopped).map((item) => item.id), ["rent"]);
  assert.equal(listingBlockedFromReopen(stopped), false);
});

test("partitions grant_failed into attention and released out of live seats", () => {
  const failed = rentedSeat("failed", "occupied", { subscriptionStatus: "grant_failed" });
  const pendingPrice = rentedSeat("price", "occupied", {
    subscription: {
      status: "active_postpaid",
      integrityState: "compatible",
      priceChange: { id: "pc", status: "pending" },
    } as ShareMarketSubscription,
  });
  const idle = rentedSeat("idle", "available");
  const released = rentedSeat("done", "retired", { readOnly: true, subscriptionStatus: "released" });
  const live = listing("alpha", "active", [failed, pendingPrice, idle, released], { shareName: "Alpha" });
  const blocked = listing("blocked", "closed", [idle], { shareName: "Blocked", canReopen: false });
  const stopped = listing("stopped", "closed", [occupiedSeat()], { shareName: "Stopped", canReopen: true });
  const partitioned = partitionOwnedListings([stopped, blocked, live]);

  assert.deepEqual(partitioned.attentionSeats.map((item) => item.seat.id), ["failed", "price"]);
  assert.deepEqual(partitioned.attentionListings.map((item) => item.id), ["blocked"]);
  assert.deepEqual(partitioned.active.map((item) => item.id), ["alpha"]);
  assert.deepEqual(partitioned.closed.map((item) => item.id), ["stopped"]);
  assert.deepEqual(listingLiveSeats(live).map((item) => item.id), ["idle"]);
});

function occupiedSeat() {
  return rentedSeat("rent", "occupied", { subscriptionStatus: "active_free" });
}

test("occupancy counts use live seats and ignore completed history", () => {
  const idle = rentedSeat("idle", "available", { position: 1 });
  const occupied = rentedSeat("rent", "occupied", { position: 2, subscriptionStatus: "active_free" });
  const failed = rentedSeat("failed", "occupied", { position: 3, subscriptionStatus: "grant_failed" });
  const released = rentedSeat("done", "retired", { position: 4, readOnly: true, subscriptionStatus: "released" });
  assert.deepEqual(
    listingOccupancyCounts(listing("live", "active", [idle, occupied, failed, released])),
    { idle: 1, remaining: 1, attention: 1, total: 2 },
  );
});

test("lowest idle price never falls back to occupied seats", () => {
  const idle = rentedSeat("idle", "available", {
    position: 1,
    isFree: false,
    dailyRateMinor: 200,
  });
  const occupied = rentedSeat("rent", "occupied", {
    position: 2,
    subscriptionStatus: "active_postpaid",
    isFree: false,
    dailyRateMinor: 100,
  });
  const current = listing("live", "active", [idle, occupied]);
  assert.equal(listingLowestIdleSeat(current)?.id, "idle");
  assert.deepEqual(listingIdleSeats(current).map((item) => item.id), ["idle"]);

  const full = listing("full", "active", [occupied]);
  assert.equal(listingLowestIdleSeat(full), null);
  assert.deepEqual(listingIdleSeats(full), []);
});

test("free idle seats beat paid idle seats for the collapsed price", () => {
  const paid = rentedSeat("paid", "available", {
    position: 1,
    isFree: false,
    dailyRateMinor: 50,
  });
  const free = rentedSeat("free", "available", {
    position: 2,
    isFree: true,
  });
  assert.equal(listingLowestIdleSeat(listing("mix", "active", [paid, free]))?.id, "free");
});

test("mixed legacy daily and monthly seats compare by monthly equivalent", () => {
  const legacyDaily = rentedSeat("legacy-daily", "available", {
    position: 1,
    isFree: false,
    dailyRateMinor: 100,
  });
  const monthly = rentedSeat("monthly", "available", {
    position: 2,
    isFree: false,
    cyclePriceMinor: 2_500,
  });
  assert.equal(
    listingLowestIdleSeat(listing("mixed-pricing", "active", [legacyDaily, monthly]))?.id,
    "monthly",
  );
});

test("recurring funding recalculates the exact automatic-renewal hold", () => {
  const funding = {
    supplierUserId: "supplier",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    pricingModel: "prepaid_calendar_month",
    billingInterval: "calendar_month",
    cyclePriceMinor: 1_200,
    renewalPolicy: "manual",
    prepaidBalanceMinor: 1_500,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 1_500,
    initialHoldMinor: 1_200,
    renewalHoldMinor: 0,
    totalRequiredHoldMinor: 1_200,
    requiredTopupMinor: 0,
    topupAvailable: true,
  } as const;
  const automatic = recurringFundingForRenewal(funding, true);
  assert.equal(automatic.renewalPolicy, "automatic");
  assert.equal(automatic.renewalHoldMinor, 1_200);
  assert.equal(automatic.totalRequiredHoldMinor, 2_400);
  assert.equal(automatic.requiredTopupMinor, 900);
  const manual = recurringFundingForRenewal(automatic, false);
  assert.equal(manual.renewalPolicy, "manual");
  assert.equal(manual.renewalHoldMinor, 0);
  assert.equal(manual.requiredTopupMinor, 0);
});

test("calendar-month holds are applied before legacy metered funding", () => {
  const funding = {
    supplierUserId: "supplier",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    fundingMode: "prepaid_then_credit",
    prepaidBalanceMinor: 1_500,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 1_500,
    creditKind: "limited",
    creditLimitMinor: 500,
    creditOutstandingMinor: 0,
    creditReservedMinor: 0,
    creditAvailableMinor: 500,
    activeDailyRateMinor: 0,
    additionalDailyRateMinor: 1_000,
    projectedDailyRateMinor: 1_000,
    requiredCoverageMinor: 1_000,
    prepaidCoverageMinor: 1_000,
    creditCoverageMinor: 0,
    requiredTopupMinor: 0,
    recommendedTopupMinor: 5_500,
    recommendedCoverageDays: 7,
    prepaidRunwaySeconds: 129_600,
    estimatedRunwaySeconds: 172_800,
    topupAvailable: true,
  } satisfies MarketFundingSummary;
  const projected = marketFundingAfterRecurringHolds(funding, 1_200);
  assert.equal(projected.prepaidHeldMinor, 1_200);
  assert.equal(projected.prepaidAvailableMinor, 300);
  assert.equal(projected.prepaidCoverageMinor, 300);
  assert.equal(projected.creditCoverageMinor, 500);
  assert.equal(projected.requiredTopupMinor, 200);
  assert.equal(projected.recommendedTopupMinor, 6_700);
  assert.equal(projected.prepaidRunwaySeconds, 25_920);
  assert.equal(projected.estimatedRunwaySeconds, 69_120);
});

test("expandable seats are live seats on active listings and remaining rentals when closed", () => {
  const failed = rentedSeat("failed", "occupied", { position: 2, subscriptionStatus: "grant_failed" });
  const idle = rentedSeat("idle", "available", { position: 1 });
  const live = listing("live", "active", [failed, idle]);
  assert.deepEqual(listingExpandableSeats(live).map((item) => item.id), ["idle"]);
  assert.equal(listingCanExpand(live), true);

  const attentionOnly = listing("busy", "active", [failed]);
  assert.deepEqual(listingExpandableSeats(attentionOnly).map((item) => item.id), []);
  assert.equal(listingCanExpand(attentionOnly), false);

  const occupied = rentedSeat("rent", "occupied", { position: 2, subscriptionStatus: "active_free" });
  const idleStopped = rentedSeat("stopped-idle", "available", { position: 1, canRepublish: true });
  const stopped = listing("stopped", "closed", [idleStopped, occupied], { canReopen: true });
  assert.deepEqual(listingExpandableSeats(stopped).map((item) => item.id), ["rent"]);
  assert.equal(listingCanExpand(stopped), true);

  const closedEmpty = listing("done", "closed", [idleStopped], { canReopen: true });
  assert.equal(listingCanExpand(closedEmpty), false);
});

test("reopen API conflicts are localized by stable error code", () => {
  const translate = (key: string) => key;
  assert.equal(
    shareMarketMutationError(
      new ApiError(409, "backend English", "share_market_reopen_required"),
      translate,
    ),
    "shareMarket.error.reopenRequired",
  );
  assert.equal(
    shareMarketMutationError(
      new ApiError(409, "backend English", "share_market_seat_not_reopenable"),
      translate,
    ),
    "shareMarket.error.seatNotReopenable",
  );
});

test("prepaid funding races are localized by stable error code", () => {
  const translate = (key: string) => key;
  assert.equal(
    shareMarketMutationError(
      new ApiError(409, "backend English", "MARKET_PREPAID_REQUIRED"),
      translate,
    ),
    "marketFunding.blocked",
  );
});

test("prepaid funding race details update only the affected supplier", () => {
  const conflict = marketFundingConflictFromError(new ApiError(
    409,
    "funding changed",
    "MARKET_PREPAID_REQUIRED",
    {
      supplierUserId: "supplier-a",
      requiredTopupMinor: 275,
      prepaidAvailableMinor: 25,
      creditAvailableMinor: 0,
    },
  ));
  assert.deepEqual(conflict, {
    supplierUserId: "supplier-a",
    requiredTopupMinor: 275,
    prepaidAvailableMinor: 25,
    creditAvailableMinor: 0,
  });

  const funding = {
    supplierUserId: "supplier-a",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    fundingMode: "prepaid",
    prepaidBalanceMinor: 25,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 10,
    creditKind: "none",
    creditLimitMinor: undefined,
    creditOutstandingMinor: 0,
    creditReservedMinor: 0,
    creditAvailableMinor: 0,
    activeDailyRateMinor: 0,
    additionalDailyRateMinor: 300,
    projectedDailyRateMinor: 300,
    requiredCoverageMinor: 300,
    prepaidCoverageMinor: 25,
    creditCoverageMinor: 0,
    requiredTopupMinor: 0,
    recommendedTopupMinor: 2_075,
    recommendedCoverageDays: 7,
    prepaidRunwaySeconds: 7_200,
    estimatedRunwaySeconds: 7_200,
    topupAvailable: true,
    topupUnavailableReason: undefined,
  } satisfies MarketFundingSummary;
  assert.deepEqual(applyMarketFundingConflict(funding, conflict!), {
    ...funding,
    prepaidAvailableMinor: 25,
    creditAvailableMinor: 0,
    requiredTopupMinor: 275,
  });
  assert.equal(
    applyMarketFundingConflict(
      { ...funding, supplierUserId: "supplier-b" },
      conflict!,
    ).requiredTopupMinor,
    0,
  );

  const recurringFunding = {
    supplierUserId: "supplier-a",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    pricingModel: "prepaid_calendar_month",
    billingInterval: "calendar_month",
    cyclePriceMinor: 300,
    renewalPolicy: "manual",
    prepaidBalanceMinor: 300,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 300,
    initialHoldMinor: 300,
    renewalHoldMinor: 0,
    totalRequiredHoldMinor: 300,
    requiredTopupMinor: 0,
    topupAvailable: true,
  } satisfies MarketRecurringFundingSummary;
  const monthlyConflict = marketFundingConflictFromError(new ApiError(
    409,
    "monthly funding changed",
    "MARKET_PREPAID_REQUIRED",
    {
      supplierUserId: "supplier-a",
      pricingModel: "prepaid_calendar_month",
      requiredTopupMinor: 125,
      prepaidAvailableMinor: 175,
    },
  ));
  assert.equal(
    applyMarketFundingConflict(funding, monthlyConflict!).requiredTopupMinor,
    0,
  );
  assert.equal(
    applyRecurringFundingConflict(recurringFunding, monthlyConflict!).requiredTopupMinor,
    125,
  );
  const legacyConflict = { ...conflict!, pricingModel: "legacy_metered_daily" };
  assert.equal(
    applyRecurringFundingConflict(recurringFunding, legacyConflict).requiredTopupMinor,
    0,
  );
  assert.equal(
    marketFundingConflictFromError(new ApiError(
      409,
      "unsafe incomplete details",
      "MARKET_PREPAID_REQUIRED",
      { requiredTopupMinor: 100 },
    )),
    null,
  );
});

test("voluntary funding defaults to the seven-day recommendation", () => {
  const funding = {
    supplierUserId: "supplier-a",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    fundingMode: "prepaid_then_credit",
    prepaidBalanceMinor: 500,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 500,
    creditKind: "limited",
    creditLimitMinor: 10_000,
    creditOutstandingMinor: 1_000,
    creditReservedMinor: 300,
    creditAvailableMinor: 8_700,
    activeDailyRateMinor: 100,
    additionalDailyRateMinor: 200,
    projectedDailyRateMinor: 300,
    requiredCoverageMinor: 300,
    prepaidCoverageMinor: 300,
    creditCoverageMinor: 0,
    requiredTopupMinor: 0,
    recommendedTopupMinor: 1_600,
    recommendedCoverageDays: 7,
    prepaidRunwaySeconds: 144_000,
    estimatedRunwaySeconds: 2_649_600,
    topupAvailable: true,
  } satisfies MarketFundingSummary;

  assert.equal(defaultMarketFundingTopupMinor(funding), 1_600);
  assert.equal(defaultMarketFundingTopupMinor({
    ...funding,
    requiredTopupMinor: 2_000,
  }), 2_000);
  assert.equal(defaultMarketFundingTopupMinor({
    ...funding,
    recommendedTopupMinor: 100_000_001,
  }), 100_000_000);
  assert.equal(marketFundingUsesCredit(funding), false);
  assert.equal(marketFundingUsesCredit({ ...funding, creditCoverageMinor: 125 }), true);
  assert.equal(marketFundingTopupUnavailableKey("region_restricted"), "marketFunding.topup.unavailable.regionRestricted");
  assert.equal(marketFundingTopupUnavailableKey("relationship_closed"), "marketFunding.topup.unavailable.relationshipClosed");
  assert.equal(marketFundingTopupUnavailableKey("future_reason"), "marketFunding.topup.unavailable");
  assert.equal(
    marketFundingTopupErrorKey(new ApiError(409, "closed", "MARKET_RELATIONSHIP_CLOSED")),
    "marketFunding.topup.unavailable.relationshipClosed",
  );
  assert.match(formatFundingRunway(129_600, "en", "Unlimited"), /1\.5/);
  assert.equal(formatFundingRunway(undefined, "en", "Unlimited"), "Unlimited");
});

test("funding decision state prioritizes blockers and material risk", () => {
  const funding = {
    supplierUserId: "supplier-a",
    supplierEmail: "supplier@example.com",
    currency: "USD",
    fundingMode: "prepaid",
    prepaidBalanceMinor: 700,
    prepaidHeldMinor: 0,
    prepaidAvailableMinor: 700,
    creditKind: "none",
    creditOutstandingMinor: 0,
    creditReservedMinor: 0,
    activeDailyRateMinor: 0,
    additionalDailyRateMinor: 100,
    projectedDailyRateMinor: 100,
    requiredCoverageMinor: 100,
    prepaidCoverageMinor: 100,
    creditCoverageMinor: 0,
    requiredTopupMinor: 0,
    recommendedTopupMinor: 0,
    recommendedCoverageDays: 7,
    prepaidRunwaySeconds: 604_800,
    estimatedRunwaySeconds: 604_800,
    topupAvailable: true,
  } satisfies MarketFundingSummary;

  assert.equal(marketFundingDecisionState(funding), "ready");
  assert.equal(marketFundingDecisionState({
    ...funding,
    prepaidRunwaySeconds: 86_400,
    estimatedRunwaySeconds: 86_400,
  }), "low_runway");
  assert.equal(marketFundingDecisionState({
    ...funding,
    creditKind: "limited",
    creditCoverageMinor: 50,
  }), "uses_credit");
  assert.equal(marketFundingDecisionState({
    ...funding,
    creditCoverageMinor: 50,
    requiredTopupMinor: 25,
  }), "shortfall");
});
