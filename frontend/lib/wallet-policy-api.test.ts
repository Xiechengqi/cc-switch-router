import assert from "node:assert/strict";
import test from "node:test";
import { ApiError } from "./api";
import { getTransferCapacity, parseTransferCapacity, type TransferCapacity } from "./wallet-policy-api";

const capacity: TransferCapacity = {
  transferableUnits: "77760000",
  committedUnits: "0",
  dailyRemainingUnits: "43200000000",
  enabled: true,
  verified: true,
  blockers: [],
};
const unavailable = {
  name: "Error",
  message: "Wallet transfer capacity unavailable; reload to retry.",
};

test("transfer capacity accepts exact units, including zero and values above Number's safe range", () => {
  assert.deepEqual(parseTransferCapacity(capacity), capacity);
  for (const units of ["0", "1", "9007199254740993", "9223372036854775807"]) {
    const value = { ...capacity, transferableUnits: units, committedUnits: units, dailyRemainingUnits: units };
    assert.deepEqual(parseTransferCapacity(value), value);
  }
});

test("transfer capacity preserves every valid server blocker combination without opening transfers", () => {
  const blockers: TransferCapacity["blockers"] = ["disabled", "paused", "identity", "finance_hold"];
  for (let mask = 0; mask < 1 << blockers.length; mask++) {
    const selected = blockers.filter((_, index) => mask & (1 << index));
    const value = { ...capacity, enabled: selected.length === 0, verified: !selected.includes("identity"), blockers: selected };
    assert.deepEqual(parseTransferCapacity(value), value);
  }
});

test("transfer capacity rejects empty, non-object and incomplete responses", () => {
  for (const value of [undefined, null, false, true, 0, "", "untrusted response", [], [capacity], {}]) {
    assert.throws(() => parseTransferCapacity(value), unavailable);
  }
  for (const key of Object.keys(capacity)) {
    const value: Record<string, unknown> = { ...capacity };
    delete value[key];
    assert.throws(() => parseTransferCapacity(value), unavailable, `missing ${key}`);
  }
});

for (const field of ["transferableUnits", "committedUnits", "dailyRemainingUnits"] as const) {
  test(`transfer capacity rejects malformed ${field} without coercing it to money`, () => {
    for (const units of [
      undefined, null, true, false, 0, 1, Number.MAX_SAFE_INTEGER, [], {},
      "", " ", " 1", "1 ", "+1", "-1", "-0", "01", "1.0", "0.1", "1e3", "0x10", "NaN",
      "1\n", "9223372036854775808", "10000000000000000000", "9".repeat(10_000),
    ]) {
      assert.throws(() => parseTransferCapacity({ ...capacity, [field]: units }), unavailable);
    }
  });
}

test("transfer capacity requires boolean capability flags and known blocker arrays", () => {
  for (const field of ["enabled", "verified"] as const) {
    for (const flag of [undefined, null, 0, 1, "true", "false", [], {}]) {
      assert.throws(() => parseTransferCapacity({ ...capacity, [field]: flag }), unavailable);
    }
  }
  for (const blockers of [undefined, null, true, "paused", {}, [null], [1], ["unknown"], ["paused", "untrusted response"]]) {
    assert.throws(() => parseTransferCapacity({ ...capacity, enabled: false, blockers }), unavailable);
  }
});

test("transfer capacity rejects contradictory capabilities and amounts above the daily allowance", () => {
  for (const value of [
    { ...capacity, enabled: false },
    { ...capacity, verified: false },
    { ...capacity, blockers: ["paused"] },
    { ...capacity, enabled: false, blockers: ["identity"] },
    { ...capacity, enabled: false, verified: false, blockers: ["paused"] },
    { ...capacity, dailyRemainingUnits: "1" },
  ]) {
    assert.throws(() => parseTransferCapacity(value), unavailable);
  }
});

test("the transfer capacity API rejects invalid successful responses before exposing them and can retry", async (context) => {
  let body: unknown = {};
  let calls = 0;
  context.mock.method(globalThis, "fetch", async (url: RequestInfo | URL, options?: RequestInit) => {
    calls++;
    assert.equal(url, "/v1/market-wallet/transfer-capacity");
    assert.equal(options?.cache, "no-store");
    assert.equal(options?.body, undefined, "reading capacity must not perform a financial operation");
    return Response.json(body);
  });
  // The generic browser-test fallback that originally crashed the full wallet.
  await assert.rejects(getTransferCapacity(), unavailable);
  body = { ...capacity, committedUnits: undefined };
  await assert.rejects(getTransferCapacity(), unavailable);
  body = capacity;
  assert.deepEqual(await getTransferCapacity(), capacity);
  assert.equal(calls, 3);
});

test("the transfer capacity API rejects invalid JSON and retains structured HTTP failures", async (context) => {
  context.mock.method(globalThis, "fetch", async () => new Response("not json", { status: 200 }));
  await assert.rejects(getTransferCapacity(), unavailable);
  context.mock.method(globalThis, "fetch", async () => Response.json({
    message: "Transfers paused", code: "WALLET_TRANSFER_PAUSED",
  }, { status: 503 }));
  await assert.rejects(getTransferCapacity(), (cause: unknown) =>
    cause instanceof ApiError && cause.status === 503 && cause.code === "WALLET_TRANSFER_PAUSED");
});
