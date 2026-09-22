import assert from "node:assert/strict";
import test from "node:test";
import {
  encodeHostTransferDocument,
  parseHostTransferLines,
} from "./host-utils";

test("host transfer text preserves legacy and calendar-month pricing", () => {
  const parsed = parseHostTransferLines([
    "203.0.113.10:22|legacy|500|USD|||",
    "[2001:db8::10]:2222|monthly|3000|USD|||prepaid_calendar_month",
  ].join("\n"));

  assert.equal(parsed.errorLine, undefined);
  assert.equal(parsed.document?.version, 2);
  assert.deepEqual(parsed.document?.hosts, [
    {
      ip: "203.0.113.10",
      port: 22,
      note: "legacy",
      dailyRateMinor: 500,
      cyclePriceMinor: undefined,
      pricingModel: "legacy_metered_daily",
      billingInterval: undefined,
      currency: "USD",
      freeDurationDays: undefined,
      expectedFingerprint: undefined,
    },
    {
      ip: "2001:db8::10",
      port: 2222,
      note: "monthly",
      dailyRateMinor: undefined,
      cyclePriceMinor: 3_000,
      pricingModel: "prepaid_calendar_month",
      billingInterval: "calendar_month",
      currency: "USD",
      freeDurationDays: undefined,
      expectedFingerprint: undefined,
    },
  ]);

  const reparsed = parseHostTransferLines(encodeHostTransferDocument(parsed.document!));
  assert.deepEqual(reparsed, parsed);
});

test("host transfer text keeps version-1 zero-price rows free", () => {
  const parsed = parseHostTransferLines("203.0.113.20:22|free|0|USD|7|SHA256:legacy");

  assert.equal(parsed.errorLine, undefined);
  assert.equal(parsed.document?.version, 1);
  assert.deepEqual(parsed.document?.hosts[0], {
    ip: "203.0.113.20",
    port: 22,
    note: "free",
    dailyRateMinor: undefined,
    cyclePriceMinor: undefined,
    pricingModel: "free",
    billingInterval: undefined,
    currency: "USD",
    freeDurationDays: 7,
    expectedFingerprint: "SHA256:legacy",
  });
});

test("host transfer text rejects zero for an explicit paid pricing model", () => {
  const line = "203.0.113.30:22|invalid|0|USD|||prepaid_calendar_month";
  assert.deepEqual(parseHostTransferLines(line), { errorLine: line });
});
