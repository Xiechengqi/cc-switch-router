import assert from "node:assert/strict";
import test from "node:test";

import { clientMarketCleanupReasonKey, clientMarketRentalHasBanner } from "./client-market-rental-state";
import type { ClientMarketRental } from "./types";

const ended: ClientMarketRental = {
  installationId: "installation-1",
  hostId: "host-1",
  providerId: "provider-1",
  hostOwnerEmail: "provider@example.com",
  clientOwnerEmail: "client@example.com",
  status: "released",
  pricingModel: "free",
  offerRevision: 1,
  paymentMethodKinds: [],
  isClientOwner: true,
  canRelease: false,
  canFinalizeRelease: false,
  hostCleanupPending: true,
  cleanupFailureKind: "cleanup_ssh_unreachable",
  nextCleanupProbeAt: "2026-10-13T02:17:18Z",
  cleanupProbeAttempts: 1,
  providerTerminalAccessActive: false,
  canManageProviderTerminal: false,
  updatedAt: "2026-10-13T02:02:18Z",
};

test("ended rentals remain visible only while their Host is quarantined", () => {
  assert.equal(clientMarketRentalHasBanner(ended), true);
  assert.equal(clientMarketRentalHasBanner({ ...ended, hostCleanupPending: false }), false);
  assert.equal(clientMarketRentalHasBanner({ ...ended, isClientOwner: false }), false);
});

test("quarantined Host reason stays a safe, compact category", () => {
  assert.equal(clientMarketCleanupReasonKey("cleanup_ssh_unreachable"), "clientMarket.release.reason.unreachable");
  assert.equal(clientMarketCleanupReasonKey("cleanup_fingerprint_mismatch"), "clientMarket.release.reason.safety");
  assert.equal(clientMarketCleanupReasonKey("cleanup_purge_failed"), "clientMarket.release.reason.purge");
});


test("ended rentals with unsettled wallet funds remain visible after Host cleanup", () => {
  const rental = { ...ended, hostCleanupPending: false, walletContract: { status: "review_required", trialSecondsRemaining: 0, renewalPolicy: "manual" } };
  assert.equal(clientMarketRentalHasBanner(rental), true);
  assert.equal(clientMarketRentalHasBanner({ ...rental, walletContract: { ...rental.walletContract, status: "ended" } }), false);
});
