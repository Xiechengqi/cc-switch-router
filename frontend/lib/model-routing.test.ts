import assert from "node:assert/strict";
import test from "node:test";
import {
  buildUnifiedModelCurl,
  canonicalModelRoutes,
  clientBelongsToViewer,
  clientListTabFromQuery,
  configuredEligibleRouteShareIds,
  consumeModelRouteDeepLink,
  modelRouteDeepLinkShareId,
  patchDraftModelRoute,
  searchForClientListTab,
  validateModelRoutes,
  type DraftModelRoute,
} from "@/lib/model-routing";
import type {
  DashboardClient,
  ShareView,
  UserModelRoutingResponse,
  UserModelRoutingShare,
} from "@/lib/types";

const shares: UserModelRoutingShare[] = [
  {
    shareId: "share-codex",
    shareName: "Codex",
    subdomain: "codex-share",
    directApiUrl: "https://codex-share.example.com",
    access: "owner",
    freeAccess: false,
    apps: ["codex"],
    isOnline: true,
  },
  {
    shareId: "share-claude",
    shareName: "Claude",
    subdomain: "claude-share",
    directApiUrl: "https://claude-share.example.com",
    access: "free",
    freeAccess: true,
    apps: ["claude"],
    isOnline: true,
  },
];

test("unified curl examples shell-quote values and encode Gemini model paths", () => {
  const codex = buildUnifiedModelCurl(
    "https://api.example.com/",
    "key-with-'quote",
    {
      appType: "codex",
      requestedModel: "owner's-$(touch /tmp/not-run)",
    },
  );
  assert.match(codex, /key-with-'"'"'quote/);
  assert.match(codex, /owner'"'"'s-\$\(touch \/tmp\/not-run\)/);
  assert.ok(!codex.includes("https://api.example.com//v1"));

  const gemini = buildUnifiedModelCurl("https://api.example.com", "key", {
    appType: "gemini",
    requestedModel: "publishers/google/gemini:pro?mode#one",
  });
  assert.match(
    gemini,
    /publishers%2Fgoogle%2Fgemini%3Apro%3Fmode%23one:generateContent/,
  );
});

test("model route keys are trimmed, deterministic, and case-sensitive", () => {
  const routes = canonicalModelRoutes([
    { appType: "codex", requestedModel: " GPT-5.6 ", targetShareId: " one " },
    { appType: "claude", requestedModel: "sonnet", targetShareId: "two" },
  ]);
  assert.deepEqual(routes, [
    { appType: "claude", requestedModel: "sonnet", targetShareId: "two" },
    { appType: "codex", requestedModel: "GPT-5.6", targetShareId: "one" },
  ]);
  assert.equal(
    validateModelRoutes([
      ...routes,
      { appType: "codex", requestedModel: "gpt-5.6", targetShareId: "two" },
    ]),
    null,
  );
  assert.equal(
    validateModelRoutes([
      ...routes,
      { appType: "codex", requestedModel: " GPT-5.6 ", targetShareId: "two" },
    ]),
    "duplicate",
  );
});

test("changing an app atomically moves only the edited route to an eligible Share", () => {
  const routes: DraftModelRoute[] = [
    {
      clientId: "route-1",
      appType: "codex",
      requestedModel: "gpt-5.6",
      targetShareId: "share-codex",
    },
  ];
  const updated = patchDraftModelRoute(
    routes,
    "route-1",
    { appType: "claude" },
    shares,
  );
  assert.equal(updated.length, 1);
  assert.deepEqual(updated[0], {
    clientId: "route-1",
    appType: "claude",
    requestedModel: "gpt-5.6",
    targetShareId: "share-claude",
  });
  assert.equal(routes[0]?.targetShareId, "share-codex");
});

test("mine tab query state preserves unrelated dashboard parameters", () => {
  const mine = searchForClientListTab(
    "focusKind=share&focusId=s-1&region=jp",
    "mine",
  );
  assert.equal(new URLSearchParams(mine).get("tab"), "mine");
  assert.equal(new URLSearchParams(mine).get("focusId"), "s-1");
  assert.equal(clientListTabFromQuery("offline", "mine", true), "mine");
  assert.equal(clientListTabFromQuery("offline", null, true), "offline");
  assert.equal(clientListTabFromQuery("mine", null, true), "all");
  assert.equal(clientListTabFromQuery("all", "mine", false), "all");

  const other = searchForClientListTab(
    "tab=mine&shareId=s-2&action=add-route&region=jp",
    "online",
  );
  const params = new URLSearchParams(other);
  assert.equal(params.get("tab"), null);
  assert.equal(params.get("shareId"), null);
  assert.equal(params.get("action"), null);
  assert.equal(params.get("region"), "jp");
});

test("add-route deep links are consumed without removing the mine tab", () => {
  const search = "tab=mine&shareId=share-claude&action=add-route&region=jp";
  assert.equal(modelRouteDeepLinkShareId(search), "share-claude");
  const consumed = new URLSearchParams(consumeModelRouteDeepLink(search));
  assert.equal(consumed.get("tab"), "mine");
  assert.equal(consumed.get("region"), "jp");
  assert.equal(consumed.get("shareId"), null);
  assert.equal(consumed.get("action"), null);
});

test("only currently eligible configured targets extend the mine view", () => {
  const profile: UserModelRoutingResponse = {
    enabled: true,
    apiBaseUrl: "https://api.example.com",
    revision: 2,
    eligibleShares: shares,
    routes: [
      {
        id: "one",
        appType: "claude",
        requestedModel: "sonnet",
        targetShareId: "share-claude",
        createdAt: "2026-01-01T00:00:00Z",
        updatedAt: "2026-01-01T00:00:00Z",
      },
      {
        id: "stale",
        appType: "codex",
        requestedModel: "old",
        targetShareId: "share-revoked",
        createdAt: "2026-01-01T00:00:00Z",
        updatedAt: "2026-01-01T00:00:00Z",
      },
    ],
  };
  assert.deepEqual(
    [...configuredEligibleRouteShareIds(profile)],
    ["share-claude"],
  );
});

test("mine membership accepts owners, live ShareTo grants, and explicitly routed free Shares", () => {
  const client = {
    installation: { id: "client-1", ownerEmail: "host@example.com" },
    shareIds: ["owned", "shared", "free"],
  } as DashboardClient;
  const baseShare = {
    shareName: "Share",
    subdomain: "share-name",
    userGrants: {},
  } as ShareView;
  const shareById = new Map<string, ShareView>([
    ["owned", { ...baseShare, shareId: "owned", ownerEmail: "owner@example.com" }],
    [
      "shared",
      {
        ...baseShare,
        shareId: "shared",
        userGrants: {
          alias: {
            email: "viewer@example.com",
            role: "shareto",
            active: true,
            policy: { tokenPeriod: "lifetime", expiresAt: 2_000 },
          },
        },
      },
    ],
    ["free", { ...baseShare, shareId: "free", freeAccess: true }],
  ]);

  assert.equal(
    clientBelongsToViewer(client, shareById, "OWNER@example.com", new Set(), 1_000),
    true,
  );
  assert.equal(
    clientBelongsToViewer(client, shareById, "viewer@example.com", new Set(), 1_000),
    true,
  );
  assert.equal(
    clientBelongsToViewer(client, shareById, "viewer@example.com", new Set(), 3_000),
    false,
  );
  assert.equal(
    clientBelongsToViewer(client, shareById, "free-user@example.com", new Set(["free"]), 3_000),
    true,
  );
});
