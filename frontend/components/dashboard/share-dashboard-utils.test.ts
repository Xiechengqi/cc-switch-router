import assert from "node:assert/strict";
import test from "node:test";

import {
  providerAccountIdentity,
  providerAccountLevel,
  providerAccountTierLabel,
  recentSharePerformance,
  shareAppProviderRuntime,
} from "@/components/dashboard/share-dashboard-utils";
import type { ShareUpstreamProvider } from "@/lib/types";
import type { ShareRequestLog } from "@/lib/types";

function cursorApiKeyRuntime(
  overrides: Partial<ShareUpstreamProvider> = {},
): ShareUpstreamProvider {
  return {
    kind: "cursor_apikey",
    providerType: "cursor_apikey",
    providerName: "Cursor API Key",
    accountLabel: "owner@example.com",
    accountEmail: "owner@example.com",
    subscriptionLevel: "Cursor Pro+",
    ...overrides,
  };
}

test("Cursor API-key cards prefer the verified account and subscription", () => {
  const runtime = cursorApiKeyRuntime();

  assert.equal(providerAccountIdentity(runtime), "owner@example.com");
  assert.equal(providerAccountLevel(runtime), "Cursor Pro+");
  assert.equal(providerAccountTierLabel(runtime), "Cursor Pro+");
});

test("Cursor API-key cards use the account as the level when no plan is known", () => {
  const runtime = cursorApiKeyRuntime({ subscriptionLevel: undefined });

  assert.equal(providerAccountIdentity(runtime), "owner@example.com");
  assert.equal(providerAccountLevel(runtime), "owner@example.com");
  assert.equal(providerAccountTierLabel(runtime), "owner@example.com");
});

test("legacy Cursor API-key descriptors keep their provider-name fallback", () => {
  const runtime = cursorApiKeyRuntime({
    accountLabel: undefined,
    accountEmail: undefined,
    subscriptionLevel: undefined,
  });

  assert.equal(providerAccountIdentity(runtime), "Cursor API Key");
  assert.equal(providerAccountLevel(runtime), "Cursor API Key");
  assert.equal(providerAccountTierLabel(runtime), "Cursor API Key");
});

test("Cursor account fields survive the app-provider runtime projection", () => {
  const runtime = shareAppProviderRuntime({
    id: "cursor-provider",
    name: "Cursor API Key",
    app: "codex",
    providerType: "cursor_apikey",
    accountLabel: "owner@example.com",
    accountEmail: "owner@example.com",
    subscriptionLevel: "Cursor Pro+",
  });

  assert.equal(providerAccountIdentity(runtime), "owner@example.com");
  assert.equal(providerAccountLevel(runtime), "Cursor Pro+");
});

function performanceLog(overrides: Partial<ShareRequestLog> = {}): ShareRequestLog {
  return {
    requestId: "request-performance",
    model: "gpt-test",
    requestAgent: "codex",
    statusCode: 200,
    latencyMs: 1_000,
    firstTokenMs: 500,
    inputTokens: 100,
    outputTokens: 10,
    isStreaming: true,
    streamStatus: "completed",
    usageState: "observed",
    createdAt: 1,
    ...overrides,
  };
}

test("TPS excludes terminal-flush-sized generation windows", () => {
  const performance = recentSharePerformance([
    performanceLog({ latencyMs: 10_000, firstTokenMs: 9_999, outputTokens: 480 }),
  ]);

  assert.equal(performance.ttftSampleCount, 1);
  assert.equal(performance.tpsSampleCount, 0);
  assert.equal(performance.averageTps, null);
});

test("TPS retains generation windows at the reliability boundary", () => {
  const performance = recentSharePerformance([
    performanceLog({ latencyMs: 1_000, firstTokenMs: 900, outputTokens: 10 }),
  ]);

  assert.equal(performance.tpsSampleCount, 1);
  assert.equal(performance.averageTps, 100);
});
