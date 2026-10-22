import assert from "node:assert/strict";
import test from "node:test";
import { ApiError } from "./api";
import { earningConversionDefinitelyRejected, earningConversionResultMatches, readPendingEarningConversion, writePendingEarningConversion,
  EARNING_CONVERSION_PENDING_PREFIX, type PendingEarningConversion } from "./earning-wallet-conversion";

function storage() {
  const items = new Map<string, string>();
  return { getItem: (key: string) => items.get(key) ?? null, setItem: (key: string, value: string) => { items.set(key, value); } };
}
const pending: PendingEarningConversion = { actor: "provider", key: "conversion_123", quote: {
  id: "quote", amountUnits: "8640001", termsVersion: 1, expiresAt: "2026-01-01T00:00:00Z",
} };
test("same-user recovery preserves exact units and original idempotency key even after quote expiry", () => {
  const cache = storage();
  writePendingEarningConversion(cache, pending);
  assert.deepEqual(readPendingEarningConversion(cache, "provider"), pending);
  assert.equal(readPendingEarningConversion(cache, "other"), null);
});
test("corrupt metadata never creates a new conversion or crosses an account", () => {
  const cache = storage();
  for (const value of [null, {}, { ...pending, actor: "other" }, { ...pending, key: "bad key" },
    { ...pending, quote: { ...pending.quote, amountUnits: "1.1" } },
    { ...pending, quote: { ...pending.quote, amountUnits: "9223372036854775808" } },
    { ...pending, quote: { ...pending.quote, termsVersion: 2 } }]) {
    cache.setItem(EARNING_CONVERSION_PENDING_PREFIX + "provider", JSON.stringify(value));
    assert.throws(() => readPendingEarningConversion(cache, "provider"));
  }
});
test("a success must match the original quote, amount and terms", () => {
  const result = { id: "conversion", quoteId: "quote", amountUnits: "8640001", termsVersion: 1, journalId: "journal", createdAt: "t" };
  assert.equal(earningConversionResultMatches(result, pending), true);
  for (const changes of [{ amountUnits: "8640000" }, { quoteId: "other" }, { termsVersion: 2 }, { journalId: "" }]) {
    assert.equal(earningConversionResultMatches({ ...result, ...changes }, pending), false);
  }
});
test("submission requires verified recovery storage, not a silently lost key", () => {
  assert.throws(() => writePendingEarningConversion({ setItem() {}, getItem: () => null }, pending));
});
test("timeouts and session failures keep recovery metadata; only explicit business rejections clear it", () => {
  for (const status of [401, 403, 408, 429, 500, 503, 504]) {
    assert.equal(earningConversionDefinitelyRejected(new ApiError(status, "network or session")), false);
  }
  assert.equal(earningConversionDefinitelyRejected(new ApiError(409, "unknown conflict")), false);
  assert.equal(earningConversionDefinitelyRejected(new Error("connection lost")), false);
  assert.equal(earningConversionDefinitelyRejected(new ApiError(409, "changed", "EARNING_CONVERSION_INSUFFICIENT")), true);
  assert.equal(earningConversionDefinitelyRejected(new ApiError(409, "expired", "EARNING_CONVERSION_QUOTE_EXPIRED")), true);
  assert.equal(earningConversionDefinitelyRejected(new ApiError(404, "route missing")), false);
  assert.equal(earningConversionDefinitelyRejected(new ApiError(404, "pruned", "EARNING_CONVERSION_QUOTE_NOT_FOUND")), true);
});
