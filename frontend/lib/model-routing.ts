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
  | "too_many";

export function buildUnifiedModelCurl(
  baseUrlValue: string,
  token: string,
  route?: Pick<UserModelRouteInput, "appType" | "requestedModel">,
) {
  const baseUrl = baseUrlValue.replace(/\/+$/, "");
  const apiKey = token || "<YOUR_API_KEY>";
  const model = route?.requestedModel.trim() || "<MODEL>";
  let url = `${baseUrl}/v1/responses`;
  let authHeader = `Authorization: Bearer ${apiKey}`;
  let body: unknown = { model, input: "Hello" };

  if (route?.appType === "claude") {
    url = `${baseUrl}/v1/messages`;
    authHeader = `x-api-key: ${apiKey}`;
    body = {
      model,
      max_tokens: 32,
      messages: [{ role: "user", content: "Hello" }],
    };
  } else if (route?.appType === "gemini") {
    const wireModel = route.requestedModel.trim()
      ? encodeURIComponent(model)
      : model;
    url = `${baseUrl}/v1beta/models/${wireModel}:generateContent`;
    authHeader = `x-goog-api-key: ${apiKey}`;
    body = { contents: [{ parts: [{ text: "Hello" }] }] };
  }

  return [
    "curl -sS \\",
    `  ${shellQuote(url)} \\`,
    `  -H ${shellQuote(authHeader)} \\`,
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

export function clientBelongsToViewer(
  client: DashboardClient,
  shareById: ReadonlyMap<string, ShareView>,
  viewerEmailValue: string,
  explicitlyRoutedShareIds: ReadonlySet<string> = new Set<string>(),
  nowMs = Date.now(),
) {
  const viewerEmail = normalizeEmail(viewerEmailValue);
  if (!viewerEmail) return false;
  const ownerEmail = normalizeEmail(
    client.clientTunnel?.ownerEmail || client.installation.ownerEmail,
  );
  if (ownerEmail === viewerEmail) return true;
  return (client.shareIds || []).some((shareId) => {
    if (explicitlyRoutedShareIds.has(shareId)) return true;
    const share = shareById.get(shareId);
    if (!share) return false;
    return (
      normalizeEmail(share.ownerEmail) === viewerEmail ||
      shareGrantAllowsViewer(share, viewerEmail, nowMs)
    );
  });
}
