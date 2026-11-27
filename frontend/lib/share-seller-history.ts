/*
 * The seller's ended Share rentals ("租用历史" at the bottom of My listings).
 * The server sends an allow-list: the buyer's email, when the rental ran and
 * what the Provider earned. Never the buyer's own charges or audit fields.
 */
import { authFetch } from "./auth";
import { parseJson } from "./api";

export type ShareSellerHistoryRecord = {
  id: string;
  shareId: string;
  shareName: string | null;
  shareSubdomain: string | null;
  appType: string | null;
  seatPosition: number | null;
  buyerEmail: string | null;
  activatedAt: string | null;
  endedAt: string | null;
  pricingModel: string | null;
  dailyRateMinor: number | null;
  cyclePriceMinor: number | null;
  releaseReason: string | null;
  /** Provider net earnings plus termination compensation; null when unknown. */
  incomeUnits: string | null;
};
export type ShareSellerHistoryPage = {
  items: ShareSellerHistoryRecord[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
};

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const nullableText = (v: unknown) => v === null || typeof v === "string";
const nullableInteger = (v: unknown) => v === null || Number.isSafeInteger(v);
const count = (v: unknown) => Number.isSafeInteger(v) && (v as number) >= 0;

function validRecord(v: unknown): v is ShareSellerHistoryRecord {
  return isObject(v) && typeof v.id === "string" && typeof v.shareId === "string"
    && ["shareName", "shareSubdomain", "appType", "buyerEmail", "activatedAt", "endedAt", "pricingModel", "releaseReason"].every((key) => nullableText(v[key]))
    && ["seatPosition", "dailyRateMinor", "cyclePriceMinor"].every((key) => nullableInteger(v[key]))
    && (v.incomeUnits === null || (typeof v.incomeUnits === "string" && /^-?\d+$/.test(v.incomeUnits)));
}

export function parseShareSellerHistoryPage(v: unknown): ShareSellerHistoryPage {
  if (!isObject(v) || !Array.isArray(v.items) || !v.items.every(validRecord)
    || !count(v.total) || !count(v.page) || !count(v.pageSize) || !count(v.totalPages)
    || v.items.length > (v.pageSize as number)) throw new Error("Invalid Share seller rental history page");
  return v as ShareSellerHistoryPage;
}

export async function getShareSellerHistory(page = 1, signal?: AbortSignal) {
  if (!Number.isInteger(page) || page < 1 || page > 4294967295) throw new Error("Invalid rental history page number");
  const query = new URLSearchParams({ page: String(page) });
  return parseShareSellerHistoryPage(
    await parseJson<unknown>(await authFetch(`/v1/share-market/me/rental-history?${query}`, { cache: "no-store", signal })),
  );
}

export const shareSellerHistoryLabel = (record: ShareSellerHistoryRecord) => {
  const name = record.shareName || record.shareSubdomain || record.shareId;
  return record.seatPosition != null ? `${name} · #${record.seatPosition}` : name;
};
