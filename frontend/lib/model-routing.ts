import type {
  DashboardClient,
  ModelRoutingApp,
  ShareView,
  UserModelRouteInput,
  UserModelRoutingResponse,
  UserModelRoutingShare,
} from "@/lib/types";
import { shellQuote } from "@/lib/share-model-probe";

export const MAX_USER_MODEL_ROUTES = 100;
/**
 * Reserved `requestedModel` value meaning "every model for this app".
 *
 * Deliberately all-or-nothing: the Router accepts `*` on its own but rejects any
 * other model name containing `*`, so a catch-all can never degrade into prefix
 * or regex matching. Exact routes always win over it.
 */
export const WILDCARD_MODEL = "*";
export const CLIENT_LIST_TABS = [
  "mine",
  "all",
  "online",
  "reconnecting",
  "degraded",
  "offline",
] as const;

export type ClientListTab = (typeof CLIENT_LIST_TABS)[number];
export type DraftModelRoute = UserModelRouteInput & { clientId: string };
export type ModelRouteValidationError =
  | "required"
  | "duplicate"
  | "too_many"
  | "pattern";
export const MODEL_ROUTING_PROTOCOLS = ["claude", "codex", "gemini"] as const;
export type ModelRoutingProtocol = (typeof MODEL_ROUTING_PROTOCOLS)[number];
export type ModelRoutingProtocolMode =
  | "empty"
  | "passthrough"
  | "exact"
  | "mixed";

export type ModelRoutingProtocolSlot = {
  appType: ModelRoutingProtocol;
  passthrough: DraftModelRoute | null;
  exact: DraftModelRoute[];
};

export function protocolSlotMode(
  slot: Pick<ModelRoutingProtocolSlot, "passthrough" | "exact">,
): ModelRoutingProtocolMode {
  if (slot.passthrough && slot.exact.length) return "mixed";
  if (slot.passthrough) return "passthrough";
  if (slot.exact.length) return "exact";
  return "empty";
}

export function protocolHasAttention(
  routes: Array<UserModelRouteInput | DraftModelRoute>,
  eligibleShares: UserModelRoutingShare[],
  appType: ModelRoutingApp,
) {
  const eligibleIds = new Set(
    sharesForProtocol(eligibleShares, appType).map((share) => share.shareId),
  );
  return routes.some(
    (route) =>
      route.appType === appType &&
      route.targetShareId &&
      !eligibleIds.has(route.targetShareId),
  );
}

/**
 * Pick the protocol tab to open first: anything that needs attention, else the
 * first protocol that already has a route, else OpenAI. OpenAI is the default
 * empty-state tab because it is the most common unified-entry protocol.
 */
export function defaultModelRoutingProtocol(
  routes: Array<UserModelRouteInput | DraftModelRoute>,
  eligibleShares: UserModelRoutingShare[],
): ModelRoutingProtocol {
  const slots = groupModelRoutesByProtocol(
    routes.map((route, index) =>
      "clientId" in route
        ? (route as DraftModelRoute)
        : { ...route, clientId: `saved:${index}` },
    ),
  );
  const attention = slots.find((slot) =>
    protocolHasAttention(routes, eligibleShares, slot.appType),
  );
  if (attention) return attention.appType;
  const configured = slots.find((slot) => protocolSlotMode(slot) !== "empty");
  if (configured) return configured.appType;
  return "codex";
}

export function defaultTestModelForProtocol(
  slot: Pick<ModelRoutingProtocolSlot, "exact">,
) {
  return slot.exact.find((route) => route.requestedModel.trim())?.requestedModel.trim() || "";
}

export function groupModelRoutesByProtocol(
  routes: DraftModelRoute[],
): ModelRoutingProtocolSlot[] {
  return MODEL_ROUTING_PROTOCOLS.map((appType) => {
    const forApp = routes.filter((route) => route.appType === appType);
    const passthrough = forApp.find((route) => isWildcardModel(route.requestedModel)) || null;
    const exact = forApp.filter((route) => !isWildcardModel(route.requestedModel));
    return { appType, passthrough, exact };
  });
}

export function sharesForProtocol(
  shares: UserModelRoutingShare[],
  appType: ModelRoutingApp,
) {
  return shares.filter((share) => share.apps.includes(appType));
}

export function firstShareForProtocol(
  shares: UserModelRoutingShare[],
  appType: ModelRoutingApp,
) {
  return sharesForProtocol(shares, appType)[0] || null;
}

