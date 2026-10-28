import assert from "node:assert/strict";
import test from "node:test";
import {
  clientCheckoutState,
  preserveClientDrafts,
  trialHours,
  trialDuration,
  walletLifecycleKey,
  walletReleaseScheduled,
} from "./client-checkout";
import type { ClientMarketAllocationQuote } from "./types";

const quote: ClientMarketAllocationQuote = {
  id: "quote",
  status: "active",
  expiresAt: "2030-01-01T00:00:00Z",
  fundingRail: "platform_wallet",

  items: [
    {
      id: "item",
      hostId: "host",
      providerId: "provider",
      hostOwnerEmail: "provider@example.com",
      pricingModel: "prepaid_calendar_month",
      cyclePriceMinor: 1000,
      offerRevision: 2,
      trialSecondsRemaining: 43200,
      trialAllowanceSeconds: 43200,
      trialSecondsConsumed: 0,
    },
  ],
  walletFunding: {
    assetCode: "USD",
    availableUnits: "86400000",
    heldUnits: "0",
    committedUnits: "0",
    requiredUnits: "86400000",
    shortfallUnits: "0",
  },
};

test("trial defers payment but still requires the first-period balance", () => {
  assert.deepEqual(clientCheckoutState(quote), {
    paid: true,
    trial: true,
    unsupported: false,
    shortfall: false,
  });
  const exhausted = {
    ...quote,
    items: quote.items.map((item) => ({ ...item, trialSecondsRemaining: 0, trialSecondsConsumed: 43200 })),
    walletFunding: {
      ...quote.walletFunding!,
      availableUnits: "0",
      requiredUnits: "86400000",
      shortfallUnits: "86400000",
    },
  };
  assert.deepEqual(clientCheckoutState(exhausted), {
    paid: true,
    trial: false,
    unsupported: false,
    shortfall: true,
  });
  assert.equal(trialHours(1, "en"), "0.01");
  assert.equal(trialDuration(1, "en"), "1 sec");
  assert.equal(trialDuration(0, "zh-CN"), "0秒");
  assert.equal(trialDuration(3661, "zh-CN"), "1小时 1分钟 1秒");
  assert.equal(trialDuration(-1, "en"), "—");
  assert.equal(clientCheckoutState({ ...quote, walletFunding: { ...quote.walletFunding!, availableUnits: "0", shortfallUnits: "86400000" } }).shortfall, true);
  assert.equal(clientCheckoutState({ ...quote, walletFunding: { ...quote.walletFunding!, requiredUnits: "0" } }).unsupported, true);
});

test("missing wallet or trial contract cannot fall back to provider payment", () => {
  assert.equal(
    clientCheckoutState({ ...quote, walletFunding: undefined }).unsupported,
    true,
  );
  assert.equal(
    clientCheckoutState({ ...quote, fundingRail: "legacy_provider" })
      .unsupported,
    true,
  );
  assert.equal(
    clientCheckoutState({
      ...quote,
      items: quote.items.map((item) => ({
        ...item,
        trialSecondsRemaining: NaN,
      })),
    }).unsupported,
    true,
  );
  assert.equal(
    clientCheckoutState({
      ...quote,
      walletFunding: undefined,
      fundingRail: "legacy_provider",
      items: quote.items.map((item) => ({
        ...item,
        cyclePriceMinor: undefined,
        pricingModel: "free" as const,
      })),
    }).unsupported,
    false,
  );
});

test("requote preserves entries by Host, never moves a password to a different Host", () => {
  const draft = {
    subdomain: "chosen-client",
    password: "in-memory-only",
    autoRenew: true,
  };
  const preserved = preserveClientDrafts(quote, { item: draft });
  assert.deepEqual(preserved.host, draft);
  assert.equal(preserved["different-host"], undefined);
});

test("trial scope accounting fails closed rather than promising an unknown free window", () => {
  for (const patch of [
    { trialAllowanceSeconds: undefined }, { trialSecondsConsumed: undefined },
    { trialAllowanceSeconds: -1 }, { trialAllowanceSeconds: 2592001 },
    { trialSecondsConsumed: -1 }, { trialSecondsRemaining: 0 },
    { trialSecondsRemaining: 0.5 }, { trialAllowanceSeconds: Infinity },
  ]) {
    const item = { ...quote.items[0], ...patch } as typeof quote.items[number];
    assert.equal(clientCheckoutState({ ...quote, items: [item] }).unsupported, true);
  }
  for (const consumed of [0, 1, 7200]) {
    const item = { ...quote.items[0], trialAllowanceSeconds: 0, trialSecondsConsumed: consumed, trialSecondsRemaining: 0 };
    assert.equal(clientCheckoutState({ ...quote, items: [item] }).unsupported, false);
  }
});

test("malformed Client funding cannot throw or enable checkout", () => {
  for (const value of ["-1", "NaN", "1.2", "9223372036854775808"]) {
    const result = clientCheckoutState({ ...quote, walletFunding: { ...quote.walletFunding!, committedUnits: value } });
    assert.equal(result.unsupported, true);
    assert.equal(result.shortfall, false);
  }
  const contract = { status: "active", renewalPolicy: "manual", trialSecondsRemaining: 0 };
  assert.equal(walletReleaseScheduled(contract, "prepaid_calendar_month"), true);
  assert.equal(walletReleaseScheduled({ ...contract, status: "trialing" }, "prepaid_calendar_month"), false);
  assert.equal(walletReleaseScheduled({ ...contract, status: "suspended" }, "prepaid_calendar_month"), false);
  assert.equal(walletReleaseScheduled(contract, "legacy_metered_daily"), false);
});

test("wallet lifecycle distinguishes trial, renewal, expiry and uncertain cleanup", () => {
  const contract = {
    status: "active",
    renewalPolicy: "manual",
    periodEnd: "2030-01-01T00:00:00Z",
    trialSecondsRemaining: 0,
  };
  assert.equal(
    walletLifecycleKey(contract),
    "clientCheckout.lifecycle.expires",
  );
  assert.equal(
    walletLifecycleKey({ ...contract, renewalPolicy: "automatic" }),
    "clientCheckout.lifecycle.renews",
  );
  assert.equal(
    walletLifecycleKey({ ...contract, status: "trialing" }),
    "clientCheckout.lifecycle.trial",
  );
  assert.equal(
    walletLifecycleKey({ ...contract, status: "review_required" }),
    "clientCheckout.lifecycle.review",
  );
  assert.equal(
    walletLifecycleKey({ ...contract, status: "unrecognized" }),
    "clientCheckout.lifecycle.unknown",
  );
});
