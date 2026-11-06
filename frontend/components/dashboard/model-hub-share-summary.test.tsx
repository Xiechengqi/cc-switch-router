import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { messages, type AppLocale, type MessageKey } from "@/lib/i18n";
import type { UserModelRoutingShare } from "@/lib/types";
import { RoutingShareSummary } from "./model-hub-share-summary";
import { SharePoolPicker } from "./model-hub-share-picker";
import { ModelHubRuleEditor } from "./model-hub-rule-editor";
import { ModelHubRecentResult } from "./model-hub-recent-result";

// Static markup only: no browser, screenshots, layout engine or UI automation.
function translator(locale: AppLocale) {
  return (key: MessageKey, params?: Record<string, string | number>) =>
    Object.entries(params || {}).reduce((text, [name, value]) => text.replaceAll(`{${name}}`, String(value)), messages[locale][key]);
}
const now = Math.floor(Date.now() / 1000);
const share: UserModelRoutingShare = {
  shareId: "fixture", shareName: "Fixture", subdomain: "fixture-subdomain", directApiUrl: "https://fixture.example.test", access: "shared", freeAccess: false, isOnline: true, apps: ["codex", "claude"],
  appCapabilities: [
    { app: "codex", kind: "official_oauth", accountEmail: "protocol-account@example.test", modelPolicy: { mode: "single", upstreamModel: "test-model" }, healthFingerprint: "current", liveModelHealthStatus: { state: "healthy", appType: "codex", healthFingerprint: "current", checkedAt: now - 5, staleAt: now + 3900, reasonCode: "probe_success", recentResults: [], source: "periodic" } },
    { app: "claude", kind: "claude_oauth", accountEmail: "other-protocol@example.test" },
  ],
};

