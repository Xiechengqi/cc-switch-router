import { buddyQuotaPresentation } from "@/lib/buddy-quota-summary";
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

import {
  formatCompactQuotaTier,
  formatUnobservedQuotaTier,
  providerAccountIdentity,
  providerAccountLevel,
  providerAccountTierLabel,
  providerQuotaStatusLine,
  providerStatusIdentity,
  quotaSummary,
  providerQuotaDetails,
  recentSharePerformance,
  shareAppProviderRuntime,
  utilizationPercentForDisplay,
  buddyOfficialUsageText,
} from "@/components/dashboard/share-dashboard-utils";
import type { ShareUpstreamProvider } from "@/lib/types";
import type { ShareRequestLog } from "@/lib/types";

const buddyQuota = JSON.parse(readFileSync(new URL("../../../tests/fixtures/buddy-quota-presentation-v1.json", import.meta.url), "utf8")) as NonNullable<ShareUpstreamProvider["quota"]>;

test("Buddy quota cards compact packages and use actual expiry without a line break", (t) => {
  t.mock.method(Date, "now", () => Date.parse("2026-11-04T00:00:00Z"));
  for (const product of ["workbuddy_oauth", "codebuddy_oauth"]) {
    for (const app of ["claude", "codex", "gemini"]) {
      const runtime: ShareUpstreamProvider = { providerType: product, kind: "official_oauth", app, quota: structuredClone(buddyQuota) };
      const text = quotaSummary(runtime, "zh-CN");
      assert.equal(text, "注册积分奖励 0/1500 credits · 基础积分 0.5/500 credits · 最近到期 28d9h · 官方用量（账号）： 今日 0.45 · 本月 0.45 credits");
      assert.doesNotMatch(text, /codebuddy package|%|采集|范围|7 日|周期结束|UTC|2026-|其他 share/);
      const english = quotaSummary(runtime, "en");
      assert.match(english, /^Registration reward 0\/1500 credits/);
      assert.match(english, /Official usage \(account\): today 0.45 · this month 0.45 credits/);
      assert.equal(english.split("\n").length, 1);
    }
  }
});

test("Buddy card identity preserves the Server account display name across app-provider projections", () => {
  for (const providerType of ["workbuddy_oauth", "codebuddy_oauth"]) {
    for (const app of ["claude", "codex", "gemini"]) {
      for (const accountLabel of ["中文昵称", "owner@example.com", "138****0000", `${providerType}-account-fixture`]) {
        const runtime = shareAppProviderRuntime({
          id: "buddy-provider", name: "WorkBuddy", kind: providerType, providerType, app,
          accountLabel, accountEmail: "legacy@example.com",
        });
        assert.equal(providerAccountIdentity(runtime), accountLabel);
        assert.equal(providerStatusIdentity(runtime), accountLabel);
        assert.equal(providerStatusIdentity({ ...runtime, providerName: "Renamed provider" }), accountLabel);
      }
    }
  }
});

test("Buddy cards with missing account labels never substitute provider names or legacy email", () => {
  for (const providerType of ["workbuddy_oauth", "codebuddy_oauth"]) {
    for (const accountLabel of [undefined, "", "   "]) {
      const runtime = { providerType, providerName: "WorkBuddy", accountLabel, accountEmail: "legacy@example.com" };
      assert.equal(providerAccountIdentity(runtime), "-");
      assert.equal(providerStatusIdentity(runtime), "-");
      assert.equal(providerStatusIdentity({ ...runtime, providerType: undefined, kind: providerType }), "-");
    }
  }
  assert.equal(providerStatusIdentity({ providerType: "codex_oauth", providerName: "OpenAI Official" }), "OpenAI Official");
});

