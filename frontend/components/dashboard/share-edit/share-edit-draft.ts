import { shareProviderSupportedApps, SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import type {
  ShareSettingsPatch,
  ShareUserGrantMap,
  ShareUserPolicy,
  ShareUserUsageEditMap,
  ShareView,
} from "@/lib/types";
import { isRouterShareMarketManagedGrant } from "@/lib/share-settings";
import { millionsInputToTokens, tokensToMillionsInput } from "@/lib/token-units";
import {
  DEFAULT_PARALLEL_LIMIT,
  DEFAULT_TOKEN_LIMIT,
  isPermanentExpiryDate,
  isUnlimitedExpiry,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  PERMANENT_EXPIRES_AT_ISO,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/components/dashboard/share-dashboard-utils";

export type PriceApp = CoreShareApp;

export const PRICE_APPS: Array<{ key: PriceApp; label: string }> = [
  { key: "claude", label: SHARE_APP_LABELS.claude },
  { key: "codex", label: SHARE_APP_LABELS.codex },
  { key: "gemini", label: SHARE_APP_LABELS.gemini },
];

export type ShareEditDraft = {
  description: string;
  freeAccess: boolean;
  tokenLimitInput: string;
  tokenLimitUnlimited: boolean;
  lastFiniteTokenLimit: number;
  parallelLimitInput: string;
  parallelLimitUnlimited: boolean;
  lastFiniteParallelLimit: number;
  expiresAtInput: string;
  expiresPermanent: boolean;
  userGrants: ShareUserGrantMap;
  userUsageEdits: ShareUserUsageEditMap;
  enabledApps: Record<PriceApp, boolean>;
};

export function normalizedUniqueEmails(values: string[]) {
  return Array.from(new Set(values.map((value) => value.trim().toLowerCase()).filter(Boolean))).sort();
}

export function toLocalDateTimeValue(value?: string) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (num: number) => String(num).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function fromLocalDateTimeValue(value: string) {
  if (!value.trim()) return undefined;
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toISOString() : value;
}

function defaultUserPolicy(share: ShareView): ShareUserPolicy {
  const tokenLimit = share.tokenLimit ?? UNLIMITED_TOKEN_LIMIT;
  const parallelLimit = Number.isFinite(share.parallelLimit) ? share.parallelLimit : UNLIMITED_PARALLEL_LIMIT;
  const permanent = isPermanentExpiryDate(share.expiresAt) || isUnlimitedExpiry(share.expiresAt);
  return {
    parallelLimit: isUnlimitedParallelLimit(parallelLimit) ? undefined : parallelLimit,
    tokenLimit: isUnlimitedTokenLimit(tokenLimit) ? undefined : tokenLimit,
    tokenPeriod: "lifetime",
    expiresAt: permanent ? undefined : new Date(share.expiresAt).getTime(),
  };
}

export function buildShareEditDraft(share: ShareView): ShareEditDraft {
  const activeShareApps = shareProviderSupportedApps(share);

  const userGrants: ShareUserGrantMap = { ...(share.userGrants || {}) };
  const ownerEmail = (share.ownerEmail || "").trim().toLowerCase();
  if (ownerEmail && !userGrants[ownerEmail]) {
    userGrants[ownerEmail] = { email: ownerEmail, role: "owner", active: true, policy: defaultUserPolicy(share) };
  }
  const tokenLimit = share.tokenLimit ?? UNLIMITED_TOKEN_LIMIT;
  const parallelLimit = Number.isFinite(share.parallelLimit) ? share.parallelLimit : UNLIMITED_PARALLEL_LIMIT;
  const tokenUnlimited = isUnlimitedTokenLimit(tokenLimit);
  const parallelUnlimited = isUnlimitedParallelLimit(parallelLimit);
  const permanent = isPermanentExpiryDate(share.expiresAt) || isUnlimitedExpiry(share.expiresAt);
  return {
    description: share.description || "",
    freeAccess: share.freeAccess,
    tokenLimitInput: tokenUnlimited ? "" : tokensToMillionsInput(tokenLimit),
    tokenLimitUnlimited: tokenUnlimited,
    lastFiniteTokenLimit: !tokenUnlimited && tokenLimit > 0 ? tokenLimit : DEFAULT_TOKEN_LIMIT,
    parallelLimitInput: parallelUnlimited ? String(UNLIMITED_PARALLEL_LIMIT) : String(parallelLimit),
    parallelLimitUnlimited: parallelUnlimited,
    lastFiniteParallelLimit: !parallelUnlimited && parallelLimit > 0 ? parallelLimit : DEFAULT_PARALLEL_LIMIT,
    expiresAtInput: permanent ? "" : toLocalDateTimeValue(share.expiresAt),
    expiresPermanent: permanent,
    userGrants,
    userUsageEdits: {},
    enabledApps: {
      claude: activeShareApps.includes("claude") && (share.support ? share.support.claude !== false : true),
      codex: activeShareApps.includes("codex") && (share.support ? share.support.codex !== false : true),
      gemini: activeShareApps.includes("gemini") && (share.support ? share.support.gemini !== false : true),
    },
  };
}

export function buildShareEditPatch(draft: ShareEditDraft, share: ShareView, activeShareApps: PriceApp[]): ShareSettingsPatch {
  const tokenLimit = draft.tokenLimitUnlimited
    ? UNLIMITED_TOKEN_LIMIT
    : millionsInputToTokens(draft.tokenLimitInput) ?? 0;
  const parallelLimit = draft.parallelLimitUnlimited ? UNLIMITED_PARALLEL_LIMIT : Number.parseInt(draft.parallelLimitInput, 10);
  const expiresIso = draft.expiresPermanent ? PERMANENT_EXPIRES_AT_ISO : fromLocalDateTimeValue(draft.expiresAtInput);
  const ownerEmail = (share.ownerEmail || "").trim().toLowerCase();
  const accessEmails = normalizedUniqueEmails(
    Object.values(draft.userGrants)
      .filter((grant) => grant.active !== false && grant.role === "shareto")
      .map((grant) => grant.email),
  );
  const activeGrantEmails = new Set([ownerEmail, ...accessEmails].filter(Boolean));
  const defaultPolicy: ShareUserPolicy = {
    parallelLimit: parallelLimit >= 0 ? parallelLimit : undefined,
    tokenLimit: tokenLimit >= 0 ? tokenLimit : undefined,
    tokenPeriod: "lifetime",
    expiresAt: !draft.expiresPermanent && expiresIso ? new Date(expiresIso).getTime() : undefined,
  };
  const userGrants: ShareUserGrantMap = {};
  for (const [key, grant] of Object.entries(draft.userGrants)) {
    if (!isRouterShareMarketManagedGrant(grant)) continue;
    const email = (grant.email || key).trim().toLowerCase();
    if (email) userGrants[email] = grant;
  }
  for (const email of activeGrantEmails) {
    const previous = draft.userGrants[email];
    if (isRouterShareMarketManagedGrant(previous)) continue;
    userGrants[email] = { ...previous, email, role: email === ownerEmail ? "owner" : "shareto", active: true, policy: previous?.policy ?? { ...defaultPolicy } };
  }
  const patch: ShareSettingsPatch = {
    description: draft.description.trim() || null,
    freeAccess: draft.freeAccess,
    tokenLimit,
    parallelLimit,
    support: { claude: Boolean(draft.enabledApps.claude), codex: Boolean(draft.enabledApps.codex), gemini: Boolean(draft.enabledApps.gemini) },
  };
  patch.userGrants = userGrants;
  if (Object.keys(draft.userUsageEdits).length > 0) {
    patch.userUsageEdits = draft.userUsageEdits;
  }
  if (expiresIso) patch.expiresAt = expiresIso;
  return patch;
}

export function shareEditPatchFingerprint(patch: ShareSettingsPatch) {
  return JSON.stringify(patch);
}
