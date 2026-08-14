import type {
  ShareTokenPeriod,
  ShareUserGrantMap,
} from "@/lib/types";

export type ShareUserPolicyBatchPatch = {
  parallelLimit?: { value?: number };
  tokenLimit?: {
    value?: number;
    period: ShareTokenPeriod;
    periodAnchorAtMs?: number;
  };
  expiresAt?: { value?: number };
};

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

    const policy = { ...grant.policy };
    if (patch.parallelLimit) {
      policy.parallelLimit = patch.parallelLimit.value;
    }
    if (patch.tokenLimit) {
      policy.tokenLimit = patch.tokenLimit.value;
      policy.tokenPeriod = patch.tokenLimit.period;
      policy.tokenPeriodAnchorAtMs = patch.tokenLimit.periodAnchorAtMs;
    }
    if (patch.expiresAt) {
      policy.expiresAt = patch.expiresAt.value;
    }
    updated[key] = { ...grant, policy };
  }

  return updated;
}
