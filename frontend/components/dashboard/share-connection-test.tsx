"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Check, ChevronDown, ChevronRight, Copy, Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { testShareConnection } from "@/lib/api";
import type { ShareConnectionTestResponse, ShareView } from "@/lib/types";

type TFn = ReturnType<typeof useLocaleText>["t"];

/** 每 app 写死的 curl 参数 */
const APP_PROBE = {
  claude: {
    method: "POST",
    path: "/v1/messages",
    body: JSON.stringify({
      model: "claude-opus-4-7",
      max_tokens: 1,
      messages: [{ role: "user", content: "hi" }],
    }),
  },
  codex: {
    method: "POST",
    path: "/v1/responses",
    body: JSON.stringify({
      model: "gpt-5.5",
      input: "hi",
      max_output_tokens: 16,
    }),
  },
  gemini: {
    method: "POST",
    path: "/v1beta/models/gemini-flash-2.5:generateContent",
    body: JSON.stringify({
      contents: [{ parts: [{ text: "hi" }] }],
      generationConfig: { maxOutputTokens: 1 },
    }),
  },
} as const;

function buildCurlCommand(baseUrl: string, app: keyof typeof APP_PROBE, apiToken: string) {
  const probe = APP_PROBE[app];
  const url = `${baseUrl}${probe.path}`;
  const bearer = apiToken
    ? `Bearer ${apiToken}`
    : "Bearer <your-api-token>";
  return [
    `curl -sS -X ${probe.method} \\`,
    `  '${url}' \\`,
    `  -H 'Authorization: ${bearer}' \\`,
    `  -H 'Content-Type: application/json' \\`,
    `  -d '${probe.body}'`,
  ].join("\n");
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
  canExecute,
}: {
  share: ShareView;
  app: "claude" | "codex" | "gemini";
  apiToken: string;
  baseUrl: string;
  canExecute: boolean;
}) {
  const { t } = useLocaleText();
  const [testState, setTestState] = React.useState<TestState>("idle");
  const [result, setResult] = React.useState<ShareConnectionTestResponse | null>(null);
  const [errorMsg, setErrorMsg] = React.useState("");
  const [resultOpen, setResultOpen] = React.useState(false);

  const isBound = !!(share.bindings?.[app]);
  const curlCmd = React.useMemo(
    () => (baseUrl ? buildCurlCommand(baseUrl, app, apiToken) : ""),
    [baseUrl, app, apiToken],
  );

  const runTest = React.useCallback(async () => {
    if (!canExecute || !isBound || testState === "running") return;
    setTestState("running");
    setErrorMsg("");
    try {
      const response = await testShareConnection(share.shareId, { app, timeoutMs: 15000 });
      setResult(response);
      setResultOpen(true);
      setTestState("done");
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
      setTestState("error");
    }
  }, [canExecute, isBound, testState, share.shareId, app]);

  const running = testState === "running";

  let disabledReason: string | null = null;
  if (!isBound) disabledReason = t("dashboard.connectDialog.test.notBound");
  else if (!canExecute) disabledReason = t("dashboard.connectDialog.test.needPermission");

  const statusColor = result?.response
    ? result.response.statusCode < 300
      ? "text-emerald-700"
      : result.response.statusCode < 500
        ? "text-amber-700"
        : "text-red-700"
    : "text-slate-500";

  return (
    <div className={`grid gap-2 rounded-lg border px-3 py-2.5 text-sm ${isBound ? "border-slate-200 bg-slate-50" : "border-slate-100 bg-slate-50/40 opacity-60"}`}>
      {/* Header row: app label + disabled reason or test button */}
      <div className="flex items-center justify-between gap-2">
        <span className="font-mono text-xs font-semibold uppercase tracking-wide text-slate-600">
          {app}
        </span>
        {disabledReason ? (
          <span className="text-[11px] text-slate-400">{disabledReason}</span>
        ) : (
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
        )}
      </div>

      {/* curl preview */}
      {curlCmd ? (
        <div className="grid gap-1">
          <div className="flex items-start gap-1">
            <span className="mt-0.5 shrink-0 text-[10px] font-semibold uppercase tracking-wide text-slate-400">
              {t("dashboard.connectDialog.test.curlLabel")}
            </span>
          </div>
          <div className="relative rounded border border-slate-200 bg-white">
            <pre className="overflow-x-auto px-3 py-2 text-[11px] leading-relaxed text-slate-800">{curlCmd}</pre>
            <span className="absolute right-1.5 top-1.5">
              <InlineCopyButton value={curlCmd} t={t} />
            </span>
          </div>
        </div>
      ) : null}

      {/* Error from fetch itself (network / auth) */}
      {testState === "error" && errorMsg ? (
        <div className="rounded border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700">
          {t("dashboard.connectDialog.test.networkError", { message: errorMsg })}
        </div>
      ) : null}

      {/* Result panel */}
      {result ? (
        <div className="grid gap-1">
          {/* Summary line – always visible, click to expand */}
          <button
            type="button"
            onClick={() => setResultOpen((v) => !v)}
            className="flex items-center gap-2 text-xs"
          >
            {resultOpen ? <ChevronDown className="h-3.5 w-3.5 text-slate-400" /> : <ChevronRight className="h-3.5 w-3.5 text-slate-400" />}
            {result.error ? (
              <span className="text-red-600">{result.error}</span>
            ) : result.response ? (
              <>
                <span className={`font-semibold ${statusColor}`}>
                  {result.response.statusCode} {result.response.statusText}
                </span>
                <span className="text-slate-400">·</span>
                <span className="text-slate-500">
                  {t("dashboard.connectDialog.test.durationMs", { ms: String(result.durationMs) })}
                </span>
              </>
            ) : null}
          </button>

          {resultOpen && result.response ? (
            <div className="grid gap-2 rounded border border-slate-200 bg-white px-3 py-2">
              {/* Response headers */}
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

              {/* Response body */}
              <div className="grid gap-0.5">
                <div className="flex items-center justify-between gap-1">
                  <span className="text-[10px] font-semibold uppercase tracking-wide text-slate-400">
                    {t("dashboard.connectDialog.test.body")}
                  </span>
                  <InlineCopyButton value={result.response.bodyText} t={t} />
                </div>
                <pre className="max-h-48 overflow-auto rounded border border-slate-100 bg-slate-50 px-2 py-1.5 text-[11px] leading-relaxed text-slate-800">
                  {result.response.bodyText || "(empty)"}
                </pre>
                {result.response.bodyTruncated ? (
                  <span className="text-[10px] text-slate-400">
                    {t("dashboard.connectDialog.test.bodyTruncated")}
                  </span>
                ) : null}
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
