import assert from "node:assert/strict";
import test from "node:test";
import { exactTransferMinor, marketFinanceActions, marketFinanceActionsForMode, validTronTransactionId } from "./market-finance-actions";

test("owner transfer actions follow the audited offline settlement states", () => {
  assert.deepEqual(marketFinanceActions("requested"), ["approve", "reject"]);
  assert.deepEqual(marketFinanceActions("approved"), ["begin_transfer", "reject"]);
  assert.deepEqual(marketFinanceActions("transfer_in_progress"), ["mark_paid", "mark_failed", "review"]);
  assert.deepEqual(marketFinanceActions("review_required"), ["mark_paid", "mark_failed"]);
  assert.deepEqual(marketFinanceActions("paid"), []);
  assert.deepEqual(marketFinanceActions("rejected"), []);
});

test("pausing finance stops new Provider transfers but preserves in-flight settlement", () => {
  assert.deepEqual(marketFinanceActionsForMode("requested", "withdrawals", "disabled"), ["reject"]);
  assert.deepEqual(marketFinanceActionsForMode("approved", "withdrawals", "shadow"), ["reject"]);
  assert.deepEqual(marketFinanceActionsForMode("transfer_in_progress", "withdrawals", "disabled"), ["mark_paid", "mark_failed", "review"]);
  assert.deepEqual(marketFinanceActionsForMode("requested", "withdrawals", "enabled"), ["approve", "reject"]);
  assert.deepEqual(marketFinanceActionsForMode("requested", "withdrawals", "enabled", true), ["reject"]);
  assert.deepEqual(marketFinanceActionsForMode("approved", "withdrawals", "enabled", true), ["reject"]);
  assert.deepEqual(marketFinanceActionsForMode("transfer_in_progress", "withdrawals", "enabled", true), ["mark_paid", "mark_failed", "review"]);
});

test("offline transfer amount must be exact cents and a safe JSON number", () => {
  assert.equal(exactTransferMinor("8640000"), 100);
  assert.equal(exactTransferMinor("8640001"), null);
  assert.equal(exactTransferMinor("0"), null);
  assert.equal(exactTransferMinor("not-a-number"), null);
  assert.equal(exactTransferMinor("778222015609621621600"), null);
});

test("owner transaction reference is a 64-character TRON hash", () => {
  assert.equal(validTronTransactionId("f".repeat(64)), true);
  assert.equal(validTronTransactionId(` ${"a".repeat(64)} `), true);
  assert.equal(validTronTransactionId("f".repeat(63)), false);
  assert.equal(validTronTransactionId("x".repeat(64)), false);
});
