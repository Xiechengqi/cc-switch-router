import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parseClientSellerSummary } from "./client-seller-summary";

const summary = { ownedClients: 1, listed: 0, rented: 0, history: 0, canSell: true };

test("seller summary accepts only whole counts and an explicit verdict", () => {
  assert.equal(parseClientSellerSummary(summary).canSell, true);
  for (const patch of [{ ownedClients: -1 }, { listed: 1.5 }, { rented: "1" }, { history: null }, { canSell: "true" }]) {
    assert.throws(() => parseClientSellerSummary({ ...summary, ...patch }));
  }
  assert.throws(() => parseClientSellerSummary(null));
});

test("My listings follows the server's verdict and falls back to the catalog", () => {
  const online = readFileSync(new URL("../components/dashboard/client-market/online-market.tsx", import.meta.url), "utf8");
  assert.match(online, /const canSell = authenticated && sellerSummary\?\.canSell === true;/);
  assert.match(online, /showSelling = canSell \|\| \(!sellingKnown && workspace === "selling"\)/);
  assert.match(online, /if \(view === workspace\) return;\s*selectWorkspace\("catalog"\);/);
});
