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
  // "全部" is never disabled; an empty continent is.
  assert.match(shop, /disabled=\{!selected && value !== "all" && members\.length === 0\}/);
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

test("Client listing dialog: Router select, no consent checkbox, no dividers, trial in hours", () => {
  const dialog = readFileSync(new URL("./client-listing-add-dialog.tsx", import.meta.url), "utf8");
  const terms = readFileSync(new URL("./client-listing-terms.tsx", import.meta.url), "utf8");
  assert.match(dialog, /<CompactSelect/);
  assert.doesNotMatch(dialog, /<select\b|type="checkbox"|border-t|shareMarketHref|listing\.change"/);
  assert.match(dialog, /acknowledged: true/);
  // A re-check keeps the last result instead of flashing back to "checking".
  assert.match(dialog, /const lastProbe = probe\?\.id === selectedId \? probe : null;/);
  assert.doesNotMatch(terms, /addTrial|setLimit|removeTrial|removeLimit|trialDays/);
  assert.match(terms, /trialHours: paid && term \? Number\(term\) : 0/);
  assert.match(terms, /noTrialPlaceholder/);
  const css = readFileSync(new URL("../../../app/globals.css", import.meta.url), "utf8");
  assert.match(css, /@layer base \{\s*a \{\s*color: inherit;/, "link colors must not override Tailwind utilities");
});

test("Client catalog: a Mine tab left of All shows the buyer's rentals and their history", () => {
  const shop = readFileSync(new URL("./online-shop.tsx", import.meta.url), "utf8");
  const catalog = readFileSync(new URL("./shop-catalog.tsx", import.meta.url), "utf8");
  const market = readFileSync(new URL("./online-market.tsx", import.meta.url), "utf8");
  // The Mine chip renders before the continent chips and is disabled with nothing rented.
  assert.match(shop, /\{mine \? \([\s\S]*?disabled=\{!mine\.selected && !mine\.enabled\}[\s\S]*?SHOP_CONTINENTS\.map\(/);
  assert.match(shop, /const selected = !mine\?\.selected && value === continent;/);
  assert.match(catalog, /mine\?\.selected \? \(\s*mine\.view\s*\) :/);
  assert.match(catalog, /clientMarket\.catalog\.mineDisabledHint/);
  // Guests and buyers without any rental cannot pick Mine.
  assert.match(market, /const mineEnabled = authenticated && !!rentalCounts && rentalCounts\.current \+ rentalCounts\.history > 0;/);
  // Rental history appears only under Mine.
  assert.match(market, /mine=\{\{[\s\S]*?<ClientRentalRecords historyOnly/);
  assert.equal(market.match(/<ClientRentalRecords/g)?.length, 1);
  assert.match(market, /\{authenticated && !mine \? \(\s*<MyClientRentals/);
});

test("every rental history block uses the shared Share-style section titled 租用历史", () => {
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");
  for (const path of [
    "./rental-records.tsx",
    "./seller-workspace.tsx",
    "../account-share-rentals.tsx",
    "../share-market/seller-rental-history.tsx",
  ]) {
    assert.match(read(path), /<RentalHistorySection\b/, `${path} must use RentalHistorySection`);
  }
  assert.match(read("./seller-workspace.tsx"), /<MarketPagination page=\{shownHistoryPage\}/);
  assert.match(read("../../../lib/client-market-shop-messages.ts"), /"lease\.history": "租用历史"/);
  assert.match(read("../../common/rental-history-section.tsx"), /className="text-sm font-semibold text-muted-foreground"/);
});

test("Share catalog: All sits right of Mine, history only under Mine, seller history in My listings", () => {
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");
  const filters = read("../share-market/market-listing-filters.tsx");
  assert.match(filters, /shareMarket\.catalog\.mine[\s\S]*?shareMarket\.catalog\.all/);
  const buyer = read("../share-market/buyer-catalog.tsx");
  assert.match(buyer, /mineEnabled=\{authed && \(rentedShareIds\.size > 0 \|\| historyCount > 0\)\}/);
  assert.match(buyer, /\{authed && mine \? \(\s*<AccountShareRentals\s+historyOnly/);
  assert.match(read("../share-market/owner-workspace.tsx"), /<ShareSellerRentalHistory\b/);
});
