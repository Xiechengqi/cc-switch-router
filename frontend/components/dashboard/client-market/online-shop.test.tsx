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
  assert.match(css, /\.button--primary \{\s*--button-bg: rgb\(var\(--router-card\)\);/);
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
  // Running rentals show only under Mine too; an order switches to it.
  assert.equal(market.match(/<MyClientRentals/g)?.length, 1);
  assert.match(market, /mine=\{\{[\s\S]*?<MyClientRentals/);
  assert.match(market, /setNewLease\(id\);[\s\S]*?setMineSelected\(true\);/);
});

test("the tenant's rental card follows the Share card and opens the console in the dashboard window", () => {
  const rentals = readFileSync(new URL("./my-rentals.tsx", import.meta.url), "utf8");
  const actions = readFileSync(new URL("./lease-actions.tsx", import.meta.url), "utf8");
  assert.match(rentals, /className=\{MARKET_SHARE_CARD_GRID_CLASS\}/);
  assert.match(rentals, /"grid min-w-0 scroll-mt-20 content-start gap-2\.5 rounded-xl border bg-white p-3 shadow-sm outline-none"/);
  assert.match(rentals, /openConsole\(\{ clientId: lease\.leaseId, kind: "console", url: lease\.clientUrl/);
  assert.match(rentals, /t\("dashboard\.clientConsole"\)/);
  // No new tab, setup checklist, IP-checker hint or details link.
  assert.doesNotMatch(rentals, /target="_blank"|Onboarding|clientMarket\.onboarding|ipHint|clientMarket\.mine\.details|clientMarketTenantHref/);
  // Workspace backups are gone from every rental action.
  assert.doesNotMatch(actions, /backup|lease\.downloaded/);
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
  assert.match(buyer, /const mineEnabled = authed && \(rentedShareIds\.size > 0 \|\| historyCount > 0\);/);
  assert.match(buyer, /mineEnabled=\{mineEnabled\}/);
  assert.match(buyer, /\{mine \? \(\s*<AccountShareRentals\s+historyOnly/);
  assert.match(read("../share-market/owner-workspace.tsx"), /<ShareSellerRentalHistory\b/);
});

test("continent and country counts follow the listings on screen, rented ones included unless rentable-only is on", () => {
  const shop = readFileSync(new URL("./online-shop.tsx", import.meta.url), "utf8");
  const catalog = readFileSync(new URL("./shop-catalog.tsx", import.meta.url), "utf8");
  // The cards come from the pool the toggle filters, so `total` is what the list shows.
  assert.match(catalog, /const pool = listed\.filter\(\(item\) => !rentableOnly \|\| onlineListingRentable\(item\)\);\s*const cards = sortShopCountries\(aggregateOnlineShopCountries\(pool\)/);
  assert.match(shop, /list\.reduce\(\(sum, card\) => sum \+ card\.total, 0\)/);
  assert.match(shop, /<span className=\{shopChipCount\(selected\)\}>\{card\.total\}<\/span>/);
  assert.doesNotMatch(shop, /\{card\.idle\}<\/span>|sum \+ card\.idle/);
});

test("both shops remember the \"Mine\" tab across a reload", () => {
  const market = readFileSync(new URL("./online-market.tsx", import.meta.url), "utf8");
  const buyer = readFileSync(new URL("../share-market/buyer-catalog.tsx", import.meta.url), "utf8");
  assert.match(market, /const \[mineSelected, setMineSelected\] = usePersistentState\(SHOP_MINE_KEY, false\);/);
  // Until the counts arrive a remembered "Mine" stays; a known-empty one falls back.
  assert.match(market, /const mine = mineSelected && authenticated && \(rentalCounts === null \|\| mineEnabled\);/);
  assert.match(buyer, /const \[mineChosen, setMine\] = usePersistentState\(SHARE_CATALOG_MINE_KEY, initialMine\);/);
  // A `?view=rentals` link still opens "Mine" over a remembered "All".
  assert.match(buyer, /if \(initialMine\) setMine\(true\);/);
  // A remembered "Mine" with nothing behind it shows the catalog, without forgetting the choice.
  assert.match(buyer, /const mine = mineChosen && mineEnabled;/);
});
