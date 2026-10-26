import assert from "node:assert/strict";
import test from "node:test";

import type { ShareUserGrant, ShareView } from "@/lib/types";
import { buildShareEditDraft, buildShareEditPatch } from "./share-edit-draft";

function grant(
  email: string,
  overrides: Partial<ShareUserGrant> = {},
): ShareUserGrant {
  return {
    email,
    role: email.endsWith("owner@example.com") ? "owner" : "shareto",
    active: true,
    policy: { tokenPeriod: "lifetime" },
    ...overrides,
  };
}

function shareView(userGrants: Record<string, ShareUserGrant>): ShareView {
  return {
    shareId: "share-jobmarsh",
    capacityPoolId: "pool-1",
    shareName: "jobmarsh",
    ownerEmail: "owner@example.com",
    freeAccess: false,
    subdomain: "jobmarsh--example",
    appType: "claude",
    bindings: { claude: "provider-claude" },
    tokenLimit: -1,
    parallelLimit: -1,
    tokensUsed: 0,
    requestsCount: 0,
    shareStatus: "active",
    createdAt: "2026-08-25T00:00:00Z",
    expiresAt: "2099-12-31T23:59:59Z",
    isOnline: true,
    routeState: "active",
    activeRequests: 0,
    onlineRate24h: 1,
    userGrants,
  };
}

test("unchanged settings send no grants, runtime snapshots, or market tombstones", () => {
  const share = shareView({
    "owner@example.com": grant("owner@example.com", {
      role: "owner",
      manager: "owner",
    }),
    "active@example.com": grant("active@example.com", {
      manager: "routerShareMarket",
      entitlementId: "entitlement-active",
    }),
    "revoked@example.com": grant("revoked@example.com", {
      active: false,
      manager: "routerShareMarket",
      entitlementId: "entitlement-revoked",
    }),
  });
  const patch = buildShareEditPatch(buildShareEditDraft(share), share, ["claude"]);
  assert.deepEqual(patch, {});
});

test("buildShareEditPatch writes a manual shareto over a revoked market tombstone", () => {
  const share = shareView({
    "owner@example.com": grant("owner@example.com", {
      role: "owner",
      manager: "owner",
    }),
    "renter@example.com": grant("renter@example.com", {
      active: false,
      manager: "routerShareMarket",
      entitlementId: "entitlement-revoked",
      revokedAtMs: 1,
      revision: 7,
    }),
  });
  const draft = buildShareEditDraft(share);
  draft.userGrants["renter@example.com"] = grant("renter@example.com", {
    role: "shareto",
    active: true,
    manager: "manual",
    policy: { tokenPeriod: "lifetime", tokenLimit: 800 },
  });
  const patch = buildShareEditPatch(draft, share, ["claude"]);
  const restored = patch.userGrantEdits?.["renter@example.com"];
  assert.equal(patch.userGrants, undefined);
  assert.equal(restored?.expectedRevision, 7);
  assert.equal(restored?.policy?.tokenLimit, 800);
});

test("multi-user edits are sparse and never include observed usage", () => {
  const share = shareView({
    "a@example.com": grant("a@example.com", { revision: 3, policy: { tokenPeriod: "lifetime", parallelLimit: 2 } }),
    "b@example.com": grant("b@example.com", { revision: 9, policy: { tokenPeriod: "lifetime", parallelLimit: 4 } }),
    "c@example.com": grant("c@example.com", { revision: 1 }),
  });
  const draft = buildShareEditDraft(share);
  for (const email of ["a@example.com", "b@example.com"]) {
    draft.userGrants[email] = { ...draft.userGrants[email], policy: { tokenPeriod: "lifetime", parallelLimit: 8 } };
  }
  const patch = buildShareEditPatch(draft, share, ["claude"]);
  assert.deepEqual(Object.keys(patch), ["userGrantEdits"]);
  assert.deepEqual(Object.keys(patch.userGrantEdits!), ["a@example.com", "b@example.com"]);
  assert.equal(patch.userGrantEdits?.["a@example.com"].expectedRevision, 3);
  assert.equal(patch.userGrantEdits?.["b@example.com"].expectedRevision, 9);
  assert.equal(patch.userUsageEdits, undefined);
});

test("revoking a user drops only that user's staged calibration", () => {
  const share = shareView({ "a@example.com": grant("a@example.com", { revision: 4 }) });
  const draft = buildShareEditDraft(share);
  draft.userUsageEdits["a@example.com"] = { action: "set", targetTokens: 10 };
  delete draft.userGrants["a@example.com"];
  assert.deepEqual(buildShareEditPatch(draft, share, ["claude"]), {
    userGrantEdits: { "a@example.com": { expectedRevision: 4, policy: null } },
  });
});
