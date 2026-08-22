import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_CATALOG_AVAILABILITY,
  catalogSeatPreview,
  initialCatalogSeat,
  preserveCatalogSeat,
} from "./buyer-catalog-utils";
import {
  marketProviderStatusView,
  primaryMarketCapability,
} from "./market-utils";
import type { ShareMarketAppCapability } from "@/lib/types";

const seat = (id: string, status = "available", readOnly = false) => ({ id, status, readOnly });

test("catalog defaults to idle seats", () => {
  assert.equal(DEFAULT_CATALOG_AVAILABILITY, "idle");
});

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

test("provider panel reproduces quota, masked identity and app model rows", () => {
  const view = marketProviderStatusView(
    {
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
  assert.equal(view.identityLine, "p***@example.com");
  assert.match(view.modelsLine, /Claude: grok-4\.6/);
  assert.match(view.modelsLine, /Codex: gpt-5/);
  assert.match(view.toneClassName, /emerald/);
});

test("provider panel keeps degraded and API providers safe", () => {
  const view = marketProviderStatusView(
    {
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
