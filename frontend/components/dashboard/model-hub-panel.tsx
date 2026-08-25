"use client";

import { Button, toast } from "@heroui/react";
import {
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
  UserModelRouteInput,
  UserModelRoutingResponse,
} from "@/lib/types";
import {
  buildUnifiedModelCurl,
  canonicalModelRoutes,
  MAX_USER_MODEL_ROUTES,
  patchDraftModelRoute,
  preferredModelRoutingApp,
  validateModelRoutes,
  type DraftModelRoute,
} from "@/lib/model-routing";

const APP_OPTIONS: Array<{ value: ModelRoutingApp; label: string }> = [
  { value: "codex", label: "Codex / OpenAI" },
  { value: "claude", label: "Claude / Anthropic" },
  { value: "gemini", label: "Gemini" },
];

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
  addRouteForShare: (shareId: string) => void;
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

  const addRouteForShare = React.useCallback((shareId: string) => {
    const share = profile?.eligibleShares.find((candidate) => candidate.shareId === shareId);
    if (!share) return;
    setRoutes((current) => current.length >= MAX_USER_MODEL_ROUTES
      ? current
      : [
          ...current,
          {
            clientId: `new:${crypto.randomUUID()}`,
            appType: preferredModelRoutingApp(share),
            requestedModel: "",
            targetShareId: shareId,
          },
        ]);
    setError("");
  }, [profile]);

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
    updateRoute,
    removeRoute,
  };
}

