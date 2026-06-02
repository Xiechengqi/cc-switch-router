import type { ShareSettingsPatch, ShareView } from "@/lib/types";

export const UNLIMITED_TOKEN_LIMIT = -1;
export const UNLIMITED_PARALLEL_LIMIT = -1;
export const MIN_PARALLEL_LIMIT = 3;
export const PERMANENT_EXPIRES_AT_ISO = "2099-12-31T23:59:59Z";

export type ShareSettingsDraft = {
  description: string;
  forSale: "Yes" | "No" | "Free";
  marketAccessMode: "selected" | "all";
  sharedWithEmails: string[];
  tokenLimit: number;
  parallelLimit: number;
  expiresAt: string;
  pricing: Record<string, number>;
};

export function normalizeEmailList(value: string | string[]) {
  const items = Array.isArray(value) ? value : value.split(/[,\s]+/);
  return Array.from(
    new Set(
      items
        .map((item) => item.trim().toLowerCase())
        .filter(Boolean)
        .filter((item) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(item)),
    ),
  ).sort();
}

export function isPermanentExpiry(value?: string | null) {
  if (!value) return false;
  const date = new Date(value);
  return !Number.isNaN(date.getTime()) && date.getUTCFullYear() >= 2099;
}

export function toDateTimeLocal(value?: string | null) {
  if (!value || isPermanentExpiry(value)) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function fromDateTimeLocal(value: string) {
  if (!value.trim()) return "";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "" : date.toISOString();
}

export function draftFromShare(share: ShareView): ShareSettingsDraft {
  return {
    description: share.description || "",
    forSale: (["Yes", "No", "Free"].includes(share.forSale) ? share.forSale : "No") as "Yes" | "No" | "Free",
    marketAccessMode: share.marketAccessMode === "all" ? "all" : "selected",
    sharedWithEmails: normalizeEmailList(share.sharedWithEmails || []),
    tokenLimit: Number.isFinite(share.tokenLimit) ? share.tokenLimit : UNLIMITED_TOKEN_LIMIT,
    parallelLimit: Number.isFinite(share.parallelLimit) ? share.parallelLimit : UNLIMITED_PARALLEL_LIMIT,
    expiresAt: share.expiresAt || PERMANENT_EXPIRES_AT_ISO,
    pricing: share.forSaleOfficialPricePercentByApp || {},
  };
}

export function buildShareSettingsPatch(draft: ShareSettingsDraft): ShareSettingsPatch {
  return {
    description: draft.description.trim() || null,
    forSale: draft.forSale,
    marketAccessMode: draft.marketAccessMode,
    sharedWithEmails: normalizeEmailList(draft.sharedWithEmails),
    tokenLimit: draft.tokenLimit,
    parallelLimit: draft.parallelLimit,
    expiresAt: draft.expiresAt,
    forSaleOfficialPricePercentByApp: draft.pricing,
  };
}

export function validateShareSettingsDraft(draft: ShareSettingsDraft) {
  const errors: string[] = [];
  if (draft.description.length > 200) errors.push("Description must be 200 characters or fewer.");
  if (draft.tokenLimit !== UNLIMITED_TOKEN_LIMIT && (!Number.isFinite(draft.tokenLimit) || draft.tokenLimit <= 0)) {
    errors.push("Token limit must be positive or unlimited.");
  }
  if (
    draft.parallelLimit !== UNLIMITED_PARALLEL_LIMIT &&
    (!Number.isFinite(draft.parallelLimit) || draft.parallelLimit < MIN_PARALLEL_LIMIT)
  ) {
    errors.push(`Parallel limit must be at least ${MIN_PARALLEL_LIMIT} or unlimited.`);
  }
  const expires = new Date(draft.expiresAt).getTime();
  if (!draft.expiresAt || !Number.isFinite(expires)) errors.push("Expiration time is invalid.");
  for (const value of Object.values(draft.pricing)) {
    if (!Number.isFinite(value) || value < 1 || value > 100) {
      errors.push("Model pricing must be between 1 and 100.");
      break;
    }
  }
  return errors;
}