for (const locale of ["en", "zh-CN"] as const) {
  const t = translator(locale);
  test(`${locale}: summary is a two-line, phrasing-only row with account detail in the tooltip`, () => {
    const props = { share, shareId: share.shareId, app: "codex" as const, locale, t };
    const html = renderToStaticMarkup(<RoutingShareSummary {...props} />);
    assert.match(html, /fixture-subdomain/);
    assert.match(html, />[^<]*test-model[^<]*</, "second line names the model this protocol reaches");
    assert.match(html, /model-route-dot[^"]*bg-emerald-500[^"]*model-route-dot-live/);
    assert.ok(html.includes(t("common.online")));
    assert.match(html, /title="[^"]*protocol-account@example.test/, "account identity moves to the tooltip");
    assert.doesNotMatch(html, />[^<]*protocol-account@example.test/, "account identity is not rendered as row text");
    assert.doesNotMatch(html, /other-protocol@example.test|bg-emerald-50(?!0)|<div|<button|<details|<input|<a /);
    const revoked = renderToStaticMarkup(<RoutingShareSummary {...props} share={undefined} />);
    assert.ok(revoked.includes(t("modelHub.targetUnavailable")));
    assert.match(revoked, />fixture</, "a missing target still shows its id so it can be removed");
    assert.doesNotMatch(revoked, /protocol-account|fixture-subdomain/);
    const offline = renderToStaticMarkup(<RoutingShareSummary {...props} share={{ ...share, isOnline: false }} />);
    assert.ok(offline.includes(t("common.offline")));
    assert.doesNotMatch(offline, /model-route-dot-live|bg-emerald-500/);
  });

  test(`${locale}: every row carries its own arrow toward the other pane; no checkboxes or central actions`, () => {
    const offline = { ...share, shareId: "offline", subdomain: "offline-subdomain", isOnline: false };
    const html = renderToStaticMarkup(<SharePoolPicker shares={[share, offline]} selected={[share.shareId, "revoked"]} app="codex" routeKey="codex" disabled={false} onChange={() => { throw new Error("static render must not mutate"); }} locale={locale} t={t} />);
    assert.ok(html.includes(t("modelHub.availableShares")));
    assert.ok(html.includes(t("modelHub.selectedShares")));
    assert.ok(html.includes(t("modelHub.candidatePrimary")));
    assert.ok(html.includes(t("modelHub.targetUnavailable")));
    assert.match(html, /offline-subdomain/);
    assert.match(html, /2\/6/);
    assert.equal([...html.matchAll(/type="checkbox"/g)].length, 0);
    assert.doesNotMatch(html, /model-route-transfer-actions/);
    assert.equal([...html.matchAll(/type="search"/g)].length, 0, "search stays hidden while the pool fits");
    assert.ok(html.includes(`aria-label="${t("modelHub.addToQueue", { name: "offline-subdomain" })}"`));
    assert.ok(html.includes(`aria-label="${t("modelHub.removeFromQueue", { name: "fixture-subdomain" })}"`));
    assert.ok(html.includes(`aria-label="${t("modelHub.removeFromQueue", { name: "revoked" })}"`), "missing targets stay removable");
    assert.match(html, /<ol[\s\S]*fixture-subdomain[\s\S]*revoked/);
    for (const button of html.matchAll(/<button\b[\s\S]*?<\/button>/g)) {
      assert.doesNotMatch(button[0].slice(7), /<button\b|<details\b|<summary\b/);
    }
    const many = Array.from({ length: 8 }, (_, index) => ({ ...share, shareId: `extra-${index}`, subdomain: `extra-${index}` }));
    const crowded = renderToStaticMarkup(<SharePoolPicker shares={many} selected={["extra-0"]} app="codex" routeKey="codex" disabled={false} onChange={() => { }} locale={locale} t={t} />);
    assert.equal([...crowded.matchAll(/type="search"/g)].length, 1, "only the pool is searchable; the queue holds at most six");
    const full = renderToStaticMarkup(<SharePoolPicker shares={many} selected={many.slice(0, 6).map(item => item.shareId)} app="codex" routeKey="codex" disabled={false} onChange={() => { }} locale={locale} t={t} />);
    assert.ok(full.includes(t("modelHub.queueFull")));
    for (const add of full.matchAll(/<button\b[^>]*aria-label="[^"]*extra-[67][^"]*"[^>]*>/g)) assert.match(add[0], /disabled=""/);
    const blocked = renderToStaticMarkup(<SharePoolPicker shares={[share]} selected={[]} app="codex" routeKey="codex" disabled={false} addBlockedReason={t("modelHub.validationTooMany")} onChange={() => { }} locale={locale} t={t} />);
    assert.ok(blocked.includes(t("modelHub.validationTooMany")));
    assert.match(blocked, /<button[^>]*disabled=""/);
    const locked = renderToStaticMarkup(<SharePoolPicker shares={[share, offline]} selected={[share.shareId]} app="codex" routeKey="codex" disabled onChange={() => { }} locale={locale} t={t} />);
    for (const button of locked.matchAll(/<button\b[^>]*>/g)) assert.match(button[0], /disabled=""/);
  });

  test(`${locale}: all protocol summaries keep the observed-protocol attribution in the tooltip`, () => {
    const source = { ...share, liveModelHealthStatus: share.appCapabilities![0].liveModelHealthStatus };
    const html = renderToStaticMarkup(<RoutingShareSummary share={source} shareId={share.shareId} app="claude" locale={locale} t={t} />);
    assert.match(html, /bg-emerald-500/);
    assert.match(html, /title="[^"]*other-protocol@example.test/);
    assert.doesNotMatch(html, /protocol-account@example.test/);
    assert.match(html, /title="[^"]*Share · /);
    assert.match(html, /title="[^"]*codex/);
  });
}

test("recent observations are independent of the browser clock", async () => {
  const { currentModelRouteResult } = await import("./model-hub-recent-result");
  const result = { routeId: "r", revision: 1, requestSequence: "2", observedAt: "2099-01-01T00:00:00Z", source: "request" as const, phase: "response_headers" as const, continuedSearch: false, usedBackup: false };
  const profile = { enabled: true, revision: 1, apiBaseUrl: "", eligibleShares: [], routes: [{ id: "r", appType: "codex" as const, requestedModel: "*", targetShareId: "s", createdAt: "", updatedAt: "" }], routeRecentResults: [result] };
  assert.equal(currentModelRouteResult(profile, "r", 0), result);
  assert.equal(currentModelRouteResult(profile, "r", Number.MAX_SAFE_INTEGER), result);
  assert.equal(currentModelRouteResult({ ...profile, revision: 2 }, "r"), undefined);
  assert.equal(currentModelRouteResult({ ...profile, routes: [] }, "r"), undefined);
  assert.equal(currentModelRouteResult({ ...profile, routeRecentResults: [{ ...result, observedAt: "invalid" }] }, "r"), undefined);
});

