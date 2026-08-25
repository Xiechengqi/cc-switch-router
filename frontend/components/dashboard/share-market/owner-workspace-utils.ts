import type { MessageKey } from "@/lib/i18n";
import type {
  ShareMarketListing,
  ShareMarketOwnedShare,
  ShareMarketSeat,
} from "@/lib/types";
import { isSeatIdle } from "@/components/dashboard/share-market/market-utils";
import { needsRentalAttention } from "@/components/dashboard/share-market/subscription-utils";

const ACTIVE_SEAT_STATUSES = new Set(["available", "reserved", "occupied", "revoking"]);

export function reopenableListingSeats(
  listing: Pick<ShareMarketListing, "seats">,
) {
  return listing.seats.filter((seat) => seat.canRepublish);
}

export function activeListingSeatCount(
  listing: Pick<ShareMarketListing, "seats">,
) {
  return listing.seats.filter(
    (seat) => !seat.readOnly && ACTIVE_SEAT_STATUSES.has(seat.status),
  ).length;
}

export function canCreateOwnedShareListing(share: ShareMarketOwnedShare) {
  return share.canCreateListing;
}

export function ownedShareBlockedReasonKey(
  reason?: string,
): MessageKey {
  switch (reason) {
    case "active_listing":
      return "shareMarket.dialog.blocked.activeListing";
    case "reopen_required":
      return "shareMarket.dialog.blocked.reopenRequired";
    case "active_rentals":
      return "shareMarket.dialog.blocked.activeRentals";
    case "public_access_enabled":
      return "shareMarket.dialog.blocked.freeAccess";
    case "share_inactive":
      return "shareMarket.dialog.blocked.inactive";
    case "pending_share_edit":
      return "shareMarket.dialog.blocked.pendingShareEdit";
    case "client_upgrade_required":
      return "shareMarket.dialog.blocked.clientUpgrade";
    default:
      return "shareMarket.dialog.blocked.unknown";
  }
}

export function reopenBlockedReasonKey(reason?: string): MessageKey {
  switch (reason) {
    case "owner_only":
      return "shareMarket.reopen.blocked.ownerOnly";
    case "share_missing":
      return "shareMarket.reopen.blocked.shareMissing";
    case "owner_changed":
      return "shareMarket.reopen.blocked.ownerChanged";
    case "share_inactive":
      return "shareMarket.reopen.blocked.shareInactive";
    case "public_access_enabled":
      return "shareMarket.reopen.blocked.freeAccess";
    case "pending_share_edit":
      return "shareMarket.reopen.blocked.pendingShareEdit";
    case "client_upgrade_required":
      return "shareMarket.reopen.blocked.clientUpgrade";
    case "another_listing_active":
      return "shareMarket.reopen.blocked.anotherListing";
    case "seat_limit":
      return "shareMarket.reopen.blocked.seatLimit";
    default:
      return "shareMarket.reopen.blocked.unknown";
  }
}

export function ownedShareReopenListingId(
  share: Pick<ShareMarketOwnedShare, "reopenListingId">,
) {
  return share.reopenListingId || null;
}

export function seatCanBeRepublished(
  seat: Pick<ShareMarketSeat, "canRepublish">,
) {
  return seat.canRepublish;
}

export function isCompletedSeat(
  seat: Pick<ShareMarketSeat, "status" | "readOnly" | "subscription">,
) {
  return seat.readOnly
    || seat.status === "retired"
    || seat.subscription?.status === "released";
}

export function needsOwnedSeatAttention(
  seat: Pick<ShareMarketSeat, "subscription">,
) {
  return !!seat.subscription && needsRentalAttention(seat.subscription);
}

export function isPriceOnlySeatAttention(
  seat: Pick<ShareMarketSeat, "subscription">,
) {
  const subscription = seat.subscription;
  if (!subscription || subscription.priceChange?.status !== "pending") return false;
  if (subscription.integrityState != null && subscription.integrityState !== "compatible") return false;
  return !needsRentalAttention({ ...subscription, priceChange: undefined });
}

