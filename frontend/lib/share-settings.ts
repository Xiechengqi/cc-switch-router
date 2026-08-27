import type {
  ShareSettingsPatch,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserPolicy,
  ShareView,
} from "@/lib/types";

export const UNLIMITED_TOKEN_LIMIT = -1;
export const UNLIMITED_PARALLEL_LIMIT = -1;
export const PERMANENT_EXPIRES_AT_ISO = "2099-12-31T23:59:59Z";
export const DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES = 60;
export const MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES = 10;
export const MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES = 7 * 24 * 60;

export type ShareSettingsDraft = {
  description: string;
  freeAccess: boolean;
  tokenLimit: number;
  parallelLimit: number;
  expiresAt: string;
  userGrants: ShareUserGrantMap;
};

export function isRouterShareMarketManagedGrant(
  grant: ShareUserGrant | undefined,
): boolean {
  return grant?.manager === "routerShareMarket" && grant.active !== false;
}

export function isRevokedRouterShareMarketGrant(
  grant: ShareUserGrant | undefined,
): boolean {
  return grant?.manager === "routerShareMarket" && grant.active === false;
}

export function routerShareMarketManagedEmails(
  grants: ShareUserGrantMap | undefined,
): Set<string> {
  return new Set(
    Object.entries(grants ?? {})
      .filter(([, grant]) => isRouterShareMarketManagedGrant(grant))
      .map(([key, grant]) => (grant.email || key).trim().toLowerCase())
      .filter(Boolean),
  );
}

export function ordinaryShareUserGrant(
  email: string,
  ownerEmail: string,
  previous: ShareUserGrant | undefined,
  policy: ShareUserPolicy,
): ShareUserGrant {
  const normalizedEmail = email.trim().toLowerCase();
  const isOwner = normalizedEmail === ownerEmail.trim().toLowerCase();
  return {
    ...(isRevokedRouterShareMarketGrant(previous) ? undefined : previous),
    email: normalizedEmail,
    role: isOwner ? "owner" : "shareto",
    active: true,
    manager: isOwner ? "owner" : "manual",
    entitlementId: undefined,
    revokedAtMs: undefined,
    policy,
  };
}

export function buildOrdinaryUserGrantsPatch(
  grants: ShareUserGrantMap | undefined,
  ownerEmail: string,
  defaultPolicy: ShareUserPolicy,
): ShareUserGrantMap {
  const userGrants: ShareUserGrantMap = {};
  for (const [key, grant] of Object.entries(grants ?? {})) {
    if (!isRouterShareMarketManagedGrant(grant)) continue;
    const email = (grant.email || key).trim().toLowerCase();
    if (email) userGrants[email] = grant;
  }
  const owner = ownerEmail.trim().toLowerCase();
  const activeEmails = new Set(
    [
      owner,
      ...Object.values(grants ?? {})
        .filter((grant) => grant.active !== false && grant.role === "shareto")
        .map((grant) => (grant.email || "").trim().toLowerCase()),
    ].filter(Boolean),
  );
  for (const email of activeEmails) {
    const previous = grants?.[email];
    if (isRouterShareMarketManagedGrant(previous)) continue;
    userGrants[email] = ordinaryShareUserGrant(
      email,
      owner,
      previous,
      previous?.policy ?? { ...defaultPolicy },
    );
  }
  return userGrants;
}

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
  const ownerEmail = (share.ownerEmail || "").trim().toLowerCase();
  const userGrants: ShareUserGrantMap = {};
  for (const [key, grant] of Object.entries(share.userGrants || {})) {
    const email = (grant.email || key).trim().toLowerCase();
    if (!email) continue;
    userGrants[email] = {
      ...grant,
      email,
      role: email === ownerEmail ? "owner" : "shareto",
    };
  }
  const defaultPolicy = defaultUserPolicy(share);
  if (ownerEmail && !userGrants[ownerEmail]) {
    userGrants[ownerEmail] = {
      email: ownerEmail,
      role: "owner",
      active: true,
      policy: defaultPolicy,
      manager: "owner",
    };
  }
  return {
    description: share.description || "",
    freeAccess: share.freeAccess,
    tokenLimit: Number.isFinite(share.tokenLimit) ? share.tokenLimit : UNLIMITED_TOKEN_LIMIT,
    parallelLimit: Number.isFinite(share.parallelLimit) ? share.parallelLimit : UNLIMITED_PARALLEL_LIMIT,
    expiresAt: share.expiresAt || PERMANENT_EXPIRES_AT_ISO,
    userGrants,
  };
}

export function buildShareSettingsPatch(
  draft: ShareSettingsDraft,
  share: ShareView,
): ShareSettingsPatch {
  return {
    description: draft.description.trim() || null,
    freeAccess: draft.freeAccess,
    tokenLimit: draft.tokenLimit,
    parallelLimit: draft.parallelLimit,
    expiresAt: draft.expiresAt,
    userGrants: buildOrdinaryUserGrantsPatch(
      draft.userGrants,
      share.ownerEmail || "",
      defaultUserPolicy(share),
    ),
  };
}

function defaultUserPolicy(share: ShareView): ShareUserPolicy {
  const permanent = isPermanentExpiry(share.expiresAt);
  return {
    parallelLimit:
      share.parallelLimit === UNLIMITED_PARALLEL_LIMIT
        ? undefined
        : share.parallelLimit,
    tokenLimit:
      share.tokenLimit === UNLIMITED_TOKEN_LIMIT ? undefined : share.tokenLimit,
    tokenPeriod: "lifetime",
    expiresAt: permanent ? undefined : new Date(share.expiresAt).getTime(),
  };
}

export type ShareSettingsFieldErrors = {
  description: boolean;
  tokenLimit: boolean;
  parallelLimit: boolean;
  expiresAt: boolean;
};

export function shareSettingsFieldErrors(
  draft: ShareSettingsDraft,
): ShareSettingsFieldErrors {
  const expires = new Date(draft.expiresAt).getTime();
  return {
    description: draft.description.trim().length > 200,
    tokenLimit:
      draft.tokenLimit !== UNLIMITED_TOKEN_LIMIT &&
      (!Number.isFinite(draft.tokenLimit) || draft.tokenLimit <= 0),
    parallelLimit:
      draft.parallelLimit !== UNLIMITED_PARALLEL_LIMIT &&
      (!Number.isFinite(draft.parallelLimit) || draft.parallelLimit <= 0),
    expiresAt: !draft.expiresAt || !Number.isFinite(expires),
  };
}

export function shareSettingsHasFieldErrors(errors: ShareSettingsFieldErrors) {
  return errors.description || errors.tokenLimit || errors.parallelLimit || errors.expiresAt;
}
