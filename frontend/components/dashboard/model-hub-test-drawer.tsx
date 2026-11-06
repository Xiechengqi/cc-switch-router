"use client";

// Hallmark · component: test drawer · modern-minimal · existing Router tokens
// Pre-emit critique: P4 H4 E4 S5 R5 V4. No automatic requests or retries.
import * as React from "react";
import { Button, Drawer } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { testUserModelRouting } from "@/lib/api";
import { buildUnifiedModelCurl, isWildcardModel } from "@/lib/model-routing";
import { ModelRoutingTestSession } from "@/lib/model-routing-test-session";
import type { ModelRoutingApp, UserModelRoutingResponse } from "@/lib/types";

export function ModelHubTestDrawer({ open, onOpenChange, actorKey, app, profile, disabled, model, onModel, onComplete }: {
  open: boolean; onOpenChange: (open: boolean) => void; actorKey: string; app: ModelRoutingApp;
  profile: UserModelRoutingResponse | null; disabled: boolean; model: string; onModel: (model: string) => void; onComplete: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const session = React.useMemo(() => new ModelRoutingTestSession(testUserModelRouting), [actorKey]);
  const state = React.useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const [mode, setMode] = React.useState<"primary" | "saved_route">("primary");
  React.useEffect(() => { session.start(); return () => session.stop(); }, [session]);
  React.useEffect(() => { session.invalidate(); }, [session, app, model, profile?.revision, disabled, open]);
  const supportsDiagnostic = !!profile?.testCapabilities?.savedRoute && !!profile.testCapabilities.revisionCheck;
  const unavailable = mode === "saved_route" && !supportsDiagnostic;
  React.useEffect(() => {
    if (!supportsDiagnostic && mode === "saved_route") { session.invalidate(); setMode("primary"); }
  }, [supportsDiagnostic, mode, session]);
  const [copied, setCopied] = React.useState(false);
  const curl = buildUnifiedModelCurl(profile?.apiBaseUrl || "https://api.example.test", "", { appType: app, requestedModel: model });
  return <Drawer.Backdrop isOpen={open} onOpenChange={onOpenChange}>
    <Drawer.Content placement="right"><Drawer.Dialog className="w-full max-w-2xl bg-card text-foreground">
      <Drawer.CloseTrigger className="!h-11 !w-11 !bg-secondary !text-foreground" />
      <Drawer.Header><Drawer.Heading className="pr-12 text-base">{t("modelHub.test.title")}</Drawer.Heading></Drawer.Header>
      <Drawer.Body className="grid min-w-0 content-start gap-4 overflow-y-auto pb-6">
        <p className="text-sm leading-relaxed text-muted-foreground">{t("modelHub.test.scope")}</p>
        {supportsDiagnostic ? <CompactSelect value={mode} options={[
          { value: "primary", label: t("modelHub.test.primary") }, { value: "saved_route", label: t("modelHub.test.savedRoute") },
        ]} onChange={value => { session.invalidate(); setMode(value as typeof mode); }} disabled={state.running}
          ariaLabel={t("modelHub.test.title")} triggerClassName="h-11 w-full" /> : null}
        {disabled ? <p className="text-sm text-amber-800" role="status">{t("modelHub.test.saveFirst")}</p> : null}
        {unavailable ? <p role="status" className="text-sm text-amber-800">{t("modelHub.test.capabilityMissing")}</p> : null}
        <label className="grid min-w-0 gap-2 text-sm">{t("modelHub.test.model")}
          <input value={model} disabled={state.running} onChange={event => { session.invalidate(); onModel(Array.from(event.target.value).slice(0, 200).join("")); }}
            className="h-11 min-w-0 rounded border border-input bg-card px-3 hover:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50" />
        </label>
        <Button className="min-h-11 whitespace-nowrap" isDisabled={disabled || unavailable || !profile || !model.trim() || isWildcardModel(model) || state.running}
          onClick={() => profile && void session.run(profile, app, model, mode).then(onComplete)}>
          {state.running ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
          {t(state.running ? "modelHub.test.running" : "modelHub.test.run")}
        </Button>
        <details className="min-w-0"><summary className="min-h-11 cursor-pointer py-3 text-sm focus-visible:outline-2 focus-visible:outline-ring">{t("modelHub.test.curl")}</summary>
          <Button variant="outline" className="min-h-11" onClick={() => void navigator.clipboard.writeText(curl).then(() => setCopied(true), () => setCopied(false))}>{t(copied ? "account.apiKeys.copied" : "common.copy")}</Button>
          <pre className="max-h-60 max-w-full overflow-auto whitespace-pre-wrap break-all py-3 text-xs">{curl}</pre>
        </details>
        {state.error ? <div role="alert" className="text-sm text-danger">
          <p>{state.error === "capability_missing" ? t("modelHub.test.capabilityMissing") : state.error === "snapshot_mismatch" ? t("modelHub.test.snapshotMismatch") : state.error === "invalidated" ? t("modelHub.test.invalidated") : state.error}</p>
          {state.unknown ? <p className="pt-2">{t("modelHub.test.unknown")}</p> : null}
        </div> : null}
        {state.result ? <div className="grid min-w-0 gap-3 text-sm" role="status">
          <p className={state.result.success ? "text-emerald-800" : "text-danger"}>{state.result.response ? `HTTP ${state.result.response.statusCode} · ${state.result.durationMs} ms` : state.result.error}</p>
          <p>{state.result.revision == null ? t("modelHub.test.revisionUnknown") : t("modelHub.test.revision", { revision: state.result.revision })}</p>
          {state.result.bodyState ? <p>{t(`modelHub.test.body.${state.result.bodyState}`)}</p> : null}
          {state.result.attempts ? <details><summary className="min-h-11 cursor-pointer py-3 focus-visible:outline-2 focus-visible:outline-ring">{t("modelHub.test.attempts")}</summary>
            <ol className="grid gap-2">{state.result.attempts.map((attempt, index) => <li key={index} className="break-words [overflow-wrap:anywhere]">
              {attempt.position + 1}. {profile?.eligibleShares.find(share => share.shareId === attempt.targetShareId)?.subdomain || attempt.targetShareId} · {t(`modelHub.attempt.${attempt.outcome}`)}{attempt.statusCode ? ` · HTTP ${attempt.statusCode}` : ""}
            </li>)}</ol></details> : null}
          {state.result.response ? <details className="min-w-0"><summary className="min-h-11 cursor-pointer py-3 focus-visible:outline-2 focus-visible:outline-ring">{t("modelHub.test.response")}</summary>
            <pre className="max-h-96 max-w-full overflow-auto whitespace-pre-wrap break-all text-xs">{state.result.response.bodyText}</pre>
          </details> : null}
        </div> : null}
      </Drawer.Body>
    </Drawer.Dialog></Drawer.Content>
  </Drawer.Backdrop>;
}
