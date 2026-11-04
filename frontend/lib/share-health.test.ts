import assert from "node:assert/strict";
import test from "node:test";
import { modelHealthPresentation } from "./share-health";
import { modelHealthTone, modelHealthTitle } from "@/components/dashboard/share-dashboard-utils";
import { marketProviderStatusView } from "@/components/dashboard/share-market/market-utils";
import type { ShareMarketAppCapability, ShareMarketListing, ShareModelHealthStatus, ShareView } from "./types";

const now = Date.parse("2026-11-04T10:30:11Z");
function status(overrides: Partial<ShareModelHealthStatus> = {}): ShareModelHealthStatus {
  return { state: "unavailable", appType: "codex", model: "kimi-k3-1", healthFingerprint: "current",
    checkedAt: now / 1000 - 1, staleAt: now / 1000 + 3900,
    statusCode: 429, reasonCode: "upstream_rate_limited", recentResults: ["failed", "failed", "failed"], ...overrides };
}

test("authoritative health uses amber/red/green/gray and expires without a new response", () => {
  for (const [state, color] of [["degraded", "amber"], ["unavailable", "red"], ["healthy", "emerald"], ["unknown", "slate"]] as const) {
    assert.match(modelHealthPresentation(status({ state }), "en", now).className, new RegExp(`bg-${color}-50`));
  }
  assert.match(modelHealthPresentation(status(), "zh-CN", now).title, /上游限流/);
  assert.match(modelHealthPresentation(status(), "en", now).title, /HTTP 429/);
  for (const invalid of [undefined, status({ checkedAt: undefined }), status({ checkedAt: now / 1000 + 1 }),
    status({ staleAt: now / 1000 }), status({ staleAt: NaN }), status({ checkedAt: 0 }),
    status({ state: "toString" as ShareModelHealthStatus["state"] })]) {
    assert.match(modelHealthPresentation(invalid, "en", now).className, /bg-slate-50/);
  }
});

test("Claude-primary share uses actual Codex probe for the overview, not an empty Claude bucket", (t) => {
  t.mock.method(Date, "now", () => now);
  const share = { appType: "claude", modelHealthStatus: status(),
    appRuntimes: { codex: { modelProbe: { healthFingerprint: "current" } } },
    modelHealth: { claude: [], codex: [] } } as unknown as ShareView;
  assert.match(modelHealthTone(share).className, /bg-red-50/);
  assert.match(modelHealthTone(share, "codex").className, /bg-red-50/);
  assert.match(modelHealthTone(share, "claude").className, /bg-slate-50/);
  assert.match(modelHealthTitle(share, undefined, "zh-CN"), /HTTP 429/);
  share.appRuntimes!.codex!.modelProbe!.healthFingerprint = "changed";
  assert.match(modelHealthTone(share, "codex").className, /bg-slate-50/);
});

test("missing and stale legacy model checks are never green", (t) => {
  t.mock.method(Date, "now", () => now);
  const share = { modelHealth: { claude: [] } } as unknown as ShareView;
  assert.match(modelHealthTone(share, "claude").className, /bg-slate-50/);
  share.modelHealth!.claude = [{ appType: "claude", actualModel: "model", requestedModel: "model",
    status: "success", checkedAt: now / 1000 - 4000, recentResults: ["success"] }];
  assert.match(modelHealthTone(share, "claude").className, /bg-slate-50/);
});

test("market identity richness cannot mask authoritative health or an enabled App failure", (t) => {
  t.mock.method(Date, "now", () => now);
  const caps = [
    { app: "claude", providerName: "Rich primary", subscriptionLevel: "Plan", healthState: "healthy", quota: { tiers: [] } },
    { app: "codex", providerName: "WorkBuddy", healthState: "unavailable" },
  ] as ShareMarketAppCapability[];
  const listing = { supportedApps: ["claude", "codex"], appCapabilities: caps, modelHealthStatus: status() } as ShareMarketListing;
  const labels = { unknown: "-", passthrough: "passthrough" };
  assert.match(marketProviderStatusView(listing, "en", labels).toneClassName, /bg-red-50/);
  listing.modelHealthStatus = status({ state: "degraded", recentResults: ["failed", "success"] });
  assert.match(marketProviderStatusView(listing, "en", labels).toneClassName, /bg-amber-50/);
  delete listing.modelHealthStatus;
  assert.match(marketProviderStatusView(listing, "en", labels).toneClassName, /bg-red-50/);
  listing.supportedApps = ["claude"];
  assert.match(marketProviderStatusView(listing, "en", labels).toneClassName, /bg-emerald-50/);
});
