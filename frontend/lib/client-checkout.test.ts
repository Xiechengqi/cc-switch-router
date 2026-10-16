import assert from "node:assert/strict";
import test from "node:test";
import {
  clientCheckoutState,
  preserveClientDrafts,
  trialHours,
  walletLifecycleKey,
} from "./client-checkout";
import type { ClientMarketAllocationQuote } from "./types";

const quote: ClientMarketAllocationQuote = {
  id: "quote",
  status: "active",
  expiresAt: "2030-01-01T00:00:00Z",
  fundingRail: "platform_wallet",
  funding: [],
  recurringFunding: [],
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
    },
  ],
  walletFunding: {
    assetCode: "USD",
    availableUnits: "0",
    heldUnits: "0",
    committedUnits: "0",
    requiredUnits: "0",
    shortfallUnits: "0",
  },
};

test("trial uses actual remaining allowance, while exhausted trials require wallet funds", () => {
  assert.deepEqual(clientCheckoutState(quote), {
    paid: true,
    trial: true,
    unsupported: false,
    shortfall: false,
  });
  const exhausted = {
    ...quote,
    items: quote.items.map((item) => ({ ...item, trialSecondsRemaining: 0 })),
    walletFunding: {
      ...quote.walletFunding!,
      requiredUnits: "86400000",
      shortfallUnits: "1",
    },
  };
  assert.deepEqual(clientCheckoutState(exhausted), {
    paid: true,
    trial: false,
    unsupported: false,
    shortfall: true,
  });
  assert.equal(trialHours(1, "en"), "0.01");
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
