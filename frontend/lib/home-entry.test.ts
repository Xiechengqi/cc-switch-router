import assert from "node:assert/strict";
import test from "node:test";
import { defaultDashboardRouteFromSearch } from "./dashboard-nav";
import { HOME_ENTRIES, isGatedHomePath, isHomeTaskAddress, parseHomeEntryStatus, pathForHomeEntry, resolveHomeGate } from "./home-entry";

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

test("explicit Clients navigation never returns to a saved market entry, with or without a trailing slash", () => {
  for (const pathname of ["/clients", "/clients/"]) {
    for (const entry of HOME_ENTRIES) {
      assert.deepEqual(resolveHomeGate({ pathname, search: "", authed: true, loading: false, home: { hasActivity: false, entry, source: "choice" } }), {
        screen: "content", redirect: null, needsHome: true,
      });
    }
    for (const authed of [false, true]) {
      const result = resolveHomeGate({ pathname, search: "", authed, loading: false, home: "failed" });
      assert.equal(result.screen, "content");
      assert.equal(result.redirect, null);
    }
    assert.equal(resolveHomeGate({ pathname, search: "", authed: true, loading: false, home: { hasActivity: true, entry: null, source: null } }).screen, "content");
  }
});

test("only the bare root applies saved entry preferences and safe fallbacks", () => {
  for (const entry of HOME_ENTRIES) assert.equal(resolveHomeGate({ pathname: "/", search: "", authed: true, loading: false, home: { hasActivity: true, entry, source: "choice" } }).redirect, pathForHomeEntry(entry));
  for (const authed of [false, true]) assert.equal(resolveHomeGate({ pathname: "/", search: "", authed, loading: false, home: "failed" }).redirect, "/clients/");
  assert.equal(resolveHomeGate({ pathname: "/client-market/", search: null, authed: true, loading: false, home: null }).screen, "content");
});

test("first-use guidance, loading and task deep links retain their independent behavior", () => {
  const base = { pathname: "/", search: "", authed: true, loading: false, home: null } as const;
  assert.deepEqual(resolveHomeGate(base), { screen: "loading", redirect: null, needsHome: true });
  assert.equal(resolveHomeGate({ ...base, loading: true }).needsHome, false);
  assert.equal(resolveHomeGate({ ...base, search: null }).needsHome, false);
  for (const pathname of ["/", "/clients", "/clients/"]) {
    assert.equal(resolveHomeGate({ ...base, pathname, home: { hasActivity: false, entry: null, source: null } }).screen, "start");
  }
  assert.equal(resolveHomeGate({ ...base, search: "?topup=1" }).redirect, "/account/wallet/?topup=1");
  assert.equal(resolveHomeGate({ ...base, search: "?drawerKind=market" }).redirect, "/share-market/?drawerKind=market");
  assert.deepEqual(resolveHomeGate({ ...base, pathname: "/clients/", search: "?focusId=one" }), { screen: "content", redirect: null, needsHome: false });
});
