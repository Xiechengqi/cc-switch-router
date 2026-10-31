import assert from "node:assert/strict";
import test from "node:test";
import {
  usdLimitToMicros, microsToUsdInput, usdUsageToPicos, formatQuotaPicos,
  mergeValueStatuses, compatibleValueStatus, valueStatusTextKey, valueQuotaAvailability, type ValueQuotaStatus,
  valueCapabilityReady,valueReadinessError,type ValueQuotaPreview,
  previewValueQuota, parseUsdLimit, formatUsdLimit, valuePreviewRowOutcome, valuePreviewNeedsAcknowledgement,
  valuePreviewBlocker, valueQuotaErrorMessage, valueQuotaView, normalizeValueStatuses, ValueQuotaClientError,
  splitValueStatusRows, upsertValueStatuses, valueStatusReason, formatQuotaTime,
} from "./share-value-quota";
import { applyShareUserPolicyBatch } from "../components/dashboard/share-edit/share-user-policy-batch";
import { buildShareSettingsPatch, draftFromShare, stageUserPolicy } from "./share-settings";
import type { ShareUserGrant, ShareView } from "./types";

test("readiness transport carries cancellation and rejects malformed or stale authoritative previews",async()=>{
  const original=globalThis.fetch;const controller=new AbortController();
  let body:unknown={ready:false,capabilityReady:false,checks:[],configRevision:7,rows:[]};
  try {
    globalThis.fetch=async (url,init)=>{
      assert.equal(url,"/v1/shares/share%2Ftest/value-quota-preview");
      assert.equal(init?.signal,controller.signal);
      assert.equal(init?.cache,"no-store");
      assert.deepEqual(JSON.parse(String(init?.body)),{expectedConfigRevision:7,policies:{}});
      return Response.json(body);
    };
    assert.equal((await previewValueQuota("share/test",7,{},controller.signal)).ready,false);
    for(const invalid of [null,{},[],{ready:true,configRevision:6,rows:[]},{ready:true,configRevision:7,rows:[],checks:[null]}]) {
      body=invalid;
      await assert.rejects(previewValueQuota("share/test",7,{},controller.signal),error=>valueReadinessError(error)==="invalidResponse");
    }
  }finally{globalThis.fetch=original;}
});

test("capability readiness requires every independent authoritative check, not policy history or a generic ready flag",()=>{
  const checks=["router_protocol","sqlite_authority","metering_storage","acknowledged_tariff","text_surface"].map(code=>({code,ready:true}));
  const preview:ValueQuotaPreview={ready:false,capabilityReady:true,checks,configRevision:1,rows:[]};
  assert.equal(valueCapabilityReady(preview),true,"incomplete user coverage is distinct from infrastructure capability");
  assert.equal(valueCapabilityReady({...preview,ready:true,checks:undefined}),false);
  for(const check of checks) {
    assert.equal(valueCapabilityReady({...preview,checks:checks.filter(c=>c.code!==check.code)}),false,check.code);
    assert.equal(valueCapabilityReady({...preview,checks:checks.map(c=>c===check?{...c,ready:false}:c)}),false,check.code);
  }
  assert.equal(valueCapabilityReady({...preview,checks:[...checks,{code:"text_surface",ready:false}]}),false,"all bindings must be verified");
  assert.equal(valueCapabilityReady({...preview,checks:[...checks,{code:"tariff_published",ready:false}]}),false);
  assert.equal(valueReadinessError({status:409}),"conflict");
  assert.equal(valueReadinessError({code:"quota_unavailable",details:{readinessCode:"quota_server_offline"}}),"offline");
  assert.equal(valueReadinessError({details:{readinessCode:"quota_server_upgrade_required"}}),"upgrade");
  assert.equal(valueReadinessError(new Error("sensitive upstream body")),"failed");
});

