"use client";

import { ModelHubRecentResult } from "./model-hub-recent-result";

import { Button, toast } from "@heroui/react";
import {
  ChevronDown,
  Copy,
  Eye,
  EyeOff,
  KeyRound,
  Loader2,
  Network,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
} from "lucide-react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getUserApiToken,
  getUserModelRouting,
  replaceUserModelRouting,
  resetUserApiToken,
} from "@/lib/api";
import type {
  ModelRoutingApp,
  UserApiTokenStatus,
  UserModelRoutingResponse,
} from "@/lib/types";
import {
  defaultModelRoutingProtocol,
  defaultTestModelForProtocol,
  diffModelRoutes,
  groupModelRoutesByProtocol,
  MAX_USER_MODEL_ROUTES,
  MODEL_ROUTING_PROTOCOLS,
  modelRoutesDraftFingerprint,
  preferredModelRoutingApp,
  protocolHasAttention,
  protocolSlotMode,
  sharesForProtocol,
  type DraftModelRoute,
  type ModelRoutingProtocol,
  type ModelRoutingProtocolMode,
  type ModelRouteCommand,
  type ModelRouteCommandResult,
} from "@/lib/model-routing";
import { ModelRoutingSession, routingDrafts, routingDraftIsDirty } from "@/lib/model-routing-session";
import { usePersistentState } from "@/lib/use-persistent-state";
import { cn } from "@/lib/utils";
import { ModelHubRuleEditor, modelRouteEditorId } from "./model-hub-rule-editor";
import { ModelHubDependencyProvider } from "./model-hub-dependencies";
import { ModelHubTestDrawer } from "./model-hub-test-drawer";
import type { MessageKey } from "@/lib/i18n";

const PROTOCOL_TITLE_KEYS: Record<ModelRoutingProtocol, MessageKey> = {
  claude: "modelHub.protocol.claude",
  codex: "modelHub.protocol.codex",
  gemini: "modelHub.protocol.gemini",
};
function protocolLabel(app: ModelRoutingApp, t: (key: MessageKey) => string) {
  return MODEL_ROUTING_PROTOCOLS.includes(app as ModelRoutingProtocol) ? t(PROTOCOL_TITLE_KEYS[app as ModelRoutingProtocol]) : app;
}
const PROTOCOL_STATUS_KEYS: Record<ModelRoutingProtocolMode, MessageKey> = {
  empty: "modelHub.status.empty",
  passthrough: "modelHub.status.passthrough",
  exact: "modelHub.status.exact",
  mixed: "modelHub.status.mixed",
};
const LIST_MODE_KEYS: Record<ModelRoutingProtocolMode, MessageKey> = {
  empty: "modelHub.listMode.empty",
  passthrough: "modelHub.listMode.passthrough",
  exact: "modelHub.listMode.exact",
  mixed: "modelHub.listMode.mixed",
};

export type ModelRoutingController = {
  actorKey: string;
  profile: UserModelRoutingResponse | null;
  routes: DraftModelRoute[];
  loading: boolean;
  busy: boolean;
  error: string;
  refreshError: string;
  conflict: boolean;
  dirty: boolean;
  rawToken: string;
  token: UserApiTokenStatus | null;
  showToken: boolean;
  setShowToken: React.Dispatch<React.SetStateAction<boolean>>;
  load: () => Promise<void>;
  discardChanges: () => Promise<boolean>;
  refresh: () => Promise<void>;
  save: () => Promise<void>;
  resetToken: () => Promise<boolean>;
  addRouteForShare: (shareId: string) => ModelRoutingApp | undefined;
  addExactRoute: (appType: ModelRoutingApp, shareId?: string) => void;
  /** Returns whether the draft accepted the new queue. */
  setPassthroughTargets: (appType: ModelRoutingApp, shareIds: string[], options?: { focus?: boolean }) => boolean;
  removeRoute: (clientId: string) => void;
  /** `focus: false` leaves focus with the caller, e.g. the Transfer list that
   * already keeps it on the moved row or its own arrow buttons. */
  command: (command: ModelRouteCommand, options?: { focus?: boolean }) => ModelRouteCommandResult | undefined;
};

