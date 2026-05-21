"use client";

import { Download, Eraser, Loader2, Pause, Play, RefreshCw } from "lucide-react";
import { Alert, Button, Card, Chip, ScrollShadow } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { downloadRouterLog } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import type { MessageKey } from "@/lib/i18n";

type LogStatus = "connecting" | "live" | "paused" | "disconnected";

type LogPayload = {
  line?: string;
  message?: string;
  path?: string;
  tailLines?: number;
};

const MAX_LINES = 1000;

export function LogsPanel() {
  const { t } = useLocaleText();
  const [lines, setLines] = React.useState<string[]>([]);
  const [status, setStatus] = React.useState<LogStatus>("connecting");
  const [error, setError] = React.useState("");
  const [paused, setPaused] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const sourceRef = React.useRef<EventSource | null>(null);
  const viewportRef = React.useRef<HTMLPreElement | null>(null);
  const pausedRef = React.useRef(paused);

  React.useEffect(() => {
    pausedRef.current = paused;
    setStatus(paused ? "paused" : sourceRef.current ? "live" : "disconnected");
  }, [paused]);

  const appendLine = React.useCallback((line: string) => {
    setLines((prev) => [...prev, line].slice(-MAX_LINES));
  }, []);

  const connect = React.useCallback(() => {
    sourceRef.current?.close();
    setStatus("connecting");
    setError("");
    const token = readAuthState().accessToken;
    const params = new URLSearchParams();
    if (token) params.set("accessToken", token);
    const source = new EventSource(`/v1/admin/logs/router/tail${params.toString() ? `?${params}` : ""}`);
    sourceRef.current = source;

    source.addEventListener("ready", (event) => {
      const data = parsePayload(event);
      setStatus(pausedRef.current ? "paused" : "live");
      appendLine(`[${t("logs.ready")}] ${data.path || "/var/log/cc-switch-router.log"} (${data.tailLines ?? 0})`);
    });
    source.addEventListener("line", (event) => {
      if (pausedRef.current) return;
      const data = parsePayload(event);
      if (data.line !== undefined) appendLine(data.line);
    });
    source.addEventListener("reset", (event) => {
      const data = parsePayload(event);
      appendLine(`[${t("logs.reset")}] ${data.message || ""}`.trim());
    });
    source.addEventListener("missing", (event) => {
      const data = parsePayload(event);
      setError(data.message || t("logs.missing"));
      setStatus("disconnected");
    });
    source.addEventListener("error", (event) => {
      const data = parsePayload(event);
      if (data.message) setError(data.message);
    });
    source.onerror = () => {
      setStatus("disconnected");
      setError(t("logs.disconnected"));
      source.close();
      if (sourceRef.current === source) sourceRef.current = null;
    };
  }, [appendLine, t]);

  React.useEffect(() => {
    connect();
    return () => {
      sourceRef.current?.close();
      sourceRef.current = null;
    };
  }, [connect]);

  React.useEffect(() => {
    if (paused) return;
    const viewport = viewportRef.current;
    if (!viewport) return;
    viewport.scrollTop = viewport.scrollHeight;
  }, [lines, paused]);

  async function download() {
    setBusy(true);
    setError("");
    try {
      await downloadRouterLog();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="rounded-lg">
      <Card.Header className="flex-row items-start justify-between gap-4 space-y-0">
        <div>
          <Card.Title>{t("logs.title")}</Card.Title>
          <Card.Description>{t("logs.description")}</Card.Description>
        </div>
        <Chip color={status === "live" ? "success" : status === "paused" ? "warning" : "default"} size="sm" variant="soft">
          {t(logStatusKey(status))}
        </Chip>
      </Card.Header>
      <Card.Content className="grid gap-4">
        {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" onClick={download} isDisabled={busy}>
            {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Download className="h-4 w-4" />}
            {t("logs.download")}
          </Button>
          <Button variant="outline" onClick={() => setPaused((value) => !value)}>
            {paused ? <Play className="h-4 w-4" /> : <Pause className="h-4 w-4" />}
            {paused ? t("logs.resume") : t("logs.pause")}
          </Button>
          <Button variant="outline" onClick={() => setLines([])}>
            <Eraser className="h-4 w-4" />
            {t("logs.clear")}
          </Button>
          <Button variant="outline" onClick={connect}>
            <RefreshCw className="h-4 w-4" />
            {t("logs.reconnect")}
          </Button>
        </div>
        <div className="rounded-lg border bg-slate-950 text-slate-100">
          <ScrollShadow className="h-[560px]">
            <pre ref={viewportRef} className="h-[560px] overflow-auto whitespace-pre-wrap break-words p-4 font-mono text-xs leading-5">
              {lines.length ? lines.join("\n") : t("logs.waiting")}
            </pre>
          </ScrollShadow>
        </div>
      </Card.Content>
    </Card>
  );
}

function parsePayload(event: Event): LogPayload {
  try {
    return JSON.parse((event as MessageEvent).data || "{}");
  } catch {
    return { line: (event as MessageEvent).data || "" };
  }
}

function logStatusKey(status: LogStatus): MessageKey {
  switch (status) {
    case "connecting":
      return "logs.connecting";
    case "live":
      return "logs.live";
    case "paused":
      return "logs.paused";
    case "disconnected":
      return "logs.disconnected";
  }
}