test("USD quota decimals are exact, bounded and independent of wallet currency", () => {
  assert.equal(usdLimitToMicros("0.000001"), "1");
  assert.equal(usdLimitToMicros("1000000000"), "1000000000000000");
  assert.equal(usdLimitToMicros("12.345678"), "12345678");
  assert.equal(microsToUsdInput("12345678"), "12.345678");
  for (const [input, micros] of [["$10.00", "10000000"], [" 10", "10000000"], [".5", "500000"], ["05", "5000000"],
    ["1,000", "1000000000"], ["US$ 1,250.5", "1250500000"], ["us$2", "2000000"], ["10.", "10000000"], ["1,000,000.000001", "1000000000001"]]) {
    assert.equal(usdLimitToMicros(input), micros, input);
  }
  for (const [input, error] of [["", "empty"], ["  ", "empty"], ["$", "empty"], ["0", "too_small"], ["0.000000", "too_small"],
    ["1.0000001", "too_many_decimals"], ["1000000000.000001", "too_large"], ["1e2", "format"], ["-1", "format"], ["1,00", "format"],
    ["10,0000", "format"], [",100", "format"], [".", "format"], ["1.2.3", "format"], ["$-1", "format"], ["1 000", "format"]] as const) {
    assert.deepEqual(parseUsdLimit(input), {ok: false, error}, input);
  }
  assert.equal(usdUsageToPicos("0.000000000001"), "1");
  assert.equal(usdUsageToPicos("0"), "0");
  assert.equal(usdUsageToPicos("$1,000.5"), "1000500000000000");
  assert.equal(usdUsageToPicos("1.0000000000001"), undefined);
  assert.equal(usdUsageToPicos(""), undefined);
  assert.equal(formatQuotaPicos("1"), "< US$0.000001");
  assert.equal(formatQuotaPicos("18000000000"), "US$0.018");
  assert.equal(formatQuotaPicos("1500000000000"), "US$1.50");
  assert.equal(formatQuotaPicos("1234567000000000000"), "US$1,234,567.00");
  assert.equal(formatQuotaPicos(undefined), "—");
  assert.equal(formatUsdLimit("10000000"), "US$10");
  assert.equal(formatUsdLimit("10500000"), "US$10.50");
  assert.equal(formatUsdLimit("1000000000000"), "US$1,000,000");
  assert.equal(formatUsdLimit("1"), "US$0.000001");
  assert.deepEqual(valueQuotaAvailability("999999999999", "1000000"), {remainingPicos:"1", exhausted:false, nearLimit:true, basisPoints:9999});
  assert.deepEqual(valueQuotaAvailability("1000000000001", "1000000"), {remainingPicos:"0", exhausted:true, nearLimit:true, basisPoints:10000});
  assert.equal(valueQuotaAvailability("790000000000", "1000000")?.nearLimit, false);
  assert.equal(valueQuotaAvailability(undefined,"1000000"), undefined);
});
test("preview reason codes decide schedule, acknowledgement and translated blockers", () => {
  const row = {email: "a@example.test", effectiveFromMs: 10_000, meteredFromMs: 10_000, usedPicos: "0"};
  assert.equal(valuePreviewRowOutcome({...row, reasonCode: "scheduled_next_window"}, 0), "scheduled");
  assert.equal(valuePreviewRowOutcome({...row, reasonCode: "metered_from_now"}, 0), "meteredFromNow");
  assert.equal(valuePreviewRowOutcome({...row, reasonCode: "ready"}, 0), "ready");
  assert.equal(valuePreviewRowOutcome({...row, reasonCode: "pricing_unavailable"}, 0), "blocked");
  assert.equal(valuePreviewRowOutcome({...row, reasonCode: "ready", ready: false}, 0), "blocked");
  assert.equal(valuePreviewRowOutcome({...row, reasonCode: "future_code"}, 0), "blocked");
  assert.equal(valuePreviewRowOutcome(row, 0), "scheduled", "legacy rows fall back to their start time");
  assert.equal(valuePreviewRowOutcome(row, 10_000), "ready");
  const preview: ValueQuotaPreview = {ready: true, configRevision: 1, rows: [{...row, reasonCode: "metered_from_now"}]};
  assert.equal(valuePreviewNeedsAcknowledgement(preview, 0), false);
  assert.equal(valuePreviewNeedsAcknowledgement({...preview, rows: [{...row, reasonCode: "scheduled_next_window"}]}, 0), true);
  assert.equal(valuePreviewNeedsAcknowledgement({...preview, rows: [{...row, reasonCode: "ready", usedPicos: "5"}]}, 0), true);
  assert.equal(valuePreviewBlocker(preview, 0), undefined);
  assert.deepEqual(valuePreviewBlocker({...preview, ready: false, rows: [{...row, reasonCode: "blocked_needs_reconcile"}]}, 0),
    {email: row.email, message: {key: "dashboard.userLimit.value.reason.blocked_needs_reconcile"}});
  assert.deepEqual(valuePreviewBlocker({...preview, ready: false, rows: [], reason: "Raw server text"}, 0)?.message, {raw: "Raw server text"});
  assert.deepEqual(valuePreviewBlocker({...preview, ready: false, rows: [], reasonCode: "storage_unavailable", reason: "x"}, 0)?.message,
    {key: "dashboard.userLimit.value.reason.storage_unavailable"});
});
test("every quota failure maps to a translated message, never an object dump", () => {
  assert.deepEqual(valueQuotaErrorMessage(new ValueQuotaClientError("quota_server_invalid_response")), {key: "dashboard.userLimit.value.readiness.invalidResponse"});
  assert.deepEqual(valueQuotaErrorMessage({status: 409, code: "quota_revision_conflict"}), {key: "dashboard.userLimit.value.readiness.conflict"});
  assert.deepEqual(valueQuotaErrorMessage({status: 400, code: "quota_target_invalid"}), {key: "dashboard.userLimit.value.error.targetInvalid"});
  assert.deepEqual(valueQuotaErrorMessage({code: "quota_unavailable", details: {readinessCode: "quota_server_timeout"}}), {key: "dashboard.userLimit.value.readiness.timeout"});
  assert.deepEqual(valueQuotaErrorMessage({code: "blocked_needs_reconcile"}), {key: "dashboard.userLimit.value.reason.blocked_needs_reconcile"});
  assert.deepEqual(valueQuotaErrorMessage(new Error("boom")), {key: "dashboard.userLimit.value.readiness.failed"});
  assert.equal(String(new ValueQuotaClientError("x")).includes("[object"), false);
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
test("grantee rows are self-contained and scheduled policies keep the enforced view", () => {
  const rows = normalizeValueStatuses([{...status, email: "User@Example.Test", syncedAtMs: undefined, syncedAt: 42},
    {...status, email: "next@example.test", limitUsdMicros: null, pendingLimitUsdMicros: "10000000", pendingEffectiveFromMs: 99}, null, {}]);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].email, "user@example.test");
  assert.equal(rows[0].syncedAtMs, 42);
  assert.equal(valueQuotaView(undefined, "user@example.test", rows, true)?.enforced?.limitMicros, "1000000");
  assert.deepEqual(valueQuotaView(undefined, "next@example.test", rows, true), {scheduled: {limitMicros: "10000000", effectiveFromMs: 99}});
  assert.equal(valueQuotaView(undefined, "absent@example.test", rows, true), undefined, "no row yet is not a zero row");
  const scheduled = {...grant, policy: {tokenPeriod: "day" as const, tokenLimit: 100},
    pendingValueQuota: {policy: grant.policy, effectiveFromMs: 5000, retainAccess: true}};
  // Token policy enforced: Server reports the user as `scheduled`, so only the schedule shows.
  const scheduledRow = {...status, meteringStatus: "scheduled", limitUsdMicros: null, pendingLimitUsdMicros: "1000000", pendingEffectiveFromMs: 5000};
  assert.deepEqual(valueQuotaView(scheduled, grant.email, [scheduledRow]), {scheduled: {limitMicros: "1000000", effectiveFromMs: 5000}});
  assert.deepEqual(valueQuotaView(scheduled, grant.email, []), {scheduled: {limitMicros: "1000000", effectiveFromMs: 5000}});
  assert.equal(valueQuotaView(grant, grant.email, [status])?.enforced?.status, status);
  assert.equal(valueQuotaView({...grant, policy: {tokenPeriod: "day", tokenLimit: 1}}, grant.email, [status]), undefined);
});

