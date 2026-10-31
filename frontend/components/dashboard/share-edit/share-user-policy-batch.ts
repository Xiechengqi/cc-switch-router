import type {
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserPolicy,
} from "@/lib/types";
import { stageUserPolicy } from "@/lib/share-settings";

export type ShareUserPolicyBatchPatch = {
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
