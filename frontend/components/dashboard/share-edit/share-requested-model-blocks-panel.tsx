"use client";

import { Button, Input } from "@heroui/react";
import * as React from "react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { getShareRequestedModelBlocks, replaceShareRequestedModelBlocks } from "@/lib/api";
import { SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import type { ShareRequestedModelBlocks } from "@/lib/types";

const APPS = ["claude", "codex", "gemini"] as const satisfies readonly CoreShareApp[];
type App = (typeof APPS)[number];

export function ShareRequestedModelBlocksPanel({
  shareId,
  apps,
  enabledApps,
  editable,
  t,
}: {
  shareId: string;
  apps: string[];
  enabledApps?: Partial<Record<App, boolean>>;
  editable: boolean;
  t: TFn;
}) {
  const [policy, setPolicy] = React.useState<ShareRequestedModelBlocks | null>(null);
  const [draft, setDraft] = React.useState<ShareRequestedModelBlocks["blockedModelsByApp"]>({});
  const [inputs, setInputs] = React.useState<Partial<Record<App, string>>>({});
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const visibleApps = APPS.filter(
    (app) => apps.includes(app) || Boolean(policy?.blockedModelsByApp[app]?.length),
  );

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
  const payload = React.useMemo(() => {
    const next: ShareRequestedModelBlocks["blockedModelsByApp"] = { ...draft };
    for (const app of APPS) {
      if (enabledApps?.[app] !== false) continue;
      const saved = policy?.blockedModelsByApp[app] || [];
      if (saved.length) next[app] = saved;
      else delete next[app];
    }
    return next;
  }, [draft, enabledApps, policy]);
  const dirty = policy != null && JSON.stringify(payload) !== JSON.stringify(policy.blockedModelsByApp || {});
  const save = async () => {
    if (!policy || !dirty) return;
    setBusy(true);
    setError("");
    try {
      const next = await replaceShareRequestedModelBlocks(shareId, policy.revision, payload);
      setPolicy(next);
      setDraft(next.blockedModelsByApp || {});
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const canSave = editable && dirty && policy != null;

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-sm font-semibold text-slate-900">{t("dashboard.requestedModelBlocks.title")}</div>
          <p className="mt-1 text-xs leading-5 text-slate-500">{t("dashboard.requestedModelBlocks.hint")}</p>
        </div>
        {canSave ? (
          <Button size="sm" variant="primary" isPending={busy} onPress={() => void save()}>
            {t("common.save")}
          </Button>
        ) : null}
      </div>
      {error ? <div className="text-xs text-danger">{error}</div> : null}
      {!policy && !error ? <div className="text-xs text-slate-500">{t("common.loading")}</div> : null}
      {policy ? (
        <div className="grid gap-2">
          {visibleApps.map((app) => {
            const models = draft[app] || [];
            const appEnabled = enabledApps?.[app] !== false;
            const canEditApp = editable && appEnabled;
            return (
              <div
                key={app}
                className={`min-w-0 rounded-xl px-3 py-2.5 ${
                  appEnabled ? "bg-emerald-50/70" : "bg-slate-50 text-slate-500"
                }`}
              >
                <div className="flex min-w-0 items-start gap-2.5">
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
}
