import type {
  DashboardClient,
  ModelRoutingApp,
  ShareUpstreamProvider,
  ShareView,
  UserModelRouteInput,
  UserModelRoutingResponse,
  UserModelRoutingShare,
  UserModelRoutingShareCapability,
} from "@/lib/types";
import { shellQuote } from "@/lib/share-model-probe";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import type { Announcements } from "@dnd-kit/core";
import {
  isApiProviderRuntime,
  providerActualModelNames,
  providerApiEndpoint,
  providerQuotaStatusLine,
  providerStatusIdentity,
} from "@/components/dashboard/share-dashboard-utils";
import { CORE_SHARE_APPS, SHARE_APP_LABELS } from "@/lib/share-app";
import { modelHealthPresentation } from "@/lib/share-health";
import type { ShareProviderStatusPanelView } from "@/components/dashboard/share-provider-status-panel";

export const MAX_USER_MODEL_ROUTES = 100;
export const MAX_MODEL_ROUTE_CANDIDATES = 5;
export const MAX_MODEL_ROUTE_TARGETS = MAX_MODEL_ROUTE_CANDIDATES + 1;
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

/** One ordered queue, including the primary. Never sort candidate IDs. */
export function modelRouteTargets(route?: UserModelRouteInput | null): string[] {
  return route ? [route.targetShareId, ...(route.candidates || [])].filter(Boolean) : [];
}

export function moveModelRouteTarget(ids: string[], from: number, to: number): string[] {
  if (!Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0
    || from >= ids.length || to >= ids.length || from === to) return ids;
  const next = [...ids];
  next.splice(to, 0, next.splice(from, 1)[0]);
  return next;
}

export function modelRouteDragAnnouncements(
  ids: string[], names: string[], firstOrder: number,
  t: (key: MessageKey, params?: Record<string, string | number>) => string,
): Announcements {
  const name = (id: string | number) => names[ids.indexOf(String(id))] || "Share";
  return {
    onDragStart: ({ active }) => t("modelHub.dragStarted", { name: name(active.id) }),
    onDragOver: ({ active, over }) => over ? t("modelHub.dragPosition", {
      name: name(active.id), order: ids.indexOf(String(over.id)) + firstOrder,
    }) : undefined,
    onDragEnd: ({ active, over }) => over ? t("modelHub.moved", {
      name: name(active.id), order: ids.indexOf(String(over.id)) + firstOrder,
    }) : t("modelHub.dragCancelled"),
    onDragCancel: () => t("modelHub.dragCancelled"),
  };
}

export function replacePassthroughTargets(
  routes: DraftModelRoute[], app: ModelRoutingApp, ids: string[],
): DraftModelRoute[] {
  if (ids.length > MAX_MODEL_ROUTE_TARGETS || ids.some(id => !id.trim())
    || new Set(ids).size !== ids.length) return routes;
  const existing = routes.find(route => route.appType === app && isWildcardModel(route.requestedModel));
  if (!ids.length) return routes.filter(route => route !== existing);
  if (!existing && routes.length >= MAX_USER_MODEL_ROUTES) return routes;
  const next = {
    ...(existing || newDraftModelRoute(app, WILDCARD_MODEL, ids[0])),
    targetShareId: ids[0], candidates: ids.slice(1),
  };
  return existing ? routes.map(route => route === existing ? next : route) : [...routes, next];
}
export type ModelRouteValidationError =
  | "required"
  | "duplicate"
  | "too_many"
  | "pattern"
  | "too_many_candidates"
  | "duplicate_candidate";
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
      modelRouteTargets(route).some(id => !eligibleIds.has(id)),
  );
}

/** Preserve saved stale entries for repair, but never introduce an unavailable
 * target under a new protocol/model key or promote an unavailable backup. */
export function hasNewUnavailableRouteTarget(
  routes: UserModelRouteInput[], profile: UserModelRoutingResponse,
): boolean {
  const key = (route: UserModelRouteInput) => `${route.appType}\u0000${route.requestedModel}`;
  const saved = new Map(canonicalModelRoutes(profile.routes).map(route => [key(route), route]));
  return canonicalModelRoutes(routes).some(route => {
    const previous = saved.get(key(route));
    const eligible = new Set(sharesForProtocol(profile.eligibleShares, route.appType).map(share => share.shareId));
    return (previous?.targetShareId !== route.targetShareId && !eligible.has(route.targetShareId))
      || (route.candidates || []).some(id => !previous?.candidates?.includes(id) && !eligible.has(id));
  });
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
    // A user can type a second `*` into an exact-model draft. Keep that invalid
    // row visible for repair instead of hiding it behind the first queue.
    const exact = forApp.filter((route) => route !== passthrough);
    return { appType, passthrough, exact };
  });
}

export function sharesForProtocol(
  shares: UserModelRoutingShare[],
  appType: ModelRoutingApp,
) {
  return shares.filter((share) => share.apps.includes(appType));
}

export type ModelRoutingShareAccountLines = {
  primaryLine: string;
  identityLine: string;
  modelsLine: string;
};

function routingCapabilityRuntime(
  capability: UserModelRoutingShareCapability,
): ShareUpstreamProvider {
  return {
    app: capability.app,
    kind: capability.kind || capability.providerType,
    providerType: capability.providerType || capability.kind,
    providerName: capability.providerName,
    accountLabel: capability.accountLabel,
    accountEmail: capability.accountEmail,
    apiUrl: capability.apiUrl,
    subscriptionLevel: capability.subscriptionLevel,
    quota: capability.quota,
    models: capability.models,
    modelPolicy: capability.modelPolicy,
  };
}

