import test from "node:test";
import assert from "node:assert/strict";
import { parseShareTermination, parseShareTerminationPreview, shareTerminationHref, shareTerminationNoteValid, shareTerminationUnits, type ShareTermination } from "./share-termination";
import { shareTerminationMessagesEn, shareTerminationMessagesZh } from "./share-termination-messages";

const at = "2026-11-02T00:00:00Z";
const record: ShareTermination = {
  id: "termination", subscriptionId: "rental", shareId: "share", seatId: "seat", installationId: "installation", shareName: "Share",
  buyerUserId: "buyer", providerUserId: "provider", buyerEmail: "buyer@example.test", providerEmail: "provider@example.test",
  periodAmountUnits: "432000", timeEarnedUnits: "0", quotaEarnedUnits: "0", remainingUnits: "432000",
  compensationCapUnits: "43200", compensationUnits: "0", returnedUnits: "388800", heldUnits: "43200",
  rateBps: 1000, policyVersion: 1, version: 2, reason: "provider_issue", claim: "", responsibility: "undetermined", status: "disputed",
  cutoffAt: at, revokedAt: at, updatedAt: at, responseDeadlineAt: "2026-11-05T00:00:00Z", providerRespondedAt: null, decisionNote: null, serverNow: at,
  actionRequired: true, permissions: { buyer: false, provider: true, owner: false, withdraw: false, waive: true, respond: true, resolve: false, retryRevoke: false },
  events: [{ id: "event", action: "settled", createdAt: at, detail: {} }], eventsNextCursor: "event",
};
const preview = { subscriptionId: "rental", previewId: "preview", serverNow: at, expiresAt: "2026-11-02T00:02:00Z",
  periodAmountUnits: "432000", remainingUnits: "432000", compensationCapUnits: "43200", returnUnits: "388800", rateBps: 1000, policyVersion: 1, usagePending: true };

test("Share settlement keeps exact sub-cent money and accepts a zero-compensation trial preview", () => {
  assert.deepEqual(parseShareTermination(record), record);
  assert.deepEqual(parseShareTerminationPreview(preview), preview);
  assert.equal(shareTerminationUnits("9223372036854775807"), true);
  for (const v of [1, "-1", "1e2", "00", "1.0", "9223372036854775808"]) assert.equal(shareTerminationUnits(v), false);
  assert.equal(parseShareTerminationPreview({ ...preview, periodAmountUnits: "0", remainingUnits: "0", compensationCapUnits: "0", returnUnits: "0", policyVersion: 0 }).returnUnits, "0");
});
test("Share response parsing rejects unsafe identities, money, dates, permissions and history", () => {
  for (const patch of [{ shareName: {} }, { claim: null }, { decisionNote: [] }, { buyerEmail: null }, { version: 0 },
    { policyVersion: -1 }, { status: "released" }, { responsibility: "provider" }, { periodAmountUnits: 1 },
    { compensationUnits: "9223372036854775808" }, { serverNow: "today" }, { revokedAt: undefined },
    { permissions: { ...record.permissions, resolve: "true" } }, { eventsNextCursor: "" },
    { events: [{ ...record.events[0], detail: [] }] }]) assert.throws(() => parseShareTermination({ ...record, ...patch }));
  for (const patch of [{ subscriptionId: "" }, { policyVersion: undefined }, { remainingUnits: "500000" },
    { compensationCapUnits: "500000" }, { returnUnits: "388801" }, { expiresAt: at }]) assert.throws(() => parseShareTerminationPreview({ ...preview, ...patch }));
});
test("Share explanation bounds are UTF-8 bytes and deep links cannot escape their local route", () => {
  assert.equal(shareTerminationNoteValid(""), true);
  assert.equal(shareTerminationNoteValid("中".repeat(666)), true);
  assert.equal(shareTerminationNoteValid("中".repeat(667)), false);
  assert.equal(shareTerminationHref("old/id?x=1"), "/share-market/?terminationId=old%2Fid%3Fx%3D1");
});
test("Share termination messages have complete bilingual coverage", () => {
  assert.deepEqual(Object.keys(shareTerminationMessagesEn).sort(), Object.keys(shareTerminationMessagesZh).sort());
  for (const [key, value] of Object.entries(shareTerminationMessagesZh)) assert.ok(value, key);
});
