import assert from "node:assert/strict";
import test from "node:test";
import { mergeHosts, mergeRentalMap } from "./client-market-refresh";
import type { ClientMarketHost, ClientMarketRental, WalletLifecycle } from "./types";

const host: ClientMarketHost = {
  id: "host-1",
  providerId: "provider-1",
  marketProviderId: "rank-1",
  hostOwnerEmail: "provider@example.com",
  pricingModel: "prepaid_calendar_month",
  cyclePriceMinor: 500,
  billingInterval: "calendar_month",
  currency: "USD",
  trialHours: null,
  defaultTrialHours: 12,
  offerRevision: 1,
  eligibility: { allowed: true, status: "allowed" },
  status: "idle",
  clientConnection: { state: "offline" },
  ipIntel: { query: "192.0.2.1", countryCode: "SG", source: "test" },
  updatedAt: "2026-10-29T04:00:00Z",
};

const lifecycle: WalletLifecycle = {
  id: "contract-1",
  status: "trial",
  trialSecondsRemaining: 3600,
  renewalPolicy: "auto",
  periodEnd: null,
  nextRentalTrial: {
    epoch: 1,
    allowanceSeconds: 43_200,
    consumedSeconds: 3600,
    remainingSeconds: 39_600,
    lastResetAt: null,
  },
};

const rental: ClientMarketRental = {
  installationId: "installation-1",
  hostId: host.id,
  providerId: "provider-1",
  hostOwnerEmail: "provider@example.com",
  clientOwnerEmail: "buyer@example.com",
  status: "active",
  pricingModel: "prepaid_calendar_month",
  cyclePriceMinor: 500,
  billingInterval: "calendar_month",
  walletContractId: "contract-1",
  walletContract: lifecycle,
  openWalletDispute: false,
  currency: "USD",
  offerRevision: 1,
  activatedAt: "2026-10-29T04:00:00Z",
  expiresAt: "2026-11-29T04:00:00Z",
  isClientOwner: true,
  canRelease: true,
  canFinalizeRelease: false,
  hostCleanupPending: false,
  providerTerminalAccessActive: false,
  canManageProviderTerminal: false,
  updatedAt: "2026-10-29T04:00:00Z",
};

test("equivalent Client polls preserve collection and record identity", () => {
  const hosts = [host];
  const rentals = new Map([[rental.installationId, rental]]);
  assert.equal(mergeHosts(hosts, [structuredClone(host)]), hosts);
  assert.equal(mergeRentalMap(rentals, [structuredClone(rental)]), rentals);
  assert.equal(mergeHosts(hosts, [{ ...host, contacts: [] }]), hosts);
  assert.equal(mergeRentalMap(rentals, [{ ...rental, contacts: [] }]), rentals);
});

test("Host prices, trial policy, Provider identity and metadata refresh without updatedAt changing", () => {
  const updates: Partial<ClientMarketHost> = {
    marketProviderId: "rank-2",
    pricingModel: "free",
    cyclePriceMinor: 600,
    billingInterval: "other",
    freeDurationDays: 7,
    trialHours: 0,
    defaultTrialHours: 24,
    hostname: "new-host",
    canWebTerminal: true,
    eligibility: { allowed: false, status: "access_required" },
    clientConnection: { state: "online", lastHeartbeatAt: "2026-10-29T05:00:00Z" },
    ipIntel: { ...host.ipIntel!, city: "Singapore" },
    contacts: [{ channel: "telegram", handle: "provider" }],
  };
  for (const [field, value] of Object.entries(updates)) {
    const changed = { ...host, [field]: value };
    const merged = mergeHosts([host], [changed]);
    assert.equal(merged[0], changed, `${field} must not remain stale`);
    assert.equal(merged[0].updatedAt, host.updatedAt);
  }
});

test("rental money, dates, disputes and terminal permissions refresh independently of updatedAt", () => {
  const updates: Partial<ClientMarketRental> = {
    pricingModel: "free",
    cyclePriceMinor: 600,
    billingInterval: "other",
    freeDurationDays: 7,
    walletContractId: "contract-2",
    openWalletDispute: true,
    activatedAt: "2026-10-29T05:00:00Z",
    expiresAt: "2026-12-29T04:00:00Z",
    providerTerminalAuthorizedUntil: "2026-10-30T04:00:00Z",
    providerTerminalAccessActive: true,
    canManageProviderTerminal: true,
    contacts: [{ channel: "custom", handle: "provider@example.com" }],
  };
  const previous = new Map([[rental.installationId, rental]]);
  for (const [field, value] of Object.entries(updates)) {
    const changed = { ...rental, [field]: value };
    const merged = mergeRentalMap(previous, [changed]);
    assert.notEqual(merged, previous, field);
    assert.equal(merged.get(rental.installationId), changed, field);
    assert.equal(changed.updatedAt, rental.updatedAt);
  }
});

test("wallet lifecycle and next-rental trial changes never reuse stale Client records", () => {
  const updates: Partial<WalletLifecycle> = {
    status: "active",
    trialSecondsRemaining: 3590,
    renewalPolicy: "manual",
    periodEnd: "2026-11-29T04:00:00Z",
    nextRentalTrial: { ...lifecycle.nextRentalTrial!, epoch: 2, remainingSeconds: 43_200 },
  };
  const previous = new Map([[rental.installationId, rental]]);
  for (const [field, value] of Object.entries(updates)) {
    const changed = { ...rental, walletContract: { ...lifecycle, [field]: value } };
    assert.equal(mergeRentalMap(previous, [changed]).get(rental.installationId), changed, field);
  }
  const cleared = { ...rental, walletContract: undefined };
  assert.equal(mergeRentalMap(previous, [cleared]).get(rental.installationId), cleared);
});

test("Host reordering and record removal preserve the authoritative response semantics", () => {
  const secondHost = { ...host, id: "host-2" };
  const previous = [host, secondHost];
  const reordered = mergeHosts(previous, [structuredClone(secondHost), structuredClone(host)]);
  assert.notEqual(reordered, previous);
  assert.equal(reordered[0], secondHost);
  assert.equal(reordered[1], host);
  assert.deepEqual(mergeHosts(previous, []), []);
  const secondRental = { ...rental, installationId: "installation-2" };
  const rentals = new Map([[rental.installationId, rental], [secondRental.installationId, secondRental]]);
  const remaining = mergeRentalMap(rentals, [structuredClone(secondRental)]);
  assert.deepEqual([...remaining.keys()], [secondRental.installationId]);
  assert.equal(remaining.get(secondRental.installationId), secondRental);
  assert.equal(mergeRentalMap(rentals, []).size, 0);
});
