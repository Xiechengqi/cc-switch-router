import type { MessageKey } from "@/lib/i18n";
import type { AccountQuotaCycle, AccountResetCapability, ShareUserGrant } from "@/lib/types";
import { shareAccessQuotaMessagesEn } from "@/lib/share-access-quota-messages";

type Key = keyof typeof shareAccessQuotaMessagesEn;

const STATUS_PREFIX = "dashboard.userLimit.accountReset.status.";
const REASON_PREFIX = "dashboard.userLimit.accountReset.reason.";
const BUCKET_PREFIX = "dashboard.userLimit.accountReset.bucket.";
const BASIS_PREFIX = "dashboard.userLimit.accountReset.basis.";

function known(prefix: string, code: string | null | undefined, fallback: string): Key {
  const key = `${prefix}${code ?? ""}`;
  return (code && key in shareAccessQuotaMessagesEn ? key : `${prefix}${fallback}`) as Key;
}

/** Message keys are typed, so the lookups below stay inside known copy. */
export function accountResetStatusKey(status: AccountResetCapability["syncStatus"] | string | undefined): MessageKey {
  return known(STATUS_PREFIX, status, "unknown");
}

/**
 * Covers both the lowercase reasons the Share projection carries and the
 * uppercase codes the preview and save paths return. Unknown codes read as a
 * generic "unavailable"; callers keep the code for a diagnostic title.
 */
export function accountResetReasonKey(reason: string | null | undefined): MessageKey {
  return known(REASON_PREFIX, reason?.trim(), "unknown");
}

export function accountResetBucketKey(bucketId: string | undefined): MessageKey {
  return known(BUCKET_PREFIX, bucketId, "unknown");
}

export function accountResetBasisKey(basis: AccountQuotaCycle["basis"] | undefined): MessageKey | null {
  if (!basis) return null;
  const key = `${BASIS_PREFIX}${basis}`;
  return key in shareAccessQuotaMessagesEn ? (key as MessageKey) : null;
}

/** UPPER_SNAKE codes and lower_snake reasons; a plain word is not a code. */
const CODE = /^(?:[A-Z][A-Z0-9_]*|[a-z][a-z0-9]*(?:_[a-z0-9]+)+)$/;

/**
 * The Server code behind an error, from an `ApiError`'s code, a message that
 * is only a code, or a "CODE: detail" message.
 */
export function accountResetErrorCode(error: unknown): string | null {
  const code = error && typeof error === "object" && "code" in error ? (error as { code?: unknown }).code : undefined;
  if (typeof code === "string" && CODE.test(code.trim())) return code.trim();
  const message = (error instanceof Error ? error.message : typeof error === "string" ? error : "").trim();
  const head = /^([A-Za-z0-9_]+)\s*(?::|$)/.exec(message)?.[1];
  return head && CODE.test(head) ? head : null;
}

/**
 * An account cycle error in words. A known code reads as its copy; an
 * unknown code reads as the generic "unavailable" (the code is diagnostic
 * only); a message that carries no code, such as a network failure, passes
 * through unchanged.
 */
export function accountResetErrorText(error: unknown, t: (key: MessageKey) => string): string {
  const code = accountResetErrorCode(error);
  if (code) return t(accountResetReasonKey(code));
  return error instanceof Error ? error.message : String(error);
}

/** A cycle whose end has passed no longer describes the current window. */
export function accountCycleEnded(cycle: AccountQuotaCycle | undefined, nowMs: number): boolean {
  return cycle?.endsAtMs != null && cycle.endsAtMs <= nowMs;
}

/** The status line is only meaningful once someone is on the account cycle. */
export function accountResetInUse(grants: Iterable<ShareUserGrant>): boolean {
  for (const grant of grants) {
    if (grant.active !== false && grant.policy.tokenPeriod === "accountReset") return true;
  }
  return false;
}