test("Buddy credits are unknown when incomplete, and explicit zero remains zero", () => {
  assert.equal(buddyOfficialUsageText(undefined, "en"), "");
  const incomplete = { ...buddyQuota.providerUsage!, usage7Days: undefined };
  assert.match(buddyOfficialUsageText(incomplete, "en"), /usage is temporarily unavailable/);
  assert.doesNotMatch(buddyOfficialUsageText(incomplete, "en"), /today 0.00/);
  const zero = { ...buddyQuota.providerUsage!, usageToday: 0, usage7Days: 0, usageThisMonth: 0, usageTotal: 0, requestCount: 0 };
  assert.match(buddyOfficialUsageText(zero, "en"), /today 0.00/);
  assert.match(buddyOfficialUsageText(zero, "en"), /this month 0.00 credits/);
  const old = { ...buddyQuota, providerUsage: undefined };
  assert.doesNotMatch(quotaSummary({ providerType: "workbuddy_oauth", quota: old }, "en"), /Official usage/);
});

test("Buddy package detail countdowns use expiry, never cycle reset", (t) => {
  t.mock.method(Date, "now", () => Date.parse("2026-11-04T00:00:00Z"));
  const tier = { label: "package", used: 0.5, limit: 500, unit: "credits", utilization: 0.1, resetsAt: "2026-11-05T00:00:00Z", expiresAt: "2026-11-24T12:00:00Z" };
  assert.equal(formatCompactQuotaTier(tier, "en", 1, true), "package 0.5/500 credits 20d12h");
  for (const expiresAt of [undefined, "invalid", "2026-11-03T00:00:00Z"]) {
    assert.equal(formatCompactQuotaTier({ ...tier, expiresAt }, "en", 1, true), "package 0.5/500 credits");
  }
  assert.equal(formatCompactQuotaTier({ label: "1w", utilization: 1.4 }), "7d 1%");
});

test("Buddy package refresh failure keeps the independent official request ledger visible", () => {
  for (const providerType of ["workbuddy_oauth", "codebuddy_oauth"]) {
    const runtime = { providerType, quota: { ...buddyQuota, status: "failed" } };
    const text = quotaSummary(runtime, "en");
    assert.match(text, /Resource package data is unavailable/);
    assert.match(text, /today 0.45/);
    assert.doesNotMatch(text, /0.5\/500/);
    assert.match(quotaSummary(runtime, "zh-CN"), /资源包数据暂不可用/);
    assert.equal(quotaSummary({ ...runtime, providerType: "codex_oauth" }, "en"), "");
  }
});

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

test("Codex Banked Reset summary is read-only, localized, and suppresses unavailable evidence", () => {
  const originalNow = Date.now;
  Date.now = () => Date.parse("2026-08-28T00:00:00Z");
  try {
    const runtime: ShareUpstreamProvider = {
      app: "codex", providerType: "codex_oauth", kind: "official_oauth",
      quota: {
        status: "ok", plan: "ChatGPT Pro 20x", tiers: [],
        bankedReset: {
          availableCount: 3, detailsAvailable: true,
          expiresAt: ["2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z", "2026-08-29T00:00:00Z", "2026-08-27T00:00:00Z"],
        },
      },
    };
    assert.match(quotaSummary(runtime, "zh-CN"), /Banked Reset 3 次 · 24h0m \/ 2d0h 后到期/);
    assert.match(quotaSummary(runtime, "en"), /Banked Reset 3 · 24h0m \/ 2d0h until expiry/);
    runtime.quota!.bankedReset!.detailsAvailable = false;
    assert.match(quotaSummary(runtime, "zh-CN"), /Banked Reset 3 次 · 明细暂不可用/);
    runtime.quota!.bankedReset!.detailsAvailable = true;
    runtime.quota!.bankedReset!.expiresAt = ["2026-08-27T00:00:00Z"];
    assert.match(quotaSummary(runtime, "zh-CN"), /Banked Reset 3 次$/);
    runtime.quota!.bankedReset!.availableCount = 0;
    assert.doesNotMatch(quotaSummary(runtime, "zh-CN"), /Banked Reset/);
    runtime.quota!.bankedReset!.availableCount = 3;
    runtime.providerType = "claude_oauth";
    assert.doesNotMatch(quotaSummary(runtime, "zh-CN"), /Banked Reset/);
  } finally {
    Date.now = originalNow;
  }
});

