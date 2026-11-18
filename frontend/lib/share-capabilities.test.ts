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

import {
  capabilityOptions,
  capabilityPresentation,
  capabilityWarnings,
  capabilityStatus,
  capabilityPricingText,
  capabilityPermissionMode,
  capabilityPermissionChange,
  reconcileCapabilityGrants,
  capabilityRiskGroups,
  preservesCapabilitySchedule,
  type CapabilityPreview,
} from "./share-capabilities";

const search = "tool.web_search" as const;
const image = "image.generate" as const;
const fingerprint = "a".repeat(64);
function displayPreview(): CapabilityPreview {
  return {
    shareId: "share",
    configRevision: 5,
    displayVersion: 2,
    ready: true,
    legacy: false,
    expiresAt: Date.now() + 60000,
    previewCredential: "ticket",
    policy: emptyCapabilityPolicy(),
    requiredAcknowledgements: [],
    rows: SHARE_OPERATIONS.map((operation) => ({
      app: "codex",
      operation,
      bindingFingerprint: fingerprint,
      controllable: operation === search,
      implemented: operation === search || operation === image,
      providerEnabled: operation !== image,
      accountSupport: "unknown" as const,
      unavailableReason:
        operation === search
          ? null
          : operation === image
            ? ("provider_disabled" as const)
            : ("driver_not_implemented" as const),
      modelScope: "request" as const,
      pricingCoverage: "request_dependent" as const,
      execution: "unknown",
      measurement: "estimated",
      nativeUnit: operation,
      pricing: "missing",
      admission: "estimated_reservation",
      scopes: [],
    })),
  };
}

test("display facts filter unimplemented operations and separate Provider setup from account evidence", () => {
  const preview = displayPreview();
  const options = capabilityOptions(preview, emptyCapabilityPolicy());
  assert.deepEqual(
    options.map((o) => [o.operation, o.available, o.configurable]),
    [
      [image, false, true],
      [search, true, false],
    ],
  );
  const saved = {
    ...emptyCapabilityPolicy(),
    allowedOperations: ["video.generate" as const],
  };
  assert.equal(
    capabilityOptions(preview, saved).find(
      (o) => o.operation === "video.generate",
    )?.selected,
    true,
  );
  assert.equal(
    capabilityStatus(preview.rows.find((r) => r.operation === image)!, 0),
    "需先在供应商设置中开启",
  );
  assert.equal(
    capabilityStatus(preview.rows.find((r) => r.operation === search)!, 0),
    "账号待验证",
  );
  assert.match(capabilityPricingText(preview.rows[0], 0), /实际模型与参数/);
});

test("individual permission modes retain the existing access when choosing a subset", () => {
  assert.equal(capabilityPermissionMode(undefined), "inherit");
  assert.deepEqual(
    capabilityPermissionChange("custom", undefined, [search, image]),
    [search, image],
  );
  assert.deepEqual(
    capabilityPermissionChange("custom", [search], [search, image]),
    [search],
  );
  assert.deepEqual(
    capabilityPermissionChange("disabled", [search], [search]),
    [],
  );
  assert.equal(capabilityPermissionChange("inherit", [], [search]), undefined);
});

test("narrowing Share permissions updates ordinary users without mutating Market contracts or usage", () => {
  const ordinary = {
    email: "a@example.com",
    active: true,
    role: "shareto" as const,
    policy: {
      tokenPeriod: "day" as const,
      tokenLimit: 100,
      allowedOperations: [search, image],
    },
    usage: { tokensUsed: 42 },
  };
  const market = {
    ...ordinary,
    email: "market@example.com",
    manager: "routerShareMarket" as const,
  };
  const grants = {
    [ordinary.email]: ordinary,
    [market.email]: market,
  } as ShareUserGrantMap;
  const result = reconcileCapabilityGrants(
    { ...emptyCapabilityPolicy(), allowedOperations: [search] },
    grants,
  );
  assert.deepEqual(result[ordinary.email].policy.allowedOperations, [search]);
  assert.deepEqual(grants[ordinary.email].policy.allowedOperations, [
    search,
    image,
  ]);
  assert.equal(result[market.email], market);
  assert.equal(result[ordinary.email].usage, ordinary.usage);
});

