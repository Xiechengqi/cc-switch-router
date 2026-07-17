import { shareAccessApps } from "@/lib/share-app";
import type { ShareAccessByApp, ShareSettingsPatch, ShareView } from "@/lib/types";

export const UNLIMITED_TOKEN_LIMIT = -1;
export const UNLIMITED_PARALLEL_LIMIT = -1;
export const PERMANENT_EXPIRES_AT_ISO = "2099-12-31T23:59:59Z";

export type ShareSettingsDraft = {
  description: string;
  forSale: "Yes" | "No" | "Free";
  saleMarketKind: "token" | "share";
  marketAccessMode: "selected" | "all";
  sharedWithEmails: string[];
  accessByApp: ShareAccessByApp;
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
  const accessByApp = effectiveShareAccessByApp(share);
  return {
    description: share.description || "",
    forSale: (["Yes", "No", "Free"].includes(share.forSale) ? share.forSale : "No") as "Yes" | "No" | "Free",
    saleMarketKind: share.saleMarketKind === "share" ? "share" : "token",
    marketAccessMode: share.marketAccessMode === "all" ? "all" : "selected",
    sharedWithEmails: normalizeEmailList(share.sharedWithEmails || []),
    accessByApp,
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
    saleMarketKind: draft.saleMarketKind,
    marketAccessMode: draft.marketAccessMode,
    sharedWithEmails: normalizeEmailList(draft.sharedWithEmails),
    accessByApp: draft.accessByApp,
    tokenLimit: draft.tokenLimit,
    parallelLimit: draft.parallelLimit,
    expiresAt: draft.expiresAt,
    forSaleOfficialPricePercentByApp:
      draft.forSale === "Yes" && draft.saleMarketKind === "token" ? draft.pricing : {},
  };
}

function effectiveShareAccessByApp(share: ShareView): ShareAccessByApp {
  if (share.accessByApp && Object.keys(share.accessByApp).length > 0) return share.accessByApp;
  const result: ShareAccessByApp = {};
  for (const app of shareAccessApps(share)) {
    result[app] = {
      sharedWithEmails: normalizeEmailList(share.sharedWithEmails || []),
      marketAccessMode: share.marketAccessMode === "all" ? "all" : "selected",
    };
  }
  return result;
}

export function validateShareSettingsDraft(draft: ShareSettingsDraft) {
  const errors: string[] = [];
  if (draft.description.length > 200) errors.push("Description must be 200 characters or fewer.");
  if (draft.tokenLimit !== UNLIMITED_TOKEN_LIMIT && (!Number.isFinite(draft.tokenLimit) || draft.tokenLimit <= 0)) {
    errors.push("Token limit must be positive or unlimited.");
  }
  if (
    draft.parallelLimit !== UNLIMITED_PARALLEL_LIMIT &&
    (!Number.isFinite(draft.parallelLimit) || draft.parallelLimit <= 0)
  ) {
    errors.push("Parallel limit must be positive or unlimited.");
  }
  const expires = new Date(draft.expiresAt).getTime();
  if (!draft.expiresAt || !Number.isFinite(expires)) errors.push("Expiration time is invalid.");
  if (draft.forSale === "Yes" && draft.saleMarketKind === "token") {
    for (const value of Object.values(draft.pricing)) {
      if (!Number.isInteger(value) || value < 1 || value > 100) {
        errors.push("Model pricing must be between 1 and 100.");
        break;
      }
    }
  }
  return errors;
}
