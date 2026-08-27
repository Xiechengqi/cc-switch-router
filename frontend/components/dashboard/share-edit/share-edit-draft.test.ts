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

test("buildShareEditPatch keeps active Share Market grants and ignores tombstones", () => {
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
  assert.equal(patch.userGrants?.["active@example.com"]?.manager, "routerShareMarket");
  assert.equal(patch.userGrants?.["active@example.com"]?.entitlementId, "entitlement-active");
  assert.equal(patch.userGrants?.["revoked@example.com"], undefined);
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
  const restored = patch.userGrants?.["renter@example.com"];
  assert.equal(restored?.active, true);
  assert.equal(restored?.role, "shareto");
  assert.equal(restored?.manager, "manual");
  assert.equal(restored?.entitlementId, undefined);
  assert.equal(restored?.revokedAtMs, undefined);
  assert.equal(restored?.policy.tokenLimit, 800);
});
