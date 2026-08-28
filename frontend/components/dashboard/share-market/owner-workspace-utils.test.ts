import assert from "node:assert/strict";
import test from "node:test";
import type {
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
