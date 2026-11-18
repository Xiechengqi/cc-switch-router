import type {
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserPolicy,
} from "@/lib/types";
import { stageUserPolicy } from "@/lib/share-settings";

export type ShareUserPolicyBatchPatch = {
  allowedOperations?: { value?: ShareUserPolicy["allowedOperations"] };
  parallelLimit?: { value?: number };
  tokenLimit?: {
    value?: number;
    valueQuota?: ShareUserPolicy["valueQuota"];
    period: ShareTokenPeriod;
    periodAnchorAtMs?: number;
  };
  expiresAt?: { value?: number };
};

/** The policy a batch patch asks for, built on the grant's scheduled policy when it has one. */
export function patchedUserPolicy(grant: ShareUserGrant, patch: ShareUserPolicyBatchPatch): ShareUserPolicy {
  const policy = { ...(grant.pendingValueQuota?.policy ?? grant.policy) };
  if (patch.allowedOperations) policy.allowedOperations = patch.allowedOperations.value;
  if (patch.parallelLimit) {
    policy.parallelLimit = patch.parallelLimit.value;
  }
  if (patch.tokenLimit) {
    policy.tokenLimit = patch.tokenLimit.value;
    if (patch.tokenLimit.valueQuota) policy.valueQuota = patch.tokenLimit.valueQuota;
    else delete policy.valueQuota;
    policy.tokenPeriod = patch.tokenLimit.period;
    policy.tokenPeriodAnchorAtMs = patch.tokenLimit.periodAnchorAtMs;
  }
  if (patch.expiresAt) {
    policy.expiresAt = patch.expiresAt.value;
  }
  return policy;
}

export function applyShareUserPolicyBatch(
  value: ShareUserGrantMap,
  selectedEmails: ReadonlySet<string>,
  patch: ShareUserPolicyBatchPatch,
): ShareUserGrantMap {
  const normalizedEmails = new Set(
    Array.from(selectedEmails, (email) => email.trim().toLowerCase()).filter(Boolean),
  );
  const updated: ShareUserGrantMap = { ...value };

  for (const [key, grant] of Object.entries(value)) {
    const email = grant.email.trim().toLowerCase();
    if (!normalizedEmails.has(email)) continue;
    updated[key] = stageUserPolicy(grant, patchedUserPolicy(grant, patch));
  }

  return updated;
}

export const USD_PARALLEL_LIMIT = 4;

/** Retain a historical value when another field changes; never offer new USD limits above 4. */
export function validParallelLimit(limit: number | undefined, usd: boolean, previous?: number): boolean {
  return limit == null || (Number.isSafeInteger(limit) && limit >= 1
    && (!usd || limit <= USD_PARALLEL_LIMIT || limit === previous));
}

/** Include active USD grants while a change of unit is still awaiting activation. */
export function batchUsesUsd(grants: ShareUserGrant[], selected: ReadonlySet<string>, patch: ShareUserPolicyBatchPatch): boolean {
  return selected.size > 0 && Boolean(patch.tokenLimit?.valueQuota || grants.some(grant =>
    selected.has(grant.email) && (grant.policy.valueQuota || grant.pendingValueQuota?.policy.valueQuota)));
}
