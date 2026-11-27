import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { AUDIT_MONEY_FIELDS } from "./client-rental-audit";
import { rentalAuditMessagesEn, rentalAuditMessagesZh } from "./client-rental-audit-messages";
import { SHARE_AUDIT_ATTENTION, SHARE_AUDIT_STATUSES, parseShareAuditDetail, parseShareAuditPage, type ShareAuditDetail, type ShareAuditRecord } from "./share-rental-audit";

const at = "2026-01-01T00:00:00Z";
function record(): ShareAuditRecord {
  return { id: "subscription", shareId: "share", shareName: "Claude Max", shareSubdomain: "claude-max", appType: "claude", seatId: "seat", seatLabel: "1", seatPosition: 1,
    installationId: "client", buyerEmail: "buyer@example.test", providerEmail: "provider@example.test", provenance: "order_snapshot",
    createdAt: at, activatedAt: at, endedAt: null, contractId: "contract", status: "paid", attention: "none", terminationId: null, sharePresent: true,
    pricingModel: "daily", dailyRateMinor: 3, offerRevision: 2, renewalPolicy: "manual", expiresAt: at, releaseReason: null,
    money: Object.fromEntries(AUDIT_MONEY_FIELDS.map(k => [k, "86400"])) as ShareAuditRecord["money"] };
}
const page = () => ({ items: [record()], page: 1, pageSize: 5, totalPages: 1, total: 1, serverNow: at });
const detail = (): ShareAuditDetail => ({ record: record(), periods: [], periodsNextCursor: null, events: [], eventsNextCursor: null, trial: null, settlement: null, serverNow: at });
const source = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("Share audit keeps a deleted Share's frozen identity and exact ledger money", () => {
  const removed = { ...record(), sharePresent: false, provenance: "historical_record" as const, shareName: null, shareSubdomain: null, money: null };
  assert.equal(parseShareAuditPage({ ...page(), items: [removed] }).items[0].sharePresent, false);
  assert.equal(parseShareAuditPage(page()).items[0].money?.chargedUnits, "86400");
  const trial = { phase: "trial", tokenAllowance: "1000", tokenBudget: "250", valueAllowanceUsdMicros: null, valueBudgetUsdMicros: null };
  const settlement = { status: "disputed", heldUnits: "10", returnedUnits: "20", compensationUnits: "0", responsibility: null, cutoffAt: at };
  assert.equal(parseShareAuditDetail({ ...detail(), trial, settlement }, "subscription").trial?.tokenBudget, "250");
});

test("Share audit rejects guessed statuses, malformed money and foreign detail before rendering", () => {
  for (const patch of [{ status: "active" }, { attention: "cleanup_failed" }, { sharePresent: "yes" }, { shareId: "" }, { money: { ...record().money, chargedUnits: "1.5" } }]) {
    assert.throws(() => parseShareAuditPage({ ...page(), items: [{ ...record(), ...patch }] }));
  }
  for (const patch of [{ pageSize: 50 }, { total: 2 }, { page: 2 }]) assert.throws(() => parseShareAuditPage({ ...page(), ...patch }));
  assert.throws(() => parseShareAuditDetail(detail(), "another-subscription"));
  assert.throws(() => parseShareAuditDetail({ ...detail(), trial: { phase: "trial", tokenAllowance: -1, tokenBudget: null, valueAllowanceUsdMicros: null, valueBudgetUsdMicros: null } }, "subscription"));
  assert.throws(() => parseShareAuditDetail({ ...detail(), settlement: { status: "", heldUnits: "0", returnedUnits: "0", compensationUnits: "0", responsibility: null, cutoffAt: at } }, "subscription"));
});

test("every Share status and follow-up has bilingual copy", () => {
  for (const key of [...SHARE_AUDIT_STATUSES.map(v => `shareRentalAudit.status.${v}`), ...SHARE_AUDIT_ATTENTION.map(v => `shareRentalAudit.attention.${v}`)]) {
    assert.ok(key in rentalAuditMessagesEn, key);
    assert.ok((rentalAuditMessagesZh as Record<string, string>)[key]?.trim(), key);
  }
});

test("every event name the server may send has a label", () => {
  const detail = source("../src/share_market_audit/detail.rs");
  const body = detail.slice(detail.indexOf("fn safe_action"), detail.indexOf("_ => \"updated\""));
  // `"a" | "b" => "c"` sends c; a bare alternative or `=> value` sends itself.
  const names = new Set<string>();
  for (const line of body.split("\n").slice(1)) {
    const quoted = [...line.matchAll(/"([a-z_]+)"/g)].map(m => m[1]);
    if (/=> "/.test(line)) names.add(quoted.at(-1)!);
    else quoted.forEach(name => names.add(name));
  }
  names.add("updated");
  assert.ok(names.size > 10);
  for (const name of names) {
    assert.ok(`shareRentalAudit.event.${name}` in rentalAuditMessagesEn || `rentalAudit.event.${name}` in rentalAuditMessagesEn, name);
  }
});

test("only the Router owner gets the Share audit tab, and it pauses market polling", () => {
  const workspace = source("components/dashboard/share-market/share-market-workspace.tsx");
  assert.match(workspace, /showAuditTab = authed && session\?\.canAuditClientMarket === true/);
  assert.match(workspace, /showAuditTab \? \[\{ id: "audit" as const/);
  assert.match(workspace, /if \(authLoading \|\| workspace === "audit"\) return;/);
  assert.match(workspace, /showAuditTab && workspaceValue === "audit" \? \(\s*<ShareRentalAuditPanel/);
  assert.match(source("components/dashboard/share-market/rental-audit-panel.tsx"), /<ShareAudit key=\{actor\}/);
});
