import assert from "node:assert/strict";
import test from "node:test";
import { AUDIT_MONEY_FIELDS, AUDIT_STATUSES, AUDIT_ATTENTION, parseAuditPage, parseAuditPageNumber, parseAuditDetail, auditFiltersFromDates, type AuditRecord, type AuditDetail } from "./client-rental-audit";
import { rentalAuditMessagesEn, rentalAuditMessagesZh } from "./client-rental-audit-messages";
import { formatWalletUnits } from "./market-wallet-money";
const at = "2026-01-01T00:00:00Z";
function record(): AuditRecord {
  return { id: "job:original", jobId: "original", installationId: "old-client", hostId: "old-host", buyerEmail: "buyer@example.test", providerEmail: "provider@example.test", clientSubdomain: "old-name",
    hostIp: "192.0.2.10", hostPort: 22, countryCode: "SG", provenance: "order_snapshot", createdAt: at, activatedAt: at, endedAt: at, cleanupConfirmedAt: null, contractId: "contract", status: "ended", cleanupStatus: "failed", attention: "cleanup_failed", terminationId: "termination", hostPresent: false, pricingModel: "prepaid_calendar_month", cyclePriceMinor: 5, dailyRateMinor: null, offerRevision: 1, trialSecondsRemaining: 0, renewalPolicy: "manual", money: Object.fromEntries(AUDIT_MONEY_FIELDS.map(k => [k, "43200"])) as AuditRecord["money"] };
}
function detail(): AuditDetail { return { record: record(), periods: [], periodsNextCursor: null, events: [], eventsNextCursor: null, trialConsumedSeconds: 100, settlement: null, serverNow: at }; }
test("rental audit accepts exact sub-cent ledger money and nullable historical metadata", () => {
  const r = record();
  assert.equal(parseAuditPage({ items: [r], page: 1, pageSize: 5, totalPages: 1, total: 1, serverNow: at }).items[0].money?.chargedUnits, "43200");
  assert.equal(formatWalletUnits(r.money!.chargedUnits), "$0.005");
  r.hostIp = null; r.provenance = "historical_record"; r.money = null;
  assert.equal(parseAuditDetail({ ...detail(), record: r }, r.id).record.money, null);
});
test("rental audit rejects malformed money, identity, capabilities and pagination before rendering", () => {
  const page = { items: [record()], page: 1, pageSize: 5, totalPages: 1, total: 1, serverNow: at };
  for (const bad of ["1e8", "-1", "1.2", "9223372036854775808", 43200, null]) {
    assert.throws(() => parseAuditPage({ ...page, items: [{ ...record(), money: { ...record().money, chargedUnits: bad } }] }));
  }
  for (const patch of [{ page: 0 }, { page: 1.5 }, { page: 2 }, { page: 4_294_967_296 }, { pageSize: 50 }, { totalPages: 0 }, { totalPages: 2 }, { total: -1 }, { total: 5 }, { total: 1.5 }, { total: Number.MAX_SAFE_INTEGER + 1 }, { items: [] }, { serverNow: "broken" }, { items: [record(), record()] }, { items: [{ ...record(), cleanupStatus: "complete-ish" }] }]) assert.throws(() => parseAuditPage({ ...page, ...patch }));
  assert.throws(() => parseAuditDetail(detail(), "different-record"));
  assert.throws(() => parseAuditDetail({ ...detail(), periodsNextCursor: 50 }, record().id));
  assert.throws(() => parseAuditDetail({ ...detail(), eventsNextCursor: "missing" }, record().id));
});
test("numbered audit pages validate direct jumps, five-row pages and partial/empty last pages", () => {
  const five = Array.from({ length: 5 }, (_, n) => ({ ...record(), id: `job:${n}` }));
  assert.equal(parseAuditPage({ items: five, total: 23, page: 3, pageSize: 5, totalPages: 5, serverNow: at }).page, 3);
  assert.equal(parseAuditPage({ items: five.slice(0, 3), total: 23, page: 5, pageSize: 5, totalPages: 5, serverNow: at }).items.length, 3);
  assert.equal(parseAuditPage({ items: [], total: 0, page: 1, pageSize: 5, totalPages: 1, serverNow: at }).total, 0);
  assert.throws(() => parseAuditPage({ items: [...five, { ...record(), id: "sixth" }], total: 6, page: 1, pageSize: 5, totalPages: 2, serverNow: at }));
  assert.throws(() => parseAuditPage({ items: five, total: 23, page: 5, pageSize: 5, totalPages: 5, serverNow: at }));
  assert.throws(() => parseAuditPage({ items: [record()], nextCursor: null, total: 1, serverNow: at }));
});
test("audit page input submits only bounded whole page numbers", () => {
  for (const [input, expected] of [["1", 1], [" 3 ", 3], ["25", 25]] as const) assert.equal(parseAuditPageNumber(input, 25), expected);
  for (const input of ["", " ", "0", "-1", "1.5", "1e1", "+1", "01", "26", "NaN", "Infinity", "4294967296", "9007199254740992", "1;DROP"]) assert.equal(parseAuditPageNumber(input, 25), null);
  assert.equal(parseAuditPageNumber("1", 1), 1);
  assert.equal(parseAuditPageNumber("4294967295", 4_294_967_295), 4_294_967_295);
  assert.equal(parseAuditPageNumber("1", 0), null);
  assert.equal(parseAuditPageNumber("1", NaN), null);
});
test("date filters include the selected final UTC day and do not leak form-only fields", () => {
  assert.deepEqual(auditFiltersFromDates({ buyerEmail: "buyer@example.test", startDate: "2026-10-01", endDate: "2026-10-02" }), { buyerEmail: "buyer@example.test", from: "2026-10-01T00:00:00.000Z", until: "2026-10-03T00:00:00.000Z" });
  assert.throws(() => auditFiltersFromDates({ startDate: "2026-10-03", endDate: "2026-10-01" }));
  assert.throws(() => auditFiltersFromDates({ startDate: "not-a-date" }));
  for (const day of ["2026-02-30", "2026-13-01", "26-10-01", "2026-10-01T12:00:00Z"]) assert.throws(() => auditFiltersFromDates({ startDate: day }));
});
test("all rental states, follow-up states and audit money have matching translations", () => {
  assert.deepEqual(Object.keys(rentalAuditMessagesEn).sort(), Object.keys(rentalAuditMessagesZh).sort());
  for (const key of [...AUDIT_STATUSES.map(v => `rentalAudit.status.${v}`), ...AUDIT_ATTENTION.map(v => `rentalAudit.attention.${v}`), ...AUDIT_MONEY_FIELDS.map(v => `rentalAudit.${v}`)]) {
    assert.ok(rentalAuditMessagesEn[key as keyof typeof rentalAuditMessagesEn]); assert.ok(rentalAuditMessagesZh[key as keyof typeof rentalAuditMessagesZh]);
  }
});
