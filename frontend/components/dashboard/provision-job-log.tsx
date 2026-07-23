"use client";

import * as React from "react";
import { Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";

type JobLogTone = "default" | "phase" | "installer" | "command" | "error" | "warn" | "muted" | "success";

type AnsiStyle = {
  dim: boolean;
  italic: boolean;
  bold: boolean;
  color: string;
  bg: string;
};

const ANSI_RE = /\x1b\[([0-9;]*)m/g;

function stripAnsi(input: string) {
  return input.replace(/\x1b\[[0-9;]*m/g, "");
}

function ansiColorClass(code: number) {
  switch (code) {
    case 30:
    case 90:
      return "text-slate-500";
    case 31:
    case 91:
      return "text-rose-600";
    case 32:
    case 92:
      return "text-emerald-700";
    case 33:
    case 93:
      return "text-amber-700";
    case 34:
    case 94:
      return "text-sky-700";
    case 35:
    case 95:
      return "text-fuchsia-700";
    case 36:
    case 96:
      return "text-cyan-700";
    case 37:
    case 97:
      return "text-slate-800";
    default:
      return "";
  }
}

function ansiBgClass(code: number) {
  switch (code) {
    case 44:
    case 104:
      return "rounded-sm bg-sky-100 px-0.5 text-sky-900";
    case 41:
    case 101:
      return "rounded-sm bg-rose-100 px-0.5 text-rose-800";
    case 42:
    case 102:
      return "rounded-sm bg-emerald-100 px-0.5 text-emerald-800";
    case 43:
    case 103:
      return "rounded-sm bg-amber-100 px-0.5 text-amber-900";
    default:
      return "";
  }
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
        style.bold = false;
        style.color = "";
        style.bg = "";
        break;
      case 1:
        style.bold = true;
        break;
      case 2:
        style.dim = true;
        break;
      case 3:
        style.italic = true;
        break;
      case 22:
        style.dim = false;
        style.bold = false;
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
      case 40:
      case 41:
      case 42:
      case 43:
      case 44:
      case 45:
      case 46:
      case 47:
      case 100:
      case 101:
      case 102:
      case 103:
      case 104:
      case 105:
      case 106:
      case 107:
        style.bg = ansiBgClass(code);
        break;
      case 49:
        style.bg = "";
        break;
    }
  }
}

function ansiClassName(style: AnsiStyle) {
  return [
    style.color,
    style.bg,
    style.dim ? "opacity-60" : "",
    style.italic ? "italic" : "",
    style.bold ? "font-semibold" : "",
  ]
    .filter(Boolean)
    .join(" ");
}

function ansiToParts(input: string) {
  const parts: { text: string; className: string }[] = [];
  const style: AnsiStyle = { dim: false, italic: false, bold: false, color: "", bg: "" };
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

function classifyLine(raw: string): JobLogTone {
  const line = stripAnsi(raw).trim();
  if (!line) return "muted";
  const lower = line.toLowerCase();

  if (
    lower.includes("error:") ||
    lower.includes("failed") ||
    lower.includes("conflict") ||
    lower.includes("provisioning error") ||
    lower.includes("开通失败")
  ) {
    return "error";
  }
  if (lower.includes("[sensitive output redacted]") || lower.includes("warning") || lower.includes("retry")) {
    return "warn";
  }
  if (
    lower.includes("install-client.sh") ||
    lower.startsWith("remote installer output") ||
    lower.includes('bash "$script"')
  ) {
    return "installer";
  }
  if (
    lower.startsWith("curl ") ||
    lower.startsWith("chmod ") ||
    lower.startsWith("sleep ") ||
    lower.startsWith("mkdir ") ||
    lower.startsWith("cc-switch-server ") ||
    lower.includes("ensure_client_market_deps")
  ) {
    return "command";
  }
  if (
    lower.includes("completed") ||
    lower.includes("succeeded") ||
    lower.includes("waiting for tunnel") ||
    lower.startsWith("starting provisioning") ||
    lower.startsWith("reserved one matching host") ||
    lower.startsWith("remote installer completed")
  ) {
    return "phase";
  }
  if (lower.includes("success") || lower.includes("client created")) {
    return "success";
  }
  return "default";
}

function lineShellClass(tone: JobLogTone) {
  switch (tone) {
    case "installer":
      return "rounded-md bg-sky-50/90 px-1.5 py-0.5 ring-1 ring-inset ring-sky-200/80";
    case "command":
      return "rounded-md bg-slate-100/80 px-1.5 py-0.5";
    case "error":
      return "rounded-md bg-rose-50 px-1.5 py-0.5 ring-1 ring-inset ring-rose-200/70";
    case "warn":
      return "rounded-md bg-amber-50/90 px-1.5 py-0.5";
    case "phase":
      return "rounded-md bg-emerald-50/80 px-1.5 py-0.5";
    case "success":
      return "rounded-md bg-emerald-50 px-1.5 py-0.5 ring-1 ring-inset ring-emerald-200/70";
    case "muted":
      return "opacity-50";
    default:
      return "";
  }
}

function lineTextClass(tone: JobLogTone) {
  switch (tone) {
    case "installer":
      return "text-sky-900";
    case "command":
      return "text-slate-800";
    case "error":
      return "text-rose-700";
    case "warn":
      return "text-amber-800";
    case "phase":
      return "font-medium text-emerald-800";
    case "success":
      return "font-medium text-emerald-800";
    default:
      return "text-slate-700";
  }
}

function highlightInstallScriptText(text: string, tone: JobLogTone) {
  if (tone !== "installer" && !text.toLowerCase().includes("install-client.sh")) {
    return text;
  }
  const marker = "install-client.sh";
  const index = text.toLowerCase().indexOf(marker);
  if (index < 0) return text;
  const end = index + marker.length;
  return (
    <>
      {text.slice(0, index)}
      <mark className="rounded bg-sky-200/80 px-0.5 font-semibold text-sky-950">{text.slice(index, end)}</mark>
      {text.slice(end)}
    </>
  );
}

function ProvisionLogLine({ line }: { line: string }) {
  const tone = classifyLine(line);
  const parts = ansiToParts(line);
  const hasAnsiColor = parts.some((part) => part.className.includes("text-") || part.className.includes("bg-"));

  return (
    <div className={cn("whitespace-pre-wrap break-words", lineShellClass(tone))}>
      {parts.map((part, index) => (
        <span
          key={index}
          className={cn(!hasAnsiColor || !part.className ? lineTextClass(tone) : undefined, part.className)}
        >
          {highlightInstallScriptText(part.text, tone)}
        </span>
      ))}
    </div>
  );
}

export function ProvisionJobLog({
  log,
  phase,
}: {
  log: string;
  phase: "running" | "failed" | "success";
}) {
  const { t } = useLocaleText();
  const viewportRef = React.useRef<HTMLDivElement | null>(null);
  const lines = React.useMemo(() => (log ? log.replace(/\r\n/g, "\n").split("\n") : []), [log]);
  const hasInstaller = lines.some((line) => stripAnsi(line).toLowerCase().includes("install-client.sh"));

  React.useEffect(() => {
    const node = viewportRef.current;
    if (!node) return;
    node.scrollTop = node.scrollHeight;
  }, [log]);

  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="font-mono text-[10px] uppercase tracking-[0.14em] text-slate-500">
          {t("createClient.log")}
        </div>
        <div className="inline-flex items-center gap-2 text-[11px]">
          {phase === "running" ? (
            <span className="inline-flex items-center gap-1.5 rounded-full border border-sky-200 bg-sky-50 px-2 py-0.5 font-medium text-sky-800">
              <Loader2 className="h-3 w-3 animate-spin" />
              {t("createClient.provisioning")}
            </span>
          ) : null}
          {hasInstaller ? (
            <span className="rounded-full border border-sky-200/80 bg-sky-50 px-2 py-0.5 font-mono text-[10px] font-semibold tracking-wide text-sky-800">
              install-client.sh
            </span>
          ) : null}
        </div>
      </div>
      <div className="overflow-hidden rounded-xl border border-slate-200 bg-white shadow-[inset_0_1px_0_rgba(255,255,255,0.8)]">
        <div
          ref={viewportRef}
          className="max-h-[min(48vh,360px)] overflow-auto bg-gradient-to-b from-slate-50 to-white p-3 font-mono text-[11px] leading-5"
        >
          {lines.length ? (
            <div className="grid gap-1">
              {lines.map((line, index) =>
                line.length === 0 && index === lines.length - 1 ? null : (
                  <ProvisionLogLine key={`${index}-${line.slice(0, 32)}`} line={line || " "} />
                ),
              )}
            </div>
          ) : (
            <p className="text-slate-400">{t("createClient.logWaiting")}</p>
          )}
        </div>
      </div>
    </div>
  );
}
