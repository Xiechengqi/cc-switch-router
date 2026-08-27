"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Check, ChevronDown, Copy, Loader2, RefreshCw } from "lucide-react";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { refreshShareUsage, testShareConnection } from "@/lib/api";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import { buildShareProbeCurl } from "@/lib/share-model-probe";
import type {
  ShareConnectionTestResponse,
  ShareUpstreamProvider,
  ShareView,
} from "@/lib/types";

type TFn = ReturnType<typeof useLocaleText>["t"];
type TestApp = "claude" | "codex" | "gemini";
type TestOperation = "text" | "image_generation" | "image_edit" | "video_generation";

function runtimeForApp(share: ShareView, app: TestApp) {
  return share.appRuntimes?.[app];
}

function modelPolicyDescription(runtime: ShareUpstreamProvider | undefined, t: TFn) {
  if (runtime?.modelPolicy?.mode === "single") {
    return t("dashboard.connectDialog.test.policySingle", {
      model: runtime.modelPolicy.upstreamModel,
    });
  }
  if (runtime?.modelPolicy?.mode === "passthrough") {
    return t("dashboard.connectDialog.test.policyPassthrough", {
      provider: runtime.providerName || runtime.providerType || t("dashboard.connectDialog.test.providerUnknown"),
    });
  }
  return t("dashboard.connectDialog.test.policyUnknown");
}

function InlineCopyButton({ value, t }: { value: string; t: TFn }) {
  const [copied, setCopied] = React.useState(false);
  const copy = React.useCallback(
    async (event: React.MouseEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (!value) return;
      try {
        await navigator.clipboard.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      } catch {
        // 静默失败
      }
    },
    [value],
  );
  return (
    <span className="relative inline-flex shrink-0">
      <button
        type="button"
        onClick={copy}
        disabled={!value}
        title={copied ? t("dashboard.connectDialog.copyOk") : t("dashboard.connectDialog.copy")}
        aria-label={copied ? t("dashboard.connectDialog.copyOk") : t("dashboard.connectDialog.copy")}
        className="inline-flex h-6 w-6 items-center justify-center rounded text-slate-400 hover:bg-slate-100 hover:text-slate-700 disabled:cursor-not-allowed disabled:opacity-40"
      >
        <Copy className="h-3.5 w-3.5" />
      </button>
      {copied ? (
        <span
          role="status"
          aria-live="polite"
          className="pointer-events-none absolute -top-6 right-0 inline-flex animate-fade-in-up items-center gap-1 rounded bg-emerald-600 px-1.5 py-0.5 text-[11px] font-medium text-white shadow-sm"
        >
          <Check className="h-2.5 w-2.5" />
          {t("dashboard.connectDialog.copyOk")}
        </span>
      ) : null}
    </span>
  );
}

type TestState = "idle" | "running" | "done" | "error";