test("grouped risk review retains exact per-user and per-binding acceptance identities", () => {
  const preview = displayPreview();
  const row = preview.rows.find((r) => r.operation === search)!;
  row.scopes = ["a@example.com", "b@example.com"].map((scope, i) => ({
    scope,
    quotaUnit: "tokens",
    reason: "resource_not_in_token_ledger",
    coverage: "partial",
    condition: `condition-${i}`,
  }));
  preview.requiredAcknowledgements = row.scopes.map((scope) => ({
    condition: scope.condition!,
    scope: scope.scope,
    operation: search,
    quotaUnit: "tokens",
    bindingFingerprint: fingerprint,
    actor: "",
    acceptedAtMs: 0,
  }));
  const groups = capabilityRiskGroups(preview);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0].conditions, ["condition-0", "condition-1"]);
  const saved = {
    ...emptyCapabilityPolicy(),
    coverageAcknowledgements: [preview.requiredAcknowledgements[0]],
  };
  assert.deepEqual(capabilityRiskGroups(preview, saved)[0].conditions, [
    "condition-1",
  ]);
});

test("capability edits preserve a pending quota without treating unchanged financial edits as permission changes", () => {
  const policy = {
    tokenPeriod: "day" as const,
    tokenLimit: 100,
    allowedOperations: [search],
  };
  const pendingPolicy = {
    tokenPeriod: "day" as const,
    valueQuota: {
      limitUsdMicros: "1000000",
      tariffPolicyId: "api-equivalent-v1" as const,
      unpricedUsage: "reject" as const,
    },
    allowedOperations: [search],
  };
  const grant = {
    email: "a@example.com",
    active: true,
    role: "shareto" as const,
    policy,
    pendingValueQuota: {
      policy: pendingPolicy,
      effectiveFromMs: 9999999999999,
      retainAccess: true,
    },
  };
  assert.equal(
    preservesCapabilitySchedule(grant, { ...policy, allowedOperations: [] }),
    true,
  );
  assert.equal(
    preservesCapabilitySchedule(grant, {
      ...pendingPolicy,
      allowedOperations: [],
    }),
    true,
  );
  assert.equal(
    preservesCapabilitySchedule(grant, {
      ...policy,
      tokenLimit: 200,
      allowedOperations: [],
    }),
    false,
  );
  assert.equal(preservesCapabilitySchedule(grant, policy), false);
  assert.equal(preservesCapabilitySchedule(grant, pendingPolicy), false);
  const zeroPending = {
    ...pendingPolicy,
    valueQuota: { ...pendingPolicy.valueQuota, unpricedUsage: "zero" as const },
  };
  const zeroGrant = {
    ...grant,
    pendingValueQuota: { ...grant.pendingValueQuota, policy: zeroPending },
  };
  const omittedDefault = {
    ...zeroPending,
    allowedOperations: [],
    valueQuota: { ...zeroPending.valueQuota, unpricedUsage: undefined },
  };
  assert.equal(preservesCapabilitySchedule(zeroGrant, omittedDefault), true);
  assert.equal(preservesCapabilitySchedule(grant, omittedDefault), false);
  assert.equal(
    preservesCapabilitySchedule(grant, {
      ...pendingPolicy,
      valueQuota: {
        ...pendingPolicy.valueQuota,
        tariffPolicyId: "api-equivalent-v2",
      },
    }),
    true,
  );
});

test("malformed display facts cannot turn unknown support or prices into verified capability", () => {
  const preview = displayPreview();
  assert.equal(
    parseCapabilityPreview(preview, "share", { expectedConfigRevision: 5 }),
    preview,
  );
  for (const change of [
    { accountSupport: "yes" },
    { implemented: "true" },
    { pricingCoverage: "full" },
    { unavailableReason: "new_unknown_value" },
  ]) {
    assert.throws(
      () =>
        parseCapabilityPreview(
          { ...preview, rows: [{ ...preview.rows[0], ...change }] },
          "share",
          { expectedConfigRevision: 5 },
        ),
      /invalid or stale/,
    );
  }
});

