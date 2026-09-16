"use client";

import { Button, Input, Tooltip } from "@heroui/react";
import { Info } from "lucide-react";
import * as React from "react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { getShareRequestedModelBlocks, replaceShareRequestedModelBlocks } from "@/lib/api";
import { SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import type { ShareRequestedModelBlocks } from "@/lib/types";

const APPS = ["claude", "codex", "gemini"] as const satisfies readonly CoreShareApp[];
type App = (typeof APPS)[number];
type BlockedModelsByApp = ShareRequestedModelBlocks["blockedModelsByApp"];

function blockedModelsFingerprint(value: BlockedModelsByApp) {
  return JSON.stringify(value || {});
}

function freezeDisabledAppBlocks(
  draft: BlockedModelsByApp,
  enabledApps: Partial<Record<App, boolean>> | undefined,
  saved: BlockedModelsByApp | undefined,
): BlockedModelsByApp {
  const next: BlockedModelsByApp = { ...draft };
  for (const app of APPS) {
    if (enabledApps?.[app] !== false) continue;
    const kept = saved?.[app] || [];
    if (kept.length) next[app] = kept;
    else delete next[app];
  }
  return next;
}

export type ShareRequestedModelBlocksHandle = {
  dirty: boolean;
  reset: () => void;
  save: () => Promise<void>;
};

export const ShareRequestedModelBlocksPanel = React.forwardRef<
  ShareRequestedModelBlocksHandle,
  {
    shareId: string;
    apps: string[];
    enabledApps?: Partial<Record<App, boolean>>;
    editable: boolean;
    t: TFn;
    onStateChange?: (state: { dirty: boolean }) => void;
  }
>(function ShareRequestedModelBlocksPanel(
  { shareId, apps, enabledApps, editable, t, onStateChange },
  ref,
) {
  const [policy, setPolicy] = React.useState<ShareRequestedModelBlocks | null>(null);
  const [draft, setDraft] = React.useState<BlockedModelsByApp>({});
  const [inputs, setInputs] = React.useState<Partial<Record<App, string>>>({});
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const visibleApps = APPS.filter(
    (app) => apps.includes(app) || Boolean(policy?.blockedModelsByApp[app]?.length),
  );
  const payload = React.useMemo(
    () => freezeDisabledAppBlocks(draft, enabledApps, policy?.blockedModelsByApp),
    [draft, enabledApps, policy],
  );
  const dirty = policy != null && blockedModelsFingerprint(payload) !== blockedModelsFingerprint(policy.blockedModelsByApp || {});

  React.useEffect(() => {
    let cancelled = false;
    setPolicy(null);
    setError("");
    void getShareRequestedModelBlocks(shareId)
      .then((next) => {
        if (cancelled) return;
        setPolicy(next);
        setDraft(next.blockedModelsByApp || {});
      })
      .catch((reason) => !cancelled && setError(reason instanceof Error ? reason.message : String(reason)));
    return () => {
      cancelled = true;
    };
  }, [shareId]);

  React.useEffect(() => {
    onStateChange?.({ dirty });
  }, [dirty, onStateChange]);

  const add = (app: App) => {
    const model = (inputs[app] || "").trim();
    if (!model || model.length > 200 || model.includes("*")) return;
    setDraft((current) => ({
      ...current,
      [app]: Array.from(new Set([...(current[app] || []), model])).sort(),
    }));
    setInputs((current) => ({ ...current, [app]: "" }));
  };
  const remove = (app: App, model: string) =>
    setDraft((current) => ({
      ...current,
      [app]: (current[app] || []).filter((item) => item !== model),
    }));
  const reset = React.useCallback(() => {
    if (!policy) return;
    setDraft(policy.blockedModelsByApp || {});
    setInputs({});
    setError("");
  }, [policy]);
  const save = React.useCallback(async () => {
    if (!policy || !dirty) return;
    setBusy(true);
    setError("");
    try {
      const next = await replaceShareRequestedModelBlocks(shareId, policy.revision, payload);
      setPolicy(next);
      setDraft(next.blockedModelsByApp || {});
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      throw reason;
    } finally {
      setBusy(false);
    }
  }, [dirty, payload, policy, shareId]);

  React.useImperativeHandle(ref, () => ({ dirty, reset, save }), [dirty, reset, save]);

  return (
    <div className="grid gap-3">
      <div className="flex min-w-0 items-center gap-1">
        <div className="text-sm font-semibold text-slate-900">{t("dashboard.requestedModelBlocks.title")}</div>
        <Tooltip>
          <Tooltip.Trigger>
            <Button
              isIconOnly
              size="sm"
              variant="ghost"
              className="h-6 w-6 min-w-6 text-muted-foreground"
              aria-label={t("dashboard.requestedModelBlocks.hint")}
            >
              <Info className="h-3.5 w-3.5" />
            </Button>
          </Tooltip.Trigger>
          <Tooltip.Content className="max-w-xs text-xs leading-5">
            {t("dashboard.requestedModelBlocks.hint")}
          </Tooltip.Content>
        </Tooltip>
      </div>
      {error ? <div className="text-xs text-danger">{error}</div> : null}
      {!policy && !error ? <div className="text-xs text-slate-500">{t("common.loading")}</div> : null}
      {policy ? (
        <div className="grid grid-cols-3 gap-2">
          {visibleApps.map((app) => {
            const models = draft[app] || [];
            const appEnabled = enabledApps?.[app] !== false;
            const canEditApp = editable && appEnabled && !busy;
            return (
              <div
                key={app}
                className={`min-w-0 rounded-xl px-3 py-2.5 ${
                  appEnabled ? "bg-emerald-50/70" : "bg-slate-50 text-slate-500"
                }`}
              >
                <div className="flex min-w-0 items-start gap-2">
                  <ShareAppLogo app={app} size={16} className={`mt-0.5 ${appEnabled ? "" : "opacity-60"}`} />
                  <div className="min-w-0 flex-1">
                    <div className="flex min-w-0 items-center justify-between gap-2">
                      <span className={`truncate text-sm font-medium ${appEnabled ? "text-slate-900" : "text-slate-500"}`}>
                        {SHARE_APP_LABELS[app]}
                      </span>
                      <span className="shrink-0 text-[11px] text-slate-400">
                        {models.length
                          ? t("dashboard.requestedModelBlocks.count", { count: models.length })
                          : t("dashboard.requestedModelBlocks.empty")}
                      </span>
                    </div>
                    {!appEnabled ? (
                      <p className="mt-1 text-[11px] leading-4 text-slate-400">
                        {editable
                          ? t("dashboard.requestedModelBlocks.appDisabled")
                          : t("dashboard.requestedModelBlocks.appDisabledReadonly")}
                      </p>
                    ) : null}
                    {models.length ? (
                      <div className="mt-2 flex flex-wrap gap-1.5">
                        {models.map((model) => (
                          <span
                            key={model}
                            className={`inline-flex max-w-full items-center gap-1 rounded-full px-2.5 py-1 text-xs ${
                              appEnabled
                                ? "border border-rose-200 bg-white text-rose-700"
                                : "border border-slate-200 bg-white/70 text-slate-500"
                            }`}
                          >
                            <span className="truncate">{model}</span>
                            {canEditApp ? (
                              <button
                                type="button"
                                className="text-rose-500 hover:text-rose-700"
                                aria-label={`${t("common.delete")} ${model}`}
                                onClick={() => remove(app, model)}
                              >
                                ×
                              </button>
                            ) : null}
                          </span>
                        ))}
                      </div>
                    ) : null}
                    {canEditApp ? (
                      <div className="mt-2 flex min-w-0 items-center gap-2">
                        <Input
                          className="min-w-0 flex-1"
                          value={inputs[app] || ""}
                          maxLength={200}
                          placeholder={t("dashboard.requestedModelBlocks.placeholder")}
                          onChange={(event) =>
                            setInputs((current) => ({ ...current, [app]: event.target.value }))
                          }
                          onKeyDown={(event) => {
                            if (event.key === "Enter") {
                              event.preventDefault();
                              add(app);
                            }
                          }}
                        />
                        <Button size="sm" variant="secondary" onPress={() => add(app)}>
                          {t("common.add")}
                        </Button>
                      </div>
                    ) : null}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
});
