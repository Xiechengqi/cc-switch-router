import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { providerCategory, providerCategoryLabel, providerLogo } from "./provider-presentation";
import { shareProviderLogoEntries, ShareProviderLogo } from "@/components/dashboard/share-provider-logo";
import { listingFamilyTabs, listingMatchesFamily } from "@/components/dashboard/share-market/buyer-catalog-utils";
import type { PublicProviderIdentity, ShareMarketListing, ShareView } from "./types";

const fixtures = JSON.parse(readFileSync(new URL("../../assets/provider-identity-fixtures.json", import.meta.url), "utf8"));
const identity: PublicProviderIdentity = fixtures.find((f: { name: string }) => f.name === "family.minimax_cn").appProviders.codex[0].providerIdentity;

test("all provider families use the same frozen assets and categories", () => {
  for (const fixture of fixtures) {
    const apps = fixture.appProviders as Record<string, { providerIdentity: PublicProviderIdentity }[]>;
    const id = Object.values(apps)[0][0].providerIdentity;
    assert.equal(providerCategory(id), fixture.expectedCategory);
    assert.ok(providerLogo(id), fixture.name);
    assert.equal(providerLogo(id)?.color, "#00A67E");
  }
  assert.equal(providerCategoryLabel("openai", "zh-CN"), "OpenAI");
  assert.equal(providerCategoryLabel("anthropic", "en"), "Anthropic");
  assert.equal(providerCategoryLabel("minimax", "zh-CN"), "MiniMax");
});

test("one Bundle exposes one logo and one market category with multiple App APIs", () => {
  const share = { providerIdentity: identity, support: { claude: true, codex: true }, appType: "codex" } as ShareView;
  const listing = { providerIdentity: identity, providerFamily: "multi", providerFamilies: ["anthropic", "openai"], seats: [] } as unknown as ShareMarketListing;
  assert.equal(shareProviderLogoEntries(share).length, 1);
  assert.equal(shareProviderLogoEntries(share)[0].provider.providerIdentity?.icon, "minimax");
  assert.equal(listingMatchesFamily(listing, "minimax"), true);
  assert.equal(listingMatchesFamily(listing, "anthropic"), false);
  assert.equal(listingMatchesFamily(listing, "openai"), false);
  assert.deepEqual(listingFamilyTabs([listing]).map((tab) => tab.value), ["minimax"]);
});

test("saved logo/color override never changes category, unknown icon is neutral", () => {
  const override = { ...identity, icon: "OpenAI", iconColor: "#123456" };
  assert.equal(providerLogo(override)?.key, "openai");
  assert.equal(providerLogo(override)?.color, "#123456");
  assert.equal(providerCategory(override), "minimax");
  assert.equal(providerLogo({ ...identity, icon: "https://evil.example/icon.svg" }), null);
  assert.equal(providerLogo({ ...identity, icon: "__proto__" }), null);
  assert.equal(providerCategory({ ...identity, category: "__proto__" }), "unknown");
  assert.equal(providerCategory(undefined), "unknown");
});

test("rendering preserves inline colors, raster/color assets and safe fallbacks", () => {
  const render = (id: PublicProviderIdentity) => renderToStaticMarkup(createElement(ShareProviderLogo, { provider: { providerIdentity: id } }));
  assert.match(render({ ...identity, icon: "openai", iconColor: "#123456" }), /color:#123456/);
  assert.match(render({ ...identity, icon: "codebuddy" }), /data:image\/svg\+xml;base64,/);
  assert.match(render({ ...identity, icon: "cursor" }), /data:image\/png;base64,/);
  assert.match(render({ ...identity, icon: "cursor" }), /style="width:16px;height:16px"/);
  assert.doesNotMatch(render({ ...identity, icon: "<svg onload=evil>" }), /onload=evil/);
  assert.equal(providerLogo({ ...identity, icon: "openai", iconColor: undefined })?.color, "#111827");
});