test("only a complete current result with no options or retained state hides the capability card", () => {
  const preview = displayPreview();
  preview.rows = [];
  const policy = emptyCapabilityPolicy();
  const input = {
    preview,
    policy,
    savedPolicy: policy,
    current: true,
    loading: false,
  };
  assert.equal(capabilityPresentation(input).mode, "hidden");
  for (const change of [
    { current: false },
    { loading: true },
    { error: "offline" },
    { preview: { ...preview, ready: false } },
    { preview: { ...preview, displayVersion: undefined } },
    { preview: { ...preview, expiresAt: 0 } },
    {
      preview: {
        ...preview,
        unavailableBindings: [{ app: "codex", reason: "runtime_unavailable" }],
      },
    },
  ]) {
    assert.equal(
      capabilityPresentation({ ...input, ...change }).mode,
      "compact",
    );
  }
  assert.equal(
    capabilityPresentation({
      ...input,
      current: false,
      preview: undefined,
      error: "offline",
    }).status,
    "unavailable",
  );
  const supported = capabilityPresentation({
    ...input,
    preview: displayPreview(),
  });
  assert.equal(
    supported.mode,
    "full",
    "available but disabled capabilities remain configurable",
  );
  const setup = displayPreview();
  setup.rows = setup.rows.filter((row) => row.operation === image);
  assert.equal(
    capabilityPresentation({ ...input, preview: setup }).mode,
    "compact",
  );
});

test("hiding never drops a pending removal, migration, historical evidence or save review", () => {
  const preview = displayPreview();
  preview.rows = [];
  const policy = emptyCapabilityPolicy();
  const input = {
    preview,
    policy,
    savedPolicy: policy,
    current: true,
    loading: false,
  };
  const original = JSON.stringify(input);
  for (const change of [
    { savedPolicy: undefined },
    { savedPolicy: { ...policy, allowedOperations: [search] } },
    { reviewing: true },
    {
      preview: {
        ...preview,
        requiredAcknowledgements: [
          {
            condition: "review",
            scope: "owner",
            operation: search,
            quotaUnit: "tokens",
            bindingFingerprint: fingerprint,
            actor: "",
            acceptedAtMs: 0,
          },
        ],
      },
    },
    {
      preview: {
        ...preview,
        blockers: [{ scope: "owner", blocker: "MARKET_CAPABILITY_CONFLICT" }],
      },
    },
    {
      preview: {
        ...preview,
        incidents: [
          {
            scope: "owner",
            bindingFingerprint: fingerprint,
            requestId: "incident",
            resource: "image.output",
            detectedAtMs: 0,
          },
        ],
      },
    },
    {
      preview: {
        ...preview,
        recentUsage: [
          {
            scope: "owner",
            requestId: "usage",
            attempt: 1,
            admittedAtMs: 0,
            meteringStatus: "not_in_usd_ledger",
            dimensions: [],
          },
        ],
      },
    },
  ]) {
    assert.equal(capabilityPresentation({ ...input, ...change }).mode, "full");
  }
  assert.equal(JSON.stringify(input), original);
});

test("selected capability metering gaps remain visible with deduplicated detail", () => {
  const row = displayPreview().rows.find((row) => row.operation === search)!;
  row.selected = true;
  row.scopes = ["a", "b"].map((scope) => ({
    scope,
    quotaUnit: "tokens",
    coverage: "partial",
    reason: "resource_not_in_token_ledger",
  }));
  assert.equal(capabilityWarnings([row], 0).length, 1);
  assert.match(capabilityWarnings([row], 0)[0].label, /不计入 Token/);
  row.scopes[0].reason = "model_or_resource_price_unknown";
  assert.match(capabilityWarnings([row], 0)[0].label, /不扣美元/);
  assert.deepEqual(capabilityWarnings([{ ...row, selected: false }], 0), []);
});
