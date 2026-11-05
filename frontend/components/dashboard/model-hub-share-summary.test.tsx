import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { messages, type AppLocale, type MessageKey } from "@/lib/i18n";
import type { UserModelRoutingShare } from "@/lib/types";
import { RoutingShareDetails, RoutingShareSummary } from "./model-hub-share-summary";
import { SharePoolPicker } from "./model-hub-share-picker";

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
    assert.equal([...html.matchAll(/type="search"/g)].length, 2);
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
