import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { clientRentalLabel, parseClientRentalPage, parseClientRentalDetail, type ClientRentalRecord, type ClientRentalDetail } from "./client-rentals";
import { clientRentalsMessagesEn, clientRentalsMessagesZh } from "./client-rentals-messages";
import { formatWalletUnits } from "./market-wallet-money";

const at = "2026-01-01T00:00:00Z";
function record(): ClientRentalRecord {
  return { id: "job:original", installationId: "old-client", providerEmail: "provider@example.test", clientSubdomain: "original-name", countryCode: null,
    provenance: "historical_record", createdAt: at, activatedAt: at, endedAt: at, cleanupConfirmedAt: null,
    contractId: "contract", status: "ended", cleanupStatus: "pending", attention: "cleanup_pending", terminationId: "termination",
    pricingModel: "prepaid_calendar_month", cyclePriceMinor: 5, dailyRateMinor: null, offerRevision: 1, trialSecondsRemaining: 0,
    renewalPolicy: "manual", group: "history", periodEnd: at, expiresAt: null, freeDurationDays: null, contractStatus: "ending",
    money: { chargedUnits: "432000", returnedUnits: "345600", serviceUnits: "43200", compensationUnits: "43200", unfulfilledUnits: "0", netUnits: "86400" } };
}
const page = () => ({ items: [record()], view: "history", total: 1, page: 1, totalPages: 1, pageSize: 5, focusedId: null, serverNow: at });
const detail = (): ClientRentalDetail => ({ record: record(), rental: null, periods: [], periodsNextCursor: null, events: [], eventsNextCursor: null, trialConsumedSeconds: 30, serverNow: at, settlement: null });
const source = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("personal history keeps exact wallet amounts and stopped-but-unsettled services", () => {
  const value = parseClientRentalPage(page(), "history");
  assert.equal(value.items[0].attention, "cleanup_pending");
  assert.equal(formatWalletUnits(value.items[0].money!.netUnits), "$0.01");
  assert.equal(formatWalletUnits(value.items[0].money!.compensationUnits), "$0.005");
  assert.equal(parseClientRentalDetail(detail(), record().id).rental, null);
  assert.equal(clientRentalLabel({ ...record(), clientSubdomain: null }), "old-client");
  assert.equal(parseClientRentalPage({ ...page(), items: [{ ...record(), money: null }] }, "history").items[0].money, null);
});
test("personal pagination rejects wrong actors' shapes, duplicate records and guessed groups", () => {
  for (const patch of [{ view: "current" }, { page: 0 }, { page: 2 }, { pageSize: 50 }, { total: 2 }, { totalPages: 2 }, { focusedId: "not-on-page" }, { items: [record(), record()], total: 2 }, { items: [{ ...record(), group: "current" }] }, { items: [{ ...record(), status: "paid" }] }]) {
    assert.throws(() => parseClientRentalPage({ ...page(), ...patch }, "history"));
  }
  const five = Array.from({ length: 5 }, (_, n) => ({ ...record(), id: `rental-${n}` }));
  assert.equal(parseClientRentalPage({ ...page(), items: five, total: 23, page: 3, totalPages: 5 }, "history").page, 3);
  assert.equal(parseClientRentalPage({ ...page(), items: five.slice(0, 3), total: 23, page: 5, totalPages: 5 }, "history").items.length, 3);
  assert.equal(parseClientRentalPage({ ...page(), items: [], total: 0 }, "history").total, 0);
});
test("personal money is integer evidence, never recomputed prices or duplicated compensation", () => {
  for (const amount of ["1e3", "-1", "1.5", "01", "9223372036854775808", 432000, null]) {
    assert.throws(() => parseClientRentalPage({ ...page(), items: [{ ...record(), money: { ...record().money, chargedUnits: amount } }] }, "history"));
  }
  assert.throws(() => parseClientRentalPage({ ...page(), items: [{ ...record(), money: { ...record().money, netUnits: "129600" } }] }, "history"));
});
test("history cannot acquire live actions and detail pagination stays inside the record", () => {
  assert.throws(() => parseClientRentalDetail({ ...detail(), rental: { installationId: "old-client", isClientOwner: true } }, record().id));
  assert.throws(() => parseClientRentalDetail(detail(), "someone-else"));
  assert.throws(() => parseClientRentalDetail({ ...detail(), periodsNextCursor: 1 }, record().id));
  assert.throws(() => parseClientRentalDetail({ ...detail(), eventsNextCursor: "foreign" }, record().id));
  const current = { ...record(), group: "current", status: "trialing", endedAt: null };
  assert.throws(() => parseClientRentalDetail({ ...detail(), record: current, rental: { installationId: "another-client", isClientOwner: true } }, record().id));
});
test("current rental actions require a valid matching live lifecycle", () => {
  const current = { ...record(), group: "current", status: "trialing", endedAt: null };
  const rental = {
    installationId: current.installationId, hostId: "host", providerId: "provider", hostOwnerEmail: "provider@example.test",
    clientOwnerEmail: "buyer@example.test", status: "active", pricingModel: "prepaid_calendar_month", offerRevision: 1, updatedAt: at,
    isClientOwner: true, canRelease: true, canFinalizeRelease: false, hostCleanupPending: false,
    providerTerminalAccessActive: false, canManageProviderTerminal: true,
    walletContract: { id: current.contractId, status: "trialing", trialSecondsRemaining: 30, renewalPolicy: "manual", periodEnd: null },
  };
  assert.ok(parseClientRentalDetail({ ...detail(), record: current, rental }, record().id).rental);
  for (const patch of [{ isClientOwner: false }, { updatedAt: "bad-date" }, { cyclePriceMinor: -1 }, { walletContract: {} },
    { walletContractId: "foreign-contract" }, { walletContract: { ...rental.walletContract, id: "foreign-contract" } },
    { walletContract: { ...rental.walletContract, trialSecondsRemaining: -1 } }, { canRelease: "true" }]) {
    assert.throws(() => parseClientRentalDetail({ ...detail(), record: current, rental: { ...rental, ...patch } }, record().id));
  }
});
test("account and market reuse one buyer view; Owner audit is gated before any read", () => {
  assert.match(source("components/dashboard/account-client-section.tsx"), /role === "user"[\s\S]*<ClientRentalRecords/);
  const market = source("components/dashboard/client-market-page.tsx");
  assert.ok(market.indexOf("<ClientRentalRecords historyOnly") < market.indexOf("<ClientRentalAuditPanel"));
  const shared = source("components/dashboard/client-market/rental-records.tsx");
  assert.match(shared, /key=\{actor\}/);
  assert.match(shared, /view="current"/); assert.match(shared, /view="history"/);
  assert.doesNotMatch(shared, /getClientMarketHosts|localStorage|sessionStorage/);
  assert.match(shared, /document.visibilityState === "visible"/);
  assert.match(shared, /!expanded/);
  assert.match(market, /onChanged=\{onRentalChanged\}/);
  assert.match(market, /revision=\{terminationWork.revision \+ rentalRevision\}/);
  const audit = source("components/dashboard/client-market/rental-audit-panel.tsx");
  assert.match(audit, /session\.canAuditClientMarket === true/);
  assert.match(audit, /\[expanded, setExpanded\] = React\.useState\(false\)/);
  assert.match(audit, /if \(!active\) \{[\s\S]*?return;[\s\S]*?getRentalAudit\(/);
  assert.match(audit, /request.current\?\.abort\(\); setRead\(null\); setSelected\(null\)/);
  const row = source("components/dashboard/client-market/host-row.tsx");
  assert.match(row, /if \(!historyOpen \|\| !canReadHostHistory\) return/);
});
test("personal records have complete bilingual copy", () => {
  assert.deepEqual(Object.keys(clientRentalsMessagesEn).sort(), Object.keys(clientRentalsMessagesZh).sort());
  for (const text of [...Object.values(clientRentalsMessagesEn), ...Object.values(clientRentalsMessagesZh)]) assert.ok(text.trim());
});
