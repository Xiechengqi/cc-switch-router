"use client";

import { Download, Eraser, Loader2, Pause, Play, RefreshCw } from "lucide-react";
import { Alert, Button, Card, Chip, ScrollShadow } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { downloadRouterLog } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import type { MessageKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type LogStatus = "connecting" | "live" | "paused" | "disconnected";

type LogPayload = {
  line?: string;
  message?: string;
  path?: string;
  tailLines?: number;
};

const MAX_LINES = 1000;

const LOG_LEVELS = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"] as const;
type LogLevel = (typeof LOG_LEVELS)[number];

export function LogsPanel() {
  const { t } = useLocaleText();
  const [lines, setLines] = React.useState<string[]>([]);
  const [selectedLevels, setSelectedLevels] = React.useState<Set<LogLevel>>(() => new Set(LOG_LEVELS));
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

  const filteredLines = React.useMemo(
    () => lines.filter((line) => matchesLevelFilter(line, selectedLevels)),
    [lines, selectedLevels],
  );

  const levelFilterActive = selectedLevels.size < LOG_LEVELS.length;

  React.useEffect(() => {
    if (paused) return;
    const viewport = viewportRef.current;
    if (!viewport) return;
    viewport.scrollTop = viewport.scrollHeight;
  }, [filteredLines, paused]);

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
        <div className="grid gap-2 rounded-lg border border-slate-200 bg-slate-50/70 p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-xs font-medium text-slate-600">{t("logs.filterLevels")}</span>
            {levelFilterActive ? (
              <span className="text-xs tabular-nums text-slate-500">
                {t("logs.filteredCount", { visible: filteredLines.length, total: lines.length })}
              </span>
            ) : null}
          </div>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" onClick={() => setSelectedLevels(new Set(LOG_LEVELS))}>
              {t("logs.filterAll")}
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => setSelectedLevels(new Set(["TRACE", "DEBUG", "WARN", "ERROR"]))}
            >
              {t("logs.filterNonInfo")}
            </Button>
            <Button size="sm" variant="outline" onClick={() => setSelectedLevels(new Set(["WARN", "ERROR"]))}>
              {t("logs.filterWarnUp")}
            </Button>
            <Button size="sm" variant="outline" onClick={() => setSelectedLevels(new Set(["ERROR"]))}>
              {t("logs.filterErrorOnly")}
            </Button>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {LOG_LEVELS.map((level) => {
              const active = selectedLevels.has(level);
              return (
                <button
                  key={level}
                  type="button"
                  onClick={() => toggleLogLevel(level, setSelectedLevels)}
                  className={cn(
                    "inline-flex h-7 items-center rounded-md border px-2.5 text-[11px] font-semibold tracking-wide transition-colors",
                    active ? logLevelActiveClass(level) : "border-slate-200 bg-white text-slate-400 hover:border-slate-300 hover:text-slate-600",
                  )}
                  aria-pressed={active}
                >
                  {level}
                </button>
              );
            })}
          </div>
        </div>
        <div className="rounded-lg border border-slate-200 bg-white text-slate-900">
          <ScrollShadow className="h-[560px]">
            <pre ref={viewportRef} className="h-[560px] overflow-auto whitespace-pre-wrap break-words p-4 font-mono text-xs leading-5">
              {filteredLines.length ? (
                filteredLines.map((line, index) => (
                  <React.Fragment key={`${index}-${line.slice(0, 24)}`}>
                    <AnsiLogLine line={line} />
                    {index < filteredLines.length - 1 ? "\n" : null}
                  </React.Fragment>
                ))
              ) : lines.length ? (
                t("logs.noMatchingLines")
              ) : (
                t("logs.waiting")
              )}
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

function AnsiLogLine({ line }: { line: string }) {
  return <>{ansiToParts(line).map((part, index) => <span key={index} className={part.className}>{part.text}</span>)}</>;
}

type AnsiPart = {
  text: string;
  className: string;
};

type AnsiStyle = {
  dim: boolean;
  italic: boolean;
  color: string;
};

const ANSI_RE = /\x1b\[([0-9;]*)m/g;

function ansiToParts(input: string): AnsiPart[] {
  const parts: AnsiPart[] = [];
  const style: AnsiStyle = { dim: false, italic: false, color: "" };
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  ANSI_RE.lastIndex = 0;
  while ((match = ANSI_RE.exec(input)) !== null) {
    if (match.index > lastIndex) {
      parts.push({ text: input.slice(lastIndex, match.index), className: ansiClassName(style) });
    }
    applyAnsiCodes(style, match[1]);
    lastIndex = ANSI_RE.lastIndex;
  }

  if (lastIndex < input.length) {
    parts.push({ text: input.slice(lastIndex), className: ansiClassName(style) });
  }
  return parts.length ? parts : [{ text: input, className: "" }];
}

function applyAnsiCodes(style: AnsiStyle, rawCodes: string) {
  const codes = rawCodes
    .split(";")
    .filter(Boolean)
    .map((code) => Number.parseInt(code, 10))
    .filter(Number.isFinite);
  const normalized = codes.length ? codes : [0];
  for (const code of normalized) {
    switch (code) {
      case 0:
        style.dim = false;
        style.italic = false;
        style.color = "";
        break;
      case 2:
        style.dim = true;
        break;
      case 3:
        style.italic = true;
        break;
      case 22:
        style.dim = false;
        break;
      case 23:
        style.italic = false;
        break;
      case 30:
      case 31:
      case 32:
      case 33:
      case 34:
      case 35:
      case 36:
      case 37:
      case 90:
      case 91:
      case 92:
      case 93:
      case 94:
      case 95:
      case 96:
      case 97:
        style.color = ansiColorClass(code);
        break;
      case 39:
        style.color = "";
        break;
    }
  }
}

function ansiClassName(style: AnsiStyle) {
  return [style.color, style.dim ? "opacity-60" : "", style.italic ? "italic" : ""].filter(Boolean).join(" ");
}

function ansiColorClass(code: number) {
  switch (code) {
    case 30:
      return "text-slate-500";
    case 31:
      return "text-red-600";
    case 32:
      return "text-emerald-700";
    case 33:
      return "text-amber-700";
    case 34:
      return "text-sky-700";
    case 35:
      return "text-fuchsia-700";
    case 36:
      return "text-cyan-700";
    case 37:
      return "text-slate-900";
    case 90:
      return "text-slate-500";
    case 91:
      return "text-red-600";
    case 92:
      return "text-emerald-600";
    case 93:
      return "text-amber-700";
    case 94:
      return "text-sky-600";
    case 95:
      return "text-fuchsia-600";
    case 96:
      return "text-cyan-600";
    case 97:
      return "text-slate-900";
    default:
      return "";
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

function stripAnsi(input: string) {
  return input.replace(/\x1b\[[0-9;]*m/g, "");
}

function detectLogLevel(line: string): LogLevel | null {
  const stripped = stripAnsi(line);
  const tracingMatch = stripped.match(/\s(ERROR|WARN|INFO|DEBUG|TRACE)\s/);
  if (tracingMatch) return tracingMatch[1] as LogLevel;
  const bracketMatch = stripped.match(/\[(ERROR|WARN|INFO|DEBUG|TRACE)\]/i);
  if (bracketMatch) return bracketMatch[1].toUpperCase() as LogLevel;
  const jsonMatch = stripped.match(/"level"\s*:\s*"(trace|debug|info|warn|error)"/i);
  if (jsonMatch) return jsonMatch[1].toUpperCase() as LogLevel;
  return null;
}

function isPanelMetaLine(line: string) {
  const stripped = stripAnsi(line).trimStart();
  return stripped.startsWith("[");
}

function matchesLevelFilter(line: string, selectedLevels: Set<LogLevel>) {
  if (selectedLevels.size === LOG_LEVELS.length) return true;
  const level = detectLogLevel(line);
  if (!level) return isPanelMetaLine(line);
  return selectedLevels.has(level);
}

function toggleLogLevel(level: LogLevel, setSelectedLevels: React.Dispatch<React.SetStateAction<Set<LogLevel>>>) {
  setSelectedLevels((current) => {
    const next = new Set(current);
    if (next.has(level)) {
      if (next.size === 1) return current;
      next.delete(level);
    } else {
      next.add(level);
    }
    return next;
  });
}

function logLevelActiveClass(level: LogLevel) {
  switch (level) {
    case "TRACE":
      return "border-slate-300 bg-slate-100 text-slate-700";
    case "DEBUG":
      return "border-cyan-200 bg-cyan-50 text-cyan-800";
    case "INFO":
      return "border-emerald-200 bg-emerald-50 text-emerald-800";
    case "WARN":
      return "border-amber-200 bg-amber-50 text-amber-800";
    case "ERROR":
      return "border-red-200 bg-red-50 text-red-800";
  }
}