export function newDraftModelRoute(
  appType: ModelRoutingApp,
  requestedModel: string,
  targetShareId: string,
): DraftModelRoute {
  const id = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random()}`;
  return {
    clientId: `new:${id}`,
    appType,
    requestedModel,
    targetShareId,
  };
}

export function isWildcardModel(requestedModel: string) {
  return requestedModel.trim() === WILDCARD_MODEL;
}

/**
 * True when this app is a *pure passthrough*: a single `*` route and nothing
 * else. The Router treats that shape specially — the unified entry point becomes
 * a plain forwarding layer for that Share, so `GET /v1/models` is forwarded to
 * it and returns its real catalog instead of a synthesised list. Adding any
 * exact route takes the app out of this mode. See PROTOCOL.md 9.2.
 */
export function isPassthroughOnlyApp(
  routes: Array<UserModelRouteInput | DraftModelRoute>,
  appType: ModelRoutingApp,
) {
  const forApp = routes.filter((route) => route.appType === appType);
  return forApp.length === 1 && isWildcardModel(forApp[0].requestedModel);
}

export function hasWildcardForApp(
  routes: Array<UserModelRouteInput | DraftModelRoute>,
  appType: ModelRoutingApp,
) {
  return routes.some(
    (route) => route.appType === appType && isWildcardModel(route.requestedModel),
  );
}

export function buildUnifiedModelCurl(
  baseUrlValue: string,
  token: string,
  route?: Pick<UserModelRouteInput, "appType" | "requestedModel">,
) {
  const baseUrl = baseUrlValue.replace(/\/+$/, "");
  const apiKey = token || "<YOUR_API_KEY>";
  // A wildcard route has no callable model name of its own — it forwards
  // whatever the client asks for — so the sample stays a placeholder.
  const requested = route?.requestedModel.trim() || "";
  const model = !requested || requested === WILDCARD_MODEL ? "<MODEL>" : requested;
  let url = `${baseUrl}/v1/responses`;
  let authHeader = `Authorization: Bearer ${apiKey}`;
  let body: unknown = { model, input: "Hello" };

  const extraHeaders: string[] = [];
  if (route?.appType === "claude") {
    url = `${baseUrl}/v1/messages`;
    authHeader = `x-api-key: ${apiKey}`;
    extraHeaders.push("anthropic-version: 2023-06-01");
    body = {
      model,
      max_tokens: 32,
      messages: [{ role: "user", content: "Hello" }],
    };
  } else if (route?.appType === "gemini") {
    const wireModel = model === "<MODEL>" ? model : encodeURIComponent(model);
    url = `${baseUrl}/v1beta/models/${wireModel}:generateContent`;
    authHeader = `x-goog-api-key: ${apiKey}`;
    body = { contents: [{ parts: [{ text: "Hello" }] }] };
  }

  return [
    "curl -sS \\",
    `  ${shellQuote(url)} \\`,
    `  -H ${shellQuote(authHeader)} \\`,
    ...extraHeaders.map((header) => `  -H ${shellQuote(header)} \\`),
    `  -H ${shellQuote("content-type: application/json")} \\`,
    `  -d ${shellQuote(JSON.stringify(body))}`,
  ].join("\n");
}

export function canonicalModelRoutes(
  routes: Array<UserModelRouteInput | DraftModelRoute>,
): UserModelRouteInput[] {
  return routes
    .map((route) => ({
      appType: route.appType,
      requestedModel: route.requestedModel.trim(),
      targetShareId: route.targetShareId.trim(),
    }))
    .sort((left, right) => {
      if (left.appType !== right.appType) {
        return left.appType < right.appType ? -1 : 1;
      }
      if (left.requestedModel === right.requestedModel) return 0;
      return left.requestedModel < right.requestedModel ? -1 : 1;
    });
}

export function validateModelRoutes(
  routes: Array<UserModelRouteInput | DraftModelRoute>,
): ModelRouteValidationError | null {
  const normalized = canonicalModelRoutes(routes);
  if (normalized.length > MAX_USER_MODEL_ROUTES) return "too_many";
  if (
    normalized.some(
      (route) => !route.requestedModel || !route.targetShareId,
    )
  ) {
    return "required";
  }
  if (
    normalized.some(
      (route) =>
        route.requestedModel !== WILDCARD_MODEL &&
        route.requestedModel.includes(WILDCARD_MODEL),
    )
  ) {
    return "pattern";
  }
  const keys = new Set(
    normalized.map(
      (route) => `${route.appType}\u0000${route.requestedModel}`,
    ),
  );
  return keys.size === normalized.length ? null : "duplicate";
}

export function preferredModelRoutingApp(
  share?: UserModelRoutingShare,
): ModelRoutingApp {
  if (share?.apps.includes("codex")) return "codex";
  if (share?.apps.includes("claude")) return "claude";
  return share?.apps[0] || "gemini";
}

export function patchDraftModelRoute(
  routes: DraftModelRoute[],
  clientId: string,
  patch: Partial<UserModelRouteInput>,
  eligibleShares: UserModelRoutingShare[],
): DraftModelRoute[] {
  return routes.map((route) => {
    if (route.clientId !== clientId) return route;
    const next = { ...route, ...patch };
    if (patch.appType) {
      const currentShare = eligibleShares.find(
        (share) => share.shareId === next.targetShareId,
      );
      if (!currentShare?.apps.includes(patch.appType)) {
        next.targetShareId =
          eligibleShares.find((share) => share.apps.includes(patch.appType!))
            ?.shareId || "";
      }
    }
    return next;
  });
}

export function normalizeClientListTab(
  value: unknown,
  hasViewerIdentity: boolean,
): ClientListTab {
  if (
    typeof value !== "string" ||
    !(CLIENT_LIST_TABS as readonly string[]).includes(value)
  ) {
    return "all";
  }
  return value === "mine" && !hasViewerIdentity
    ? "all"
    : (value as ClientListTab);
}

export function clientListTabFromQuery(
  storedValue: unknown,
  queryTab: string | null,
  hasViewerIdentity: boolean,
): ClientListTab {
  if (queryTab === "mine") {
    return normalizeClientListTab("mine", hasViewerIdentity);
  }
  const stored = normalizeClientListTab(storedValue, hasViewerIdentity);
  return stored === "mine" ? "all" : stored;
}

export function searchForClientListTab(
  currentSearch: string,
  tab: ClientListTab,
): string {
  const params = new URLSearchParams(currentSearch);
  if (tab === "mine") params.set("tab", "mine");
  else params.delete("tab");
  if (tab !== "mine") {
    params.delete("shareId");
    params.delete("action");
  }
  return params.toString();
}

export function modelRouteDeepLinkShareId(
  currentSearch: string,
): string | null {
  const params = new URLSearchParams(currentSearch);
  if (params.get("tab") !== "mine" || params.get("action") !== "add-route") {
    return null;
  }
  return params.get("shareId")?.trim() || null;
}

export function consumeModelRouteDeepLink(currentSearch: string): string {
  const params = new URLSearchParams(currentSearch);
  params.delete("shareId");
  params.delete("action");
  return params.toString();
}

export function configuredEligibleRouteShareIds(
  profile: UserModelRoutingResponse | null,
): ReadonlySet<string> {
  if (!profile) return new Set<string>();
  const eligible = new Set(
    profile.eligibleShares.map((share) => share.shareId),
  );
  return new Set(
    profile.routes
      .map((route) => route.targetShareId)
      .filter((shareId) => eligible.has(shareId)),
  );
}

function normalizeEmail(value?: string) {
  return value?.trim().toLowerCase() || "";
}

function shareGrantAllowsViewer(
  share: ShareView,
  viewerEmail: string,
  nowMs: number,
) {
  return Object.entries(share.userGrants || {}).some(([key, grant]) => {
    const identifiesViewer =
      normalizeEmail(key) === viewerEmail ||
      normalizeEmail(grant.email) === viewerEmail;
    return (
      identifiesViewer &&
      grant.role === "shareto" &&
      grant.active === true &&
      (grant.policy.expiresAt == null || grant.policy.expiresAt > nowMs)
    );
  });
}

export function clientOwnedByViewer(
  client: DashboardClient,
  viewerEmailValue: string,
) {
  const viewerEmail = normalizeEmail(viewerEmailValue);
  if (!viewerEmail) return false;
  return (
    normalizeEmail(
      client.clientTunnel?.ownerEmail || client.installation.ownerEmail,
    ) === viewerEmail
  );
}

export function shareBelongsToViewer(
  share: ShareView,
  viewerEmailValue: string,
  explicitlyRoutedShareIds: ReadonlySet<string> = new Set<string>(),
  nowMs = Date.now(),
) {
  const viewerEmail = normalizeEmail(viewerEmailValue);
  if (!viewerEmail) return false;
  if (explicitlyRoutedShareIds.has(share.shareId)) return true;
  return (
    normalizeEmail(share.ownerEmail) === viewerEmail ||
    shareGrantAllowsViewer(share, viewerEmail, nowMs)
  );
}

export function listViewerShares(
  shares: ShareView[],
  clients: DashboardClient[],
  viewerEmailValue: string,
  explicitlyRoutedShareIds: ReadonlySet<string> = new Set<string>(),
  nowMs = Date.now(),
) {
  const viewerEmail = normalizeEmail(viewerEmailValue);
  if (!viewerEmail) return [];
  const hostedShareIds = new Set(
    clients
      .filter((client) => clientOwnedByViewer(client, viewerEmail))
      .flatMap((client) => client.shareIds || []),
  );
  const seen = new Set<string>();
  return shares.filter((share) => {
    if (seen.has(share.shareId)) return false;
    const mine =
      hostedShareIds.has(share.shareId) ||
      shareBelongsToViewer(share, viewerEmail, explicitlyRoutedShareIds, nowMs);
    if (!mine) return false;
    seen.add(share.shareId);
    return true;
  });
}

export function clientBelongsToViewer(
  client: DashboardClient,
  shareById: ReadonlyMap<string, ShareView>,
  viewerEmailValue: string,
  explicitlyRoutedShareIds: ReadonlySet<string> = new Set<string>(),
  nowMs = Date.now(),
) {
  if (clientOwnedByViewer(client, viewerEmailValue)) return true;
  return (client.shareIds || []).some((shareId) => {
    const share = shareById.get(shareId);
    return share
      ? shareBelongsToViewer(share, viewerEmailValue, explicitlyRoutedShareIds, nowMs)
      : explicitlyRoutedShareIds.has(shareId);
  });
}
