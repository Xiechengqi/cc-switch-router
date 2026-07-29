import type { ShareMarketSeat, ShareMarketSubscription } from "@/lib/types";

export type SeatSortKey =
  | "online"
  | "share"
  | "seat"
  | "parallel"
  | "tokens"
  | "amount"
  | "status"
  | "owner";

export type SeatSortPrefs = { key: SeatSortKey | null; dir: "asc" | "desc" };

export const CLEARED_SEAT_SORT: SeatSortPrefs = { key: null, dir: "asc" };

export const SEAT_SORT_COLUMN_LABELS = {
  online: "shareMarket.col.online",
  share: "shareMarket.col.share",
  seat: "shareMarket.col.seat",
  parallel: "shareMarket.col.parallel",
  tokens: "shareMarket.col.tokens",
  amount: "shareMarket.col.amount",
  status: "shareMarket.col.status",
  owner: "shareMarket.owner",
} as const;

export type SeatRowLike = {
  listing: {
    shareName: string;
    shareOnline: boolean;
    ownerEmail: string;
    subdomain?: string;
  };
  seat: Pick<
    ShareMarketSeat,
    "position" | "parallelLimit" | "tokenLimit" | "priceMinor" | "isFree" | "status" | "currency" | "readOnly"
  >;
  subscription?: Pick<ShareMarketSubscription, "status">;
  statusKey: string;
  searchText: string;
};

export function toggleSeatSort(current: SeatSortPrefs, key: SeatSortKey): SeatSortPrefs {
  if (current.key !== key) return { key, dir: "asc" };
  if (current.dir === "asc") return { key, dir: "desc" };
  return CLEARED_SEAT_SORT;
}

function cmp(a: string | number, b: string | number) {
  if (typeof a === "number" && typeof b === "number") return a - b;
  return String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: "base" });
}

export function sortSeatRows<T extends SeatRowLike>(rows: T[], prefs: SeatSortPrefs): T[] {
  const dir = prefs.dir === "asc" ? 1 : -1;
  const key = prefs.key;
  return [...rows].sort((left, right) => {
    const lifecycle = Number(isRetiredSeatRow(left)) - Number(isRetiredSeatRow(right));
    if (lifecycle !== 0) return lifecycle;
    if (!key) return 0;
    let result = 0;
    switch (key) {
      case "online":
        result = Number(left.listing.shareOnline) - Number(right.listing.shareOnline);
        break;
      case "share":
        result = cmp(left.listing.shareName, right.listing.shareName);
        break;
      case "seat":
        result = left.seat.position - right.seat.position;
        break;
      case "parallel":
        result = cmp(
          left.seat.parallelLimit ?? Number.POSITIVE_INFINITY,
          right.seat.parallelLimit ?? Number.POSITIVE_INFINITY,
        );
        break;
      case "tokens":
        result = cmp(
          left.seat.tokenLimit ?? Number.POSITIVE_INFINITY,
          right.seat.tokenLimit ?? Number.POSITIVE_INFINITY,
        );
        break;
      case "amount":
        result = cmp(
          left.seat.isFree ? 0 : left.seat.priceMinor ?? 0,
          right.seat.isFree ? 0 : right.seat.priceMinor ?? 0,
        );
        break;
      case "status":
        result = cmp(left.statusKey, right.statusKey);
        break;
      case "owner":
        result = cmp(left.listing.ownerEmail, right.listing.ownerEmail);
        break;
    }
    return result * dir;
  });
}

export function isRetiredSeatRow(row: SeatRowLike) {
  return row.seat.readOnly
    || row.seat.status === "retired"
    || row.subscription?.status === "released"
    || row.subscription?.status === "grant_failed";
}

export function sortSeatsByLifecycle<T extends ShareMarketSeat>(seats: T[]): T[] {
  return [...seats].sort((left, right) => {
    const lifecycle = Number(left.readOnly || left.status === "retired")
      - Number(right.readOnly || right.status === "retired");
    return lifecycle || left.position - right.position;
  });
}
