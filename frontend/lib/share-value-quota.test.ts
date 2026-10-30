import assert from "node:assert/strict";
import test from "node:test";
import {
  usdLimitToMicros, microsToUsdInput, usdUsageToPicos, formatQuotaPicos,
  mergeValueStatuses, compatibleValueStatus, valueStatusTextKey, valueQuotaAvailability, type ValueQuotaStatus,
} from "./share-value-quota";
import { applyShareUserPolicyBatch } from "../components/dashboard/share-edit/share-user-policy-batch";
import { buildShareSettingsPatch, draftFromShare } from "./share-settings";
import type { ShareUserGrant, ShareView } from "./types";

test("USD quota decimals are exact, bounded and independent of wallet currency", () => {
  assert.equal(usdLimitToMicros("0.000001"), "1");
  assert.equal(usdLimitToMicros("1000000000"), "1000000000000000");
  assert.equal(usdLimitToMicros("12.345678"), "12345678");
  assert.equal(microsToUsdInput("12345678"), "12.345678");
  for (const invalid of ["0", "01", "1.", ".1", "1e2", "-1", " 1", "1.0000001", "1000000000.000001"]) {
    assert.equal(usdLimitToMicros(invalid), undefined, invalid);
  }
  assert.equal(usdUsageToPicos("0.000000000001"), "1");
  assert.equal(usdUsageToPicos("0"), "0");
  assert.equal(usdUsageToPicos("1.0000000000001"), undefined);
  assert.equal(formatQuotaPicos("1"), "< US$0.000001");
  assert.equal(formatQuotaPicos("18000000000"), "US$0.018");
  assert.equal(formatQuotaPicos(undefined), "—");
  assert.deepEqual(valueQuotaAvailability("999999999999", "1000000"), {remainingPicos:"1", exhausted:false, nearLimit:true});
  assert.deepEqual(valueQuotaAvailability("1000000000001", "1000000"), {remainingPicos:"0", exhausted:true, nearLimit:true});
  assert.equal(valueQuotaAvailability(undefined,"1000000"), undefined);
});
const status: ValueQuotaStatus = {
  email: "user@example.test", policyEpoch: "epoch", windowId: "Day:1:2", windowStartsAtMs: 1, resetsAtMs: 2,
  meteredFromMs: 0, activePricingRevision: "sha256:tariff", usagePricingRevisionsDigest: "sha256:usage",
  usageWatermark: 2, limitUsdMicros: "1000000", usedPicos: "100", observedPicos: "100",
  manualOffsetPicos: "0", pendingRiskPicos: "0", meteringStatus: "observed", syncedAtMs: 10,
};
test("a higher authoritative revision may lower usage; stale windows and watermarks cannot replace it", () => {
  assert.equal(mergeValueStatuses([status], [{...status, usageWatermark: 1, usedPicos: "999"}])[0], status);
  assert.equal(mergeValueStatuses([status], [{...status, windowId: "older", windowStartsAtMs: 0}])[0], status);
  assert.equal(mergeValueStatuses([status], [{...status, usageWatermark: 3, usedPicos: "90"}])[0].usedPicos, "90");
  assert.equal(mergeValueStatuses([status], [{...status, windowId: "next", windowStartsAtMs: 2, usageWatermark: 0}])[0].windowId, "next");
  assert.equal(valueStatusTextKey(status, 100_000), "dashboard.userLimit.value.stale");
  assert.equal(valueStatusTextKey(undefined), "dashboard.userLimit.value.unsynced");
});
const money = {limitUsdMicros: "1000000", tariffPolicyId: "api-equivalent-v1" as const};
const grant: ShareUserGrant = {
  email: status.email, role: "shareto", active: true,
  policy: {tokenPeriod: "day", valueQuota: money},
  valueMetering: {quotaPolicyEpoch: "epoch", valueMeteredFromMs: 0, effectiveFromMs: 1, pricingRevision: "sha256:tariff"},
};
test("private status belongs to the active policy, not another user's or an old epoch's response", () => {
  assert.equal(compatibleValueStatus([status], grant), status);
  assert.equal(compatibleValueStatus([status], {...grant, active: false}), undefined);
  assert.equal(compatibleValueStatus([status], {...grant, email: "other@example.test"}), undefined);
  assert.equal(compatibleValueStatus([status], {...grant, valueMetering: {...grant.valueMetering!, quotaPolicyEpoch: "new"}}), undefined);
  assert.equal(compatibleValueStatus([status], {...grant, policy: {tokenPeriod: "day", tokenLimit: 100}}), undefined);
});
test("batch quota switching removes the other unit and unrelated edits preserve the scheduled policy", () => {
  const scheduled = {...grant, policy: {tokenPeriod: "day" as const, tokenLimit: 100},
    pendingValueQuota: {policy: grant.policy, effectiveFromMs: 5000, retainAccess: true}};
  const one = {[grant.email]: scheduled};
  const edited = applyShareUserPolicyBatch(one, new Set([grant.email]), {parallelLimit: {value: 2}});
  assert.equal(edited[grant.email].policy.valueQuota, money);
  assert.equal(edited[grant.email].policy.parallelLimit, 2);
  const tokens = applyShareUserPolicyBatch(edited, new Set([grant.email]), {tokenLimit: {value: 1000, period: "day"}});
  assert.equal(tokens[grant.email].policy.valueQuota, undefined);
  assert.equal(tokens[grant.email].policy.tokenLimit, 1000);
  const dollars = applyShareUserPolicyBatch(tokens, new Set([grant.email]), {tokenLimit: {valueQuota: money, period: "day"}});
  assert.equal(dollars[grant.email].policy.tokenLimit, undefined);
  assert.equal(dollars[grant.email].policy.valueQuota, money);
  assert.equal(one[grant.email].policy.tokenLimit, 100);
});
test("pending policy is an editable draft, not a token-to-money usage conversion", () => {
  const scheduled = {...grant, policy: {tokenPeriod: "day" as const, tokenLimit: 100},
    pendingValueQuota: {policy: grant.policy, effectiveFromMs: 5000, retainAccess: true}};
  const share = {shareId: "share", ownerEmail: "owner@example.test", userGrants: {[grant.email]: scheduled}} as unknown as ShareView;
  const draft = draftFromShare(share);
  assert.equal(draft.userGrants[grant.email].policy.valueQuota, money);
  assert.equal(share.userGrants![grant.email].policy.tokenLimit, 100);
  assert.deepEqual(buildShareSettingsPatch(draft, share), {});
});
