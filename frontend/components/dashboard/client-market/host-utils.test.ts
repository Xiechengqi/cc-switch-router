import assert from "node:assert/strict";
import test from "node:test";
import {clientMarketWorkspaceFromQuery,clientMarketTenantFromQuery,onlineShopSpecLine,
  onlineListingAvailability,onlineListingRentable,aggregateOnlineShopCountries,shopContinentForCountry,SHOP_CONTINENTS,
  sortShopListings,matchesShopQuery,endReasonKey} from "./host-utils";

test("seller and rental links select their workspace", () => {
  assert.equal(clientMarketWorkspaceFromQuery("selling", null), "selling");
  assert.equal(clientMarketWorkspaceFromQuery(null, "mine"), "selling");
  assert.equal(clientMarketWorkspaceFromQuery("rentals", null), "catalog");
  assert.equal(clientMarketWorkspaceFromQuery(null, null), "catalog");
  assert.equal(clientMarketWorkspaceFromQuery("audit", null), "audit");
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
  assert.equal(onlineListingRentable({ state: "listed", exclusiveManagement: true, occupied: false, healthy: true }), true);
  assert.equal(onlineListingRentable({ state: "listed", exclusiveManagement: true, occupied: true, healthy: true }), false);
  assert.equal(onlineListingRentable({ state: "listed", exclusiveManagement: true, occupied: false, healthy: false }), false);
  assert.equal(onlineListingRentable({ state: "listed", occupied: false, healthy: true }), false);
  const cards = aggregateOnlineShopCountries([
    { country: "jp", state: "listed", exclusiveManagement: true, occupied: false, healthy: true, cyclePriceMinor: 500 },
    { country: "JP", state: "listed", exclusiveManagement: true, occupied: false, healthy: true, cyclePriceMinor: 0 },
    { country: "JP", state: "listed", exclusiveManagement: true, occupied: true, healthy: true, cyclePriceMinor: 100 },
    { country: null, state: "listed", exclusiveManagement: true, occupied: false, healthy: true, cyclePriceMinor: 0 },
  ]);
  const japan = cards.find((card) => card.code === "JP");
  assert.equal(japan?.idle, 2);
  assert.equal(japan?.total, 3);
  assert.equal(japan?.hasFreeIdle, true);
  assert.equal(japan?.minMonthlyMinor, 500);
  assert.equal(cards.find((card) => card.code === "unknown")?.continent, "other");
});

test("region availability and starting price use the signed-in buyer's terms", () => {
  const listing = { country: "US", state: "listed", exclusiveManagement: true, occupied: false, healthy: true, cyclePriceMinor: 1200 };
  const [buyer] = aggregateOnlineShopCountries([listing]);
  const [owner] = aggregateOnlineShopCountries([{ ...listing, owned: true, ownerRental: true }]);
  const [legacy] = aggregateOnlineShopCountries([{ ...listing, owned: true }]);
  assert.equal(buyer.minMonthlyMinor, 1200);
  assert.equal(buyer.hasFreeIdle, false);
  assert.equal(owner.idle, 1);
  assert.equal(owner.hasFreeIdle, true);
  assert.equal(legacy.total, 1);
  assert.equal(legacy.idle, 0);
  assert.equal(legacy.hasFreeIdle, false);
  assert.equal(legacy.minMonthlyMinor, null);
});

test("cards use the shared availability reason and apply self-rental eligibility separately", () => {
  const listing = { state: "listed", occupied: false, healthy: false, exclusiveManagement: true };
  assert.equal(onlineListingAvailability(listing), "unavailable");
  for (const availability of ["upgrading", "switching", "offline", "unavailable", "occupied"] as const) {
    assert.equal(onlineListingAvailability({ ...listing, availability }), availability);
  }
  assert.equal(onlineListingAvailability({ ...listing, availability: "available", owned: true }), "upgrade_required");
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

test("shop order puts rentable Clients first, then the longest connected, then the 30-day rate", () => {
  const base = { owned: false, ownerRental: true, exclusiveManagement: true, healthy: true, occupied: false, state: "listed" };
  const rows = [
    { ...base, installationId: "young", registeredAt: "2026-06-01T00:00:00Z", onlineRate30d: 99 },
    { ...base, installationId: "busy", registeredAt: "2025-01-01T00:00:00Z", availability: "occupied" as const, occupied: true },
    { ...base, installationId: "old-low", registeredAt: "2026-01-01T00:00:00Z", onlineRate30d: 80 },
    { ...base, installationId: "old-high", registeredAt: "2026-01-01T00:00:00Z", onlineRate30d: 95 },
    { ...base, installationId: "unknown-age" },
  ];
  assert.deepEqual(sortShopListings(rows).map((row) => row.installationId), ["old-high", "old-low", "young", "unknown-age", "busy"]);
});

test("shop search matches title, public name, seller and country, case-insensitively", () => {
  const listing = { title: "Tokyo Home", label: "client-8f3a21c0", sellerEmail: "seller@example.test", country: "JP" };
  const name = (code: string) => (code === "JP" ? "Japan" : code);
  for (const query of ["", "tokyo", "8F3A", "SELLER@", "jp", "japan"]) assert.equal(matchesShopQuery(listing, query, name), true, query);
  assert.equal(matchesShopQuery(listing, "germany", name), false);
  assert.equal(matchesShopQuery({ ...listing, country: null }, "unknown", name), false);
});

test("rental end reasons include seller reclaim and completed trials", () => {
  assert.equal(endReasonKey("provider_reclaim"), "clientMarket.endReason.provider_reclaim");
  assert.equal(endReasonKey("trial_completed"), "clientMarket.endReason.trial_completed");
  assert.equal(endReasonKey("voluntary"), "clientTermination.voluntary");
  assert.equal(endReasonKey("something_new"), "clientTermination.systemExit");
  assert.equal(endReasonKey(null), "clientTermination.systemExit");
});