/** The three account-card lines used by a Share card, built from the routing projection. */
export function modelRoutingShareAccountLines(
  share: UserModelRoutingShare,
  locale: AppLocale,
  passthroughLabel: string,
  unavailableLabel: string,
  appType?: ModelRoutingApp,
): ModelRoutingShareAccountLines {
  const apps = CORE_SHARE_APPS.filter((app) => share.apps.includes(app) && (!appType || app === appType));
  const capabilities = apps.flatMap((app) => {
    const capability = share.appCapabilities?.find((item) => item.app === app);
    return capability ? [{ app, runtime: routingCapabilityRuntime(capability) }] : [];
  });
  const primary = capabilities[0]?.runtime;
  if (!primary) {
    return { primaryLine: "-", identityLine: unavailableLabel, modelsLine: "-" };
  }
  const apiProvider = isApiProviderRuntime(primary);
  const models = capabilities
    .map(({ app, runtime }) => `${SHARE_APP_LABELS[app]}: ${providerActualModelNames(runtime, passthroughLabel)}`)
    .join(" · ");
  return {
    primaryLine: apiProvider ? providerApiEndpoint(primary) : providerQuotaStatusLine(primary, locale),
    identityLine: apiProvider ? "-" : providerStatusIdentity(primary),
    modelsLine: models || providerActualModelNames(primary, passthroughLabel),
  };
}

/** Routing is protocol-scoped; a representative App cannot lend its green
 * status (or its account) to another protocol. Legacy responses stay unknown. */
export function modelRoutingShareAccountView(
  share: UserModelRoutingShare,
  app: ModelRoutingApp,
  locale: AppLocale,
  labels: { passthrough: string; unavailable: string },
  now = Date.now(),
): ShareProviderStatusPanelView {
  const capability = share.apps.includes(app)
    ? share.appCapabilities?.find(item => item.app === app) : undefined;
  const status = capability?.liveModelHealthStatus || capability?.modelHealthStatus;
  const applies = status?.appType === app && !!capability?.healthFingerprint
    && status.healthFingerprint === capability.healthFingerprint;
  const health = modelHealthPresentation(applies ? status : undefined, locale, now);
  const lines = modelRoutingShareAccountLines(share, locale, labels.passthrough, labels.unavailable, app);
  return {
    ...lines,
    primaryMonospace: !!capability && isApiProviderRuntime(routingCapabilityRuntime(capability)),
    toneClassName: health.className,
    panelTitle: health.title,
    healthLabel: [health.label, health.detailLabel !== health.label ? health.detailLabel : ""].filter(Boolean).join(" · "),
  };
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
      candidates: (route.candidates || []).map((candidate) => candidate.trim()).filter(Boolean),
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
  if (keys.size !== normalized.length) return "duplicate";
  // Failover candidates: no duplicates within a route, none equal to the
  // primary target, and within the per-route cap.
  for (const route of normalized) {
    const candidates = route.candidates || [];
    if (candidates.length > MAX_MODEL_ROUTE_CANDIDATES) return "too_many_candidates";
    const seen = new Set([route.targetShareId]);
    for (const candidate of candidates) {
      if (seen.has(candidate)) return "duplicate_candidate";
      seen.add(candidate);
    }
  }
  return null;
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
    if (patch.targetShareId && patch.targetShareId !== route.targetShareId) {
      next.candidates = (next.candidates || []).filter(id => id !== patch.targetShareId);
    }
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

function hrefWithSearch(href: string, nextSearch: string): string {
  const url = new URL(href, "https://cc-switch.local");
  url.search = nextSearch;
  return `${url.pathname}${url.search}${url.hash}`;
}

/** Same-path query update for static export: do not wait on `useSearchParams`. */
export function hrefWithClientListTab(href: string, tab: ClientListTab): string {
  const url = new URL(href, "https://cc-switch.local");
  return hrefWithSearch(href, searchForClientListTab(url.searchParams.toString(), tab));
}

export function replaceClientListTabQuery(tab: ClientListTab) {
  if (typeof window === "undefined") return;
  window.history.replaceState(
    window.history.state,
    "",
    hrefWithClientListTab(window.location.href, tab),
  );
}

export function windowHasMineClientListTab(): boolean {
  if (typeof window === "undefined") return false;
  return new URLSearchParams(window.location.search).get("tab") === "mine";
}

export function windowSearchString(): string {
  if (typeof window === "undefined") return "";
  return new URLSearchParams(window.location.search).toString();
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

export function hrefWithConsumedModelRouteDeepLink(href: string): string {
  const url = new URL(href, "https://cc-switch.local");
  return hrefWithSearch(href, consumeModelRouteDeepLink(url.searchParams.toString()));
}

export function replaceConsumedModelRouteDeepLink() {
  if (typeof window === "undefined") return;
  window.history.replaceState(
    window.history.state,
    "",
    hrefWithConsumedModelRouteDeepLink(window.location.href),
  );
}

export function configuredEligibleRouteShareIds(
  profile: UserModelRoutingResponse | null,
): ReadonlySet<string> {
  if (!profile) return new Set<string>();
  return new Set(
    profile.routes
      .flatMap((route) => modelRouteTargets(route).filter(id =>
        profile.eligibleShares.some(share => share.shareId === id && share.apps.includes(route.appType)),
      )),
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
