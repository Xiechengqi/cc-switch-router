import type { MessageKey } from "@/lib/i18n";
import type { ListingAvailability } from "@/lib/online-client-market";

export const CLIENT_MARKET_POLL_MS = 20_000;

export const ROUTER_OPEN_LOGIN_EVENT = "router-open-login";

export type ClientMarketWorkspace = "catalog" | "selling" | "audit";

/** The requested tab. Whether the viewer may see it is decided later, once the
 * seller summary and session capability are known. */
export function clientMarketWorkspaceFromQuery(view: string | null, tab: string | null): ClientMarketWorkspace {
  if (view === "selling" || tab === "mine") return "selling";
  if (view === "audit") return "audit";
  return "catalog";
}

export function clientMarketTenantFromQuery(view: string | null, rentalId: string | null): boolean {
  return view === "rentals" || Boolean(rentalId?.trim());
}

export const SHOP_CONTINENTS = [
  "all",
  "asia",
  "europe",
  "north_america",
  "south_america",
  "oceania",
  "africa",
  "other",
] as const;

export type ShopContinent = (typeof SHOP_CONTINENTS)[number];

export const SHOP_UNKNOWN_COUNTRY = "unknown";

const SHOP_CONTINENT_MEMBERS: Record<Exclude<ShopContinent, "all" | "other">, readonly string[]> = {
  africa: ["DZ", "AO", "BJ", "BW", "BF", "BI", "CV", "CM", "CF", "TD", "KM", "CG", "CD", "CI", "DJ", "EG", "GQ", "ER", "SZ", "ET", "GA", "GM", "GH", "GN", "GW", "KE", "LS", "LR", "LY", "MG", "MW", "ML", "MR", "MU", "MA", "MZ", "NA", "NE", "NG", "RW", "ST", "SN", "SC", "SL", "SO", "ZA", "SS", "SD", "TZ", "TG", "TN", "UG", "ZM", "ZW", "EH", "RE", "YT", "SH"],
  asia: ["AF", "AM", "AZ", "BH", "BD", "BT", "BN", "KH", "CN", "CY", "GE", "HK", "IN", "ID", "IR", "IQ", "IL", "JP", "JO", "KZ", "KW", "KG", "LA", "LB", "MO", "MY", "MV", "MN", "MM", "NP", "KP", "OM", "PK", "PS", "PH", "QA", "SA", "SG", "KR", "LK", "SY", "TW", "TJ", "TH", "TL", "TR", "TM", "AE", "UZ", "VN", "YE", "IO"],
  europe: ["AX", "AL", "AD", "AT", "BY", "BE", "BA", "BG", "HR", "CZ", "DK", "EE", "FO", "FI", "FR", "DE", "GI", "GR", "GG", "VA", "HU", "IS", "IE", "IM", "IT", "JE", "LV", "LI", "LT", "LU", "MT", "MD", "MC", "ME", "NL", "MK", "NO", "PL", "PT", "RO", "RU", "SM", "RS", "SK", "SI", "ES", "SJ", "SE", "CH", "UA", "GB", "XK"],
  north_america: ["AI", "AG", "AW", "BS", "BB", "BZ", "BM", "BQ", "VG", "CA", "KY", "CR", "CU", "CW", "DM", "DO", "SV", "GL", "GD", "GP", "GT", "HT", "HN", "JM", "MQ", "MX", "MS", "NI", "PA", "PR", "BL", "KN", "LC", "MF", "PM", "VC", "SX", "TT", "TC", "US", "VI"],
  south_america: ["AR", "BO", "BR", "CL", "CO", "EC", "FK", "GF", "GY", "PY", "PE", "SR", "UY", "VE"],
  oceania: ["AS", "AU", "CX", "CC", "CK", "FJ", "PF", "GU", "HM", "KI", "MH", "FM", "NR", "NC", "NZ", "NU", "NF", "MP", "PW", "PG", "PN", "WS", "SB", "TK", "TO", "TV", "VU", "WF"],
};

const SHOP_COUNTRY_CONTINENT = new Map<string, Exclude<ShopContinent, "all" | "other">>(
  (Object.entries(SHOP_CONTINENT_MEMBERS) as Array<[Exclude<ShopContinent, "all" | "other">, readonly string[]]>)
    .flatMap(([continent, codes]) => codes.map((code) => [code, continent] as const)),
);