function bySeatPosition<T extends Pick<ShareMarketSeat, "position">>(left: T, right: T) {
  return left.position - right.position;
}

export function listingAttentionSeats<T extends Pick<ShareMarketSeat, "position" | "subscription">>(
  listing: { seats: T[] },
) {
  return listing.seats.filter(needsOwnedSeatAttention).sort(bySeatPosition);
}

export function listingLiveSeats<T extends Pick<ShareMarketSeat, "position" | "status" | "readOnly" | "subscription">>(
  listing: { seats: T[] },
) {
  return listing.seats
    .filter((seat) => !isCompletedSeat(seat) && !needsOwnedSeatAttention(seat))
    .sort(bySeatPosition);
}

export function listingIdleSeats<T extends Pick<ShareMarketSeat, "position" | "status" | "readOnly" | "subscription">>(
  listing: { seats: T[] },
) {
  return listingLiveSeats(listing).filter(isSeatIdle);
}

export function listingLowestIdleSeat<
  T extends Pick<ShareMarketSeat, "position" | "status" | "readOnly" | "subscription" | "isFree" | "dailyRateMinor">,
>(listing: { seats: T[] }) {
  const idle = listingIdleSeats(listing);
  if (!idle.length) return null;
  const amount = (seat: T) => seat.isFree ? 0 : seat.dailyRateMinor ?? 0;
  const lowest = Math.min(...idle.map(amount));
  return idle.find((seat) => amount(seat) === lowest) || idle[0];
}

export function listingClosedRentalSeats<T extends Pick<ShareMarketSeat, "position" | "status" | "readOnly" | "subscription">>(
  listing: { seats: T[] },
) {
  return listingLiveSeats(listing).filter((seat) =>
    !isSeatIdle(seat) && ACTIVE_SEAT_STATUSES.has(seat.status),
  );
}

export function listingExpandableSeats<
  T extends Pick<ShareMarketSeat, "position" | "status" | "readOnly" | "subscription">,
>(listing: { status: string; seats: T[] }) {
  return listing.status === "closed"
    ? listingClosedRentalSeats(listing)
    : listingLiveSeats(listing);
}

export function listingCanExpand(
  listing: { status: string; seats: Array<Pick<ShareMarketSeat, "position" | "status" | "readOnly" | "subscription">> },
) {
  return listingExpandableSeats(listing).length > 0;
}

export function listingLiveSeatCount(
  listing: { seats: Array<Pick<ShareMarketSeat, "status" | "readOnly" | "subscription">> },
) {
  return listing.seats.filter((seat) => !isCompletedSeat(seat)).length;
}

export function listingBlockedFromReopen(
  listing: Pick<ShareMarketListing, "status" | "canReopen">,
) {
  return listing.status === "closed" && !listing.canReopen;
}

export type OwnedAttentionSeat<TListing extends ShareMarketListing = ShareMarketListing> = {
  listing: TListing;
  seat: TListing["seats"][number];
};

function compareOwnedListings(left: ShareMarketListing, right: ShareMarketListing) {
  return left.shareName.localeCompare(right.shareName) || left.id.localeCompare(right.id);
}

export function partitionOwnedListings<TListing extends ShareMarketListing>(listings: TListing[]) {
  const attentionSeats: Array<OwnedAttentionSeat<TListing>> = [];
  const attentionListings: TListing[] = [];
  const active: TListing[] = [];
  const closed: TListing[] = [];

  for (const listing of [...listings].sort(compareOwnedListings)) {
    for (const seat of listingAttentionSeats(listing)) {
      attentionSeats.push({ listing, seat });
    }
    if (listingBlockedFromReopen(listing)) attentionListings.push(listing);
    else if (listing.status === "closed") closed.push(listing);
    else active.push(listing);
  }

  return { attentionSeats, attentionListings, active, closed };
}
