import assert from "node:assert/strict";
import test from "node:test";
import {
  buildUnifiedModelCurl,
  canonicalModelRoutes,
  clientBelongsToViewer,
  defaultModelRoutingProtocol,
  defaultTestModelForProtocol,
  firstShareForProtocol,
  groupModelRoutesByProtocol,
  protocolHasAttention,
  hasWildcardForApp,
  isWildcardModel,
  isPassthroughOnlyApp,
  clientListTabFromQuery,
  configuredEligibleRouteShareIds,
  consumeModelRouteDeepLink,
  modelRouteDeepLinkShareId,
  patchDraftModelRoute,
  protocolSlotMode,
  searchForClientListTab,
  sharesForProtocol,
  validateModelRoutes,
  WILDCARD_MODEL,
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
  const claude = buildUnifiedModelCurl("https://api.example.com", "key", {
    appType: "claude",
    requestedModel: "claude-opus-4",
  });
  assert.match(claude, /anthropic-version: 2023-06-01/);

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

test("the standalone wildcard is accepted but partial patterns are not", () => {
  assert.equal(
    validateModelRoutes([
      { appType: "codex", requestedModel: "gpt-5.6-sol", targetShareId: "share-codex" },
      { appType: "codex", requestedModel: WILDCARD_MODEL, targetShareId: "share-claude" },
    ]),
    null,
  );

  // Anything else containing `*` must be refused, otherwise the catch-all would
  // read as prefix/suffix matching the Router deliberately does not implement.
  for (const pattern of ["gpt-*", "*-turbo", "a*b", "**"]) {
    assert.equal(
      validateModelRoutes([
        { appType: "codex", requestedModel: pattern, targetShareId: "share-codex" },
      ]),
      "pattern",
      `${pattern} must be rejected`,
    );
  }

  // One wildcard per app: a second one for the same app is an ordinary duplicate.
  assert.equal(
    validateModelRoutes([
      { appType: "codex", requestedModel: WILDCARD_MODEL, targetShareId: "share-codex" },
      { appType: "codex", requestedModel: WILDCARD_MODEL, targetShareId: "share-claude" },
    ]),
    "duplicate",
  );
  assert.equal(
    validateModelRoutes([
      { appType: "codex", requestedModel: WILDCARD_MODEL, targetShareId: "share-codex" },
      { appType: "claude", requestedModel: WILDCARD_MODEL, targetShareId: "share-claude" },
    ]),
    null,
  );
});

test("wildcard helpers ignore surrounding whitespace and stay per-app", () => {
  assert.equal(isWildcardModel(" * "), true);
  assert.equal(isWildcardModel("gpt-*"), false);
  assert.equal(isWildcardModel(""), false);

  const routes = [
    { appType: "codex" as const, requestedModel: "*", targetShareId: "share-codex" },
    { appType: "claude" as const, requestedModel: "opus", targetShareId: "share-claude" },
  ];
  assert.equal(hasWildcardForApp(routes, "codex"), true);
  assert.equal(hasWildcardForApp(routes, "claude"), false);
  assert.equal(hasWildcardForApp(routes, "gemini"), false);
});

test("wildcard routes render curl with a placeholder instead of the reserved token", () => {
  // `model: "*"` would be forwarded verbatim and rejected upstream, so the
  // sample must never suggest it as a callable model name.
  const codex = buildUnifiedModelCurl("https://api.example.com", "sk-test", {
    appType: "codex",
    requestedModel: WILDCARD_MODEL,
  });
  assert.match(codex, /"model":"<MODEL>"/);
  assert.ok(!codex.includes('"model":"*"'));

  const gemini = buildUnifiedModelCurl("https://api.example.com", "sk-test", {
    appType: "gemini",
    requestedModel: WILDCARD_MODEL,
  });
  assert.match(gemini, /\/v1beta\/models\/<MODEL>:generateContent/);

  // Exact routes keep percent-encoding their real model name.
  const exact = buildUnifiedModelCurl("https://api.example.com", "sk-test", {
    appType: "gemini",
    requestedModel: "gemini/pro",
  });
  assert.match(exact, /gemini%2Fpro:generateContent/);
});

test("wildcards sort ahead of exact keys so canonical drafts stay stable", () => {
  const canonical = canonicalModelRoutes([
    { appType: "codex", requestedModel: "gpt-5.6-sol", targetShareId: "share-a" },
    { appType: "codex", requestedModel: " * ", targetShareId: "share-c" },
    { appType: "claude", requestedModel: "*", targetShareId: "share-d" },
  ]);
  assert.deepEqual(
    canonical.map((route) => `${route.appType}:${route.requestedModel}`),
    ["claude:*", "codex:*", "codex:gpt-5.6-sol"],
  );
});

test("pure passthrough is a lone wildcard, and any exact route ends it", () => {
  const wildcard = { appType: "codex" as const, requestedModel: "*", targetShareId: "share-c", clientId: "1" };
  const exact = { appType: "codex" as const, requestedModel: "gpt-5.6-sol", targetShareId: "share-a", clientId: "2" };
  const otherApp = { appType: "claude" as const, requestedModel: "claude-opus-5", targetShareId: "share-b", clientId: "3" };

  assert.equal(isPassthroughOnlyApp([wildcard], "codex"), true);
  // Another app's routes are irrelevant to this app's shape.
  assert.equal(isPassthroughOnlyApp([wildcard, otherApp], "codex"), true);
  // One exact route alongside it and no single upstream represents the entry point.
  assert.equal(isPassthroughOnlyApp([wildcard, exact], "codex"), false);
  assert.equal(isPassthroughOnlyApp([exact], "codex"), false);
  assert.equal(isPassthroughOnlyApp([], "codex"), false);
  // The app with no wildcard is never passthrough, even when another one is.
  assert.equal(isPassthroughOnlyApp([wildcard, otherApp], "claude"), false);
});

test("protocol slots keep one passthrough and many exact models without emitting empty apps", () => {
  const routes: DraftModelRoute[] = [
    { clientId: "w", appType: "codex", requestedModel: "*", targetShareId: "share-a" },
    { clientId: "e", appType: "codex", requestedModel: "gpt-5.5", targetShareId: "share-b" },
    { clientId: "c", appType: "claude", requestedModel: "opus", targetShareId: "share-c" },
  ];
  const slots = groupModelRoutesByProtocol(routes);
  assert.deepEqual(slots.map((slot) => slot.appType), ["claude", "codex", "gemini"]);
  assert.equal(protocolSlotMode(slots[0]), "exact");
  assert.equal(protocolSlotMode(slots[1]), "mixed");
  assert.equal(protocolSlotMode(slots[2]), "empty");
  assert.equal(slots[1].passthrough?.targetShareId, "share-a");
  assert.deepEqual(slots[1].exact.map((route) => route.requestedModel), ["gpt-5.5"]);
  assert.equal(slots[2].passthrough, null);
  assert.deepEqual(slots[2].exact, []);

  const saved = canonicalModelRoutes([
    ...slots.flatMap((slot) => [
      ...(slot.passthrough ? [slot.passthrough] : []),
      ...slot.exact,
    ]),
  ]);
  assert.deepEqual(
    saved.map((route) => `${route.appType}:${route.requestedModel}`),
    ["claude:opus", "codex:*", "codex:gpt-5.5"],
  );
});

test("the first protocol tab prefers attention, then a configured protocol, then OpenAI", () => {
  const mixed: UserModelRoutingShare[] = [
    shares[0],
    shares[1],
    {
      shareId: "share-gemini",
      shareName: "Gemini",
      subdomain: "gemini-share",
      directApiUrl: "https://gemini-share.example.com",
      access: "owner",
      freeAccess: false,
      apps: ["gemini"],
      isOnline: true,
    },
  ];
  assert.equal(defaultModelRoutingProtocol([], mixed), "codex");
  assert.equal(
    defaultModelRoutingProtocol(
      [{ appType: "gemini", requestedModel: "gemini-pro", targetShareId: "share-gemini" }],
      mixed,
    ),
    "gemini",
  );
  assert.equal(
    defaultModelRoutingProtocol(
      [
        { appType: "claude", requestedModel: "opus", targetShareId: "share-claude" },
        { appType: "codex", requestedModel: "gpt-5.6", targetShareId: "missing" },
      ],
      mixed,
    ),
    "codex",
  );
  assert.equal(
    protocolHasAttention(
      [{ appType: "codex", requestedModel: "gpt-5.6", targetShareId: "missing" }],
      mixed,
      "codex",
    ),
    true,
  );
  const slots = groupModelRoutesByProtocol([
    { clientId: "1", appType: "codex", requestedModel: "gpt-5.6", targetShareId: "share-codex" },
    { clientId: "2", appType: "codex", requestedModel: "*", targetShareId: "share-codex" },
  ]);
  assert.equal(defaultTestModelForProtocol(slots[1]), "gpt-5.6");
});

test("protocol share pickers stay inside the enabled app and prefer the first eligible Share", () => {
  const mixed: UserModelRoutingShare[] = [
    { ...shares[1], shareId: "share-multi", apps: ["claude", "codex"] },
    shares[0],
    shares[1],
  ];
  assert.deepEqual(
    sharesForProtocol(mixed, "codex").map((share) => share.shareId),
    ["share-multi", "share-codex"],
  );
  assert.equal(firstShareForProtocol(mixed, "gemini"), null);
  assert.equal(firstShareForProtocol(mixed, "claude")?.shareId, "share-multi");
});
