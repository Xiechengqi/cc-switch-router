import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("shop filters and cards use Tailwind classes instead of stripped custom CSS", () => {
  const shop = readFileSync(new URL("./online-shop.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("../../../app/globals.css", import.meta.url), "utf8");
  assert.match(shop, /justify-between/);
  assert.match(shop, /ml-auto inline-flex h-11 shrink-0/);
  assert.doesNotMatch(shop, /clientMarket\.shop\.mine/);
  assert.match(shop, /rounded-full/);
  // Country chips stay on one row and scroll sideways on narrow screens.
  assert.match(shop, /flex min-w-0 gap-1\.5 overflow-x-auto/);
  assert.match(shop, /cards\.length > 0/);
  assert.match(shop, /ONLINE_CLIENT_CARD_GRID_CLASS/);
  assert.match(shop, /grid-cols-1 gap-3 sm:grid-cols-2 md:grid-cols-3 xl:grid-cols-4/);
  assert.match(shop, /clientMarket\.shop\.sellingSearch/);
  assert.match(shop, /clientMarket\.shop\.filter\.listed/);
  assert.match(shop, /clientMarket\.shop\.listAdd/);
  assert.doesNotMatch(shop, /filter\.listable/);
  assert.doesNotMatch(shop, /canAdd/);
  assert.doesNotMatch(css, /client-market-shop-continent/);
});

test("shop cards lead with the egress and never show the tenant", () => {
  const shop = readFileSync(new URL("./online-shop.tsx", import.meta.url), "utf8");
  assert.match(shop, /<header[\s\S]*?<ClientEgressLine listing=\{listing\} emphasize \/>/);
  assert.doesNotMatch(shop, /clientMarket\.shop\.network/);
  assert.doesNotMatch(shop, /tenantEmail/);
  assert.match(shop, /clientDisplayName\(listing\)/);
});
