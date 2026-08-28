import assert from "node:assert/strict";
import test from "node:test";

import {
  formatCompactQuotaTier,
  providerAccountIdentity,
  providerAccountLevel,
  providerAccountTierLabel,
  providerQuotaStatusLine,
  quotaSummary,
  recentSharePerformance,
  shareAppProviderRuntime,
  utilizationPercentForDisplay,
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

test("share quota utilization is a 0-100 percent, so 1 stays 1%", () => {
  assert.equal(utilizationPercentForDisplay(1), 1);
  assert.equal(utilizationPercentForDisplay(55), 55);
  assert.equal(utilizationPercentForDisplay(100), 100);
  assert.equal(utilizationPercentForDisplay(1.4), 1);
  assert.equal(utilizationPercentForDisplay(0.1, 1), 0.1);
  assert.equal(formatCompactQuotaTier({ label: "1w", utilization: 1 }), "7d 1%");
  assert.equal(formatCompactQuotaTier({ label: "1w", utilization: 55 }), "7d 55%");
  assert.equal(formatCompactQuotaTier({ label: "1w", utilization: 100 }), "7d 100%");
});

test("ChatGPT provider cards keep a 1% weekly window instead of scaling it to 100%", () => {
  const now = Date.parse("2026-08-28T00:00:00.000Z");
  const originalNow = Date.now;
  Date.now = () => now;
  try {
    const runtime: ShareUpstreamProvider = {
      kind: "codex_oauth",
      app: "codex",
      providerName: "OpenAI Official",
      providerType: "codex_oauth",
      quota: {
        status: "ok",
        plan: "ChatGPT Pro 20x",
        subscriptionPeriodEnd: "2026-09-24T00:00:00.000Z",
        tiers: [{
          label: "1w",
          utilization: 1,
          resetsAt: "2026-09-03T12:00:00.000Z",
        }],
      },
    };
    const line = quotaSummary(runtime, "zh-CN");
    assert.match(line, /ChatGPT Pro 20x/);
    assert.match(line, /7d 1%/);
    assert.doesNotMatch(line, /7d 100%/);
    assert.equal(providerQuotaStatusLine(runtime, "zh-CN"), line);
  } finally {
    Date.now = originalNow;
  }
});

test("Ollama display-only windows keep one-decimal percents without a second scale", () => {
  const line = quotaSummary({
    kind: "official_oauth",
    app: "codex",
    providerName: "Ollama Cloud",
    providerType: "ollama_cloud",
    quota: {
      status: "ok",
      plan: "free",
      tiers: [{ label: "weekly", utilization: 0.1 }],
    },
  }, "en");
  assert.match(line, /Weekly 0\.1%/);
});
