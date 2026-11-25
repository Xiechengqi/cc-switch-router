import assert from "node:assert/strict";
import test from "node:test";
import {clientMarketWorkspaceFromQuery,clientMarketTenantFromQuery,onlineShopSpecLine,
  onlineListingRentable,aggregateOnlineShopCountries,shopContinentForCountry,SHOP_CONTINENTS} from "./host-utils";

test("seller and rental links select their workspace", () => {
  assert.equal(clientMarketWorkspaceFromQuery("selling", null), "selling");
  assert.equal(clientMarketWorkspaceFromQuery(null, "mine"), "selling");
  assert.equal(clientMarketWorkspaceFromQuery("rentals", null), "catalog");
  assert.equal(clientMarketWorkspaceFromQuery(null, null), "catalog");
  assert.equal(clientMarketTenantFromQuery("rentals", null), true);
  assert.equal(clientMarketTenantFromQuery(null, "rental-1"), true);
  assert.equal(clientMarketTenantFromQuery("selling", null), false);
});

test("online shop spec line uses GiB memory and leaves disk off the card", () => {
  assert.equal(onlineShopSpecLine(undefined), "");
  assert.equal(onlineShopSpecLine({ arch: "x86_64", cpu: 4, memoryBytes: 3.8 * 2 ** 30 }), "x86_64 · 4C · 3.8G");
  assert.equal(onlineShopSpecLine({ arch: "aarch64", cpu: null, memoryBytes: null }), "aarch64");
});

test("online shop cards count rentable listings and ignore occupied or offline ones", () => {
  assert.equal(onlineListingRentable({ state: "listed", occupied: false, healthy: true }), true);
  assert.equal(onlineListingRentable({ state: "listed", occupied: true, healthy: true }), false);
  assert.equal(onlineListingRentable({ state: "listed", occupied: false, healthy: false }), false);
  const cards = aggregateOnlineShopCountries([
    { country: "jp", state: "listed", occupied: false, healthy: true, cyclePriceMinor: 500 },
    { country: "JP", state: "listed", occupied: false, healthy: true, cyclePriceMinor: 0 },
    { country: "JP", state: "listed", occupied: true, healthy: true, cyclePriceMinor: 100 },
    { country: null, state: "listed", occupied: false, healthy: true, cyclePriceMinor: 0 },
  ]);
  const japan = cards.find((card) => card.code === "JP");
  assert.equal(japan?.idle, 2);
  assert.equal(japan?.total, 3);
  assert.equal(japan?.hasFreeIdle, true);
  assert.equal(japan?.minMonthlyMinor, 500);
  assert.equal(cards.find((card) => card.code === "unknown")?.continent, "other");
});

test("shop continents keep ISO regions distinct and group unknown codes as other", () => {
  assert.equal(shopContinentForCountry("HK"), "asia");
  assert.equal(shopContinentForCountry("MO"), "asia");
  assert.equal(shopContinentForCountry("TW"), "asia");
  assert.equal(shopContinentForCountry("CN"), "asia");
  assert.equal(shopContinentForCountry("au"), "oceania");
  assert.equal(shopContinentForCountry("US"), "north_america");
  assert.equal(shopContinentForCountry("BR"), "south_america");
  assert.equal(shopContinentForCountry("DE"), "europe");
  assert.equal(shopContinentForCountry("ZA"), "africa");
  assert.equal(shopContinentForCountry("AQ"), "other");
  assert.equal(shopContinentForCountry(""), "other");
  assert.equal(shopContinentForCountry("CHN"), "other");
  assert.equal(SHOP_CONTINENTS[0], "all");
});
