import type {
  ShareMarketListing,
  ShareMarketProviderFamily,
  ShareMarketSeat,
  ShareMarketSubscription,
} from "@/lib/types";
import {
  PROVIDER_FAMILY_ORDER,
  listingIdleCount,
} from "@/components/dashboard/share-market/market-utils";
import {
  groupActiveRentalsByShare,
  sortShareMarketSubscriptions,
} from "@/components/dashboard/share-market/subscription-utils";

type SelectableSeat = Pick<ShareMarketSeat, "id" | "status" | "readOnly">;

function isIdle(seat: SelectableSeat) {
  return seat.status === "available" && !seat.readOnly;
}

export function initialCatalogSeat<T extends SelectableSeat>(seats: T[], explicit?: T) {
  if (explicit) return explicit;
  const idle = seats.filter(isIdle);
  return idle.length === 1 ? idle[0] : undefined;
}

export function preserveCatalogSeat<T extends Pick<ShareMarketSeat, "id">>(
  seats: T[],
  selectedId: string,
) {
  return seats.find((seat) => seat.id === selectedId);
}

export function catalogSeatPreview<T extends SelectableSeat>(seats: T[]) {
  return seats.filter(isIdle).slice(0, 2);
}

export const MARKET_SHARE_CARD_SEAT_PREVIEW_LIMIT = 2;

export function marketShareCardSeatPreview<T extends SelectableSeat>(
  seats: T[],
  preferredIds: string[] = [],
) {
  const preferred = preferredIds
    .map((id) => seats.find((seat) => seat.id === id))
    .filter((seat): seat is T => !!seat);
  const idle = seats.filter(isIdle);
  const seen = new Set<string>();
  const preview: T[] = [];
  for (const seat of [...preferred, ...idle, ...seats]) {
    if (seen.has(seat.id)) continue;
    seen.add(seat.id);
    preview.push(seat);
    if (preview.length === MARKET_SHARE_CARD_SEAT_PREVIEW_LIMIT) break;
  }
  return {
    preview,
    hiddenCount: Math.max(0, seats.length - preview.length),
    idleHiddenCount: Math.max(0, idle.length - preview.filter(isIdle).length),
    idleCount: idle.length,
  };
}

export function listingMatchesFamily(
  listing: Pick<ShareMarketListing, "providerFamily" | "providerFamilies">,
  family: ShareMarketProviderFamily | "all",
) {
  return family === "all"
    || listing.providerFamily === family
    || listing.providerFamilies.includes(family);
}

export function listingMatchesQuery(
  listing: Pick<
    ShareMarketListing,
    "shareName" | "subdomain" | "ownerEmail" | "supportedApps" | "appCapabilities"
  >,
  query: string,
) {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return true;
  return [
    listing.shareName,
    listing.subdomain,
    listing.ownerEmail,
    ...listing.supportedApps,
    ...listing.appCapabilities.flatMap((item) => [
      item.providerName,
      item.providerType,
      item.subscriptionLevel,
      item.upstreamModel,
      ...(item.models ?? []),
    ]),
  ].filter(Boolean).join(" ").toLocaleLowerCase().includes(needle);
}

export function listingFamilyTabs(listings: ShareMarketListing[]) {
  return PROVIDER_FAMILY_ORDER.map((value) => ({
    value,
    idle: listings.reduce((sum, listing) => {
      if (!listingMatchesFamily(listing, value)) return sum;
      return sum + listingIdleCount(listing);
    }, 0),
  })).filter((item) => listings.some((listing) => listingMatchesFamily(listing, item.value)));
}

export function filterMarketListings(
  listings: ShareMarketListing[],
  family: ShareMarketProviderFamily | "all",
  query: string,
) {
  return listings.filter((listing) =>
    listingMatchesFamily(listing, family) && listingMatchesQuery(listing, query)
  );
}

export const MARKET_CATALOG_PAGE_SIZE = 12;

export function rentedShareIdsFromSubscriptions(subscriptions: ShareMarketSubscription[]) {
  return new Set(groupActiveRentalsByShare(subscriptions).map((group) => group.shareId));
}

export function mergeCatalogWithRentedListings(
  catalogListings: ShareMarketListing[],
  rentedListings: ShareMarketListing[],
) {
  const byShareId = new Map<string, ShareMarketListing>();
  for (const listing of catalogListings) byShareId.set(listing.shareId, listing);
  for (const listing of rentedListings) byShareId.set(listing.shareId, listing);
  return [...byShareId.values()];
}

export function listingCreatedAtMs(listing: Pick<ShareMarketListing, "createdAt">) {
  const timestamp = Date.parse(listing.createdAt);
  return Number.isFinite(timestamp) ? timestamp : 0;
}

export function sortMergedCatalogListings(
  listings: ShareMarketListing[],
  subscriptions: ShareMarketSubscription[],
) {
  const groups = new Map(
    groupActiveRentalsByShare(subscriptions).map((group) => [group.shareId, group]),
  );
  return [...listings].sort((left, right) => {
    const leftGroup = groups.get(left.shareId);
    const rightGroup = groups.get(right.shareId);
    if (!!leftGroup !== !!rightGroup) return leftGroup ? -1 : 1;
    if (leftGroup && rightGroup) {
      return Number(rightGroup.attention) - Number(leftGroup.attention)
        || sortShareMarketSubscriptions(leftGroup.subscription, rightGroup.subscription);
    }
    return listingCreatedAtMs(right) - listingCreatedAtMs(left)
      || right.id.localeCompare(left.id);
  });
}

export function filterMergedCatalogListings(
  listings: ShareMarketListing[],
  {
    mine,
    family,
    query,
    rentedShareIds,
  }: {
    mine: boolean;
    family: ShareMarketProviderFamily | "all";
    query: string;
    rentedShareIds: Set<string>;
  },
) {
  return listings.filter((listing) => {
    if (mine && !rentedShareIds.has(listing.shareId)) return false;
    return listingMatchesFamily(listing, family) && listingMatchesQuery(listing, query);
  });
}

export function paginateListings<T>(
  listings: T[],
  page: number,
  pageSize = MARKET_CATALOG_PAGE_SIZE,
) {
  const total = listings.length;
  const pageCount = Math.max(1, Math.ceil(total / pageSize) || 1);
  const currentPage = Math.min(Math.max(1, page), pageCount);
  const start = (currentPage - 1) * pageSize;
  return {
    items: listings.slice(start, start + pageSize),
    page: currentPage,
    pageCount,
    pageSize,
    total,
  };
}

export function pageForShareId(
  listings: Array<Pick<ShareMarketListing, "shareId">>,
  shareId: string,
  pageSize = MARKET_CATALOG_PAGE_SIZE,
) {
  const index = listings.findIndex((listing) => listing.shareId === shareId);
  if (index < 0) return 1;
  return Math.floor(index / pageSize) + 1;
}
