import type {
  ShareMarketListing,
  ShareMarketProviderFamily,
  ShareMarketSeat,
} from "@/lib/types";
import {
  PROVIDER_FAMILY_ORDER,
  listingIdleCount,
} from "@/components/dashboard/share-market/market-utils";

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
