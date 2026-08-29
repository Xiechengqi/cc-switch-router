import assert from "node:assert/strict";
import test from "node:test";
import {
  catalogSeatPreview,
  filterMarketListings,
  filterMergedCatalogListings,
  initialCatalogSeat,
  listingFamilyTabs,
  MARKET_CATALOG_PAGE_SIZE,
  MARKET_RENTAL_HISTORY_PAGE_SIZE,
  marketShareCardSeatPreview,
  mergeCatalogWithRentedListings,
  pageForShareId,
  paginateListings,
  preserveCatalogSeat,
  rentedShareIdsFromSubscriptions,
  sortMergedCatalogListings,
} from "./buyer-catalog-utils";
import {
  integrityReasonText,
  marketProviderStatusView,
  primaryMarketCapability,
} from "./market-utils";
import { mergeShareMarketSubscriptionPage } from "./subscription-utils";
import type {
  ShareMarketAppCapability,
  ShareMarketListing,
  ShareMarketSubscription,
} from "@/lib/types";

const seat = (id: string, status = "available", readOnly = false) => ({ id, status, readOnly });

test("multiple idle seats require an explicit selection", () => {
  assert.equal(initialCatalogSeat([seat("a"), seat("b")]), undefined);
  assert.equal(initialCatalogSeat([seat("a")])?.id, "a");
});

test("a selected seat is preserved by id without falling back", () => {
  assert.equal(preserveCatalogSeat([seat("b")], "a"), undefined);
  assert.equal(preserveCatalogSeat([seat("a"), seat("b")], "b")?.id, "b");
});

test("compact cards preview at most two idle seats", () => {
  assert.deepEqual(
    catalogSeatPreview([seat("a"), seat("busy", "occupied"), seat("b"), seat("c")]).map((item) => item.id),
    ["a", "b"],
  );
});

test("workspace cards keep at most two seat rows and prefer the caller's seats", () => {
  const seats = [seat("a"), seat("busy", "occupied"), seat("b"), seat("c")];
  const rented = marketShareCardSeatPreview(seats, ["busy"]);
  assert.deepEqual(rented.preview.map((item) => item.id), ["busy", "a"]);
  assert.equal(rented.hiddenCount, 2);
  assert.equal(rented.idleHiddenCount, 2);
  const listed = marketShareCardSeatPreview(seats);
  assert.deepEqual(listed.preview.map((item) => item.id), ["a", "b"]);
  assert.equal(listed.hiddenCount, 2);
  assert.equal(listed.idleCount, 3);
});

test("family and search filters match catalog listing fields", () => {
  const listings = [
    {
      id: "openai",
      shareName: "alpha",
      subdomain: "alpha-route",
      ownerEmail: "owner@example.com",
      providerFamily: "openai",
      providerFamilies: ["openai"],
      supportedApps: ["codex"],
      appCapabilities: [{ providerName: "OpenAI Official", models: ["gpt-5"] }],
      seats: [{ status: "available", readOnly: false }],
    },
    {
      id: "anthropic",
      shareName: "bravo",
      subdomain: "bravo-route",
      ownerEmail: "other@example.com",
      providerFamily: "anthropic",
      providerFamilies: ["anthropic"],
      supportedApps: ["claude"],
      appCapabilities: [{ providerName: "Anthropic", models: ["opus"] }],
      seats: [{ status: "occupied", readOnly: false }],
    },
  ] as ShareMarketListing[];

  assert.deepEqual(listingFamilyTabs(listings).map((item) => item.value), ["anthropic", "openai"]);
  assert.deepEqual(filterMarketListings(listings, "openai", "").map((item) => item.id), ["openai"]);
  assert.deepEqual(filterMarketListings(listings, "all", "opus").map((item) => item.id), ["anthropic"]);
});

const capability = (
  overrides: Partial<ShareMarketAppCapability> = {},
): ShareMarketAppCapability => ({
  app: "codex",
  providerFamily: "openai",
  providerName: "OpenAI Official",
  providerType: "codex_oauth",
  modelMode: "fixed",
  upstreamModel: "gpt-5",
  models: ["gpt-5"],
  available: true,
  healthState: "healthy",
  ...overrides,
});

test("provider panel prefers a capability with public account status", () => {
  const detailed = capability({
    app: "codex",
    accountHint: "private@example.com",
    quota: {
      status: "ok",
      plan: "Plus",
      tiers: [{ label: "weekly", utilization: 55 }],
    },
  });
  assert.equal(
    primaryMarketCapability([
      capability({ app: "claude", providerName: "Anthropic" }),
      detailed,
    ]),
    detailed,
  );
});

