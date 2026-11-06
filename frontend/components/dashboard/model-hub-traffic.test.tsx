import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { messages, type AppLocale, type MessageKey } from "@/lib/i18n";
import type { ModelRouteTrafficEvent, UserModelRoutingShare } from "@/lib/types";
import { ModelHubTraffic, QueueRowTraffic } from "./model-hub-traffic";

// Static markup only: no browser, screenshots or UI automation.
function translator(locale: AppLocale) {
  return (key: MessageKey, params?: Record<string, string | number>) =>
    Object.entries(params || {}).reduce((text, [name, value]) => text.replaceAll(`{${name}}`, String(value)), messages[locale][key]);
}
const shares = ["a", "b"].map(id => ({ shareId: id, shareName: id, subdomain: `${id}-share`, apps: ["codex"], access: "owner",
  directApiUrl: "", freeAccess: false, isOnline: true })) as UserModelRoutingShare[];
const startedAt = new Date().toISOString();
const events: ModelRouteTrafficEvent[] = [
  { sequence: "3", appType: "codex", requestedModel: "gpt-live", routeId: "r", revision: 1, matchedWildcard: true, startedAt,
    phase: "streaming", statusCode: 200, targetShareId: "a", position: 0, continuedSearch: false, headersMs: 800,
    attempts: [{ targetShareId: "a", position: 0, outcome: "response", statusCode: 200, durationMs: 800 }] },
  { sequence: "2", appType: "codex", requestedModel: "gpt-pinned", routeId: "r2", revision: 1, matchedWildcard: false, startedAt,
    phase: "complete", statusCode: 200, targetShareId: "b", position: 1, continuedSearch: true, headersMs: 2_400, durationMs: 9_100,
    attempts: [
      { targetShareId: "a", position: 0, outcome: "response", statusCode: 429, durationMs: 300 },
      { targetShareId: "b", position: 1, outcome: "response", statusCode: 200, durationMs: 2_100 },
    ] },
  { sequence: "1", appType: "codex", requestedModel: "unknown-model", matchedWildcard: false, startedAt, phase: "complete",
    statusCode: 404, errorCode: "user_model_route_not_configured", continuedSearch: false, attempts: [], durationMs: 0 },
];

for (const locale of ["en", "zh-CN"] as const) {
  const t = translator(locale);
  test(`${locale}: live paths render each request's real attempt chain and phase`, () => {
    const html = renderToStaticMarkup(<ModelHubTraffic events={events} app="codex" shares={shares} status="ready" locale={locale} t={t} />);
    assert.match(html, /a-share[\s\S]*429[\s\S]*b-share[\s\S]*200/, "the failover chain keeps attempt order");
    assert.ok(html.includes(t("modelHub.traffic.kind.default")));
    assert.ok(html.includes(t("modelHub.traffic.kind.exact")));
    assert.ok(html.includes(t("modelHub.traffic.kind.unmatched")));
    assert.ok(html.includes(t("modelHub.traffic.noRule")));
    assert.ok(html.includes(t("modelHub.traffic.continued", { count: 1 })));
    assert.ok(html.includes(t("modelHub.traffic.inflight", { count: 1 })));
    assert.ok(html.includes(t("modelHub.traffic.headers", { seconds: "2.4" })));
    assert.match(html, /model-route-dot-live/, "a streaming request is shown as in progress");
    assert.doesNotMatch(html, /接管|主力|切换成功|took over|switched successfully/i, "no inferred failover claims");
    const claude = renderToStaticMarkup(<ModelHubTraffic events={events} app="claude" shares={shares} status="ready" locale={locale} t={t} />);
    assert.ok(claude.includes(t("modelHub.traffic.empty", { minutes: 15 })), "other protocols' events are not shown");
    const unsupported = renderToStaticMarkup(<ModelHubTraffic events={[]} app="codex" shares={shares} status="unsupported" locale={locale} t={t} />);
    assert.ok(unsupported.includes(t("modelHub.traffic.unsupported")));
  });

  test(`${locale}: queue counters show forwards, failures and landings, or an explicit idle state`, () => {
    const busy = renderToStaticMarkup(<QueueRowTraffic traffic={{ forwarded: 3, failed: 1, landedAfterFailure: 2, buckets: Array(15).fill(0).map((_, i) => i === 14 ? 3 : 0) }} t={t} />);
    assert.ok(busy.includes(t("modelHub.traffic.queue.forwarded", { count: 3 })));
    assert.ok(busy.includes(t("modelHub.traffic.queue.failed", { count: 1 })));
    assert.ok(busy.includes(t("modelHub.traffic.queue.landed", { count: 2 })));
    assert.equal([...busy.matchAll(/<rect\b/g)].length, 15);
    const idle = renderToStaticMarkup(<QueueRowTraffic t={t} />);
    assert.ok(idle.includes(t("modelHub.traffic.queue.idle", { minutes: 15 })));
  });
}
