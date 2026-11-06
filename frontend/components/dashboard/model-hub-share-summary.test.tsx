import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { messages, type AppLocale, type MessageKey } from "@/lib/i18n";
import type { UserModelRoutingShare } from "@/lib/types";
import { RoutingShareDetails, RoutingShareSummary } from "./model-hub-share-summary";
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
  test(`${locale}: summary uses a fixed-width, text-labelled health panel with phrasing-only markup`, () => {
    const props = { share, shareId: share.shareId, app: "codex" as const, locale, t };
    const html = renderToStaticMarkup(<RoutingShareSummary {...props} />);
    assert.match(html, /model-route-account/);
    assert.match(html, /model-route-health/);
    assert.match(html, /bg-emerald-50/);
    assert.ok(html.includes(t("dashboard.modelHealth.healthy")));
    assert.match(html, /protocol-account@example.test/);
    assert.doesNotMatch(html, /other-protocol@example.test|<div|<button|<details|<input|<a /);
    assert.match(renderToStaticMarkup(<RoutingShareDetails {...props} />), /<details[\s\S]*<summary/);
    const revoked = renderToStaticMarkup(<RoutingShareSummary {...props} share={undefined} />);
    assert.match(revoked, /bg-slate-50/);
    assert.ok(revoked.includes(t("modelHub.targetUnavailable")));
    assert.doesNotMatch(revoked, /protocol-account|fixture-subdomain/);
  });

  test(`${locale}: Transfer exposes checked lists and central actions, including offline and missing targets`, () => {
    const offline = { ...share, shareId: "offline", subdomain: "offline-subdomain", isOnline: false };
    const html = renderToStaticMarkup(<SharePoolPicker shares={[share, offline]} selected={[share.shareId, "revoked"]} app="codex" routeKey="codex" disabled={false} onChange={() => { throw new Error("static render must not mutate"); }} locale={locale} t={t} />);
    assert.ok(html.includes(t("modelHub.availableShares")));
    assert.ok(html.includes(t("modelHub.selectedShares")));
    assert.ok(html.includes(t("modelHub.candidatePrimary")));
    assert.ok(html.includes(t("modelHub.targetUnavailable")));
    assert.match(html, /offline-subdomain/);
    assert.match(html, /2\/6/);
    assert.equal([...html.matchAll(/type="search"/g)].length, 0, "search stays hidden while both lists fit");
    const many = Array.from({ length: 8 }, (_, index) => ({ ...share, shareId: `extra-${index}`, subdomain: `extra-${index}` }));
    const crowded = renderToStaticMarkup(<SharePoolPicker shares={many} selected={["extra-0"]} app="codex" routeKey="codex" disabled={false} onChange={() => { }} locale={locale} t={t} />);
    assert.equal([...crowded.matchAll(/type="search"/g)].length, 2);
    assert.equal([...html.matchAll(/type="checkbox"/g)].length, 5);
    assert.doesNotMatch(html, / checked=""/, "all transfer selections start unchecked");
    assert.ok(html.includes(t("modelHub.transferInLabel", { count: 0 })));
    assert.ok(html.includes(t("modelHub.transferOutLabel", { count: 0 })));
    assert.match(html, /model-route-transfer-actions/);
    assert.match(html, /<ol[\s\S]*fixture-subdomain[\s\S]*revoked/);
    for (const button of html.matchAll(/<button\b[\s\S]*?<\/button>/g)) {
      assert.doesNotMatch(button[0].slice(7), /<button\b|<details\b|<summary\b/);
    }
    const blocked = renderToStaticMarkup(<SharePoolPicker shares={[share]} selected={[]} app="codex" routeKey="codex" disabled={false} addBlockedReason={t("modelHub.validationTooMany")} onChange={() => { }} locale={locale} t={t} />);
    assert.ok(blocked.includes(t("modelHub.validationTooMany")));
    assert.match(blocked, /<button[^>]*disabled=""/);
    const locked = renderToStaticMarkup(<SharePoolPicker shares={[share, offline]} selected={[share.shareId]} app="codex" routeKey="codex" disabled onChange={() => { }} locale={locale} t={t} />);
    for (const input of locked.matchAll(/<input\b[^>]*type="checkbox"[^>]*>/g)) assert.match(input[0], /disabled=""/);
  });

  test(`${locale}: all protocol summaries use the Share account color while preserving observed-protocol attribution`, () => {
    const source = { ...share, liveModelHealthStatus: share.appCapabilities![0].liveModelHealthStatus };
    const html = renderToStaticMarkup(<RoutingShareSummary share={source} shareId={share.shareId} app="claude" locale={locale} t={t} />);
    assert.match(html, /bg-emerald-50/);
    assert.match(html, /other-protocol@example.test/);
    assert.doesNotMatch(html, /protocol-account@example.test/);
    assert.match(html, /Share · /);
    assert.match(html, /title="[^"]*codex/);
  });
}

test("collapsed dependency views do not mount repeated full reference lists", async () => {
  const { RoutingDependencyContext, RoutingShareDependencies } = await import("./model-hub-dependencies");
  const { buildShareDependencies } = await import("@/lib/model-routing");
  const routes = Array.from({ length: 50 }, (_, index) => ({ clientId: `r${index}`, appType: "codex" as const, requestedModel: `unique-model-${index}`, targetShareId: share.shareId, candidates: ["b", "c"] }));
  const summaries = buildShareDependencies("saved", routes, [share]);
  const html = renderToStaticMarkup(<RoutingDependencyContext.Provider value={{ summaries, shares: [share], view: "saved", onLocate: () => {} }}>
    <RoutingShareDependencies shareId={share.shareId} t={translator("en")} />
  </RoutingDependencyContext.Provider>);
  assert.ok(html.includes("50"));
  assert.doesNotMatch(html, /unique-model-|<li/);
});

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
