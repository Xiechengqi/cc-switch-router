import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  capabilityAmount,
  parseCapabilityPreview,
  capabilityIntent,
  capabilityQuotaContext,
  emptyCapabilityPolicy,
  SHARE_OPERATIONS,
} from "./share-capabilities";
import {
  changedUserGrants,
  buildShareSettingsPatch,
  draftFromShare,
} from "./share-settings";
import type { ShareUserGrantMap, ShareView } from "./types";

test("capability wire catalog matches the Rust fixture and explicit empty permissions survive a grant edit", () => {
  const fixture = JSON.parse(
    readFileSync(
      new URL(
        "../../tests/fixtures/share-capability-policy-v1.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  assert.deepEqual(SHARE_OPERATIONS, fixture.operations);
  assert.deepEqual(emptyCapabilityPolicy(), fixture.emptyPolicy);
  const grants: ShareUserGrantMap = {
    "user@example.com": {
      email: "user@example.com",
      active: true,
      role: "shareto",
      revision: 3,
      policy: { tokenPeriod: "day", tokenLimit: 100_000_000 },
    },
  };
  const changed = structuredClone(grants);
  changed["user@example.com"].policy.allowedOperations = [];
  assert.deepEqual(changedUserGrants(changed, grants)["user@example.com"], {
    expectedRevision: 3,
    policy: {
      tokenPeriod: "day",
      tokenLimit: 100_000_000,
      allowedOperations: [],
    },
  });
});

test("quota confirmation scope changes with the user, quota unit, runtime and explicit permissions", () => {
  const policy = {
    ...emptyCapabilityPolicy(),
    allowedOperations: ["image.generate" as const],
    operationBindings: {
      "image.generate": [{ app: "codex", fingerprint: "a".repeat(64) }],
    },
  };
  const grants: ShareUserGrantMap = {
    "user@example.com": {
      email: "user@example.com",
      active: true,
      role: "shareto",
      policy: { tokenPeriod: "day", tokenLimit: 100_000_000 },
    },
  };
  const key = capabilityQuotaContext(policy, grants, -1);
  const usd = structuredClone(grants);
  usd["user@example.com"].policy = {
    tokenPeriod: "day",
    valueQuota: {
      limitUsdMicros: "100000000",
      tariffPolicyId: "api-equivalent-v2",
      unpricedUsage: "reject",
    },
  };
  assert.notEqual(capabilityQuotaContext(policy, usd, -1), key);
  const narrowed = structuredClone(grants);
  narrowed["user@example.com"].policy.allowedOperations = [];
  assert.notEqual(capabilityQuotaContext(policy, narrowed, -1), key);
  const rebound = structuredClone(policy);
  rebound.operationBindings["image.generate"][0].fingerprint = "b".repeat(64);
  assert.notEqual(capabilityQuotaContext(rebound, grants, -1), key);
  assert.notEqual(
    capabilityIntent(undefined),
    capabilityIntent(emptyCapabilityPolicy()),
  );
});

test("unrelated Share settings preserve legacy authorization, while confirmed adoption is part of the same patch", () => {
  const share = {
    shareId: "share",
    shareName: "Share",
    ownerEmail: "owner@example.com",
    description: "",
    freeAccess: false,
    tokenLimit: -1,
    parallelLimit: -1,
    expiresAt: "2099-12-31T23:59:59Z",
    userGrants: {},
  } as ShareView;
  const draft = draftFromShare(share);
  draft.description = "Updated";
  assert.equal(
    buildShareSettingsPatch(draft, share).capabilityPolicy,
    undefined,
  );
  draft.capabilityPolicy = emptyCapabilityPolicy();
  draft.capabilityConsent = {
    credential: "signed-preview",
    acceptUncovered: [],
  };
  const patch = buildShareSettingsPatch(draft, share);
  assert.deepEqual(patch.capabilityPolicy, emptyCapabilityPolicy());
  assert.equal(patch.description, "Updated");
  assert.equal(patch.userUsageEdits, undefined);
});

test("USD amounts remain integer based and absent amounts never display as zero", () => {
  assert.equal(capabilityAmount(undefined), "—");
  assert.equal(capabilityAmount("0"), "$0.000000");
  assert.equal(capabilityAmount("1000000000001"), "$1.000000");
});

test("malformed or stale capability previews cannot crash the editor or authorize saving", () => {
  const preview = {
    shareId: "share",
    configRevision: 5,
    ready: false,
    expiresAt: Date.now() + 1000,
    policy: emptyCapabilityPolicy(),
    rows: [],
    requiredAcknowledgements: [],
  };
  assert.equal(
    parseCapabilityPreview(preview, "share", { expectedConfigRevision: 5 })
      .ready,
    false,
  );
  for (const invalid of [
    { rows: [] },
    { ...preview, shareId: "another" },
    { ...preview, configRevision: 4 },
    { ...preview, requiredAcknowledgements: null },
    { ...preview, ready: true },
  ]) {
    assert.throws(
      () =>
        parseCapabilityPreview(invalid, "share", { expectedConfigRevision: 5 }),
      /invalid or stale/,
    );
  }
});
