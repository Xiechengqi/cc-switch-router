"use client";

import { Button, Input } from "@heroui/react";
import * as React from "react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { getShareRequestedModelBlocks, replaceShareRequestedModelBlocks } from "@/lib/api";
import type { ShareRequestedModelBlocks } from "@/lib/types";

const APPS = ["claude", "codex", "gemini"] as const;
type App = (typeof APPS)[number];

export function ShareRequestedModelBlocksPanel({
  shareId,
  apps,
  editable,
  t,
}: {
  shareId: string;
  apps: string[];
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
    return () => { cancelled = true; };
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
  const remove = (app: App, model: string) => setDraft((current) => ({
    ...current,
    [app]: (current[app] || []).filter((item) => item !== model),
  }));
  const dirty = policy != null && JSON.stringify(draft) !== JSON.stringify(policy.blockedModelsByApp || {});
  const save = async () => {
    if (!policy || !dirty) return;
    setBusy(true);
    setError("");
    try {
      const next = await replaceShareRequestedModelBlocks(shareId, policy.revision, draft);
      setPolicy(next);
      setDraft(next.blockedModelsByApp || {});
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally { setBusy(false); }
  };

  return (
    <div className="grid gap-3 rounded-xl border border-slate-200 bg-slate-50/60 p-3">
      <div>
        <div className="text-sm font-semibold text-slate-900">{t("dashboard.requestedModelBlocks.title")}</div>
        <p className="mt-1 text-xs text-slate-500">{t("dashboard.requestedModelBlocks.hint")}</p>
      </div>
      {error ? <div className="text-xs text-danger">{error}</div> : null}
      {!policy && !error ? <div className="text-xs text-slate-500">{t("common.loading")}</div> : null}
      {policy && visibleApps.map((app) => (
        <div key={app} className="grid gap-2">
          <div className="text-xs font-medium uppercase text-slate-500">{app}</div>
          <div className="flex flex-wrap gap-1.5">
            {(draft[app] || []).map((model) => (
              <span key={model} className="inline-flex max-w-full items-center gap-1 rounded-full border border-danger/20 bg-danger/10 px-2.5 py-1 text-xs text-danger">
                <span className="truncate">{model}</span>
                {editable ? <button type="button" aria-label={`${t("common.delete")} ${model}`} onClick={() => remove(app, model)}>×</button> : null}
              </span>
            ))}
            {!(draft[app] || []).length ? <span className="text-xs text-slate-400">{t("dashboard.requestedModelBlocks.empty")}</span> : null}
          </div>
          {editable ? (
            <div className="flex gap-2">
              <Input value={inputs[app] || ""} maxLength={200} placeholder={t("dashboard.requestedModelBlocks.placeholder")} onChange={(event) => setInputs((current) => ({ ...current, [app]: event.target.value }))} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); add(app); } }} />
              <Button variant="secondary" onPress={() => add(app)}>{t("common.add")}</Button>
            </div>
          ) : null}
        </div>
      ))}
      {editable && policy ? <div className="flex justify-end"><Button variant="primary" isDisabled={!dirty} isPending={busy} onPress={() => void save()}>{t("common.save")}</Button></div> : null}
    </div>
  );
}