export function ShareConnectionTestRow({
  share,
  app,
  apiToken,
  baseUrl,
  authenticated,
  canExecute,
}: {
  share: ShareView;
  app: TestApp;
  apiToken: string;
  baseUrl: string;
  authenticated: boolean;
  canExecute: boolean;
}) {
  const { t } = useLocaleText();
  const [testState, setTestState] = React.useState<TestState>("idle");
  const [result, setResult] = React.useState<ShareConnectionTestResponse | null>(null);
  const [errorMsg, setErrorMsg] = React.useState("");
  const [refreshState, setRefreshState] = React.useState<TestState>("idle");
  const [refreshMsg, setRefreshMsg] = React.useState("");
  const [operation, setOperation] = React.useState<TestOperation>("text");

  const isBound = !!(share.bindings?.[app]);
  const runtime = runtimeForApp(share, app);
  const probe = runtime?.modelProbe;
  const curlCmd = React.useMemo(
    () => (baseUrl && probe ? buildShareProbeCurl(baseUrl, probe, apiToken) : ""),
    [baseUrl, probe, apiToken],
  );

  const runTest = React.useCallback(async () => {
    if (
      !canExecute ||
      !isBound ||
      (operation === "text" && !probe) ||
      testState === "running"
    ) return;
    setTestState("running");
    setResult(null);
    setErrorMsg("");
    try {
      const response = await testShareConnection(share.shareId, {
        app,
        operation,
        timeoutMs: 30000,
      });
      setResult(response);
      setTestState(response.success ? "done" : "error");
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
      setTestState("error");
    }
  }, [canExecute, isBound, probe, testState, share.shareId, app, operation]);

  const runUsageRefresh = React.useCallback(async () => {
    if (!canExecute || !isBound || refreshState === "running") return;
    setRefreshState("running");
    setRefreshMsg("");
    try {
      const response = await refreshShareUsage(share.shareId, { app });
      const failed = response.refreshed.filter((item) => !item.refreshed);
      if (failed.length > 0) {
        setRefreshState("error");
        setRefreshMsg(
          failed
            .map((item) => `${item.app}: ${item.error || "failed"}`)
            .join("; "),
        );
      } else {
        setRefreshState("done");
        const labels = response.refreshed
          .map((item) => item.providerName || item.providerId || item.app)
          .join(", ");
        setRefreshMsg(labels || t("dashboard.connectDialog.test.refreshUsageDone"));
      }
    } catch (err) {
      setRefreshState("error");
      setRefreshMsg(err instanceof Error ? err.message : String(err));
    }
  }, [canExecute, isBound, refreshState, share.shareId, app, t]);

  const running = testState === "running";
  const refreshing = refreshState === "running";
  const canRefreshUsage = share.canManage;

  let disabledReason: string | null = null;
  if (!isBound) disabledReason = t("dashboard.connectDialog.test.notBound");
  else if (operation === "text" && !probe) disabledReason = t("dashboard.connectDialog.test.probeUnavailable");
  else if (!authenticated) disabledReason = t("dashboard.connectDialog.test.needAuth");
  else if (!canExecute) disabledReason = t("dashboard.connectDialog.test.needPermission");

  const statusColor = result?.response
    ? result.success
      ? "text-emerald-700"
      : result.response.statusCode < 500
        ? "text-amber-700"
        : "text-red-700"
    : "text-slate-500";

  if (!isBound) {
    return (
      <div className="flex items-center justify-between gap-3 py-2.5 text-sm">
        <span className="flex min-w-0 items-center gap-2">
          <ShareAppLogo app={app} size={14} />
          <span className="font-medium text-slate-700">{SHARE_APP_LABELS[app]}</span>
        </span>
        <span className="text-xs text-slate-400">{disabledReason}</span>
      </div>
    );
  }

  return (
    <div className="grid gap-2.5 py-3 text-sm">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <ShareAppLogo app={app} size={14} />
          <span className="shrink-0 font-medium text-slate-900">{SHARE_APP_LABELS[app]}</span>
        </div>
        {disabledReason ? (
          <span className="text-xs text-slate-400">{disabledReason}</span>
        ) : (
          <div className="flex shrink-0 items-center gap-1.5">
            {app === "codex" ? (
              <select
                value={operation}
                onChange={(event) => setOperation(event.target.value as TestOperation)}
                disabled={running}
                className="h-8 rounded-md border border-slate-200 bg-white px-2 text-xs text-slate-700"
              >
                <option value="text">{t("dashboard.connectDialog.test.operationText")}</option>
                {share.grokMediaPolicy?.imageGenerationEnabled ? (
                  <option value="image_generation">{t("dashboard.connectDialog.test.operationImageGeneration")}</option>
                ) : null}
                {share.grokMediaPolicy?.imageEditEnabled ? (
                  <option value="image_edit">{t("dashboard.connectDialog.test.operationImageEdit")}</option>
                ) : null}
                {share.grokMediaPolicy?.videoGenerationEnabled ? (
                  <option value="video_generation">{t("dashboard.connectDialog.test.operationVideoGeneration")}</option>
                ) : null}
              </select>
            ) : null}
            {canRefreshUsage ? (
              <Button
                size="sm"
                variant="ghost"
                isDisabled={refreshing}
                onClick={refreshing ? undefined : runUsageRefresh}
                aria-label={t("dashboard.connectDialog.test.refreshUsage")}
              >
                {refreshing ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <RefreshCw className="h-3.5 w-3.5" />
                )}
                {t("dashboard.connectDialog.test.refreshUsageShort")}
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="outline"
              isDisabled={running}
              onClick={running ? undefined : runTest}
            >
              {running ? (
                <>
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  {t("dashboard.connectDialog.test.running")}
                </>
              ) : (
                t("dashboard.connectDialog.test.button")
              )}
            </Button>
          </div>
        )}
      </div>

      {operation === "text" && probe ? (
        <p className="text-xs leading-5 text-slate-500">
          {modelPolicyDescription(runtime, t)}{" "}
          <span className="text-slate-400">·</span>{" "}
          {t("dashboard.connectDialog.test.testModel")}{" "}
          <code className="break-all font-mono text-slate-700">{probe.requestedModel}</code>
        </p>
      ) : null}

      {refreshMsg ? (
        <p
          className={`text-xs ${
            refreshState === "error" ? "text-red-700" : "text-emerald-700"
          }`}
        >
          {refreshState === "error"
            ? t("dashboard.connectDialog.test.refreshUsageError", { message: refreshMsg })
            : t("dashboard.connectDialog.test.refreshUsageOk", { target: refreshMsg })}
        </p>
      ) : null}

      {operation === "text" && curlCmd ? (
        <details className="group">
          <summary className="flex cursor-pointer list-none items-center gap-2 py-1 text-xs font-medium text-slate-600 marker:content-none [&::-webkit-details-marker]:hidden">
            <ChevronDown className="h-3.5 w-3.5 shrink-0 -rotate-90 text-slate-400 transition-transform group-open:rotate-0" />
            <span className="min-w-0 flex-1 font-mono">{t("dashboard.connectDialog.test.curlLabel")}</span>
            <InlineCopyButton value={curlCmd} t={t} />
          </summary>
          <pre className="overflow-x-auto whitespace-pre-wrap break-all py-1.5 font-mono text-[11px] leading-relaxed text-slate-800">
            {curlCmd}
          </pre>
        </details>
      ) : null}

      {testState === "error" && errorMsg ? (
        <p className="text-xs text-red-700">
          {t("dashboard.connectDialog.test.networkError", { message: errorMsg })}
        </p>
      ) : null}

      {result ? (
        <div className="grid gap-2">
          {result.error ? (
            <p className="text-xs text-red-600">{result.error}</p>
          ) : result.response ? (
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
              <span className={`font-semibold ${statusColor}`}>
                {result.response.statusCode} {result.response.statusText}
              </span>
              <span className="text-slate-400">·</span>
              <span className="text-slate-500">
                {t("dashboard.connectDialog.test.durationMs", { ms: String(result.durationMs) })}
              </span>
              {result.schedulingRecovery ? (
                <>
                  <span className="text-slate-400">·</span>
                  <span className="text-emerald-700">
                    {t("dashboard.connectDialog.test.schedulingRecovered")}
                  </span>
                </>
              ) : null}
            </div>
          ) : null}

          {result.response ? (
            <details className="group">
              <summary className="flex cursor-pointer list-none items-center gap-2 py-1 text-xs font-medium text-slate-600 marker:content-none [&::-webkit-details-marker]:hidden">
                <ChevronDown className="h-3.5 w-3.5 shrink-0 -rotate-90 text-slate-400 transition-transform group-open:rotate-0" />
                <span className="min-w-0 flex-1">{t("dashboard.connectDialog.test.responseToggle")}</span>
                <InlineCopyButton value={result.response.bodyText} t={t} />
              </summary>
              <div className="grid gap-3 py-1.5">
                <div className="grid gap-0.5">
                  <span className="text-[10px] font-semibold uppercase tracking-wide text-slate-400">
                    {t("dashboard.connectDialog.test.headers")}
                  </span>
                  <div className="max-h-28 overflow-y-auto font-mono text-[11px] text-slate-700">
                    {result.response.headers.map(([k, v], i) => (
                      <div key={i} className="flex gap-2 leading-relaxed">
                        <span className="shrink-0 text-slate-400">{k}:</span>
                        <span className="min-w-0 break-all">{v}</span>
                      </div>
                    ))}
                  </div>
                </div>
                <div className="grid gap-0.5">
                  <span className="text-[10px] font-semibold uppercase tracking-wide text-slate-400">
                    {t("dashboard.connectDialog.test.body")}
                  </span>
                  <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap break-all text-[11px] leading-relaxed text-slate-800">
                    {result.response.bodyText || "(empty)"}
                  </pre>
                  {result.response.bodyTruncated ? (
                    <span className="text-[10px] text-slate-400">
                      {t("dashboard.connectDialog.test.bodyTruncated")}
                    </span>
                  ) : null}
                </div>
              </div>
            </details>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
