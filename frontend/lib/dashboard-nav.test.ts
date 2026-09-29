import assert from "node:assert/strict";
import test from "node:test";
import {
  clientMarketHref,
  normalizeDashboardPath,
  pathnameForDashboardShell,
  shareMarketHref,
} from "./dashboard-nav";

test("Market Provider deep links use opaque query values without losing other scope", () => {
  const share = new URL(shareMarketHref({
    workspace: "rentals",
    shareId: "share/one",
    providerId: "mp_public/value",
  }), "https://router.test");
  assert.equal(share.pathname, "/share-market/");
  assert.equal(share.searchParams.get("view"), "rentals");
  assert.equal(share.searchParams.get("focus"), "share/one");
  assert.equal(share.searchParams.get("provider"), "mp_public/value");

  const client = new URL(
    clientMarketHref({ providerId: "mp_public/value" }),
    "https://router.test",
  );
  assert.equal(client.pathname, "/client-market/");
  assert.equal(client.searchParams.get("provider"), "mp_public/value");
});

test("Provider Rank is a first-class dashboard destination", () => {
  assert.equal(normalizeDashboardPath("/providers/"), "/providers/");
  assert.equal(pathnameForDashboardShell("/providers/?provider=mp_public"), "providers");
});