export function useModelRoutingController(active: boolean, viewerKey: string): ModelRoutingController {
  const { t } = useLocaleText();
  const session = React.useMemo(() => new ModelRoutingSession({
    getRouting: getUserModelRouting, getToken: getUserApiToken,
    replaceRouting: replaceUserModelRouting, resetToken: resetUserApiToken,
  }), [viewerKey]);
  const state = React.useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);

  React.useEffect(() => {
    session.start();
    return () => session.stop();
  }, [session]);

  React.useEffect(() => {
    if (!active) { session.suspend(); return; }
    const resume = () => {
      if (document.visibilityState === "visible") void session.resume();
      else session.suspend();
    };
    resume();
    const refresh = () => {
      if (document.visibilityState === "visible") void session.refresh();
    };
    const timer = window.setInterval(refresh, 15_000);
    document.addEventListener("visibilitychange", resume);
    window.addEventListener("focus", resume);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", resume);
      window.removeEventListener("focus", resume);
      session.suspend();
    };
  }, [active, session]);

  const dirty = routingDraftIsDirty(state);
  React.useEffect(() => {
    if (!dirty) return;
    const beforeUnload = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ""; };
    window.addEventListener("beforeunload", beforeUnload);
    return () => window.removeEventListener("beforeunload", beforeUnload);
  }, [dirty]);

  const runCommand = React.useCallback((command: ModelRouteCommand, options?: { focus?: boolean }) => {
    const result = session.command(command);
    if (result?.changed && result.focus && command.kind !== "set_model" && options?.focus !== false) {
      const focus = result.focus;
      const applyFocus = (retry = true) => {
        const section = document.getElementById(modelRouteEditorId(focus.clientId));
        if (!section && focus.control !== "add" && retry) {
          requestAnimationFrame(() => applyFocus(false));
          return;
        }
        const target = section && Array.from(section.querySelectorAll<HTMLElement>("[data-model-route-target]"))
          .find(node => node.dataset.modelRouteTarget === focus.shareId);
        const node = focus.control === "add" ? document.querySelector<HTMLElement>("[data-add-model-route]")
          : focus.control === "model" ? section?.querySelector<HTMLElement>("input") || section
          : target || section;
        node?.focus();
      };
      requestAnimationFrame(() => applyFocus());
    }
    return result;
  }, [session]);
  const addExactRoute = React.useCallback((appType: ModelRoutingApp, shareId?: string) => {
    runCommand({ kind: "add_exact", clientId: `new:${crypto.randomUUID()}`, appType, shareId });
  }, [session, runCommand]);
  const addRouteForShare = React.useCallback((shareId: string) => {
    const share = session.getSnapshot().profile?.eligibleShares.find(item => item.shareId === shareId);
    if (!share) return;
    const app = preferredModelRoutingApp(share);
    addExactRoute(app, shareId);
    return app;
  }, [addExactRoute, session]);
  const setPassthroughTargets = React.useCallback((app: ModelRoutingApp, ids: string[], options?: { focus?: boolean }) =>
    !!runCommand({ kind: "set_default_targets", appType: app, clientId: `new:${crypto.randomUUID()}`, shareIds: ids }, options)?.changed,
  [runCommand]);
  const removeRoute = React.useCallback((clientId: string) => {
    runCommand({ kind: "remove_rule", clientId });
  }, [runCommand]);

  const validationKeys: Record<NonNullable<typeof state.validation>, MessageKey> = {
    required: "modelHub.validationRequired", duplicate: "modelHub.validationDuplicate",
    too_many: "modelHub.validationTooMany", pattern: "modelHub.validationPattern",
    too_many_candidates: "modelHub.validationTooManyCandidates",
    duplicate_candidate: "modelHub.validationDuplicateCandidate",
    target_unavailable: "modelHub.targetUnavailable",
  };
  return {
    ...state,
    actorKey: viewerKey,
    error: state.error || (state.validation ? t(validationKeys[state.validation]) : "")
      || (state.commandError ? t(`modelHub.command.${state.commandError}` as MessageKey) : ""),
    dirty: routingDraftIsDirty(state),
    setShowToken: session.setShowToken,
    load: async () => { await session.load(); },
    discardChanges: () => session.load(true),
    refresh: session.refresh,
    save: session.save,
    resetToken: session.resetToken,
    addExactRoute, addRouteForShare, setPassthroughTargets, removeRoute,
    command: runCommand,
  };
}

