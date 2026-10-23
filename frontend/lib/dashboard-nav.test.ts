import assert from "node:assert/strict";
import test from "node:test";
import { existsSync, readFileSync } from "node:fs";
import {
  clientMarketHref,
  normalizeDashboardPath,
  pathnameForDashboardShell,
  shareMarketHref,
  accountNavHrefFromPathname, isAccountNavHref, PROVIDER_INCOME_HREF,
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

test("platform balance has a permanent account destination", () => {
  assert.equal(normalizeDashboardPath("/account/wallet/"), "/account/wallet/");
  assert.equal(pathnameForDashboardShell("/account/wallet/"), "account");
});

test("funds, Provider settings and compatibility routes have distinct destinations", () => {
  assert.equal(normalizeDashboardPath("/account/earnings/"), null);
  assert.equal(normalizeDashboardPath("/account/provider-settings/"), "/account/provider-settings/");
  for (const old of ["payments", "market-readiness", "market-access"]) {
    assert.equal(normalizeDashboardPath(`/account/${old}/`), "/account/provider-settings/");
  }
  assert.equal(normalizeDashboardPath("/account/billing/"), "/account/wallet/");
});
test("Provider income has no retired route or navigation compatibility shim", () => {
  assert.equal(PROVIDER_INCOME_HREF, "/account/provider-settings/#income");
  assert.equal(isAccountNavHref("/account/earnings/"), false);
  assert.equal(accountNavHrefFromPathname("/account/earnings/"), null);
  assert.equal(existsSync(new URL("../app/(dashboard)/account/earnings/page.tsx", import.meta.url)), false);
  assert.equal(existsSync(new URL("../components/dashboard/account-earnings-page.tsx", import.meta.url)), false);
  assert.doesNotMatch(readFileSync(new URL("../components/dashboard/account-shell.tsx", import.meta.url), "utf8"), /getMarketProviderFinance|account\.nav\.earnings|hasEarnings/);
});