test("Claude quota summary keeps the scoped Fable weekly pool", () => {
  const line = quotaSummary(
    {
      kind: "official_oauth",
      app: "claude",
      providerName: "Claude Official",
      providerType: "claude_oauth",
      quota: {
        status: "ok",
        plan: "Claude Max 20x",
        tiers: [
          { label: "1w", utilization: 72 },
          {
            label: "Fable 7d",
            utilization: 100,
            scope: "model_family",
            capacityPool: "claude_fable_7d_oi",
            modelFamily: "claude-fable-5",
            relativeWeeklyCapacity: 0.5,
            source: "anthropic_usage_7d_oi",
          },
        ],
      },
    },
    "en",
  );
  assert.match(line, /Claude Max 20x/);
  assert.match(line, /7d 72%/);
  assert.match(line, /Fable 7d 100%/);
});

test("Claude quota summary labels an entitled but unobserved Fable pool", () => {
  const unobservedFable = {
    name: "seven_day_fable",
    label: "Fable 7d",
    scope: "model_family",
    capacityPool: "claude_fable_7d_oi",
    modelFamily: "claude-fable-5",
    relativeWeeklyCapacity: 0.5,
    source: "claude_subscription_plan",
    reason: "awaiting_upstream_observation",
  };
  const runtime: ShareUpstreamProvider = {
    kind: "official_oauth",
    app: "claude",
    providerName: "Claude Official",
    providerType: "claude_oauth",
    quota: {
      status: "ok",
      plan: "Claude Max 20x",
      tiers: [{ label: "1w", utilization: 72 }],
      unobservedTiers: [unobservedFable],
    },
  };

  assert.equal(formatUnobservedQuotaTier(unobservedFable, "en"), "Fable 7d awaiting observation");
  assert.equal(formatUnobservedQuotaTier(unobservedFable, "zh-CN"), "Fable 7d 待观测");
  assert.match(quotaSummary(runtime, "en"), /Fable 7d awaiting observation/);
  assert.doesNotMatch(quotaSummary(runtime, "en"), /Fable 7d 0%/);
});

test("observed Fable quota wins over a duplicate unobserved hint", () => {
  const line = quotaSummary({
    kind: "official_oauth",
    app: "claude",
    providerType: "claude_oauth",
    quota: {
      status: "ok",
      tiers: [{
        name: "upstream_fable_window",
        label: "Fable 7d",
        utilization: 41,
      }],
      unobservedTiers: [{
        name: "seven_day_fable",
        label: "Fable 7-Day",
        scope: "model_family",
        capacityPool: "claude_fable_7d_oi",
        modelFamily: "claude-fable-5",
        relativeWeeklyCapacity: 0.5,
        source: "claude_subscription_plan",
        reason: "awaiting_upstream_observation",
      }],
    },
  });

  assert.match(line, /Fable 7d 41%/);
  assert.doesNotMatch(line, /awaiting observation/);
});

test("Buddy summary matches Server rules for the reported account and retains batch details", (t) => {
  t.mock.method(Date, "now", () => Date.parse("2026-11-07T00:00:00Z"));
  const tiers = [
    buddyTier("CodeBuddy个人版拉新权益包", 1084.52, 1500, 25 * 24 + 4),
    buddyTier(checkinLabel, 0, 100, 27 * 24 + 20),
    buddyTier(checkinLabel, 0, 100, 28 * 24 + 20),
    buddyTier(checkinLabel, 0, 100, 29 * 24 + 20),
    buddyTier("CodeBuddy个人体验版", 500, 500, 23 * 24 + 11),
  ];
  const before = JSON.stringify(tiers);
  for (const providerType of ["workbuddy_oauth", "codebuddy_oauth"]) {
    for (const app of ["claude", "codex", "gemini"]) {
      const runtime = { providerType, app, quota: { ...structuredClone(buddyQuota), tiers,
        providerUsage: { ...buddyQuota.providerUsage!, usageToday: 1.67, usageThisMonth: 1584.45 } } };
      assert.equal(quotaSummary(runtime, "zh-CN"), "注册积分奖励 1084.52/1500 credits · 签到 0/300 credits · 基础积分 500/500 credits · 最近到期 25d4h · 官方用量（账号）： 今日 1.67 · 本月 1584.45 credits");
      const details = providerQuotaDetails(runtime, "zh-CN");
      assert.equal(details.split(checkinLabel).length - 1, 3);
      assert.match(details, /剩余 415.48 credits/);
      assert.match(details, /积分到期：/);
      assert.match(quotaSummary(runtime, "en"), /Check-in 0\/300 credits/);
    }
  }
  assert.equal(JSON.stringify(tiers), before);
});