export function shopContinentForCountry(code: string | null | undefined): Exclude<ShopContinent, "all"> {
  const normalized = (code || "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(normalized) || normalized === SHOP_UNKNOWN_COUNTRY) return "other";
  return SHOP_COUNTRY_CONTINENT.get(normalized) || "other";
}

export type ShopCountrySort = "name" | "idle" | "price";

export type ShopCountryCard = {
  code: string;
  continent: Exclude<ShopContinent, "all">;
  idle: number;
  total: number;
  minMonthlyMinor: number | null;
  hasFreeIdle: boolean;
};

export type OnlineShopListing = {
  country: string | null;
  state: string;
  occupied: boolean;
  healthy: boolean;
  availability?: ListingAvailability;
  owned?: boolean;
  exclusiveManagement?: boolean;
  ownerRental?: boolean;
  cyclePriceMinor: number;
};

export function onlineShopSpecLine(hardware?: { arch?: string; cpu?: number | null; memoryBytes?: number | null } | null) {
  const memory = hardware?.memoryBytes;
  return [
    hardware?.arch,
    hardware?.cpu != null ? `${hardware.cpu}C` : "",
    memory != null ? `${new Intl.NumberFormat("en", { maximumFractionDigits: 1 }).format(memory / 2 ** 30)}G` : "",
  ].filter(Boolean).join(" · ");
}

type ShopAvailabilityFields = Omit<OnlineShopListing, "country" | "cyclePriceMinor">;

export function onlineListingAvailability(listing: ShopAvailabilityFields): ListingAvailability {
  if (listing.state !== "listed") return "unavailable";
  if (listing.availability !== undefined) {
    if (listing.availability !== "available") return listing.availability;
  } else {
    // Older Router responses lack an availability reason. A failed probe alone cannot identify an offline tunnel.
    if (listing.occupied) return "occupied";
    if (listing.exclusiveManagement !== true) return "upgrade_required";
    if (!listing.healthy) return "unavailable";
  }
  return listing.owned && listing.ownerRental !== true ? "upgrade_required" : "available";
}

export function onlineListingRentable(listing: ShopAvailabilityFields) {
  return onlineListingAvailability(listing) === "available";
}

const marketStates = [
  "available", "offline", "occupied", "paused", "upgrading", "switching", "unavailable",
  "locking", "unlocking", "starting", "active", "ending", "cleaning", "released",
] as const;

export function onlineStatusKey(state: string) {
  if (state === "upgrade_required") return "clientMarket.shop.upgrade";
  const known = marketStates.find(value => value === state);
  return known ? (`online.${known}` as const) : "online.unknown";
}

export function aggregateOnlineShopCountries(listings: readonly OnlineShopListing[]): ShopCountryCard[] {
  const cards = new Map<string, ShopCountryCard>();
  for (const listing of listings) {
    const code = shopCountryKey(listing.country);
    const card = cards.get(code) || {
      code,
      continent: shopContinentForCountry(code === SHOP_UNKNOWN_COUNTRY ? "" : code),
      idle: 0,
      total: 0,
      minMonthlyMinor: null,
      hasFreeIdle: false,
    };
    card.total += 1;
    if (onlineListingRentable(listing)) {
      card.idle += 1;
      if (listing.owned || listing.cyclePriceMinor <= 0) card.hasFreeIdle = true;
      else if (card.minMonthlyMinor == null || listing.cyclePriceMinor < card.minMonthlyMinor) {
        card.minMonthlyMinor = listing.cyclePriceMinor;
      }
    }
    cards.set(code, card);
  }
  return Array.from(cards.values());
}

export function shopCountryKey(code: string | null | undefined) {
  const normalized = (code || "").trim().toUpperCase();
  return /^[A-Z]{2}$/.test(normalized) ? normalized : SHOP_UNKNOWN_COUNTRY;
}

export function sortShopCountries(
  cards: readonly ShopCountryCard[],
  sort: ShopCountrySort,
  nameOf: (code: string) => string,
) {
  const ranked = (card: ShopCountryCard) => card.hasFreeIdle ? 0 : card.minMonthlyMinor ?? Number.POSITIVE_INFINITY;
  return [...cards].sort((left, right) => {
    if (sort === "idle" && left.idle !== right.idle) return right.idle - left.idle;
    if (sort === "price" && ranked(left) !== ranked(right)) return ranked(left) - ranked(right);
    const name = nameOf(left.code).localeCompare(nameOf(right.code));
    if (name !== 0) return name;
    return left.code.localeCompare(right.code);
  });
}

export const SHOP_BROWSE_KEY = "cc_switch_router_client_market_shop_browse_v1";

type ShopSortable = ShopAvailabilityFields & {
  installationId: string;
  registeredAt?: string;
  onlineRate30d?: number | null;
};

function registeredTime(value?: string) {
  const time = value ? Date.parse(value) : Number.NaN;
  return Number.isFinite(time) ? time : Number.POSITIVE_INFINITY;
}

/**
 * Shop order: rentable first, then the Client connected to the Router the
 * longest, then the higher 30-day online rate. Deterministic for equal values.
 */
export function sortShopListings<T extends ShopSortable>(listings: readonly T[]): T[] {
  return [...listings].sort((left, right) => {
    const rentable = Number(onlineListingRentable(right)) - Number(onlineListingRentable(left));
    if (rentable) return rentable;
    const age = registeredTime(left.registeredAt) - registeredTime(right.registeredAt);
    if (age) return age;
    const rate = (right.onlineRate30d ?? -1) - (left.onlineRate30d ?? -1);
    if (rate) return rate;
    return left.installationId.localeCompare(right.installationId);
  });
}

/** Case-insensitive match on the title, the public name, the seller and the country. */
export function matchesShopQuery(
  listing: { title?: string | null; label: string; sellerEmail?: string; country: string | null },
  query: string,
  countryName: (code: string) => string,
) {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const code = shopCountryKey(listing.country);
  return [listing.title, listing.label, listing.sellerEmail, code === SHOP_UNKNOWN_COUNTRY ? "" : code,
    code === SHOP_UNKNOWN_COUNTRY ? "" : countryName(code)]
    .some((part) => (part || "").toLowerCase().includes(needle));
}

export const SHOP_PAGE_SIZE = 24;

/** Why a Client rental ended, including a seller reclaim and a completed trial. */
export function endReasonKey(reason: string | null | undefined): MessageKey {
  switch (reason) {
    case "voluntary":
      return "clientTermination.voluntary";
    case "provider_issue":
      return "clientTermination.providerIssue";
    case "platform_issue":
      return "clientTermination.platformIssue";
    case "provider_reclaim":
      return "clientMarket.endReason.provider_reclaim";
    case "trial_completed":
      return "clientMarket.endReason.trial_completed";
    case "workspace_expired":
      return "clientMarket.endReason.workspace_expired";
    default:
      return "clientTermination.systemExit";
  }
}
