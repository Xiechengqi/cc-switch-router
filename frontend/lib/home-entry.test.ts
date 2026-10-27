import assert from "node:assert/strict";
import test from "node:test";
import { defaultDashboardRouteFromSearch } from "./dashboard-nav";
import { isGatedHomePath, isHomeTaskAddress, parseHomeEntryStatus, pathForHomeEntry } from "./home-entry";

test("only the bare home and clients addresses can show the start page", () => {
  assert.equal(isGatedHomePath("/"), true);
  assert.equal(isGatedHomePath("/clients"), true);
  assert.equal(isGatedHomePath("/clients/"), true);
  assert.equal(isGatedHomePath("/client-market/"), false);
  assert.equal(isGatedHomePath("/operations/"), false);
  assert.equal(isHomeTaskAddress("/", ""), false);
  assert.equal(isHomeTaskAddress("/clients/", ""), false);
  assert.equal(isHomeTaskAddress("/", "?drawerKind=market"), true);
  assert.equal(isHomeTaskAddress("/account/wallet/", "?topup=1"), true);
  assert.equal(defaultDashboardRouteFromSearch("?topup=1"), "/account/wallet/");
  assert.equal(defaultDashboardRouteFromSearch("?c2cSell=1"), "/account/wallet/");
  assert.equal(defaultDashboardRouteFromSearch("?drawerKind=market"), "/share-market/");
  assert.equal(defaultDashboardRouteFromSearch(""), "/clients/");
});

test("a saved entry maps to its page and rejects an unknown response", () => {
  assert.equal(pathForHomeEntry("wallet"), "/account/wallet/");
  assert.deepEqual(parseHomeEntryStatus({ hasActivity: false, entry: "providers", source: "choice" }), {
    hasActivity: false,
    entry: "providers",
    source: "choice",
  });
  assert.equal(parseHomeEntryStatus({ hasActivity: false, entry: "elsewhere", source: "choice" }), null);
  assert.equal(parseHomeEntryStatus({ hasActivity: true }), null);
});