test("provider panel reproduces quota, provider identity and enabled app model rows", () => {
  const view = marketProviderStatusView(
    {
      supportedApps: ["claude", "codex"],
      appCapabilities: [
        capability({
          accountHint: "private@example.com",
          quota: {
            status: "ok",
            plan: "Plus",
            tiers: [{ label: "weekly", utilization: 55 }],
          },
        }),
        capability({
          app: "claude",
          providerFamily: "xai",
          providerName: "Grok OAuth",
          providerType: "grok_oauth",
          upstreamModel: "grok-4.6",
          models: ["grok-4.6"],
        }),
      ],
    },
    "en",
    { unknown: "Unknown", passthrough: "Passthrough" },
  );
  assert.match(view.primaryLine, /Plus/);
  assert.match(view.primaryLine, /weekly 55%/i);
  assert.equal(view.identityLine, "private@example.com");
  assert.match(view.modelsLine, /Claude: grok-4\.6/);
  assert.match(view.modelsLine, /Codex: gpt-5/);
  assert.match(view.toneClassName, /emerald/);
});

test("provider panel keeps degraded and API providers safe", () => {
  const view = marketProviderStatusView(
    {
      supportedApps: ["codex"],
      appCapabilities: [
        capability({
          providerFamily: "api",
          providerName: "Private API",
          providerType: "openai_compatible",
          accountHint: undefined,
          healthState: "degraded",
        }),
      ],
    },
    "en",
    { unknown: "Unknown", passthrough: "Passthrough" },
  );
  assert.equal(view.primaryLine, "Private API");
  assert.equal(view.identityLine, "-");
  assert.match(view.toneClassName, /amber/);
  assert.doesNotMatch(JSON.stringify(view), /https?:\/\//);
});

test("provider panel uses subscription level and stable app order without quota", () => {
  const view = marketProviderStatusView(
    {
      supportedApps: ["claude", "codex"],
      appCapabilities: [
        capability({ app: "codex", subscriptionLevel: "Pro" }),
        capability({
          app: "claude",
          providerFamily: "anthropic",
          providerName: "Anthropic",
          providerType: "claude_oauth",
          upstreamModel: "claude-opus-4-1",
          models: ["claude-opus-4-1"],
        }),
      ],
    },
    "en",
    { unknown: "Unknown", passthrough: "Passthrough" },
  );
  assert.equal(view.primaryLine, "Pro");
  assert.ok(view.modelsLine.indexOf("Claude:") < view.modelsLine.indexOf("Codex:"));
});

test("provider panel excludes bound apps whose Share API is disabled", () => {
  const view = marketProviderStatusView(
    {
      supportedApps: ["codex"],
      appCapabilities: [
        capability(),
        capability({
          app: "claude",
          providerFamily: "xai",
          providerName: "Grok OAuth",
          providerType: "grok_oauth",
          upstreamModel: "grok-4.6",
          models: ["grok-4.6"],
        }),
      ],
    },
    "en",
    { unknown: "Unknown", passthrough: "Passthrough" },
  );

  assert.match(view.modelsLine, /Codex: gpt-5/);
  assert.doesNotMatch(view.modelsLine, /Claude:/);
});

test("provider panel survives omitted models on passthrough listings", () => {
  const view = marketProviderStatusView(
    {
      supportedApps: ["claude"],
      appCapabilities: [
        capability({
          app: "claude",
          providerFamily: "anthropic",
          providerName: "Claude OAuth",
          providerType: "claude_oauth",
          subscriptionLevel: "Claude Pro",
          modelMode: "passthrough",
          upstreamModel: undefined,
          models: undefined,
        }),
      ],
    },
    "en",
    { unknown: "Unknown", passthrough: "Passthrough" },
  );

  assert.equal(view.modelsLine, "Claude: Passthrough");
  assert.equal(view.identityLine, "Claude OAuth");
  assert.equal(view.primaryLine, "Claude Pro");
});

test("provider panel survives omitted models on fixed listings without an upstream model", () => {
  const view = marketProviderStatusView(
    {
      supportedApps: ["codex"],
      appCapabilities: [
        capability({
          modelMode: "fixed",
          upstreamModel: undefined,
          models: undefined,
        }),
      ],
    },
    "en",
    { unknown: "Unknown", passthrough: "Passthrough" },
  );

  assert.equal(view.modelsLine, "Codex: Unknown");
  assert.equal(view.identityLine, "OpenAI Official");
});

test("contract integrity reasons are localized without exposing internal codes", () => {
  const text = integrityReasonText(
    "share_contract_upgrade_required,contract_apps_missing,contract_apps_changed,app_scope_not_enforced,entitlement_missing,unknown_reason",
    (key) => key,
  );
  assert.equal(
    text,
    [
      "shareMarket.integrity.reason.clientUpgradeRequired",
      "shareMarket.integrity.reason.contractAppsMissing",
      "shareMarket.integrity.reason.contractAppsChanged",
      "shareMarket.integrity.reason.appScopeNotEnforced",
      "shareMarket.integrity.reason.entitlementMissing",
      "shareMarket.integrity.reason.unknown",
    ].join(" · "),
  );
  assert.doesNotMatch(text, /share_contract_upgrade_required|contract_apps|app_scope_not_enforced|entitlement_missing/);
});

const subscription = (
  id: string,
  status: string,
  updatedAt = "2026-01-01T00:00:00Z",
) => ({ id, status, updatedAt }) as ShareMarketSubscription;

test("subscription refresh retains loaded history and replaces active rows", () => {
  const history = subscription("history", "released");
  const staleActive = subscription("active", "active_free");
  const removedActive = subscription("removed", "active_free");
  const refreshedActive = subscription("active", "active_free", "2026-01-02T00:00:00Z");

  assert.deepEqual(
    mergeShareMarketSubscriptionPage(
      [history, staleActive, removedActive],
      [refreshedActive],
      false,
    ),
    [history, refreshedActive],
  );
});

test("subscription refresh preserves an active-to-terminal transition", () => {
  const active = subscription("transition", "active_postpaid");
  const released = subscription("transition", "released", "2026-01-02T00:00:00Z");
  const transitioned = mergeShareMarketSubscriptionPage([active], [released], false);

  assert.deepEqual(transitioned, [released]);
  assert.deepEqual(mergeShareMarketSubscriptionPage(transitioned, [], false), [released]);
});

test("subscription history pages append once by id", () => {
  const active = subscription("active", "active_free");
  const first = subscription("history-1", "released");
  const second = subscription("history-2", "grant_failed");

  assert.deepEqual(
    mergeShareMarketSubscriptionPage([active, first], [first, second], true),
    [active, first, second],
  );
});

const listing = (
  id: string,
  extra: Partial<ShareMarketListing> = {},
): ShareMarketListing => ({
  id,
  shareId: extra.shareId || id,
  installationId: extra.installationId || `install-${id}`,
  shareName: extra.shareName || id,
  appType: extra.appType || "codex",
  supportedApps: extra.supportedApps || ["codex"],
  providerFamily: extra.providerFamily || "openai",
  providerFamilies: extra.providerFamilies || [extra.providerFamily || "openai"],
  appCapabilities: extra.appCapabilities || [],
  ownerEmail: extra.ownerEmail || "owner@example.com",
  status: extra.status || "active",
  shareStatus: extra.shareStatus || "active",
  subdomain: extra.subdomain || id,
  shareOnline: extra.shareOnline ?? true,
  isOwner: extra.isOwner ?? false,
  canDelete: false,
  canReopen: false,
  reopenableSeatCount: 0,
  paymentMethodKinds: [],
  performance: extra.performance || {
    recentRequestCount: 0,
    ttftSampleCount: 0,
    tpsSampleCount: 0,
    windowHours: 24,
  },
  reliability: extra.reliability || {
    observedMinutes24h: 0,
    observationCoverage24h: 0,
    sufficientCoverage: false,
  },
  supportedUserTokenPeriods: extra.supportedUserTokenPeriods || ["lifetime"],
  seats: extra.seats || [],
  createdAt: extra.createdAt || "2026-01-01T00:00:00Z",
  updatedAt: extra.updatedAt || extra.createdAt || "2026-01-01T00:00:00Z",
  ...extra,
});

const rental = (
  id: string,
  extra: Partial<ShareMarketSubscription> = {},
): ShareMarketSubscription => ({
  id,
  seatId: extra.seatId || `seat-${id}`,
  listingId: extra.listingId || `listing-${id}`,
  shareId: extra.shareId || id,
  installationId: extra.installationId || `install-${id}`,
  shareName: extra.shareName || id,
  appType: extra.appType || "codex",
  apps: extra.apps || ["codex"],
  ownerEmail: extra.ownerEmail || "owner@example.com",
  status: extra.status || "active_free",
  integrityState: extra.integrityState || "compatible",
  seatPosition: extra.seatPosition || 1,
  tokenPeriod: extra.tokenPeriod || "lifetime",
  offerRevision: extra.offerRevision || 1,
  paymentMethodKinds: extra.paymentMethodKinds || [],
  canRelease: extra.canRelease ?? true,
  canForceRevoke: false,
  canRetryGrant: false,
  canProposePriceChange: false,
  createdAt: extra.createdAt || extra.updatedAt || "2026-01-01T00:00:00Z",
  updatedAt: extra.updatedAt || "2026-01-01T00:00:00Z",
  ...extra,
});

test("closed rented listings are merged over the public catalog copy", () => {
  const publicListing = listing("public", { shareId: "share-a", status: "active", createdAt: "2026-01-02T00:00:00Z" });
  const otherPublic = listing("other", { shareId: "share-b", createdAt: "2026-01-03T00:00:00Z" });
  const closedRented = listing("closed", { shareId: "share-a", status: "closed", createdAt: "2026-01-01T00:00:00Z" });
  const merged = mergeCatalogWithRentedListings([publicListing, otherPublic], [closedRented]);
  assert.deepEqual(merged.map((item) => `${item.shareId}:${item.status}`).sort(), [
    "share-a:closed",
    "share-b:active",
  ]);
});

test("mine filter is orthogonal to family and search", () => {
  const openaiRented = listing("openai-rented", {
    shareId: "share-openai",
    providerFamily: "openai",
    providerFamilies: ["openai"],
    shareName: "alpha",
  });
  const anthropicRented = listing("anthropic-rented", {
    shareId: "share-anthropic",
    providerFamily: "anthropic",
    providerFamilies: ["anthropic"],
    shareName: "bravo",
  });
  const openaiPublic = listing("openai-public", {
    shareId: "share-public",
    providerFamily: "openai",
    providerFamilies: ["openai"],
    shareName: "charlie",
  });
  const rentedShareIds = new Set(["share-openai", "share-anthropic"]);
  const listings = [openaiRented, anthropicRented, openaiPublic];
  assert.deepEqual(
    filterMergedCatalogListings(listings, { mine: true, family: "all", query: "", rentedShareIds }).map((item) => item.id),
    ["openai-rented", "anthropic-rented"],
  );
  assert.deepEqual(
    filterMergedCatalogListings(listings, { mine: true, family: "openai", query: "", rentedShareIds }).map((item) => item.id),
    ["openai-rented"],
  );
  assert.deepEqual(
    filterMergedCatalogListings(listings, { mine: true, family: "all", query: "bravo", rentedShareIds }).map((item) => item.id),
    ["anthropic-rented"],
  );
});

test("rented listings sort ahead of public catalog and attention stays first", () => {
  const publicNew = listing("public-new", { shareId: "share-public", createdAt: "2026-03-01T00:00:00Z" });
  const rentedHealthy = listing("rented-healthy", { shareId: "share-healthy", createdAt: "2026-01-01T00:00:00Z" });
  const rentedFailed = listing("rented-failed", { shareId: "share-failed", createdAt: "2026-02-01T00:00:00Z" });
  const sorted = sortMergedCatalogListings(
    [publicNew, rentedHealthy, rentedFailed],
    [
      rental("healthy", { shareId: "share-healthy", status: "active_free", updatedAt: "2026-03-01T00:00:00Z" }),
      rental("failed", { shareId: "share-failed", status: "grant_failed", updatedAt: "2026-01-01T00:00:00Z" }),
    ],
  );
  assert.deepEqual(sorted.map((item) => item.id), ["rented-failed", "rented-healthy", "public-new"]);
});

test("catalog pagination clamps and can locate a focused share", () => {
  const listings = Array.from({ length: MARKET_CATALOG_PAGE_SIZE + 3 }, (_, index) => listing(`share-${index}`, {
    shareId: `share-${index}`,
  }));
  const first = paginateListings(listings, 1);
  assert.equal(first.items.length, MARKET_CATALOG_PAGE_SIZE);
  assert.equal(first.pageCount, 2);
  assert.equal(paginateListings(listings, 9).page, 2);
  assert.equal(paginateListings([], 3).page, 1);
  assert.equal(pageForShareId(listings, `share-${MARKET_CATALOG_PAGE_SIZE}`), 2);
  assert.equal(pageForShareId(listings, "missing"), 1);
});

test("rented share ids ignore completed history", () => {
  const ids = rentedShareIdsFromSubscriptions([
    rental("live", { shareId: "share-live", status: "active_postpaid" }),
    rental("done", { shareId: "share-done", status: "released" }),
  ]);
  assert.deepEqual([...ids], ["share-live"]);
});

test("rental history paginates five rows at a time", () => {
  const rows = Array.from({ length: 12 }, (_, index) => ({ id: `history-${index}` }));
  const first = paginateListings(rows, 1, MARKET_RENTAL_HISTORY_PAGE_SIZE);
  assert.equal(first.items.length, 5);
  assert.equal(first.pageCount, 3);
  assert.deepEqual(first.items.map((item) => item.id), ["history-0", "history-1", "history-2", "history-3", "history-4"]);
  assert.equal(paginateListings(rows, 3, MARKET_RENTAL_HISTORY_PAGE_SIZE).items.length, 2);
});
