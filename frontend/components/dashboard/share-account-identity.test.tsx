import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "@/components/i18n/locale-provider";
import { ClientProvidersPanel, ShareProvidersPanel } from "./drawer-panels";
import { ShareProviderStatusPanel } from "./share-provider-status-panel";
import { modelRoutingShareAccountView } from "@/lib/model-routing";
import type { ShareView, UserModelRoutingShare } from "@/lib/types";

test("masked routing identity is identical in account text and native tooltip", () => {
  const share: UserModelRoutingShare = {
    shareId: "fixture", shareName: "Fixture", subdomain: "fixture", directApiUrl: "https://fixture.test",
    access: "shared", freeAccess: false, apps: ["codex"], isOnline: true,
    appCapabilities: [{ app: "codex", kind: "codex_oauth", accountLabel: "a***e@example.test" }],
  };
  const view = modelRoutingShareAccountView(share, "codex", "en", {passthrough: "Passthrough", unavailable: "Unavailable"});
  const html = renderToStaticMarkup(<ShareProviderStatusPanel view={view} />);
  assert.match(html, /title="a\*\*\*e@example\.test"[^>]*>a\*\*\*e@example\.test</);
  assert.doesNotMatch(html, /alice@example\.test/);
  const missing = modelRoutingShareAccountView({...share, appCapabilities: [{ app: "codex", kind: "codex_oauth", providerName: "legacy-private-account" }]}, "codex", "en", {passthrough: "Passthrough", unavailable: "Unavailable"});
  assert.equal(missing.identityLine, "-", "non-owners cannot recover an identity through a display-name fallback");
});

function source(shareId: string, owner: boolean): ShareView {
  return {
    shareId, capacityPoolId: `pool-${shareId}`, shareName: shareId, subdomain: shareId,
    freeAccess: false, canManage: owner, appType: "codex", support: { codex: true },
    tokenLimit: 0, parallelLimit: 1, tokensUsed: 0, requestsCount: 0,
    shareStatus: "active", createdAt: "2026-11-15T00:00:00Z", expiresAt: "2027-11-15T00:00:00Z",
    isOnline: true, routeState: "active", activeRequests: 0, onlineRate24h: 100,
    bindings: { codex: `provider-${shareId}` },
    appProviders: { codex: [{ id: `provider-${shareId}`, app: "codex", name: "Cursor", kind: "cursor_oauth", bundleId: "same-bundle" }] },
    appRuntimes: { cursor: { kind: "cursor_oauth", accountLabel: owner ? "owner-account@example.test" : "m***d@example.test" } },
  };
}

test("Client provider groups keep each Share's identity scope and standalone runtime", () => {
  const owner = source("owner", true);
  const shared = source("shared", false);
  for (const shares of [[owner, shared], [shared, owner]]) {
    const html = renderToStaticMarkup(<LocaleProvider><ClientProvidersPanel shares={shares} /></LocaleProvider>);
    assert.equal(html.match(/owner-account@example\.test/g)?.length, 2, "only its owner's panel text and title may contain the full account");
    assert.equal(html.match(/m\*\*\*d@example\.test/g)?.length, 2, "the non-owner panel keeps its own masked runtime");
  }
  const sharedOnly = renderToStaticMarkup(<LocaleProvider><ShareProvidersPanel share={shared} /></LocaleProvider>);
  assert.doesNotMatch(sharedOnly, /owner-account@example\.test/);
  assert.match(sharedOnly, /m\*\*\*d@example\.test/);
});