export function ModelHubPanel({ controller }: { controller: ModelRoutingController }) {
  const { t } = useLocaleText();
  const [copied, setCopied] = React.useState<"endpoint" | "token" | "curl" | "">("");
  const [resetConfirmOpen, setResetConfirmOpen] = React.useState(false);
  const profile = controller.profile;
  const maskedToken = controller.rawToken
    ? `${controller.rawToken.slice(0, 8)}${"*".repeat(16)}${controller.rawToken.slice(-4)}`
    : controller.token?.prefix
      ? `${controller.token.prefix}${"*".repeat(16)}`
      : "-";
  const firstCompleteRoute = controller.routes.find((route) => route.requestedModel.trim());
  const curl = buildUnifiedModelCurl(
    profile?.apiBaseUrl || "https://api.example.com",
    controller.rawToken,
    firstCompleteRoute,
  );

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

  if (controller.loading && !profile) {
    return (
      <section id="model-hub" className="flex min-h-32 items-center justify-center border-y border-border bg-white px-4 py-6 text-sm text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden />
        {t("common.loading")}
      </section>
    );
  }

  return (
    <section id="model-hub" className="grid min-w-0 gap-5 border-y border-border bg-white px-4 py-5 sm:px-5">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="flex items-center gap-2 text-base font-semibold text-foreground">
            <Network className="h-4 w-4 text-primary" aria-hidden />
            {t("modelHub.title")}
          </h2>
          <p className="mt-1 text-xs text-muted-foreground">{t("modelHub.optional")}</p>
        </div>
        <div className="flex items-center gap-2">
          <span className="font-mono text-[10px] text-muted-foreground">r{profile?.revision || 0}</span>
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
          <Button variant="primary" size="sm" isDisabled={!controller.dirty || controller.busy} onClick={() => void controller.save()}>
            {controller.busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            {t("common.save")}
          </Button>
        </div>
      </div>

      {controller.error ? (
        <div className="border-l-2 border-rose-500 bg-rose-50 px-3 py-2 text-sm text-rose-700" role="alert">
          {controller.error}
        </div>
      ) : null}

      <div className="grid min-w-0 gap-3 lg:grid-cols-2">
        <div className="grid min-w-0 gap-2">
          <span className="text-[11px] font-medium text-muted-foreground">{t("modelHub.endpoint")}</span>
          <div className="flex min-w-0 items-center gap-2 border border-border bg-slate-50 px-3 py-2">
            <code className="min-w-0 flex-1 truncate text-xs text-foreground" title={profile?.apiBaseUrl}>{profile?.apiBaseUrl || "-"}</code>
            <button type="button" className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-white hover:text-foreground" title={t("common.copy")} aria-label={t("common.copy")} onClick={() => void copy("endpoint", profile?.apiBaseUrl || "")}>
              <Copy className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
        <div className="grid min-w-0 gap-2">
          <span className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground"><KeyRound className="h-3.5 w-3.5" />{t("modelHub.apiKey")}</span>
          <div className="flex min-w-0 items-center gap-1 border border-border bg-slate-50 px-3 py-2">
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

      <div className="grid min-w-0 gap-3">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-sm font-semibold text-foreground">{t("modelHub.routes")}</h3>
          <Button variant="outline" size="sm" isDisabled={!profile?.eligibleShares.length || controller.busy || controller.routes.length >= MAX_USER_MODEL_ROUTES} onClick={() => profile?.eligibleShares[0] && controller.addRouteForShare(profile.eligibleShares[0].shareId)}>
            <Plus className="h-4 w-4" />
            {t("modelHub.addRoute")}
          </Button>
        </div>
        {controller.routes.length ? (
          <div className="grid min-w-0 gap-2">
            {controller.routes.map((route) => {
              const shares = (profile?.eligibleShares || []).filter((share) => share.apps.includes(route.appType));
              const targetAvailable = shares.some((share) => share.shareId === route.targetShareId);
              const targetOptions = targetAvailable || !route.targetShareId
                ? shares.map((share) => ({
                    value: share.shareId,
                    label: share.shareName || share.subdomain,
                    description: `${share.subdomain} · ${share.isOnline ? t("common.online") : t("common.offline")}`,
                  }))
                : [
                    {
                      value: route.targetShareId,
                      label: route.targetShareId,
                      description: t("modelHub.targetUnavailable"),
                    },
                    ...shares.map((share) => ({
                      value: share.shareId,
                      label: share.shareName || share.subdomain,
                      description: `${share.subdomain} · ${share.isOnline ? t("common.online") : t("common.offline")}`,
                    })),
                  ];
              return (
                <div key={route.clientId} className="grid min-w-0 items-center gap-2 border-b border-border py-2 sm:grid-cols-[180px_minmax(160px,1fr)_minmax(200px,1.3fr)_32px]">
                  <CompactSelect value={route.appType} options={APP_OPTIONS} onChange={(value) => controller.updateRoute(route.clientId, { appType: value as ModelRoutingApp })} ariaLabel={t("modelHub.app")} disabled={controller.busy} />
                  <input value={route.requestedModel} maxLength={200} disabled={controller.busy} onChange={(event) => controller.updateRoute(route.clientId, { requestedModel: event.target.value })} className="h-9 min-w-0 border border-border bg-white px-3 text-xs outline-none focus:border-primary/60 focus:ring-2 focus:ring-primary/10" aria-label={t("modelHub.model")} placeholder={t("modelHub.modelPlaceholder")} />
                  <CompactSelect value={route.targetShareId} options={targetOptions} onChange={(value) => controller.updateRoute(route.clientId, { targetShareId: value })} ariaLabel={t("modelHub.targetShare")} disabled={controller.busy || !targetOptions.length} />
                  <button type="button" disabled={controller.busy} className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-rose-50 hover:text-rose-700 disabled:opacity-40" title={t("modelHub.removeRoute")} aria-label={t("modelHub.removeRoute")} onClick={() => controller.removeRoute(route.clientId)}>
                    <Trash2 className="h-4 w-4" />
                  </button>
                </div>
              );
            })}
          </div>
        ) : (
          <div className="border border-dashed border-border px-4 py-5 text-center text-sm text-muted-foreground">{t("modelHub.empty")}</div>
        )}
      </div>

      <div className="grid min-w-0 gap-2">
        <div className="flex items-center justify-between gap-3">
          <span className="text-[11px] font-medium text-muted-foreground">curl</span>
          <button type="button" className="inline-flex h-7 items-center gap-1 rounded-md px-2 text-xs text-muted-foreground hover:bg-slate-100 hover:text-foreground" onClick={() => void copy("curl", curl)}>
            <Copy className="h-3.5 w-3.5" />
            {copied === "curl" ? t("account.apiKeys.copied") : t("common.copy")}
          </button>
        </div>
        <pre className="max-w-full overflow-x-auto border border-border bg-slate-950 px-3 py-3 font-mono text-[11px] leading-5 text-slate-100">{curl}</pre>
      </div>

      <ConfirmAlertDialog open={resetConfirmOpen} title={t("account.apiKeys.resetConfirmTitle")} description={t("modelHub.resetKeyImpact")} confirmLabel={t("account.apiKeys.resetConfirmAction")} cancelLabel={t("common.cancel")} tone="danger" busy={controller.busy} onConfirm={() => void controller.resetToken().then((reset) => reset && setResetConfirmOpen(false))} onOpenChange={(open) => !controller.busy && setResetConfirmOpen(open)} />
    </section>
  );
}
