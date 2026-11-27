import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("shop filters and cards use Tailwind classes instead of stripped custom CSS", () => {
  const shop = readFileSync(new URL("./online-shop.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("../../../app/globals.css", import.meta.url), "utf8");
  assert.match(shop, /justify-between/);
  assert.doesNotMatch(shop, /clientMarket\.shop\.mine/);
  // Share Market toolbar: gray / sky chips at h-9, continents always listed
  // (disabled when empty), the toggle, then the compact search at the far right.
  assert.match(shop, /inline-flex h-9 shrink-0 items-center gap-1\.5 rounded-lg px-2\.5 text-xs/);
  assert.match(shop, /bg-sky-100 font-semibold text-sky-800/);
  assert.match(shop, /SHOP_CONTINENTS\.map\(/);
  assert.match(shop, /disabled=\{!selected && members\.length === 0\}/);
  assert.match(shop, /<CircleDot[\s\S]*?\{rentable\.label\}[\s\S]*?<label className="flex h-9 w-44/);
  // Country chips of the chosen continent stay on one row and scroll sideways.
  assert.match(shop, /flex min-w-0 gap-1 overflow-x-auto/);
  assert.match(shop, /countries\.length > 0/);
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

test("Client market buttons never use a blue fill and in-page controls stay at h-9", () => {
  const files = ["online-shop.tsx", "shop-catalog.tsx", "my-rentals.tsx", "seller-workspace.tsx", "online-market.tsx", "lease-actions.tsx"];
  for (const file of files) {
    const text = readFileSync(new URL(`./${file}`, import.meta.url), "utf8");
    assert.doesNotMatch(text, /from-accent|to-\[#4D7CFF\]|\bbg-(?:accent|primary)\b(?!\/)/, `${file} must not fill a button with blue`);
  }
  const page = ["online-shop.tsx", "shop-catalog.tsx", "my-rentals.tsx", "seller-workspace.tsx", "online-market.tsx"];
  for (const file of page) {
    const text = readFileSync(new URL(`./${file}`, import.meta.url), "utf8");
    assert.doesNotMatch(text, /\b(?:min-)?h-11\b/, `${file} in-page controls are h-9`);
  }
  const css = readFileSync(new URL("../../../app/globals.css", import.meta.url), "utf8");
  assert.match(css, /\.button--primary \{\s*--button-bg: rgb\(var\(--router-foreground\)\);/);
  const catalog = readFileSync(new URL("./shop-catalog.tsx", import.meta.url), "utf8");
  assert.match(catalog, /continent === "all" \|\| continentOf\(shopCountryKey\(item\.country\)\) === continent/);
});
