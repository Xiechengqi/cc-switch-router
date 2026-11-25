import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("shop pills use Tailwind classes instead of stripped custom CSS", () => {
  const shop = readFileSync(new URL("./online-shop.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("../../../app/globals.css", import.meta.url), "utf8");
  assert.match(shop, /justify-between/);
  assert.match(shop, /ml-auto inline-flex h-8 shrink-0/);
  assert.match(shop, /clientMarket\.shop\.mine/);
  assert.match(shop, /rounded-full/);
  assert.match(shop, /flex min-w-0 flex-wrap gap-1\.5/);
  assert.match(shop, /w-\[7\.25rem\]/);
  assert.match(shop, /cards\.length === 0/);
  assert.match(shop, /ONLINE_CLIENT_CARD_GRID_CLASS/);
  assert.match(shop, /grid-cols-1 gap-3 sm:grid-cols-2 md:grid-cols-3 xl:grid-cols-4/);
  assert.match(shop, /clientMarket\.shop\.sellingSearch/);
  assert.match(shop, /clientMarket\.shop\.filter\.\$\{item\}/);
  assert.doesNotMatch(css, /client-market-shop-continent/);
});
