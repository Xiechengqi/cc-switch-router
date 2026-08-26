"use client";

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
  Trash2,
} from "lucide-react";
import * as React from "react";
import { CompactSelect } from "@/components/common/compact-select";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { SegmentedControl } from "@/components/common/segmented-control";
import { MarketShareIdentity } from "@/components/dashboard/share-market/market-share-identity";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getUserApiToken,
  getUserModelRouting,
  replaceUserModelRouting,
  resetUserApiToken,
  testUserModelRouting,
} from "@/lib/api";
import type {
  ModelRoutingApp,
  UserApiTokenStatus,
  UserModelRouteInput,
  UserModelRoutingResponse,
  UserModelRoutingShare,
  UserModelRoutingTestResponse,
} from "@/lib/types";
import {
  buildUnifiedModelCurl,
  canonicalModelRoutes,
  defaultModelRoutingProtocol,
  defaultTestModelForProtocol,
  firstShareForProtocol,
  groupModelRoutesByProtocol,
  isWildcardModel,
  MAX_USER_MODEL_ROUTES,
  newDraftModelRoute,
  patchDraftModelRoute,
  preferredModelRoutingApp,
  protocolHasAttention,
  protocolSlotMode,
  sharesForProtocol,
  validateModelRoutes,
  WILDCARD_MODEL,
  type DraftModelRoute,
  type ModelRoutingProtocol,
  type ModelRoutingProtocolMode,
} from "@/lib/model-routing";
import { cn } from "@/lib/utils";
import type { MessageKey } from "@/lib/i18n";

const PASSTHROUGH_UNSET = "__model_hub_passthrough_unset__";
const PROTOCOL_TITLE_KEYS: Record<ModelRoutingProtocol, MessageKey> = {
  claude: "modelHub.protocol.claude",
  codex: "modelHub.protocol.codex",
  gemini: "modelHub.protocol.gemini",
};
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
  profile: UserModelRoutingResponse | null;
  routes: DraftModelRoute[];
  loading: boolean;
  busy: boolean;
  error: string;
  dirty: boolean;
  rawToken: string;
  token: UserApiTokenStatus | null;
  showToken: boolean;
  setShowToken: React.Dispatch<React.SetStateAction<boolean>>;
  load: () => Promise<void>;
  save: () => Promise<void>;
  resetToken: () => Promise<boolean>;
  addRouteForShare: (shareId: string) => ModelRoutingApp | undefined;
  addExactRoute: (appType: ModelRoutingApp, shareId?: string) => void;
  setPassthroughShare: (appType: ModelRoutingApp, shareId: string) => void;
  updateRoute: (clientId: string, patch: Partial<UserModelRouteInput>) => void;
  removeRoute: (clientId: string) => void;
};

function toDraftRoutes(profile: UserModelRoutingResponse): DraftModelRoute[] {
  return profile.routes.map((route) => ({
    clientId: route.id,
    appType: route.appType,
    requestedModel: route.requestedModel,
    targetShareId: route.targetShareId,
  }));
}