function protocolStatusLabel(
  mode: ModelRoutingProtocolMode,
  exactCount: number,
  t: ReturnType<typeof useLocaleText>["t"],
  candidateCount = 0,
) {
  const base =
    mode === "exact" || mode === "mixed"
      ? t(PROTOCOL_STATUS_KEYS[mode], { count: exactCount })
      : t(PROTOCOL_STATUS_KEYS[mode]);
  if (candidateCount > 0) {
    return `${base} · ${t("modelHub.candidates", { count: candidateCount })}`;
  }
  return base;
}

export function ModelHubPanel({
  controller,
  forceExpanded = false,
  focusProtocol = null,
}: {
  controller: ModelRoutingController;
  forceExpanded?: boolean | number;
  focusProtocol?: ModelRoutingProtocol | null;
}) {
  const { locale, t } = useLocaleText();
  const [resetConfirmOpen, setResetConfirmOpen] = React.useState(false);
  const [reloadConfirmOpen, setReloadConfirmOpen] = React.useState(false);
  const [persistedExpanded, setPersistedExpanded, expandedHydrated] = usePersistentState<boolean>(
    "cc_switch_router_model_hub_expanded_v1",
    false,
  );
  const [persistedProtocol, setPersistedProtocol, protocolHydrated] = usePersistentState<ModelRoutingProtocol | null>(
    "cc_switch_router_model_hub_protocol_v1",
    null,
  );
  const [expanded, setExpandedState] = React.useState(controller.dirty || !!forceExpanded);
  const [activeProtocol, setActiveProtocolState] = React.useState<ModelRoutingProtocol>("codex");
  const [testModels, setTestModels] = React.useState<Record<ModelRoutingProtocol, string>>({
    claude: "",
    codex: "",
    gemini: "",
  });
  const [testOpen, setTestOpen] = React.useState(false);
  const protocolInitializedRef = React.useRef(false);
  const lastFocusRequestRef = React.useRef<{ protocol: ModelRoutingProtocol; request: boolean | number } | null>(null);

  // Wrap setters to also persist user choices. One-shot overrides (dirty
  // auto-expand, focusProtocol deep-link) call the raw state setters so they
  // do not overwrite the stored preference.
  const setExpanded = React.useCallback(
    (value: boolean | ((open: boolean) => boolean)) => {
      setExpandedState((open) => {
        const next = typeof value === "function" ? value(open) : value;
        setPersistedExpanded(next);
        return next;
      });
    },
    [setPersistedExpanded],
  );
  const setActiveProtocol = React.useCallback(
    (value: ModelRoutingProtocol) => {
      setActiveProtocolState(value);
      setPersistedProtocol(value);
    },
    [setPersistedProtocol],
  );

  // Restore persisted expanded state after hydration (dirty/forceExpanded can
  // still force it open afterwards via the effect below).
  const expandedHydratedRef = React.useRef(false);
  React.useEffect(() => {
    if (!expandedHydrated || expandedHydratedRef.current) return;
    expandedHydratedRef.current = true;
    if (persistedExpanded === true) setExpandedState(true);
  }, [expandedHydrated, persistedExpanded]);

  // Restore persisted protocol before the default-protocol effect runs.
  React.useEffect(() => {
    if (!protocolHydrated || protocolInitializedRef.current) return;
    if (persistedProtocol && MODEL_ROUTING_PROTOCOLS.includes(persistedProtocol)) setActiveProtocolState(persistedProtocol);
  }, [protocolHydrated, persistedProtocol]);

  const profile = controller.profile;
  const slots = React.useMemo(
    () => groupModelRoutesByProtocol(controller.routes),
    [controller.routes],
  );
  const activeSlot = slots.find((slot) => slot.appType === activeProtocol) || slots[1];
  const savedDrafts = React.useMemo(() => profile ? routingDrafts(profile) : [], [profile]);
  const changes = React.useMemo(() => diffModelRoutes(savedDrafts, controller.routes), [savedDrafts, controller.routes]);
  const locateRule = (clientId: string) => {
    const route = controller.routes.find(item => item.clientId === clientId);
    setExpandedState(true);
    if (!route) return;
    // A jump is a one-shot override; it must not replace the remembered tab.
    setActiveProtocolState(route.appType as ModelRoutingProtocol);
    requestAnimationFrame(() => {
      const node = document.getElementById(modelRouteEditorId(clientId));
      node?.scrollIntoView({ block: "nearest" }); node?.focus({ preventScroll: true });
    });
  };
  const maskedToken = controller.rawToken
    ? `${controller.rawToken.slice(0, 8)}${"*".repeat(16)}${controller.rawToken.slice(-4)}`
    : controller.token?.prefix
      ? `${controller.token.prefix}${"*".repeat(16)}`
      : "-";
  const testModel = testModels[activeProtocol] || "";

  React.useEffect(() => {
    if (controller.dirty || forceExpanded) setExpandedState(true);
  }, [controller.dirty, forceExpanded]);

  React.useEffect(() => {
    if (focusProtocol && (lastFocusRequestRef.current?.protocol !== focusProtocol || lastFocusRequestRef.current.request !== forceExpanded)) {
      setActiveProtocolState(focusProtocol);
      lastFocusRequestRef.current = { protocol: focusProtocol, request: forceExpanded };
      protocolInitializedRef.current = true;
      return;
    }
    if (!profile || protocolInitializedRef.current || !protocolHydrated) return;
    // No one-shot override: honor the persisted tab if the user picked one
    // before, otherwise fall back to the computed default.
    setActiveProtocolState(persistedProtocol && MODEL_ROUTING_PROTOCOLS.includes(persistedProtocol)
      ? persistedProtocol : defaultModelRoutingProtocol(controller.routes, profile.eligibleShares));
    protocolInitializedRef.current = true;
  }, [controller.routes, focusProtocol, forceExpanded, profile, protocolHydrated, persistedProtocol]);

  React.useEffect(() => {
    if (!profile) {
      protocolInitializedRef.current = false;
      return;
    }
    const saved = groupModelRoutesByProtocol(routingDrafts(profile));
    const draft = groupModelRoutesByProtocol(controller.routes);
    setTestModels((current) => {
      const next = { ...current };
      for (const slot of [...saved, ...draft]) {
        if (!next[slot.appType]) next[slot.appType] = defaultTestModelForProtocol(slot);
      }
      return next;
    });
  }, [controller.routes, profile]);

  const copy = React.useCallback(async (_kind: "endpoint" | "token" | "curl", value: string) => {
    if (!value) return;
    try {
      await navigator.clipboard.writeText(value);
      toast.success(t("common.copySuccess"));
    } catch {
      toast.danger(t("common.copyFailed"));
    }
  }, [t]);

  if (controller.loading && !profile) {
    return (
      <section id="model-hub" className="flex min-h-32 items-center justify-center border-y border-border bg-white px-4 py-6 text-sm text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden />
        {t("common.loading")}
      </section>
    );
  }

  const summary = slots.map((slot) => {
    const mode = protocolSlotMode(slot);
    const attention = protocolHasAttention(controller.routes, profile?.eligibleShares || [], slot.appType);
    return `${t(PROTOCOL_TITLE_KEYS[slot.appType])} ${attention ? t("modelHub.status.attention") : protocolStatusLabel(mode, slot.exact.length, t)}`;
  }).join(" · ");
  const mode = activeSlot ? protocolSlotMode(activeSlot) : "empty";
  const shares = sharesForProtocol(profile?.eligibleShares || [], activeProtocol);
  const attention = protocolHasAttention(controller.routes, profile?.eligibleShares || [], activeProtocol);

  return (
    <section id="model-hub" className="grid min-w-0 gap-4 border-y border-border bg-white px-4 py-4 sm:px-5">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="flex items-center gap-2 text-sm font-semibold text-foreground">
            <Network className="h-4 w-4 text-primary" aria-hidden />
            {t("modelHub.title")}
          </h2>
          <p className="mt-1 min-w-0 truncate text-xs text-muted-foreground" title={summary}>{summary}</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-slate-100 hover:text-foreground disabled:opacity-40"
            disabled={controller.loading || controller.busy}
            title={t("common.reload")}
            aria-label={t("common.reload")}
            onClick={() => void controller.load()}
          >
            <RefreshCw className={`h-4 w-4 ${controller.loading ? "animate-spin" : ""}`} />
          </button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              setExpanded((open) => !open);
            }}
          >
            <ChevronDown className={cn("h-4 w-4 transition-transform", expanded && "rotate-180")} />
            {expanded ? t("modelHub.collapseRoutes") : t("modelHub.expandRoutes")}
          </Button>
          <Button variant="primary" size="sm" isDisabled={!controller.dirty || controller.busy || controller.loading || controller.conflict} onClick={() => void controller.save()}>
            {controller.busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            {t("common.save")}
            {changes.length ? <span className="tabular-nums">({changes.length})</span> : null}
          </Button>
        </div>
      </div>

      <div className="grid min-w-0 gap-3 lg:grid-cols-2">
        <div className="grid min-w-0 gap-1.5">
          <span className="text-[11px] font-medium text-muted-foreground">{t("modelHub.endpoint")}</span>
          <div className="flex min-w-0 items-center gap-2 rounded-md border border-border bg-slate-50 px-3 py-2">
            <code className="min-w-0 flex-1 truncate text-xs text-foreground" title={profile?.apiBaseUrl}>{profile?.apiBaseUrl || "-"}</code>
            <button type="button" className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-white hover:text-foreground" title={t("common.copy")} aria-label={t("common.copy")} onClick={() => void copy("endpoint", profile?.apiBaseUrl || "")}>
              <Copy className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
        <div className="grid min-w-0 gap-1.5">
          <span className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground"><KeyRound className="h-3.5 w-3.5" />{t("modelHub.apiKey")}</span>
          <div className="flex min-w-0 items-center gap-1 rounded-md border border-border bg-slate-50 px-3 py-2">
            <code className="min-w-0 flex-1 truncate text-xs text-foreground">{controller.showToken && controller.rawToken ? controller.rawToken : maskedToken}</code>
            <button type="button" disabled={!controller.rawToken} className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-white hover:text-foreground disabled:opacity-40" title={controller.showToken ? t("account.apiKeys.hideToken") : t("account.apiKeys.showToken")} aria-label={controller.showToken ? t("account.apiKeys.hideToken") : t("account.apiKeys.showToken")} onClick={() => controller.setShowToken((value) => !value)}>
              {controller.showToken ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
            </button>
            <button type="button" disabled={!controller.rawToken} className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-white hover:text-foreground disabled:opacity-40" title={t("common.copy")} aria-label={t("common.copy")} onClick={() => void copy("token", controller.rawToken)}>
              <Copy className="h-3.5 w-3.5" />
            </button>
            <button type="button" disabled={controller.busy || controller.loading || !profile} className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-rose-600 hover:bg-white disabled:opacity-40" title={t("account.apiKeys.reset")} aria-label={t("account.apiKeys.reset")} onClick={() => setResetConfirmOpen(true)}>
              <RotateCcw className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      </div>

      {controller.error ? (
        <div className="border-l-2 border-rose-500 bg-rose-50 px-3 py-2 text-sm text-rose-700" role="alert">
          {controller.error}
        </div>
      ) : null}

      {controller.conflict ? (
        <div role="alert" className="flex flex-wrap items-center gap-2 rounded-md border border-amber-200 bg-amber-50 p-3 text-xs text-amber-800">
          <span>{t("modelHub.remoteChanged")}</span>
          <Button size="sm" variant="outline" isDisabled={controller.loading || controller.busy} onClick={() => setReloadConfirmOpen(true)}>{t("common.reload")}</Button>
        </div>
      ) : null}
      {controller.refreshError ? <p role="status" className="text-xs text-muted-foreground">{t("modelHub.refreshFailed")}</p> : null}

      {changes.length ? <details className="min-w-0 border-t border-border py-2 text-xs">
        <summary className="min-h-11 cursor-pointer py-3 font-medium focus-visible:outline-2 focus-visible:outline-ring">{t("modelHub.changes.title", { count: changes.length })}</summary>
        <ul className="grid gap-2">{changes.map(change => <li key={change.changeId} className="flex min-w-0 flex-wrap items-center justify-between gap-2">
          <span className="min-w-0 break-words [overflow-wrap:anywhere]">{t(`modelHub.changes.${change.kind}`)} · {protocolLabel((change.after || change.before)!.appType, t)} · {(change.after || change.before)?.requestedModel || "—"}</span>
          <Button size="sm" variant="outline" className="min-h-11 whitespace-nowrap" isDisabled={controller.busy || controller.loading || controller.conflict}
            onClick={() => controller.command({ kind: "restore_change", changeId: change.changeId, baseRevision: profile!.revision,
              draftFingerprint: modelRoutesDraftFingerprint(controller.routes) })}>{t("modelHub.changes.restore")}</Button>
        </li>)}</ul>
      </details> : null}

      <ModelHubDependencyProvider saved={savedDrafts} draft={controller.routes} shares={profile?.eligibleShares || []}
        dirty={controller.dirty} onLocate={locateRule} locale={locale} t={t}>
      {expanded ? (
        <div className="grid min-w-0 gap-3">
          <SegmentedControl
            value={activeProtocol}
            onChange={setActiveProtocol}
            ariaLabel={t("modelHub.protocolTabs")}
            fullWidth
            items={slots.map((slot) => {
              const slotMode = protocolSlotMode(slot);
              const slotAttention = protocolHasAttention(controller.routes, profile?.eligibleShares || [], slot.appType);
              const slotCandidates =
                (slot.passthrough?.candidates?.length || 0) +
                slot.exact.reduce((sum, route) => sum + (route.candidates?.length || 0), 0);
              return {
                id: slot.appType,
                label: (
                  <span className="inline-flex min-w-0 items-center justify-center gap-1.5">
                    <ShareAppLogo app={slot.appType} size={12} />
                    <span className="truncate">{t(PROTOCOL_TITLE_KEYS[slot.appType])}</span>
                    <span className={cn(
                      "hidden rounded px-1 py-px text-[10px] font-medium sm:inline",
                      slotAttention ? "bg-amber-100 text-amber-800" : slotMode === "empty" ? "text-slate-400" : "text-sky-700",
                    )}>
                      {slotAttention ? t("modelHub.status.attention") : protocolStatusLabel(slotMode, slot.exact.length, t, slotCandidates)}
                    </span>
                  </span>
                ),
                title: `${t(PROTOCOL_TITLE_KEYS[slot.appType])} · ${slotAttention ? t("modelHub.status.attention") : protocolStatusLabel(slotMode, slot.exact.length, t)}`,
                className: slotAttention ? "text-amber-700" : undefined,
              };
            })}
          />

          <div className="grid min-w-0 gap-3">
            <section className={cn(
              "grid min-w-0 content-start gap-3 rounded-lg border bg-slate-50/60 p-3",
              attention ? "border-amber-200" : "border-border",
            )}>
              <div className="flex min-w-0 items-center gap-2">
                <ShareAppLogo app={activeProtocol} size={14} />
                <h3 className="text-sm font-semibold text-foreground">{t("modelHub.configTitle")}</h3>
                <span className={cn(
                  "rounded-md px-1.5 py-0.5 text-[11px] font-medium",
                  attention ? "bg-amber-100 text-amber-800" : mode === "empty" ? "bg-slate-200 text-slate-600" : "bg-sky-100 text-sky-800",
                )}>
                  {attention ? t("modelHub.status.attention") : protocolStatusLabel(mode, activeSlot?.exact.length || 0, t)}
                </span>
              </div>
              {mode === "empty" ? (
                <p className="text-xs text-muted-foreground">{t("modelHub.emptyProtocol")}</p>
              ) : null}
              <p className="text-[11px] leading-4 text-slate-500">{t("modelHub.priorityHint")}</p>
              <div className="grid min-w-0 gap-3">
                <h4 className="text-xs font-medium text-muted-foreground">{t("modelHub.passthrough")}</h4>
                <ModelHubRuleEditor kind="default" route={activeSlot?.passthrough} app={activeProtocol} appLabel={t(PROTOCOL_TITLE_KEYS[activeProtocol])} shares={shares}
                  disabled={controller.busy || controller.loading || !profile}
                  addBlockedReason={!activeSlot?.passthrough && controller.routes.length >= MAX_USER_MODEL_ROUTES ? t("modelHub.validationTooMany") : undefined}
                  onAction={action => {
                    if (activeSlot?.passthrough) return !!controller.command({ ...action, clientId: activeSlot.passthrough.clientId }, { focus: false })?.changed;
                    return action.kind === "append_targets" && controller.setPassthroughTargets(activeProtocol, action.shareIds, { focus: false });
                  }} onTargets={ids => controller.setPassthroughTargets(activeProtocol, ids)} locale={locale} t={t} />
                {activeSlot?.passthrough ? <ModelHubRecentResult profile={controller.dirty ? null : profile} routeId={activeSlot.passthrough.clientId} locale={locale} t={t} /> : null}
                {!activeSlot?.passthrough && shares.length === 1 ? <Button variant="outline" className="min-h-11 w-fit whitespace-nowrap"
                  isDisabled={controller.busy || controller.loading} onClick={() => controller.setPassthroughTargets(activeProtocol, [shares[0].shareId])}>{t("modelHub.setDefault")}</Button> : null}
                <p className="text-xs text-muted-foreground">{t("modelHub.passthroughHint.none")}</p>
              </div>
              <div className="grid min-w-0 gap-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs font-medium text-slate-600">{t("modelHub.exactModels")}</span>
                  <Button
                    size="sm"
                    variant="outline"
                    isDisabled={controller.busy || controller.loading || !profile || controller.routes.length >= MAX_USER_MODEL_ROUTES}
                    data-add-model-route onClick={() => controller.addExactRoute(activeProtocol)}
                  >
                    <Plus className="h-3.5 w-3.5" />
                    {t("modelHub.addRoute")}
                  </Button>
                </div>
                {activeSlot?.exact.length ? <div className="divide-y divide-border">{activeSlot.exact.map((route, index) => (
                  <div key={route.clientId}>
                  <ModelHubRuleEditor kind="exact" position={index + 1} route={route} app={activeProtocol} appLabel={t(PROTOCOL_TITLE_KEYS[activeProtocol])} shares={shares}
                    disabled={controller.busy || controller.loading}
                    duplicate={!!route.requestedModel.trim() && controller.routes.some(other => other.clientId !== route.clientId && other.appType === route.appType && other.requestedModel.trim() === route.requestedModel.trim())}
                    onAction={action => !!controller.command({ ...action, clientId: route.clientId }, { focus: false })?.changed}
                    onTargets={shareIds => controller.command({ kind: "set_targets", clientId: route.clientId, shareIds })}
                    onModel={value => controller.command({ kind: "set_model", clientId: route.clientId, value })}
                    onRemove={() => controller.removeRoute(route.clientId)} locale={locale} t={t} />
                  <ModelHubRecentResult profile={controller.dirty ? null : profile} routeId={route.clientId} locale={locale} t={t} />
                  </div>
                ))}</div> : <p className="text-xs text-muted-foreground">{t("modelHub.exactEmpty")}</p>}
              </div>
              <p className="text-[11px] leading-5 text-slate-500">{t(LIST_MODE_KEYS[mode])}</p>
            </section>

            <Button variant="outline" className="min-h-11 w-fit whitespace-nowrap" isDisabled={!profile || controller.loading}
              onClick={() => setTestOpen(true)}>{t("modelHub.test.open")}</Button>
          </div>
        </div>
      ) : null}
      </ModelHubDependencyProvider>
      <ModelHubTestDrawer open={testOpen} onOpenChange={setTestOpen} actorKey={controller.actorKey} app={activeProtocol}
        profile={profile} disabled={controller.dirty || controller.conflict || controller.busy || controller.loading}
        model={testModel} onModel={value => setTestModels(current => ({ ...current, [activeProtocol]: value }))} onComplete={controller.refresh} />

      <ConfirmAlertDialog open={resetConfirmOpen} title={t("account.apiKeys.resetConfirmTitle")} description={t("modelHub.resetKeyImpact")} confirmLabel={t("account.apiKeys.resetConfirmAction")} cancelLabel={t("common.cancel")} tone="danger" busy={controller.busy} onConfirm={() => void controller.resetToken().then((reset) => reset && setResetConfirmOpen(false))} onOpenChange={(open) => !controller.busy && setResetConfirmOpen(open)} />
      <ConfirmAlertDialog open={reloadConfirmOpen} title={t("modelHub.unsavedTitle")}
        description={t("modelHub.reloadDiscardDescription")} confirmLabel={t("common.reload")} cancelLabel={t("common.cancel")}
        tone="warning" busy={controller.loading || controller.busy}
        onConfirm={() => void controller.discardChanges().then(loaded => loaded && setReloadConfirmOpen(false))}
        onOpenChange={setReloadConfirmOpen} />
    </section>
  );
}
