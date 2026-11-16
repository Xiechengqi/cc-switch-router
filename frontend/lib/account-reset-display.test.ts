import assert from "node:assert/strict";
import test from "node:test";

import {
  accountCycleEnded,
  accountResetBasisKey,
  accountResetBucketKey,
  accountResetErrorCode,
  accountResetErrorText,
  accountResetInUse,
  accountResetReasonKey,
  accountResetStatusKey,
} from "./account-reset-display";
import type { AccountQuotaCycle, ShareUserGrant } from "./types";

test("every Server status, reason and bucket maps to copy, unknown codes to a fallback", () => {
  assert.equal(accountResetStatusKey("initializing"), "dashboard.userLimit.accountReset.status.initializing");
  assert.equal(accountResetStatusKey("somethingNew"), "dashboard.userLimit.accountReset.status.unknown");
  assert.equal(accountResetStatusKey(undefined), "dashboard.userLimit.accountReset.status.unknown");
  assert.equal(accountResetReasonKey("history_incomplete"), "dashboard.userLimit.accountReset.reason.history_incomplete");
  assert.equal(accountResetReasonKey("cycle_expired"), "dashboard.userLimit.accountReset.reason.cycle_expired");
  assert.equal(accountResetReasonKey("QUOTA_LEDGER_CAPACITY_LIMIT"), "dashboard.userLimit.accountReset.reason.QUOTA_LEDGER_CAPACITY_LIMIT");
  assert.equal(accountResetReasonKey("NOT_A_CODE"), "dashboard.userLimit.accountReset.reason.unknown");
  assert.equal(accountResetBucketKey("shared_weekly"), "dashboard.userLimit.accountReset.bucket.shared_weekly");
  assert.equal(accountResetBucketKey("premium_monthly"), "dashboard.userLimit.accountReset.bucket.premium_monthly");
  assert.equal(accountResetBucketKey("monthly_pool"), "dashboard.userLimit.accountReset.bucket.unknown");
  assert.equal(accountResetReasonKey("non_recurring_package"), "dashboard.userLimit.accountReset.reason.non_recurring_package");
  assert.equal(accountResetBasisKey("derivedFromWindow"), "dashboard.userLimit.accountReset.basis.derivedFromWindow");
  assert.equal(accountResetBasisKey(undefined), null);
});

test("an account cycle error reads as copy whatever shape carries its code", () => {
  const t = (key: string) => `copy:${key}`;
  assert.equal(accountResetErrorText(new Error("ACCOUNT_RESET_NOT_READY"), t), "copy:dashboard.userLimit.accountReset.reason.ACCOUNT_RESET_NOT_READY");
  assert.equal(accountResetErrorText(new Error("history_incomplete"), t), "copy:dashboard.userLimit.accountReset.reason.history_incomplete");
  // "CODE: detail" from the preview endpoint.
  assert.equal(accountResetErrorText(new Error("ACCOUNT_RESET_UNSUPPORTED: upgrade cc-switch-server"), t),
    "copy:dashboard.userLimit.accountReset.reason.ACCOUNT_RESET_UNSUPPORTED");
  // An ApiError keeps the code apart from an English sentence.
  const apiError = Object.assign(new Error("Share changed; reload and preview again"), { code: "QUOTA_CYCLE_CONFLICT" });
  assert.equal(accountResetErrorText(apiError, t), "copy:dashboard.userLimit.accountReset.reason.QUOTA_CYCLE_CONFLICT");
  // Unknown codes read as the generic copy, never as the raw code.
  assert.equal(accountResetErrorText(new Error("UNKNOWN_CODE"), t), "copy:dashboard.userLimit.accountReset.reason.unknown");
  // A message with no code passes through.
  assert.equal(accountResetErrorText(new Error("network failed"), t), "network failed");
  assert.equal(accountResetErrorCode(new Error("Failed: to fetch")), null);
});

test("a cycle has ended once its end is not in the future", () => {
  const cycle = { endsAtMs: 1_000 } as AccountQuotaCycle;
  assert.equal(accountCycleEnded(cycle, 999), false);
  assert.equal(accountCycleEnded(cycle, 1_000), true);
  assert.equal(accountCycleEnded({ endsAtMs: null } as AccountQuotaCycle, 5_000), false);
  assert.equal(accountCycleEnded(undefined, 5_000), false);
});

test("the account cycle is in use only through an active grant on it", () => {
  const grant = (tokenPeriod: ShareUserGrant["policy"]["tokenPeriod"], active = true) =>
    ({ email: "a@example.com", active, policy: { tokenPeriod } }) as ShareUserGrant;
  assert.equal(accountResetInUse([grant("lifetime")]), false);
  assert.equal(accountResetInUse([grant("accountReset", false)]), false);
  assert.equal(accountResetInUse([grant("lifetime"), grant("accountReset")]), true);
});
