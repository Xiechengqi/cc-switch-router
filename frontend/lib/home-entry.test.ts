import assert from "node:assert/strict";
import test from "node:test";
import { existsSync, readFileSync } from "node:fs";
import { isClientsRoute, DASHBOARD_CLIENTS_PATH, defaultDashboardRouteFromSearch } from "./dashboard-nav";
import { legacyClientsHref, parseUiContext, rootTaskHref } from "./home-entry";

test("Clients is the real root page; exact legacy aliases retain queries and anchors", () => {
  assert.equal(DASHBOARD_CLIENTS_PATH, "/");
  assert.equal(defaultDashboardRouteFromSearch(""), "/");
  for (const path of ["/clients", "/clients/"]) {
    assert.equal(legacyClientsHref(path, "?chat=i&marketShare=s", "#details"), "/?chat=i&marketShare=s#details");
    assert.equal(isClientsRoute(path), true);
  }
  for (const path of ["/client-market/", "/clients-extra", "/clients/child"]) {
    assert.equal(legacyClientsHref(path), null);
    assert.equal(isClientsRoute(path), false);
  }
  assert.equal(isClientsRoute("/"), true);
  assert.match(readFileSync(new URL("../app/(dashboard)/page.tsx", import.meta.url), "utf8"), /ClientsPage/);
  assert.equal(existsSync(new URL("../app/page.tsx", import.meta.url)), false);
  assert.equal(existsSync(new URL("../components/start/start-gate.tsx", import.meta.url)), false);
});
test("only explicit task links dispatch away from the root", () => {
  for (const search of ["", "?tab=mine", "?chat=i", "?marketShare=s&seatId=p", "?drawerKind=share&drawerId=s"]) assert.equal(rootTaskHref(search), null);
  assert.equal(rootTaskHref("?amount=10&returnTo=%2Fshare-market%2F"), "/account/wallet/?amount=10&returnTo=%2Fshare-market%2F");
  assert.equal(rootTaskHref("?topup=t"), "/account/wallet/?topup=t");
  assert.equal(rootTaskHref("?c2cOrder=o"), "/account/wallet/?c2cOrder=o");
  assert.equal(rootTaskHref("?drawerKind=market&focus=s"), "/share-market/?drawerKind=market&focus=s");
});
test("host context fails closed and never mistakes an outage for a Router host", () => {
  assert.deepEqual(parseUiContext({ mode: "router" }), { mode: "router" });
  assert.deepEqual(parseUiContext({ mode: "share", shareId: "s", subdomain: "owner--s" }), { mode: "share", shareId: "s", subdomain: "owner--s" });
  for (const value of [null, {}, {mode:"client"}, {mode:"share"}, {mode:"share",shareId:"",subdomain:"x"}]) assert.throws(() => parseUiContext(value));
});
