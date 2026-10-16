import assert from "node:assert/strict";
import test from "node:test";

import { marketClientContext } from "./client-market-client-status";
import type { ClientMarketRental } from "./types";

const now = Date.parse("2026-10-10T00:00:00Z");
const recent = "2026-10-09T23:00:00Z";
const longAgo = "2026-09-29T18:17:36Z";

function rental(overrides: Partial<ClientMarketRental> = {}): ClientMarketRental {
  return {
    installationId: "installation-1",
    hostId: "host-1",
    providerId: "provider-1",
    hostOwnerEmail: "host@example.com",
    clientOwnerEmail: "client@example.com",
    status: "active",
    pricingModel: "free",
    offerRevision: 1,
    paymentMethodKinds: [],
    isClientOwner: true,
    canRelease: true,
    canFinalizeRelease: false,
    hostCleanupPending: false,
    providerTerminalAccessActive: false,
    canManageProviderTerminal: false,
    updatedAt: recent,
    ...overrides,
  };
}

test("market origin survives missing private rental metadata", () => {
  assert.deepEqual(marketClientContext("router_market", undefined, true, longAgo, now), {
    hint: "unknown", manageAs: null,
  });
  assert.equal(marketClientContext(undefined, undefined, true, longAgo, now), null);
});

test("permanent free market Clients explain why long offline does not release them", () => {
  assert.deepEqual(marketClientContext("router_market", rental(), true, longAgo, now), {
    hint: "long-free-no-expiry", manageAs: "client",
  });
  assert.deepEqual(marketClientContext("router_market", rental(), true, recent, now), {
    hint: "free-no-expiry", manageAs: "client",
  });
  assert.deepEqual(marketClientContext("router_market", rental({ expiresAt: "2026-10-12T00:00:00Z" }), true, longAgo, now), {
    hint: "free-timed", manageAs: "client",
  });
  assert.equal(
    marketClientContext("router_market", rental({ freeDurationDays: 7 }), true, longAgo, now)?.hint,
    "free-timed",
  );
});

test("provider management does not impersonate the Client owner", () => {
  assert.deepEqual(marketClientContext("router_market", rental({ isClientOwner: false }), true, longAgo, now), {
    hint: "long-free-no-expiry", manageAs: "provider",
  });
});

test("scheduled, releasing, failed, and ended rentals do not promise a manual release", () => {
  const scheduled = rental({
    pricingModel: "prepaid_calendar_month",
    recurring: { cancelAtPeriodEnd: true } as ClientMarketRental["recurring"],
  });
  assert.equal(marketClientContext("router_market", scheduled, true, longAgo, now)?.hint, null);
  assert.equal(marketClientContext("router_market", rental({ status: "releasing" }), true, longAgo, now)?.hint, null);
  assert.equal(marketClientContext("router_market", rental({ status: "release_failed" }), true, longAgo, now)?.hint, null);
  assert.deepEqual(marketClientContext("router_market", rental({ status: "released" }), true, longAgo, now), {
    hint: "released", manageAs: null,
  });
  assert.deepEqual(marketClientContext("router_market", rental({ status: "released", hostCleanupPending: true }), true, longAgo, now), {
    hint: "released", manageAs: "client",
  });
  assert.deepEqual(marketClientContext("router_market", rental({
    status: "released",
    walletContract: { status: "review_required", trialSecondsRemaining: 0, renewalPolicy: "manual" },
  }), true, longAgo, now), {
    hint: "released", manageAs: "client",
  });
});

test("paid long-offline reminder is display-only and invalid dates do not trigger it", () => {
  const paid = rental({ pricingModel: "legacy_metered_daily", dailyRateMinor: 100 });
  assert.equal(marketClientContext("router_market", paid, true, longAgo, now)?.hint, "long-active");
  assert.equal(marketClientContext("router_market", paid, true, "invalid", now)?.hint, "active");
  assert.equal(marketClientContext("router_market", paid, false, longAgo, now)?.hint, null);
});