for (const locale of ["en", "zh-CN"] as const) {
  const t = translator(locale);
  test(`${locale}: rule labels, duplicate errors and target focus anchors are distinct`, () => {
    const html = renderToStaticMarkup(<>{["new:first", "new:second"].map((clientId, index) => <ModelHubRuleEditor key={clientId}
      kind="exact" position={index + 1} route={{ clientId, appType: "codex", requestedModel: "", targetShareId: share.shareId }} app="codex"
      appLabel={t("modelHub.protocol.codex")} shares={[share]} disabled={false} duplicate onAction={() => false} onTargets={() => {}} locale={locale} t={t} />)}</>);
    const labels = [...html.matchAll(/<section\b[^>]*aria-label="([^"]+)"/g)].map(match => match[1]);
    assert.deepEqual(labels, [1, 2].map(order => `${t("modelHub.protocol.codex")} · ${t("modelHub.unnamedRule", { order })}`));
    assert.doesNotMatch(html, /aria-label="[^"]*new:|aria-label="codex/, "labels never expose client ids or raw protocol values");
    assert.equal([...html.matchAll(/aria-invalid="true"/g)].length, 2);
    assert.ok(html.includes(t("modelHub.validationDuplicate")));
    assert.equal([...html.matchAll(/data-model-route-target="fixture"/g)].length, 2);
    const descriptions = [...html.matchAll(/<input\b[^>]*aria-invalid="true"[^>]*aria-describedby="([^"]+)"/g)].map(match => match[1]);
    assert.equal(new Set(descriptions).size, descriptions.length);
    assert.equal(descriptions.length, 2);
    for (const id of descriptions) assert.ok(html.includes(`id="${id}"`));
  });

  test(`${locale}: an exact rule whose draft model is * keeps its model input and delete button`, () => {
    const route = { clientId: "exact", appType: "codex" as const, requestedModel: "*", targetShareId: share.shareId };
    const exact = renderToStaticMarkup(<ModelHubRuleEditor kind="exact" route={route} app="codex" appLabel={t("modelHub.protocol.codex")}
      shares={[share]} disabled={false} duplicate onAction={() => false} onTargets={() => {}} onRemove={() => {}} locale={locale} t={t} />);
    assert.match(exact, /<input\b[^>]*value="\*"/);
    assert.ok(exact.includes(`aria-label="${t("modelHub.removeRoute")}"`));
    const fallback = renderToStaticMarkup(<ModelHubRuleEditor kind="default" route={route} app="codex" appLabel={t("modelHub.protocol.codex")}
      shares={[share]} disabled={false} onAction={() => false} onTargets={() => {}} locale={locale} t={t} />);
    assert.doesNotMatch(fallback, /value="\*"/);
    assert.ok(!fallback.includes(`aria-label="${t("modelHub.removeRoute")}"`));
  });

  test(`${locale}: recent diagnostics separate response headers from body completion and localize time`, () => {
    const result = { routeId: "r", revision: 1, requestSequence: "2", observedAt: "2099-01-01T13:45:00Z", source: "diagnostic" as const,
      phase: "body_incomplete" as const, statusCode: 200, continuedSearch: true, usedBackup: true, bodyState: "timeout" as const };
    const profile = { enabled: true, revision: 1, apiBaseUrl: "", eligibleShares: [], routes: [{ id: "r", appType: "codex" as const, requestedModel: "*", targetShareId: "s", createdAt: "", updatedAt: "" }], routeRecentResults: [result] };
    const html = renderToStaticMarkup(<ModelHubRecentResult profile={profile} routeId="r" locale={locale} t={t} />);
    assert.match(html, /HTTP 200/);
    assert.ok(html.includes(t("modelHub.recent.body_incomplete")));
    assert.ok(html.includes(t("modelHub.recent.backup")));
    assert.match(html, /dateTime="2099-01-01T13:45:00Z"/i);
    assert.ok(html.includes(new Date(result.observedAt).toLocaleString(locale)));
    assert.doesNotMatch(html, />2099-01-01T13:45:00Z<|body_incomplete|snapshot_mismatch|breaker_open/);
    const headers = renderToStaticMarkup(<ModelHubRecentResult profile={{ ...profile, routeRecentResults: [{ ...result, source: "request", phase: "response_headers" }] }} routeId="r" locale={locale} t={t} />);
    assert.ok(headers.includes(t("modelHub.recent.response_headers")));
  });
}
