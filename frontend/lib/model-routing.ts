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
import { modelHealthPresentation, shareAccountHealthPresentation } from "@/lib/share-health";
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

/** Transfer uses list order, not checkbox click order. Stale selections never
 * reintroduce revoked/removed items; capacity failures are atomic. */
export function modelRouteTransferSelection(ids: string[], checked: string[]): string[] {
  const wanted = new Set(checked);
  return [...new Set(ids)].filter(id => wanted.has(id));
}

export function transferModelRouteTargets(
  selected: string[], eligible: string[], checked: string[], direction: "in" | "out",
): string[] {
  if (direction === "out") {
    const removed = new Set(modelRouteTransferSelection(selected, checked));
    return removed.size ? selected.filter(id => !removed.has(id)) : selected;
  }
  const added = modelRouteTransferSelection(eligible, checked).filter(id => id.trim() && !selected.includes(id));
  if (!added.length || selected.length + added.length > MAX_MODEL_ROUTE_TARGETS) return selected;
  return [...selected, ...added];
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
      // Demoting a kept unavailable primary to a backup is not a new grant.
      || (route.candidates || []).some(id => !previous?.candidates?.includes(id) && previous?.targetShareId !== id && !eligible.has(id));
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

/** Account content follows the selected protocol; its background follows the
 * same Share-level observation as All. Never relabel the observation's App. */
export function modelRoutingShareAccountView(
  share: UserModelRoutingShare,
  app: ModelRoutingApp,
  locale: AppLocale,
  labels: { passthrough: string; unavailable: string },
  now = Date.now(),
): ShareProviderStatusPanelView {
  const capability = share.apps.includes(app)
    ? share.appCapabilities?.find(item => item.app === app) : undefined;
  const shareStatus = share.liveModelHealthStatus || share.modelHealthStatus;
  const observedCapability = share.appCapabilities?.find(item => item.app === shareStatus?.appType);
  // Old Router responses only carry per-App evidence. Keep that compatibility
  // path strict; do not synthesize a Share observation from another App.
  const legacyStatus = capability?.liveModelHealthStatus || capability?.modelHealthStatus;
  const legacyApplies = legacyStatus?.appType === app && !!capability?.healthFingerprint
    && legacyStatus.healthFingerprint === capability.healthFingerprint;
  const health = shareStatus
    ? shareAccountHealthPresentation(shareStatus, observedCapability?.healthFingerprint, locale, now)
    : modelHealthPresentation(legacyApplies ? legacyStatus : undefined, locale, now);
  const lines = modelRoutingShareAccountLines(share, locale, labels.passthrough, labels.unavailable, app);
  return {
    ...lines,
    primaryMonospace: !!capability && isApiProviderRuntime(routingCapabilityRuntime(capability)),
    toneClassName: health.className,
    healthState: health.state,
    panelTitle: health.title,
    healthLabel: [shareStatus ? "Share" : "", health.label, health.detailLabel !== health.label ? health.detailLabel : ""].filter(Boolean).join(" · "),
  };
}

/** One muted line for compact Share rows: provider, then the models this
 * protocol actually reaches. Full quota and account text stays in the title. */
export function modelRoutingShareCompactLine(
  share: UserModelRoutingShare,
  app: ModelRoutingApp,
  passthroughLabel: string,
) {
  const capability = share.apps.includes(app)
    ? share.appCapabilities?.find(item => item.app === app) : undefined;
  const provider = capability?.providerName || share.providerIdentity?.displayName || "";
  const models = capability ? providerActualModelNames(routingCapabilityRuntime(capability), passthroughLabel) : "";
  return [provider, models].filter(value => value && value !== "-").join(" · ");
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
  let body: unknown = { model, input: "Hello", max_output_tokens: 32 };

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
    body = { contents: [{ parts: [{ text: "Hello" }] }], generationConfig: { maxOutputTokens: 32 } };
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
      (route) => !route.requestedModel || !route.targetShareId
        || Array.from(route.requestedModel).length > 200 || /[\p{Cc}]/u.test(route.requestedModel),
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

export type ModelRouteCommand =
  | { kind: "add_exact"; clientId: string; appType: ModelRoutingApp; shareId?: string }
  | { kind: "set_model"; clientId: string; value: string }
  | { kind: "remove_rule"; clientId: string }
  | { kind: "set_targets"; clientId: string; shareIds: string[] }
  | { kind: "set_default_targets"; appType: ModelRoutingApp; clientId: string; shareIds: string[] }
  | { kind: "append_targets" | "remove_targets"; clientId: string; shareIds: string[] }
  | { kind: "move_target"; clientId: string; from: number; to: number }
  | { kind: "restore_change"; changeId: string; baseRevision: number; draftFingerprint: string };
export type ModelRouteCommandError = "route_missing" | "unsupported_protocol" | "invalid_index"
  | "too_many_routes" | "too_many_targets" | "duplicate_target" | "target_unavailable"
  | "invalid_target_id" | "duplicate_model" | "change_missing" | "stale_diff" | "restore_conflict";
export type ModelRouteCommandResult = {
  routes: DraftModelRoute[]; changed: boolean;
  error?: { code: ModelRouteCommandError; clientId?: string };
  focus?: { clientId: string; shareId?: string; control: "model" | "target" | "add" };
};
export type ModelRouteChange = {
  changeId: string; clientId: string; kind: "added" | "removed" | "model" | "targets";
  before?: DraftModelRoute; after?: DraftModelRoute;
};
export function modelRoutesDraftFingerprint(routes: DraftModelRoute[]): string {
  return JSON.stringify(routes.map(route => [route.clientId, route.appType, route.requestedModel, modelRouteTargets(route)]));
}
export function diffModelRoutes(saved: DraftModelRoute[], draft: DraftModelRoute[]): ModelRouteChange[] {
  const changes: ModelRouteChange[] = [];
  const add = (kind: ModelRouteChange["kind"], before?: DraftModelRoute, after?: DraftModelRoute) => {
    const clientId = (before || after)!.clientId;
    changes.push({ changeId: `${encodeURIComponent(clientId)}:${kind}`, clientId, kind, before, after });
  };
  const current = new Map(draft.map(route => [route.clientId, route]));
  for (const before of saved) {
    const after = current.get(before.clientId);
    if (!after) add("removed", before);
    else {
      if (before.requestedModel.trim() !== after.requestedModel.trim() || before.appType !== after.appType) add("model", before, after);
      if (JSON.stringify(modelRouteTargets(before)) !== JSON.stringify(modelRouteTargets(after))) add("targets", before, after);
    }
  }
  const ids = new Set(saved.map(route => route.clientId));
  for (const after of draft) if (!ids.has(after.clientId)) add("added", undefined, after);
  return changes;
}

/** All editor mutations are atomic; no network, clock, UUID or DOM access. */
export function applyModelRouteCommand(routes: DraftModelRoute[], command: ModelRouteCommand, context: {
  eligibleShares: UserModelRoutingShare[]; savedRoutes: DraftModelRoute[]; baseRevision: number;
}): ModelRouteCommandResult {
  const unchanged = (code?: ModelRouteCommandError): ModelRouteCommandResult => ({ routes, changed: false,
    ...(code ? { error: { code, ...("clientId" in command ? { clientId: command.clientId } : {}) } } : {}) });
  let next = routes;
  let affectedId = "clientId" in command ? command.clientId : "";
  let restoring = false;
  let focusControl: "model" | "target" = command.kind === "add_exact" || command.kind === "set_model" ? "model" : "target";
  const previous = routes.find(route => route.clientId === affectedId);
  const setTargets = (route: DraftModelRoute, ids: string[]) => {
    const updated = { ...route, targetShareId: ids[0] || "", candidates: ids.slice(1) };
    return !ids.length && isWildcardModel(route.requestedModel)
      ? routes.filter(item => item.clientId !== route.clientId)
      : routes.map(item => item.clientId === route.clientId ? updated : item);
  };
  if (command.kind === "restore_change") {
    if (command.baseRevision !== context.baseRevision || command.draftFingerprint !== modelRoutesDraftFingerprint(routes)) return unchanged("stale_diff");
    const change = diffModelRoutes(context.savedRoutes, routes).find(item => item.changeId === command.changeId);
    if (!change) return unchanged("change_missing");
    restoring = true;
    affectedId = change.clientId;
    if (change.kind === "model" || change.kind === "removed" && !isWildcardModel(change.before!.requestedModel)) focusControl = "model";
    if (change.kind === "added") next = routes.filter(route => route.clientId !== change.clientId);
    else if (change.kind === "removed") {
      const savedIndex = context.savedRoutes.findIndex(route => route.clientId === change.clientId);
      const following = context.savedRoutes.slice(savedIndex + 1).find(route => routes.some(item => item.clientId === route.clientId));
      const preceding = context.savedRoutes.slice(0, savedIndex).reverse().find(route => routes.some(item => item.clientId === route.clientId));
      const index = following ? routes.findIndex(route => route.clientId === following.clientId)
        : preceding ? routes.findIndex(route => route.clientId === preceding.clientId) + 1 : routes.length;
      next = [...routes.slice(0, index), { ...change.before!, candidates: [...(change.before!.candidates || [])] }, ...routes.slice(index)];
    } else next = routes.map(route => route.clientId !== change.clientId ? route : change.kind === "model"
      ? { ...route, appType: change.before!.appType, requestedModel: change.before!.requestedModel }
      : { ...route, targetShareId: change.before!.targetShareId, candidates: [...(change.before!.candidates || [])] });
  } else if (command.kind === "add_exact") {
    if (previous) return unchanged("duplicate_model");
    next = [...routes, { clientId: command.clientId, appType: command.appType, requestedModel: "",
      targetShareId: command.shareId || firstShareForProtocol(context.eligibleShares, command.appType)?.shareId || "", candidates: [] }];
  } else if (command.kind === "set_default_targets") {
    const existing = routes.find(route => route.appType === command.appType && isWildcardModel(route.requestedModel));
    affectedId = existing?.clientId || command.clientId;
    if (existing) next = setTargets(existing, command.shareIds);
    else if (command.shareIds.length) {
      if (previous) return unchanged("duplicate_model");
      next = [...routes, { clientId: affectedId, appType: command.appType, requestedModel: WILDCARD_MODEL,
        targetShareId: command.shareIds[0], candidates: command.shareIds.slice(1) }];
    }
  } else {
    if (!previous) return unchanged("route_missing");
    if (!MODEL_ROUTING_PROTOCOLS.includes(previous.appType as ModelRoutingProtocol)) return unchanged("unsupported_protocol");
    const ids = modelRouteTargets(previous);
    switch (command.kind) {
      case "remove_rule": next = routes.filter(route => route !== previous); break;
      case "set_model": next = routes.map(route => route === previous ? { ...route, requestedModel: command.value } : route); break;
      case "set_targets": next = setTargets(previous, command.shareIds); break;
      case "append_targets": {
        if (command.shareIds.some(id => ids.includes(id)) || new Set(command.shareIds).size !== command.shareIds.length) return unchanged("duplicate_target");
        next = setTargets(previous, [...ids, ...modelRouteTransferSelection(command.shareIds, command.shareIds)]); break;
      }
      case "remove_targets": next = setTargets(previous, transferModelRouteTargets(ids, [], command.shareIds, "out")); break;
      case "move_target": {
        if (![command.from, command.to].every(index => Number.isInteger(index) && index >= 0 && index < ids.length)) return unchanged("invalid_index");
        next = setTargets(previous, moveModelRouteTarget(ids, command.from, command.to)); break;
      }
    }
  }
  if (next === routes || modelRoutesDraftFingerprint(next) === modelRoutesDraftFingerprint(routes)) return unchanged();
  const error = (code: ModelRouteCommandError) => unchanged(restoring ? "restore_conflict" : code);
  const affected = next.find(route => route.clientId === affectedId);
  if (affected && !MODEL_ROUTING_PROTOCOLS.includes(affected.appType as ModelRoutingProtocol)) return error("unsupported_protocol");
  const rawIds = affected ? [affected.targetShareId, ...(affected.candidates || [])].filter(id => id !== "") : [];
  if (rawIds.some(id => !id.trim() || id !== id.trim() || /[\p{Cc}]/u.test(id))) return error("invalid_target_id");
  if (new Set(rawIds).size !== rawIds.length) return error("duplicate_target");
  if (next.length > MAX_USER_MODEL_ROUTES) return error("too_many_routes");
  if (rawIds.length > MAX_MODEL_ROUTE_TARGETS) return error("too_many_targets");
  if (command.kind !== "set_model" && affected?.requestedModel.trim() && next.some(route => route.clientId !== affectedId && route.appType === affected.appType
    && route.requestedModel.trim() === affected.requestedModel.trim())) return error("duplicate_model");
  if (affected && command.kind !== "set_model") {
    const saved = context.savedRoutes.find(route => route.clientId === affectedId && route.appType === affected.appType
      && route.requestedModel.trim() === affected.requestedModel.trim());
    const old = routes.find(route => route.clientId === affectedId && route.appType === affected.appType
      && route.requestedModel.trim() === affected.requestedModel.trim());
    const eligible = new Set(sharesForProtocol(context.eligibleShares, affected.appType).map(share => share.shareId));
    // An unavailable target may stay or move down the queue, never up into
    // the primary slot it did not already hold.
    for (const [index, id] of modelRouteTargets(affected).entries()) {
      if (eligible.has(id)) continue;
      const preserved = index === 0 ? saved?.targetShareId === id && (restoring || old?.targetShareId === id)
        : modelRouteTargets(saved).includes(id) && (restoring || modelRouteTargets(old).includes(id));
      if (!preserved) return error("target_unavailable");
    }
  }
  if (!affected) {
    const removed = routes.find(route => route.clientId === affectedId);
    const index = routes.findIndex(route => route === removed);
    const neighbors = next.filter(route => route.appType === removed?.appType);
    const neighbor = neighbors.find(route => routes.indexOf(route) >= index) || neighbors.at(-1);
    return { routes: next, changed: true, focus: { clientId: neighbor?.clientId || affectedId,
      control: neighbor ? isWildcardModel(neighbor.requestedModel) ? "target" : "model" : "add" } };
  }
  const ids = modelRouteTargets(affected);
  let shareId = ids[0];
  if (command.kind === "move_target") shareId = modelRouteTargets(previous)[command.from];
  else if (command.kind === "append_targets") shareId = command.shareIds[0];
  else if (command.kind === "remove_targets") {
    const index = modelRouteTargets(previous).findIndex(id => command.shareIds.includes(id));
    shareId = ids[Math.min(Math.max(0, index), ids.length - 1)];
  }
  return { routes: next, changed: true, focus: { clientId: affectedId, shareId, control: focusControl } };
}

/** Only request-side evidence is offered; fixed actualModel values are not. */
export function modelRoutingSuggestions(profile: UserModelRoutingResponse, app: ModelRoutingApp, routes: DraftModelRoute[]): string[] {
  void profile; void app; void routes;
  return [];
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

/** Shares running on clients the viewer owns. */
function viewerHostedShareIds(clients: DashboardClient[], viewerEmail: string) {
  return new Set(
    clients
      .filter((client) => clientOwnedByViewer(client, viewerEmail))
      .flatMap((client) => client.shareIds || []),
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
  const hostedShareIds = viewerHostedShareIds(clients, viewerEmail);
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

/** IDs of the viewer's Shares they own or host, as opposed to ones they only use
 * (granted, rented or routed to). Ownership, not access, decides the group. */
export function ownedViewerShareIds(
  shares: ShareView[],
  clients: DashboardClient[],
  viewerEmailValue: string,
): Set<string> {
  const viewerEmail = normalizeEmail(viewerEmailValue);
  if (!viewerEmail) return new Set();
  const hosted = viewerHostedShareIds(clients, viewerEmail);
  return new Set(
    shares
      .filter((share) => hosted.has(share.shareId) || normalizeEmail(share.ownerEmail) === viewerEmail)
      .map((share) => share.shareId),
  );
}

/** One rule as seen from a Share card: its place in that rule's queue. */
export type ShareRouteChip = {
  clientId: string;
  appType: ModelRoutingApp;
  requestedModel: string;
  /** 0 is the primary target. */
  order: number;
  /** The draft differs from what is saved for this Share in this rule. */
  draft: boolean;
};

/** Per-Share rule chips for the draft, flagging what saving would change. */
export function shareRouteChips(
  draft: DraftModelRoute[],
  saved: DraftModelRoute[],
): Map<string, ShareRouteChip[]> {
  const key = (route: DraftModelRoute, shareId: string, order: number) =>
    JSON.stringify([route.clientId, route.appType, route.requestedModel, shareId, order]);
  const savedKeys = new Set(
    saved.flatMap((route) => modelRouteTargets(route).map((shareId, order) => key(route, shareId, order))),
  );
  const byShare = new Map<string, ShareRouteChip[]>();
  for (const route of draft) {
    modelRouteTargets(route).forEach((shareId, order) => {
      const chips = byShare.get(shareId) || [];
      chips.push({
        clientId: route.clientId,
        appType: route.appType,
        requestedModel: route.requestedModel,
        order,
        draft: !savedKeys.has(key(route, shareId, order)),
      });
      byShare.set(shareId, chips);
    });
  }
  return byShare;
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
