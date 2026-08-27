import assert from "node:assert/strict";
import test from "node:test";

import {
  buildOrdinaryUserGrantsPatch,
  buildShareSettingsPatch,
  draftFromShare,
  isRevokedRouterShareMarketGrant,
  isRouterShareMarketManagedGrant,
  ordinaryShareUserGrant,
  routerShareMarketManagedEmails,
} from "./share-settings";
import type { ShareUserGrant, ShareView } from "./types";

function grant(
  email: string,
  overrides: Partial<ShareUserGrant> = {},
): ShareUserGrant {
  return {
    email,
    role: "shareto",
    active: true,
    policy: { tokenPeriod: "lifetime" },
    ...overrides,
  };
}

test("active Share Market grants stay read-only", () => {
  const active = grant("renter@example.com", { manager: "routerShareMarket" });
  assert.equal(isRouterShareMarketManagedGrant(active), true);
});

test("revoked Share Market tombstones are not treated as managed users", () => {
  const tombstone = grant("renter@example.com", {
    active: false,
    manager: "routerShareMarket",
    entitlementId: "entitlement-revoked",
  });
  assert.equal(isRouterShareMarketManagedGrant(tombstone), false);
});

test("routerShareMarketManagedEmails excludes inactive tombstones", () => {
  const emails = routerShareMarketManagedEmails({
    "owner@example.com": grant("owner@example.com", {
      role: "owner",
      manager: "owner",
    }),
    "active@example.com": grant("active@example.com", {
      manager: "routerShareMarket",
    }),
    "revoked@example.com": grant("revoked@example.com", {
      active: false,
      manager: "routerShareMarket",
      entitlementId: "entitlement-revoked",
    }),
  });
  assert.deepEqual([...emails].sort(), ["active@example.com"]);
});

test("ordinaryShareUserGrant strips market identity from a tombstone", () => {
  const tombstone = grant("renter@example.com", {
    active: false,
    manager: "routerShareMarket",
    entitlementId: "entitlement-revoked",
    revokedAtMs: 1,
    policy: { tokenPeriod: "lifetime", tokenLimit: 500 },
  });
  assert.equal(isRevokedRouterShareMarketGrant(tombstone), true);
  const next = ordinaryShareUserGrant(
    "renter@example.com",
    "owner@example.com",
    tombstone,
    { tokenPeriod: "lifetime", tokenLimit: 800 },
  );
  assert.equal(next.active, true);
  assert.equal(next.role, "shareto");
  assert.equal(next.manager, "manual");
  assert.equal(next.entitlementId, undefined);
  assert.equal(next.revokedAtMs, undefined);
  assert.equal(next.policy.tokenLimit, 800);
});

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

test("buildOrdinaryUserGrantsPatch keeps active market grants and drops tombstones", () => {
  const patch = buildOrdinaryUserGrantsPatch(
    {
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
    },
    "owner@example.com",
    { tokenPeriod: "lifetime" },
  );
  assert.equal(patch["active@example.com"]?.manager, "routerShareMarket");
  assert.equal(patch["revoked@example.com"], undefined);
  assert.equal(patch["owner@example.com"]?.manager, "owner");
});

test("share-page patch writes a manual shareto over a revoked market tombstone", () => {
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
  const draft = draftFromShare(share);
  draft.userGrants["renter@example.com"] = ordinaryShareUserGrant(
    "renter@example.com",
    "owner@example.com",
    draft.userGrants["renter@example.com"],
    { tokenPeriod: "lifetime", tokenLimit: 800 },
  );
  const patch = buildShareSettingsPatch(draft, share);
  const restored = patch.userGrants?.["renter@example.com"];
  assert.equal(restored?.active, true);
  assert.equal(restored?.manager, "manual");
  assert.equal(restored?.entitlementId, undefined);
  assert.equal(restored?.revokedAtMs, undefined);
  assert.equal(restored?.policy.tokenLimit, 800);
});