const checkinLabel = "CodeBuddy个人版国内运营裂变包";
function buddyTier(label: string, used = 0, limit = 100, hours = 24) {
  return { label, used, limit, unit: "credits", utilization: used / limit * 100,
    source: "codebuddy_billing_resource_v2", expiresAt: new Date(Date.now() + hours * 3600000).toISOString() };
}

test("Buddy merging preserves duplicate batches and excludes invalid values and units", (t) => {
  t.mock.method(Date, "now", () => Date.parse("2026-11-07T00:00:00Z"));
  const tiers = [buddyTier(checkinLabel, 12.34, 80, 48), buddyTier(checkinLabel, 1.11, 120), buddyTier(checkinLabel, 1.11, 120)];
  assert.equal(buddyQuotaPresentation(tiers, "zh-CN").text, "签到 14.56/320 credits · 最近到期 1d0h");
  const text = buddyQuotaPresentation([buddyTier(checkinLabel), { ...buddyTier(checkinLabel), unit: "USD" },
    { ...buddyTier(checkinLabel), used: NaN, utilization: 0.01 }], "zh-CN").text;
  assert.match(text, /签到 0\/100 credits · 签到 0\/100 USD · 签到 <0.1%/);
  assert.match(text, /最近已知到期/);
});

test("Buddy expiry uses remaining balances, not percentages or resets, and marks uncertainty", (t) => {
  t.mock.method(Date, "now", () => Date.parse("2026-11-07T00:00:00Z"));
  const known = buddyTier(checkinLabel, 0, 100, 240);
  const unknown = { ...buddyTier(checkinLabel), expiresAt: undefined, resetsAt: new Date(Date.now() + 3600000).toISOString() };
  assert.equal(buddyQuotaPresentation([unknown], "zh-CN").text, "签到 0/100 credits · 到期时间未知");
  assert.match(buddyQuotaPresentation([known, unknown], "zh-CN").text, /最近已知到期 10d0h/);
  assert.match(buddyQuotaPresentation([known], "zh-CN", Date.now(), true).text, /最近已知到期/);
  assert.equal(buddyQuotaPresentation([buddyTier(checkinLabel, 100)], "zh-CN").text, "签到 100/100 credits");
  assert.match(buddyQuotaPresentation([{ ...known, used: 99.9999, utilization: 100 }], "zh-CN").text, /最近到期/);
  assert.match(buddyQuotaPresentation([{ ...known, expiresAt: "invalid" }], "zh-CN").text, /到期时间未知/);
  assert.match(buddyQuotaPresentation([unknown], "en").details, /Cycle resets:/);
});

test("Buddy exact expiry boundary hides expired batches but preserves details", (t) => {
  t.mock.method(Date, "now", () => Date.parse("2026-11-07T00:00:00Z"));
  const expired = buddyTier(checkinLabel, 20, 100, 0);
  const result = buddyQuotaPresentation([expired, buddyTier(checkinLabel)], "zh-CN");
  assert.equal(result.text, "签到 0/100 credits · 最近到期 1d0h · 到期状态待刷新");
  assert.match(result.details, /20\/100 credits/);
  assert.equal(buddyQuotaPresentation([expired], "zh-CN").text, "到期状态待刷新");
  assert.match(buddyQuotaPresentation([expired], "zh-CN", Date.now() - 1).text, /最近到期 1m/);
});

test("Buddy aliases require reviewed evidence; unknown plans stay separate", () => {
  const text = buddyQuotaPresentation([buddyTier("CodeBuddy 个人体验版"), buddyTier("New Plan"), buddyTier("New Plan"),
    { ...buddyTier(checkinLabel), source: undefined }, { ...buddyTier(checkinLabel), name: "premium" }], "zh-CN").text;
  assert.match(text, /基础积分/);
  assert.equal(text.split("New Plan").length - 1, 2);
  assert.equal(text.split(checkinLabel).length - 1, 2);
});
