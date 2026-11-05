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
  assert.match(modelHealthTone(share).className, /bg-slate-50/);
});

test("missing and stale legacy model checks are never green", (t) => {
  t.mock.method(Date, "now", () => now);
  const share = { modelHealth: { claude: [] } } as unknown as ShareView;
  assert.match(modelHealthTone(share, "claude").className, /bg-slate-50/);
  share.modelHealth!.claude = [{ appType: "claude", actualModel: "model", requestedModel: "model",
    status: "success", checkedAt: now / 1000 - 4000, recentResults: ["success"] }];
  assert.match(modelHealthTone(share, "claude").className, /bg-slate-50/);
});

test("live manual health changes display without replacing canonical cycle evidence", (t) => {
  t.mock.method(Date,"now",()=>now);
  const canonical=status({state:"unknown",checkedAt:undefined,staleAt:undefined,recentResults:[]});
  const live=status({state:"healthy",source:"server_manual",reasonCode:"manual_probe_verified",recentResults:[],latencyMs:123});
  const share={modelHealthStatus:canonical,liveModelHealthStatus:live,
    appRuntimes:{codex:{modelProbe:{healthFingerprint:"current"}}}} as unknown as ShareView;
  assert.match(modelHealthTone(share).className,/bg-emerald-50/);
  assert.equal(share.modelHealthStatus?.state,"unknown");
  assert.match(modelHealthTitle(share,undefined,"zh-CN"),/Server 模型测试/);
  assert.match(modelHealthTone(share,"claude").className,/bg-slate-50/);
  const presentation=modelHealthPresentation(live,"zh-CN",now);
  assert.match(presentation.detailLabel,/手动验证/);
  assert.match(presentation.title,/0\.12s/);
  live.state="degraded"; live.reasonCode="probe_slow";
  assert.match(modelHealthTone(share).className,/bg-amber-50/);
  assert.match(modelHealthPresentation(live,"zh-CN",now).detailLabel,/响应较慢/);
  live.reasonCode="probe_recovering";
  assert.match(modelHealthPresentation(live,"en",now).detailLabel,/Recovering/);
  live.reasonCode="manual_probe_failed";
  assert.equal(modelHealthPresentation(live,"zh-CN",now).detailLabel,"手动检查异常");
  assert.doesNotMatch(modelHealthPresentation(live,"en",now).detailLabel,/verified/i);
  live.reasonCode="upstream_rate_limited";
  assert.equal(modelHealthPresentation(live,"en",now).detailLabel,"Manual check failed");
  live.staleAt=now/1000;
  assert.match(modelHealthTone(share).className,/bg-slate-50/);
});

test("market uses the same live evidence and explains waiting and slow periodic checks", (t) => {
  t.mock.method(Date,"now",()=>now);
  const listing={supportedApps:["codex"],appCapabilities:[{app:"codex",providerName:"P",healthState:"healthy"}],
    modelHealthStatus:status(),liveModelHealthStatus:status({state:"degraded",source:"router_connection_test",reasonCode:"probe_slow"})} as ShareMarketListing;
  const view=marketProviderStatusView(listing,"zh-CN",{unknown:"-",passthrough:"透传"});
  assert.match(view.toneClassName,/bg-amber-50/);
  assert.match(view.healthLabel!,/响应较慢/);
  assert.match(view.panelTitle!,/Router 端到端测试/);
  const waiting=modelHealthPresentation(status({state:"unknown",checkedAt:undefined,staleAt:undefined,reasonCode:"awaiting_probe"}),"zh-CN",now);
  assert.equal(waiting.detailLabel,"等待周期检查");
  const slow=modelHealthPresentation(status({state:"degraded",reasonCode:"probe_degraded"}),"en",now);
  assert.equal(slow.detailLabel,"Slow response");
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