export function useModelRoutingController(active: boolean): ModelRoutingController {
  const { t } = useLocaleText();
  const [profile, setProfile] = React.useState<UserModelRoutingResponse | null>(null);
  const [routes, setRoutes] = React.useState<DraftModelRoute[]>([]);
  const [token, setToken] = React.useState<UserApiTokenStatus | null>(null);
  const [rawToken, setRawToken] = React.useState("");
  const [showToken, setShowToken] = React.useState(false);
  const [loading, setLoading] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const autoLoadStartedRef = React.useRef(false);

  const load = React.useCallback(async () => {
    if (!active) return;
    setLoading(true);
    setError("");
    try {
      const [routing, apiToken] = await Promise.all([
        getUserModelRouting(),
        getUserApiToken(),
      ]);
      setProfile(routing);
      setRoutes(toDraftRoutes(routing));
      setToken(apiToken.token);
      setRawToken(apiToken.apiToken || "");
      setShowToken(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [active]);

  React.useEffect(() => {
    if (!active) {
      autoLoadStartedRef.current = false;
      setProfile(null);
      setRoutes([]);
      setRawToken("");
      setToken(null);
      setError("");
      return;
    }
    if (!autoLoadStartedRef.current) {
      autoLoadStartedRef.current = true;
      void load();
    }
  }, [active, load]);

  const dirty = React.useMemo(() => {
    if (!profile) return false;
    return JSON.stringify(canonicalModelRoutes(routes)) !== JSON.stringify(canonicalModelRoutes(profile.routes));
  }, [profile, routes]);

  const addExactRoute = React.useCallback((appType: ModelRoutingApp, shareId?: string) => {
    const shares = profile?.eligibleShares || [];
    const targetShareId = shareId
      || firstShareForProtocol(shares, appType)?.shareId
      || "";
    if (shareId && !shares.some((share) => share.shareId === shareId && share.apps.includes(appType))) {
      return;
    }
    setRoutes((current) => current.length >= MAX_USER_MODEL_ROUTES
      ? current
      : [...current, newDraftModelRoute(appType, "", targetShareId)]);
    setError("");
  }, [profile]);

  const addRouteForShare = React.useCallback((shareId: string) => {
    const share = profile?.eligibleShares.find((candidate) => candidate.shareId === shareId);
    if (!share) return;
    const appType = preferredModelRoutingApp(share);
    addExactRoute(appType, shareId);
    return appType;
  }, [addExactRoute, profile]);

  const setPassthroughShare = React.useCallback((appType: ModelRoutingApp, shareId: string) => {
    setRoutes((current) => {
      const without = current.filter((route) =>
        !(route.appType === appType && isWildcardModel(route.requestedModel)),
      );
      if (!shareId) return without;
      const existing = current.find((route) =>
        route.appType === appType && isWildcardModel(route.requestedModel),
      );
      if (without.length >= MAX_USER_MODEL_ROUTES && !existing) return current;
      return [
        ...without,
        existing
          ? { ...existing, targetShareId: shareId }
          : newDraftModelRoute(appType, WILDCARD_MODEL, shareId),
      ];
    });
    setError("");
  }, []);

  const updateRoute = React.useCallback((clientId: string, patch: Partial<UserModelRouteInput>) => {
    setRoutes((current) => patchDraftModelRoute(
      current,
      clientId,
      patch,
      profile?.eligibleShares || [],
    ));
    setError("");
  }, [profile]);

  const removeRoute = React.useCallback((clientId: string) => {
    setRoutes((current) => current.filter((route) => route.clientId !== clientId));
    setError("");
  }, []);

  const save = React.useCallback(async () => {
    if (!profile || busy) return;
    const validation = validateModelRoutes(routes);
    if (validation) {
      setError(t(
        validation === "duplicate"
          ? "modelHub.validationDuplicate"
          : validation === "too_many"
            ? "modelHub.validationTooMany"
            : validation === "pattern"
              ? "modelHub.validationPattern"
              : "modelHub.validationRequired",
      ));
      return;
    }
    const normalized = canonicalModelRoutes(routes);
    setBusy(true);
    setError("");
    try {
      const updated = await replaceUserModelRouting({
        expectedRevision: profile.revision,
        routes: normalized,
      });
      setProfile(updated);
      setRoutes(toDraftRoutes(updated));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [busy, profile, routes, t]);

  const resetToken = React.useCallback(async () => {
    if (busy) return false;
    setBusy(true);
    setError("");
    try {
      const updated = await resetUserApiToken();
      setToken(updated.token);
      setRawToken(updated.apiToken);
      setShowToken(true);
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setBusy(false);
    }
  }, [busy]);

  return {
    profile,
    routes,
    loading,
    busy,
    error,
    dirty,
    rawToken,
    token,
    showToken,
    setShowToken,
    load,
    save,
    resetToken,
    addRouteForShare,
    addExactRoute,
    setPassthroughShare,
    updateRoute,
    removeRoute,
  };
}

function shareSelectOptions(
  shares: UserModelRoutingShare[],
  currentId: string,
  t: ReturnType<typeof useLocaleText>["t"],
  includeUnset?: { value: string; label: string },
) {
  const eligible = shares.map((share) => ({
    value: share.shareId,
    label: share.subdomain || share.shareName,
    content: <MarketShareIdentity source={share} />,
  }));
  const options = includeUnset
    ? [{ value: includeUnset.value, label: includeUnset.label }, ...eligible]
    : eligible;
  if (currentId && !options.some((option) => option.value === currentId)) {
    options.splice(includeUnset ? 1 : 0, 0, {
      value: currentId,
      label: currentId,
      content: (
        <span className="inline-flex min-w-0 items-center gap-1.5 text-amber-800">
          <span className="min-w-0 truncate font-mono text-xs">{currentId}</span>
          <span className="shrink-0 text-[11px]">{t("modelHub.targetUnavailable")}</span>
        </span>
      ),
    });
  }
  return options;
}

function protocolStatusLabel(
  mode: ModelRoutingProtocolMode,
  exactCount: number,
  t: ReturnType<typeof useLocaleText>["t"],
) {
  if (mode === "exact" || mode === "mixed") {
    return t(PROTOCOL_STATUS_KEYS[mode], { count: exactCount });
  }
  return t(PROTOCOL_STATUS_KEYS[mode]);
}

export function ModelHubPanel({
  controller,
  forceExpanded = false,
  focusProtocol = null,
}: {
  controller: ModelRoutingController;
  forceExpanded?: boolean;
  focusProtocol?: ModelRoutingProtocol | null;
}) {
  const { t } = useLocaleText();
  const [copied, setCopied] = React.useState<"endpoint" | "token" | "curl" | "">("");
  const [resetConfirmOpen, setResetConfirmOpen] = React.useState(false);
  const [leaveConfirmOpen, setLeaveConfirmOpen] = React.useState(false);
  const [expanded, setExpanded] = React.useState(controller.dirty || forceExpanded);
  const [activeProtocol, setActiveProtocol] = React.useState<ModelRoutingProtocol>("codex");
  const [testModels, setTestModels] = React.useState<Record<ModelRoutingProtocol, string>>({
    claude: "",
    codex: "",
    gemini: "",
  });
  const [testState, setTestState] = React.useState<"idle" | "running" | "done" | "error">("idle");
  const [testResult, setTestResult] = React.useState<UserModelRoutingTestResponse | null>(null);
  const [testError, setTestError] = React.useState("");
  const protocolInitializedRef = React.useRef(false);
  const lastFocusProtocolRef = React.useRef<ModelRoutingProtocol | null>(null);
  const profile = controller.profile;
  const slots = React.useMemo(
    () => groupModelRoutesByProtocol(controller.routes),
    [controller.routes],
  );
  const savedSlots = React.useMemo(
    () => (profile ? groupModelRoutesByProtocol(toDraftRoutes(profile)) : []),
    [profile],
  );
  const activeSlot = slots.find((slot) => slot.appType === activeProtocol) || slots[1];
  const savedSlot = savedSlots.find((slot) => slot.appType === activeProtocol);
  const maskedToken = controller.rawToken
    ? `${controller.rawToken.slice(0, 8)}${"*".repeat(16)}${controller.rawToken.slice(-4)}`
    : controller.token?.prefix
      ? `${controller.token.prefix}${"*".repeat(16)}`
      : "-";
  const testModel = testModels[activeProtocol] || "";
  const curl = buildUnifiedModelCurl(
    profile?.apiBaseUrl || "https://api.example.com",
    controller.rawToken,
    { appType: activeProtocol, requestedModel: testModel || WILDCARD_MODEL },
  );

  React.useEffect(() => {
    if (controller.dirty || forceExpanded) setExpanded(true);
  }, [controller.dirty, forceExpanded]);

  React.useEffect(() => {
    if (focusProtocol && lastFocusProtocolRef.current !== focusProtocol) {
      setActiveProtocol(focusProtocol);
      lastFocusProtocolRef.current = focusProtocol;
      protocolInitializedRef.current = true;
      return;
    }
    if (!profile || protocolInitializedRef.current) return;
    setActiveProtocol(defaultModelRoutingProtocol(controller.routes, profile.eligibleShares));
    protocolInitializedRef.current = true;
  }, [controller.routes, focusProtocol, profile]);

  React.useEffect(() => {
    if (!profile) {
      protocolInitializedRef.current = false;
      return;
    }
    const saved = groupModelRoutesByProtocol(toDraftRoutes(profile));
    const draft = groupModelRoutesByProtocol(controller.routes);
    setTestModels((current) => {
      const next = { ...current };
      for (const slot of [...saved, ...draft]) {
        if (!next[slot.appType]) next[slot.appType] = defaultTestModelForProtocol(slot);
      }
      return next;
    });
  }, [controller.routes, profile]);

  React.useEffect(() => {
    setTestState("idle");
    setTestResult(null);
    setTestError("");
  }, [activeProtocol, profile?.revision]);

  const copy = React.useCallback(async (kind: "endpoint" | "token" | "curl", value: string) => {
    if (!value) return;
    try {
      await navigator.clipboard.writeText(value);
      setCopied(kind);
      toast.success(t("common.copySuccess"));
      window.setTimeout(() => setCopied(""), 1500);
    } catch {
      toast.danger(t("common.copyFailed"));
    }
  }, [t]);

  const runTest = React.useCallback(async () => {
    if (testState === "running") return;
    const model = testModel.trim();
    if (controller.dirty || !savedSlot || protocolSlotMode(savedSlot) === "empty") return;
    if (!model || isWildcardModel(model)) return;
    setTestState("running");
    setTestResult(null);
    setTestError("");
    try {
      const response = await testUserModelRouting({
        appType: activeProtocol,
        requestedModel: model,
      });
      setTestResult(response);
      setTestState(response.success ? "done" : "error");
    } catch (err) {
      setTestError(err instanceof Error ? err.message : String(err));
      setTestState("error");
    }
  }, [activeProtocol, controller.dirty, savedSlot, testModel, testState]);

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
  const passthroughOptions = shareSelectOptions(
    shares,
    activeSlot?.passthrough?.targetShareId || "",
    t,
    { value: PASSTHROUGH_UNSET, label: t("modelHub.passthroughUnset") },
  );
  const savedMode = savedSlot ? protocolSlotMode(savedSlot) : "empty";
  const canTest = !controller.dirty && savedMode !== "empty" && !!testModel.trim() && !isWildcardModel(testModel);
  const testDisabledReason = controller.dirty
    ? t("modelHub.test.saveFirst")
    : savedMode === "empty"
      ? t("modelHub.test.empty")
      : !testModel.trim() || isWildcardModel(testModel)
        ? t("modelHub.test.needModel")
        : null;
  const targetShare = testResult?.targetShareId
    ? (profile?.eligibleShares || []).find((share) => share.shareId === testResult.targetShareId)
    : undefined;

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
              if (expanded && controller.dirty) {
                setLeaveConfirmOpen(true);
                return;
              }
              setExpanded((open) => !open);
            }}
          >
            <ChevronDown className={cn("h-4 w-4 transition-transform", expanded && "rotate-180")} />
            {expanded ? t("modelHub.collapseRoutes") : t("modelHub.expandRoutes")}
          </Button>
          <Button variant="primary" size="sm" isDisabled={!controller.dirty || controller.busy} onClick={() => void controller.save()}>
            {controller.busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            {t("common.save")}
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
            <button type="button" disabled={controller.busy} className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-rose-600 hover:bg-white disabled:opacity-40" title={t("account.apiKeys.reset")} aria-label={t("account.apiKeys.reset")} onClick={() => setResetConfirmOpen(true)}>
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
                      {slotAttention ? t("modelHub.status.attention") : protocolStatusLabel(slotMode, slot.exact.length, t)}
                    </span>
                  </span>
                ),
                title: `${t(PROTOCOL_TITLE_KEYS[slot.appType])} · ${slotAttention ? t("modelHub.status.attention") : protocolStatusLabel(slotMode, slot.exact.length, t)}`,
                className: slotAttention ? "text-amber-700" : undefined,
              };
            })}
          />

          <div className="grid min-w-0 gap-3 lg:grid-cols-[minmax(0,1.15fr)_minmax(280px,0.85fr)]">
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
              <label className="grid min-w-0 gap-1.5 text-xs text-muted-foreground">
                {t("modelHub.passthrough")}
                <CompactSelect
                  value={activeSlot?.passthrough?.targetShareId || PASSTHROUGH_UNSET}
                  options={passthroughOptions}
                  onChange={(value) => controller.setPassthroughShare(activeProtocol, value === PASSTHROUGH_UNSET ? "" : value)}
                  ariaLabel={t("modelHub.passthrough")}
                  disabled={controller.busy}
                  className="w-full"
                  triggerClassName="min-h-9 w-full"
                />
                {activeSlot?.passthrough ? (
                  <span>{t(activeSlot.exact.length ? "modelHub.passthroughHint.mixed" : "modelHub.passthroughHint.none")}</span>
                ) : null}
              </label>
              <div className="grid min-w-0 gap-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs font-medium text-slate-600">{t("modelHub.exactModels")}</span>
                  <Button
                    size="sm"
                    variant="outline"
                    isDisabled={controller.busy || controller.routes.length >= MAX_USER_MODEL_ROUTES}
                    onClick={() => controller.addExactRoute(activeProtocol)}
                  >
                    <Plus className="h-3.5 w-3.5" />
                    {t("modelHub.addRoute")}
                  </Button>
                </div>
                {activeSlot?.exact.length ? activeSlot.exact.map((route) => {
                  const options = shareSelectOptions(shares, route.targetShareId, t);
                  return (
                    <div key={route.clientId} className="grid min-w-0 items-center gap-2 sm:grid-cols-[minmax(140px,1fr)_minmax(160px,1.3fr)_32px]">
                      <input
                        value={route.requestedModel}
                        maxLength={200}
                        disabled={controller.busy}
                        onChange={(event) => controller.updateRoute(route.clientId, { requestedModel: event.target.value })}
                        className="h-9 min-w-0 rounded-md border border-border bg-white px-3 text-xs outline-none focus:border-primary/60 focus:ring-2 focus:ring-primary/10"
                        aria-label={t("modelHub.model")}
                        placeholder={t("modelHub.modelPlaceholder")}
                      />
                      <CompactSelect
                        value={route.targetShareId}
                        options={options}
                        onChange={(value) => controller.updateRoute(route.clientId, { targetShareId: value })}
                        ariaLabel={t("modelHub.targetShare")}
                        disabled={controller.busy || !options.length}
                        className="w-full"
                        triggerClassName="min-h-9 w-full"
                      />
                      <button
                        type="button"
                        disabled={controller.busy}
                        className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-rose-50 hover:text-rose-700 disabled:opacity-40"
                        title={t("modelHub.removeRoute")}
                        aria-label={t("modelHub.removeRoute")}
                        onClick={() => controller.removeRoute(route.clientId)}
                      >
                        <Trash2 className="h-4 w-4" />
                      </button>
                    </div>
                  );
                }) : (
                  <p className="text-xs text-muted-foreground">{t("modelHub.exactEmpty")}</p>
                )}
              </div>
              <p className="text-[11px] leading-5 text-slate-500">{t(LIST_MODE_KEYS[mode])}</p>
            </section>

            <section className="grid min-w-0 content-start gap-3 rounded-lg border border-border bg-white p-3">
              <div className="flex min-w-0 items-center justify-between gap-2">
                <h3 className="text-sm font-semibold text-foreground">{t("modelHub.test.title")}</h3>
                <Button
                  size="sm"
                  variant="outline"
                  isDisabled={!canTest || testState === "running"}
                  onClick={() => void runTest()}
                >
                  {testState === "running" ? (
                    <>
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("modelHub.test.running")}
                    </>
                  ) : t("modelHub.test.run")}
                </Button>
              </div>
              {testDisabledReason ? (
                <p className="text-xs text-amber-800">{testDisabledReason}</p>
              ) : savedMode === "passthrough" || savedMode === "mixed" ? (
                <p className="text-xs text-muted-foreground">{t("modelHub.test.wildcardHint")}</p>
              ) : null}
              <label className="grid min-w-0 gap-1.5 text-xs text-muted-foreground">
                {t("modelHub.test.model")}
                <input
                  value={testModel}
                  maxLength={200}
                  disabled={savedMode === "empty"}
                  onChange={(event) => setTestModels((current) => ({
                    ...current,
                    [activeProtocol]: event.target.value,
                  }))}
                  className="h-9 min-w-0 rounded-md border border-border bg-white px-3 text-xs text-foreground outline-none focus:border-primary/60 focus:ring-2 focus:ring-primary/10 disabled:bg-slate-50"
                  placeholder={t("modelHub.modelPlaceholder")}
                />
              </label>
              <div className="grid min-w-0 gap-1.5">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-[11px] font-medium text-muted-foreground">{t("modelHub.test.curl")}</span>
                  <button type="button" className="inline-flex h-7 items-center gap-1 rounded-md px-2 text-xs text-muted-foreground hover:bg-slate-100 hover:text-foreground" onClick={() => void copy("curl", curl)}>
                    <Copy className="h-3.5 w-3.5" />
                    {copied === "curl" ? t("account.apiKeys.copied") : t("common.copy")}
                  </button>
                </div>
                <pre className="max-h-36 max-w-full overflow-auto rounded-md border border-border bg-slate-950 px-3 py-2 font-mono text-[11px] leading-5 text-slate-100">{curl}</pre>
              </div>
              {testState === "error" && testError ? (
                <p className="text-xs text-rose-700">{t("modelHub.test.networkError", { message: testError })}</p>
              ) : null}
              {testResult ? (
                <div className="grid min-w-0 gap-2">
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
                    <span className={cn(
                      "font-semibold",
                      testResult.success ? "text-emerald-700" : "text-rose-700",
                    )}>
                      {testResult.response
                        ? `${testResult.response.statusCode} ${testResult.response.statusText}`
                        : testResult.error || t("modelHub.test.needRoute")}
                    </span>
                    <span className="text-slate-400">·</span>
                    <span className="text-slate-500">{t("modelHub.test.durationMs", { ms: String(testResult.durationMs) })}</span>
                    {testResult.targetShareId ? (
                      <>
                        <span className="text-slate-400">·</span>
                        <span className="text-slate-600">
                          {t("modelHub.test.target")}: {targetShare?.subdomain || testResult.targetShareId}
                        </span>
                      </>
                    ) : null}
                    {testResult.response ? (
                      <>
                        <span className="text-slate-400">·</span>
                        <span className="text-slate-600">
                          {testResult.matchedWildcard
                            ? t("modelHub.test.matchedPassthrough")
                            : t("modelHub.test.matchedExact")}
                        </span>
                      </>
                    ) : null}
                  </div>
                  {testResult.error && testResult.response ? (
                    <p className="text-xs text-rose-700">{testResult.error}</p>
                  ) : null}
                  {testResult.response ? (
                    <details className="group min-w-0">
                      <summary className="flex cursor-pointer list-none items-center gap-2 py-1 text-xs font-medium text-slate-600 marker:content-none [&::-webkit-details-marker]:hidden">
                        <ChevronDown className="h-3.5 w-3.5 shrink-0 -rotate-90 text-slate-400 transition-transform group-open:rotate-0" />
                        {t("modelHub.test.response")}
                      </summary>
                      <div className="grid gap-2 py-1.5">
                        <div className="grid gap-0.5">
                          <span className="text-[10px] font-semibold uppercase tracking-wide text-slate-400">{t("modelHub.test.headers")}</span>
                          <div className="max-h-24 overflow-y-auto font-mono text-[11px] text-slate-700">
                            {testResult.response.headers.map(([key, value], index) => (
                              <div key={`${key}:${index}`} className="flex gap-2 leading-relaxed">
                                <span className="shrink-0 text-slate-400">{key}:</span>
                                <span className="min-w-0 break-all">{value}</span>
                              </div>
                            ))}
                          </div>
                        </div>
                        <div className="grid gap-0.5">
                          <span className="text-[10px] font-semibold uppercase tracking-wide text-slate-400">{t("modelHub.test.body")}</span>
                          <pre className="max-h-40 overflow-y-auto whitespace-pre-wrap break-all text-[11px] leading-relaxed text-slate-800">
                            {testResult.response.bodyText || "(empty)"}
                          </pre>
                          {testResult.response.bodyTruncated ? (
                            <span className="text-[10px] text-slate-400">{t("modelHub.test.bodyTruncated")}</span>
                          ) : null}
                        </div>
                      </div>
                    </details>
                  ) : null}
                </div>
              ) : null}
            </section>
          </div>
        </div>
      ) : null}

      <ConfirmAlertDialog open={resetConfirmOpen} title={t("account.apiKeys.resetConfirmTitle")} description={t("modelHub.resetKeyImpact")} confirmLabel={t("account.apiKeys.resetConfirmAction")} cancelLabel={t("common.cancel")} tone="danger" busy={controller.busy} onConfirm={() => void controller.resetToken().then((reset) => reset && setResetConfirmOpen(false))} onOpenChange={(open) => !controller.busy && setResetConfirmOpen(open)} />
      <ConfirmAlertDialog
        open={leaveConfirmOpen}
        title={t("modelHub.unsavedTitle")}
        description={t("modelHub.unsavedDescription")}
        confirmLabel={t("modelHub.unsavedDiscard")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        onConfirm={() => {
          void controller.load();
          setExpanded(false);
          setLeaveConfirmOpen(false);
        }}
        onOpenChange={setLeaveConfirmOpen}
      />
    </section>
  );
}
