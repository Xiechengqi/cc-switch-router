import assert from "node:assert/strict";
import test from "node:test";
import {
  catalogSeatPreview,
  initialCatalogSeat,
  preserveCatalogSeat,
} from "./buyer-catalog-utils";
import {
  integrityReasonText,
  marketProviderStatusView,
  primaryMarketCapability,
} from "./market-utils";
import { mergeShareMarketSubscriptionPage } from "./subscription-utils";
import type {
  ShareMarketAppCapability,
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
    accountHint: "p***@example.com",
    quota: {
      status: "ok",
      plan: "Plus",
      tiers: [{ label: "weekly", utilization: 0.55 }],
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
          accountHint: "p***@example.com",
          quota: {
            status: "ok",
            plan: "Plus",
            tiers: [{ label: "weekly", utilization: 0.55 }],
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
  assert.equal(view.identityLine, "OpenAI Official · codex_oauth");
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
  assert.equal(view.identityLine, "Private API · openai_compatible");
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
  assert.equal(view.identityLine, "Claude OAuth · claude_oauth");
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
  assert.equal(view.identityLine, "OpenAI Official · codex_oauth");
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