test("a USD to USD change keeps showing the enforced limit and its live window", () => {
  const twenty = {limitUsdMicros: "20000000", tariffPolicyId: "api-equivalent-v1" as const};
  const pending = {policy: {tokenPeriod: "day" as const, valueQuota: twenty}, effectiveFromMs: 9000, retainAccess: true};
  // Dashboard drafts hold the enforced policy; share-page drafts hold the scheduled one.
  const enforcedDraft = {...grant, pendingValueQuota: pending};
  const targetDraft = {...grant, policy: pending.policy, pendingValueQuota: pending};
  const live = {...status, pendingLimitUsdMicros: "20000000", pendingEffectiveFromMs: 9000};
  for (const draft of [enforcedDraft, targetDraft]) {
    const view = valueQuotaView(draft, grant.email, [live]);
    assert.equal(view?.enforced?.limitMicros, "1000000");
    assert.equal(view?.enforced?.status, live);
    assert.deepEqual(view?.scheduled, {limitMicros: "20000000", effectiveFromMs: 9000});
  }
  assert.deepEqual(valueQuotaView(enforcedDraft, grant.email, [])?.enforced, {limitMicros: "1000000"});
  assert.equal(valueQuotaView(targetDraft, grant.email, [])?.enforced, undefined, "no live row: the enforced limit is unknown, not the scheduled one");
});
test("a grantee's USD grant without a status row reads as unsynced, never 0 / unlimited", () => {
  assert.deepEqual(valueQuotaView(grant, grant.email, [], true), {enforced: {limitMicros: "1000000"}});
  const minimal = normalizeValueStatuses([{email: grant.email, meteringStatus: "scheduled", tokenPeriod: "day",
    pendingLimitUsdMicros: "2000000", pendingTokenPeriod: "day", pendingEffectiveFromMs: 77}]);
  assert.deepEqual(valueQuotaView(undefined, grant.email, minimal, true), {scheduled: {limitMicros: "2000000", effectiveFromMs: 77}});
  assert.equal(valueQuotaView({...grant, policy: {tokenPeriod: "day", tokenLimit: 5}}, grant.email, [], true), undefined);
});
test("staging a different policy drops the schedule; re-applying the schedule keeps it", () => {
  const pending = {policy: grant.policy, effectiveFromMs: 5000, retainAccess: true};
  const scheduled = {...grant, policy: {tokenPeriod: "day" as const, tokenLimit: 100}, pendingValueQuota: pending};
  const kept = stageUserPolicy(scheduled, {...grant.policy});
  assert.equal(kept.pendingValueQuota, pending);
  assert.equal(kept.policy.tokenLimit, 100);
  const tokens = stageUserPolicy(scheduled, {tokenPeriod: "day", tokenLimit: 300});
  assert.equal(tokens.pendingValueQuota, undefined);
  assert.equal(tokens.policy.tokenLimit, 300);
  assert.deepEqual(tokens.valueMetering, grant.valueMetering, "the metering binding survives");
  const batch = applyShareUserPolicyBatch({[grant.email]: scheduled}, new Set([grant.email]), {tokenLimit: {value: 7, period: "day"}});
  assert.equal(batch[grant.email].pendingValueQuota, undefined);
  // Cancelling a schedule by restaging the enforced Token policy is still sent.
  const share = {shareId: "share", ownerEmail: "owner@example.test", userGrants: {[grant.email]: {...scheduled, revision: 4}}} as unknown as ShareView;
  const draft = draftFromShare(share);
  draft.userGrants[grant.email] = stageUserPolicy(draft.userGrants[grant.email], {tokenPeriod: "day", tokenLimit: 100});
  assert.deepEqual(buildShareSettingsPatch(draft, share).userGrantEdits, {[grant.email]: {expectedRevision: 4, policy: {tokenPeriod: "day", tokenLimit: 100}}});
});
test("USD-only edits are policy edits", () => {
  const share = {shareId: "share", ownerEmail: "owner@example.test", userGrants: {[grant.email]: {...grant, revision: 2}}} as unknown as ShareView;
  for (const policy of [{tokenPeriod: "day" as const, valueQuota: {...money, limitUsdMicros: "2000000"}}, {tokenPeriod: "day" as const}]) {
    const draft = draftFromShare(share);
    draft.userGrants[grant.email] = {...draft.userGrants[grant.email], policy};
    assert.deepEqual(buildShareSettingsPatch(draft, share).userGrantEdits, {[grant.email]: {expectedRevision: 2, policy}});
  }
});
test("correction error rows never overwrite a good status row", () => {
  const {items, errors} = splitValueStatusRows([status, {email: "Other@Example.Test", error: {code: "quota_revision_conflict"}}]);
  assert.deepEqual(items, [status]);
  assert.deepEqual(errors, [{email: "other@example.test", code: "quota_revision_conflict"}]);
  const other = {...status, email: "other@example.test"};
  const merged = upsertValueStatuses([status, other], [{...status, usageWatermark: 3, usedPicos: "5"}]);
  assert.equal(merged.length, 2);
  assert.equal(merged.find(row => row.email === other.email), other);
  assert.equal(merged.find(row => row.email === status.email)?.usedPicos, "5");
  assert.deepEqual(valueQuotaErrorMessage({code: "value_quota_not_active"}), {key: "dashboard.userLimit.value.error.notActive"});
  assert.deepEqual(valueQuotaErrorMessage({status: 400, code: "bad_request", details: {reasonCode: "invalid_policy"}}), {key: "dashboard.userLimit.value.reason.invalid_policy"});
});
test("status reasons and invalid preview rows are surfaced", () => {
  assert.equal(valueStatusReason(status), undefined);
  assert.equal(valueStatusReason({...status, meteringStatus: "observed", reasonCode: "pricing_unavailable"}), undefined);
  assert.deepEqual(valueStatusReason({...status, meteringStatus: "unavailable", reasonCode: "pricing_unavailable"}),
    {key: "dashboard.userLimit.value.reason.pricing_unavailable"});
  assert.deepEqual(valueStatusReason({...status, meteringStatus: "unavailable", reasonCode: "coverage_gap"}), {raw: "coverage_gap"});
  const preview: ValueQuotaPreview = {ready: false, configRevision: 1, rows: [], invalidRows: [{email: "a@example.test", reasonCode: "invalid_policy"}]};
  assert.deepEqual(valuePreviewBlocker(preview), {email: "a@example.test", message: {key: "dashboard.userLimit.value.reason.invalid_policy"}});
  assert.equal(formatQuotaTime(Number.NaN, "en"), "—");
  assert.equal(formatQuotaTime(Date.UTC(2026, 0, 2), "zh-CN"), new Date(Date.UTC(2026, 0, 2)).toLocaleString("zh-CN"));
});
