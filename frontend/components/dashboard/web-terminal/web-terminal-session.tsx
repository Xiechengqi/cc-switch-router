"use client";

import * as React from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { createClientMarketTerminalSession } from "@/lib/api";
import "@xterm/xterm/css/xterm.css";

const MSG_INPUT = "1";
const MSG_PING = "2";
const MSG_RESIZE = "3";
const MSG_OUTPUT = "1";

const TERMINAL_FONT_FAMILY =
  'var(--font-source-code-pro), "Source Code Pro", ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace';

function terminalWsUrl(ticket: string) {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}/v1/client-market/terminal/ws?ticket=${encodeURIComponent(ticket)}`;
}

function encodeInput(data: string) {
  const bytes = new TextEncoder().encode(data);
  let binary = "";
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return MSG_INPUT + btoa(binary);
}

function decodeOutput(payload: string) {
  const binary = atob(payload);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

function adaptiveFontSize(container: HTMLElement): number {
  const width = container.clientWidth || 640;
  const height = container.clientHeight || 360;
  const byWidth = width / 68;
  const byHeight = height / 28;
  return Math.max(11, Math.min(20, Math.round(Math.min(byWidth, byHeight))));
}

export function WebTerminalSession({
  hostId,
  active,
}: {
  hostId: string;
  /** When false (minimized / off-route), keep session but skip fit churn. */
  active: boolean;
}) {
  const { t } = useLocaleText();
  const containerRef = React.useRef<HTMLDivElement | null>(null);
  const termRef = React.useRef<Terminal | null>(null);
  const fitRef = React.useRef<FitAddon | null>(null);
  const socketRef = React.useRef<WebSocket | null>(null);
  const pingTimerRef = React.useRef<ReturnType<typeof setInterval> | null>(null);
  const [status, setStatus] = React.useState<"connecting" | "connected" | "error">("connecting");
  const [error, setError] = React.useState("");

  const sendResize = React.useCallback(() => {
    const term = termRef.current;
    const socket = socketRef.current;
    if (!term || !socket || socket.readyState !== WebSocket.OPEN) return;
    socket.send(
      MSG_RESIZE +
        JSON.stringify({
          columns: term.cols,
          rows: term.rows,
        }),
    );
  }, []);

  const fitTerminal = React.useCallback(() => {
    const container = containerRef.current;
    const term = termRef.current;
    const fit = fitRef.current;
    if (!container || !term || !fit) return;
    const nextSize = adaptiveFontSize(container);
    if (term.options.fontSize !== nextSize) {
      term.options.fontSize = nextSize;
    }
    fit.fit();
    sendResize();
  }, [sendResize]);

  React.useEffect(() => {
    let cancelled = false;
    setStatus("connecting");
    setError("");

    const connect = async () => {
      try {
        const session = await createClientMarketTerminalSession(hostId);
        if (cancelled) return;
        const container = containerRef.current;
        if (!container) {
          throw new Error(t("clientMarket.terminalMountFailed"));
        }

        const term = new Terminal({
          cursorBlink: true,
          convertEol: true,
          fontFamily: TERMINAL_FONT_FAMILY,
          fontSize: adaptiveFontSize(container),
          theme: {
            background: "#FFFFFF",
            foreground: "#0F172A",
            cursor: "#0052FF",
            cursorAccent: "#FFFFFF",
            selectionBackground: "rgba(0, 82, 255, 0.18)",
            black: "#0F172A",
            red: "#DC2626",
            green: "#059669",
            yellow: "#D97706",
            blue: "#0052FF",
            magenta: "#7C3AED",
            cyan: "#0891B2",
            white: "#F8FAFC",
            brightBlack: "#64748B",
            brightRed: "#EF4444",
            brightGreen: "#10B981",
            brightYellow: "#F59E0B",
            brightBlue: "#4D7CFF",
            brightMagenta: "#8B5CF6",
            brightCyan: "#06B6D4",
            brightWhite: "#FFFFFF",
          },
        });
        const fit = new FitAddon();
        term.loadAddon(fit);
        term.open(container);
        fit.fit();
        termRef.current = term;
        fitRef.current = fit;

        const socket = new WebSocket(terminalWsUrl(session.ticket), ["webtty"]);
        socket.binaryType = "arraybuffer";
        socketRef.current = socket;
        let opened = false;

        socket.onopen = () => {
          if (cancelled) return;
          opened = true;
          setStatus("connected");
          fitTerminal();
          term.focus();
          pingTimerRef.current = setInterval(() => {
            if (socket.readyState === WebSocket.OPEN) socket.send(MSG_PING);
          }, 30_000);
        };

        socket.onmessage = (event) => {
          const raw =
            typeof event.data === "string" ? event.data : new TextDecoder().decode(event.data);
          if (!raw) return;
          if (raw[0] === MSG_OUTPUT) {
            try {
              term.write(decodeOutput(raw.slice(1)));
            } catch {
              // Ignore malformed frames.
            }
          }
        };

        socket.onerror = () => {
          if (cancelled) return;
          setStatus("error");
          setError(t("clientMarket.terminalConnectionFailed"));
        };

        socket.onclose = () => {
          if (cancelled) return;
          if (!opened) {
            setStatus("error");
            setError(t("clientMarket.terminalConnectionFailed"));
          }
          term.writeln("");
          term.writeln(`[${t("clientMarket.terminalDisconnected")}]`);
        };

        term.onData((data) => {
          if (socket.readyState === WebSocket.OPEN) {
            socket.send(encodeInput(data));
          }
        });

        const onWindowResize = () => fitTerminal();
        window.addEventListener("resize", onWindowResize);
        const resizeObserver =
          typeof ResizeObserver !== "undefined" ? new ResizeObserver(() => fitTerminal()) : null;
        resizeObserver?.observe(container);

        return () => {
          window.removeEventListener("resize", onWindowResize);
          resizeObserver?.disconnect();
        };
      } catch (err) {
        if (cancelled) return;
        setStatus("error");
        setError(err instanceof Error ? err.message : String(err));
      }
    };

    let removeResize: (() => void) | undefined;
    void connect().then((cleanupResize) => {
      removeResize = cleanupResize;
    });

    return () => {
      cancelled = true;
      removeResize?.();
      if (pingTimerRef.current) {
        clearInterval(pingTimerRef.current);
        pingTimerRef.current = null;
      }
      if (socketRef.current) {
        socketRef.current.onopen = null;
        socketRef.current.onmessage = null;
        socketRef.current.onerror = null;
        socketRef.current.onclose = null;
        if (
          socketRef.current.readyState === WebSocket.OPEN ||
          socketRef.current.readyState === WebSocket.CONNECTING
        ) {
          socketRef.current.close();
        }
        socketRef.current = null;
      }
      if (termRef.current) {
        termRef.current.dispose();
        termRef.current = null;
      }
      fitRef.current = null;
    };
  }, [fitTerminal, hostId, t]);

  React.useEffect(() => {
    if (!active) return;
    const id = window.requestAnimationFrame(() => fitTerminal());
    return () => window.cancelAnimationFrame(id);
  }, [active, fitTerminal]);

  return (
    <div className="relative h-full w-full bg-white">
      {status === "connecting" ? (
        <div className="absolute inset-0 z-10 flex items-center justify-center gap-2 bg-white/90 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin text-primary" />
          {t("clientMarket.terminalConnecting")}
        </div>
      ) : null}
      {status === "error" && error ? (
        <div className="absolute inset-x-0 top-0 z-10 border-b border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-700">
          {error}
        </div>
      ) : null}
      <div ref={containerRef} className="h-full w-full p-1" />
    </div>
  );
}
