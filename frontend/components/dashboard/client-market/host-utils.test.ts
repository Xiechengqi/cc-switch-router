import assert from "node:assert/strict";
import test from "node:test";
import {
  encodeHostTransferDocument,
  hostMatchesMarketProvider,
  parseHostTransferLines,
  persistedClientMarketProviderSelection,
  recommendedClientMarketProviderId,
} from "./host-utils";
import type { ClientMarketProvider } from "@/lib/types";

function provider(providerId: string, freeIdle: number): ClientMarketProvider {
  return {
    providerId,
    rankState: "ranked",
    ownerEmail: `${providerId}@example.com`,
    official: providerId === "official",
    joinedAt: "2026-01-01T00:00:00Z",
    offerStableSince: "2026-01-01T00:00:00Z",
    hostTotal: freeIdle,
    idleTotal: freeIdle,
    allocatedTotal: 0,
    allocationRate: 0,
    freeHostTotal: freeIdle,
    freeAllocatedTotal: 0,
    paidHostTotal: 0,
    paidAllocatedTotal: 0,
    externalClientOwnerTotal: 0,
    externalClientsOver3Days: 0,
    externalClientsOver30Days: 0,
    anomalousHostRate: 0,
    successfulAllocations: 0,
    paymentMethodKinds: [],
    countries: [],
  };
}

test("Client creation applies Provider rank only in on mode", () => {
  const ranked = provider("ranked", 1);
  const official = provider("official", 1);
  const ordered = [ranked, official];
  assert.equal(recommendedClientMarketProviderId(ordered, "official", "off"), "official");
  assert.equal(recommendedClientMarketProviderId(ordered, "official", "shadow"), "official");
  assert.equal(recommendedClientMarketProviderId(ordered, "official", "on"), "ranked");

  assert.equal(
    recommendedClientMarketProviderId([provider("full", 0), ranked], "official", "on"),
    "ranked",
  );
  assert.equal(recommendedClientMarketProviderId([provider("full", 0)], undefined, "on"), undefined);

  assert.deepEqual(
    persistedClientMarketProviderSelection(["ranked"], ordered),
    { mode: "custom", providerIds: ["ranked"] },
  );
  assert.deepEqual(
    persistedClientMarketProviderSelection(["official"], ordered),
    { mode: "custom", providerIds: ["official"] },
  );
});

test("Client Market Provider deep-link filter fails closed for unbound hosts", () => {
  assert.equal(hostMatchesMarketProvider({ marketProviderId: "mp_selected" }, "mp_selected"), true);
  assert.equal(hostMatchesMarketProvider({ marketProviderId: "mp_other" }, "mp_selected"), false);
  assert.equal(hostMatchesMarketProvider({}, "mp_selected"), false);
  assert.equal(hostMatchesMarketProvider({}, undefined), true);
});

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
