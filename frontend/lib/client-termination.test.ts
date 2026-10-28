import test from "node:test";
import assert from "node:assert/strict";
import { parseTerminationRate, parseTerminationAward, terminationCanDispute, terminationCanDecide, terminationCanWithdraw, terminationCanRespond, terminationTimeoutDue, terminationRemainingCompensation, terminationExplanationValid, type ClientTermination } from "./client-termination";
import { clientTerminationMessagesEn, clientTerminationMessagesZh } from "./client-termination-messages";
const at = Date.parse("2026-10-01T00:00:00Z");
function settlement(patch: Partial<ClientTermination> = {}): ClientTermination {
  return { id: "termination", installationId: "client", hostId: "host", contractId: "contract", periodId: "period",
    buyerUserId: "buyer", providerUserId: "provider", policyVersion: 1, rateBps: 1000, periodAmountUnits: "432000",
    remainingUnits: "432000", compensationCapUnits: "43200", returnedUnits: "388800", compensationUnits: "43200",
    heldUnits: "0", reason: "voluntary", claim: "", responsibility: "buyer", status: "completed",
    cutoffAt: "2026-10-01T00:00:00Z", cleanupConfirmedAt: "2026-10-01T00:00:00Z", cleanupJobId: "job",
    disputeUsed: false, disputeOpenedAt: null, disputeDeadlineAt: null, providerRespondedAt: null, resolution: null, chatAvailable: false,
    version: 2, updatedAt: "2026-10-01T00:00:00Z", unlockAt: "2026-10-08T00:00:00Z", creditedUnits: "0", ...patch };
}
test("Client termination policy accepts exact percentages including zero and two decimal places", () => {
  for (const [raw, bps] of [["0", 0], ["10", 1000], ["7.25", 725], ["100.00", 10000]] as const) assert.equal(parseTerminationRate(raw), bps);
  for (const raw of ["-1", "100.01", "0.001", "1e1", "", "NaN"]) assert.equal(parseTerminationRate(raw), null);
});
test("partial compensation preserves sub-cent amounts without floating point money", () => {
  assert.equal(parseTerminationAward("0.005"), "43200");
  assert.equal(parseTerminationAward("10.00"), "86400000");
  assert.equal(parseTerminationAward("0"), "0");
  for (const raw of ["-1", "1e3", "0.0000001", "NaN", "1,000"]) assert.equal(parseTerminationAward(raw), null);
});
test("buyer dispute requires a real uncredited award and an open settlement window", () => {
  assert.equal(terminationCanDispute(settlement(), at), true);
  for (const patch of [{ disputeUsed: true }, { creditedUnits: "43200" }, { compensationCapUnits: "0" },
    { compensationUnits: "0" }, { status: "finance_pending" as const }, { unlockAt: "2026-10-01T00:00:00Z" }]) {
    assert.equal(terminationCanDispute(settlement(patch), at), false);
  }
});
test("accepted compensation disputes remain decidable past maturity but fully waived awards do not", () => {
  const later = at + 20 * 86400000;
  assert.equal(terminationCanDecide(settlement(), later), false);
  assert.equal(terminationCanDecide(settlement({ status: "disputed" }), later), true);
  assert.equal(terminationCanDecide(settlement({ compensationUnits: "0", heldUnits: "43200", unlockAt: null, status: "disputed" }), later), true);
  assert.equal(terminationCanDecide(settlement({ compensationUnits: "0", heldUnits: "0" }), at), false);
});
test("Client termination UI has matching English and Chinese resources", () => {
  assert.deepEqual(Object.keys(clientTerminationMessagesEn).sort(), Object.keys(clientTerminationMessagesZh).sort());
  for (const [key, value] of Object.entries(clientTerminationMessagesEn)) assert.ok(value && clientTerminationMessagesZh[key as keyof typeof clientTerminationMessagesEn]);
});
test("issue explanations are optional while audit responses remain required and UTF-8 bounded", () => {
  assert.equal(terminationExplanationValid(""), true);
  assert.equal(terminationExplanationValid("短"), true);
  assert.equal(terminationExplanationValid("中".repeat(666)), true);
  assert.equal(terminationExplanationValid("中".repeat(667)), false);
  assert.equal(terminationExplanationValid("", true), false);
  assert.equal(terminationExplanationValid("abc", true), true);
});
test("formal response and buyer withdrawal have an exact 72-hour deadline", () => {
  const t = settlement({ responsibility: "undetermined", status: "disputed", disputeUsed: true,
    disputeOpenedAt: "2026-10-01T00:00:00Z", disputeDeadlineAt: "2026-10-04T00:00:00Z" });
  const deadline = at + 72 * 3600000;
  assert.equal(terminationCanWithdraw(t, deadline - 1), true);
  assert.equal(terminationCanRespond(t, deadline - 1), true);
  assert.equal(terminationTimeoutDue(t, deadline), true);
  assert.equal(terminationCanRespond(t, deadline), false);
  assert.equal(terminationCanWithdraw(t, deadline), false);
  assert.equal(terminationCanDecide(t, deadline), false);
  const responded = { ...t, providerRespondedAt: "2026-10-03T00:00:00Z" };
  assert.equal(terminationTimeoutDue(responded, deadline), false);
  assert.equal(terminationCanRespond(responded, deadline), false);
  assert.equal(terminationCanWithdraw(responded, deadline), true);
  assert.equal(terminationCanDecide(responded, deadline), true);
  assert.equal(terminationCanWithdraw({ ...responded, resolution: "buyer_withdrawal", responsibility: "buyer", status: "completed" }, deadline), false);
});
test("withdrawal confirmation uses the actual remaining compensation including pre-cleanup holds", () => {
  assert.equal(terminationRemainingCompensation(settlement()), "43200");
  assert.equal(terminationRemainingCompensation(settlement({ compensationUnits: "0", heldUnits: "12345" })), "12345");
  assert.equal(terminationRemainingCompensation(settlement({ cleanupConfirmedAt: null, compensationUnits: "0", heldUnits: "0", compensationCapUnits: "999" })), "999");
  assert.equal(terminationCanWithdraw(settlement({ responsibility: "undetermined", disputeUsed: true, status: "finance_pending" }), at), false);
  assert.equal(terminationCanRespond(settlement({ responsibility: "undetermined", disputeUsed: true, status: "finance_pending", compensationUnits: "0", heldUnits: "0" }), at), true);
});
