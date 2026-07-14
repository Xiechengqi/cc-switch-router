"use client";

import { ExternalLink } from "lucide-react";
import * as React from "react";
import { ClientConsoleIframeSlot } from "@/components/dashboard/client-console/client-console-iframe-pool";
import { ClientConsoleTrafficLights } from "@/components/dashboard/client-console/client-console-traffic-lights";
import {
  CONSOLE_DOCK_HEIGHT,
  type ConsoleWindow,
  type NormalRect,
  useClientConsole,
} from "@/components/dashboard/client-console/client-console-manager";
import { useLocaleText } from "@/components/i18n/locale-provider";

const MIN_WIDTH = 420;
const MIN_HEIGHT = 280;
const RESIZE_HANDLE = 14;

function clampRect(rect: NormalRect): NormalRect {
  if (typeof globalThis.window === "undefined") return rect;
  const maxW = globalThis.window.innerWidth - 24;
  const maxH = globalThis.window.innerHeight - CONSOLE_DOCK_HEIGHT - 24;
  const width = Math.min(Math.max(rect.width, MIN_WIDTH), maxW);
  const height = Math.min(Math.max(rect.height, MIN_HEIGHT), maxH);
  const x = Math.min(Math.max(rect.x, 8), Math.max(8, globalThis.window.innerWidth - width - 8));
  const y = Math.min(Math.max(rect.y, 8), Math.max(8, globalThis.window.innerHeight - height - CONSOLE_DOCK_HEIGHT - 8));
  return { x, y, width, height };
}

function frameHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export function ClientConsoleWindowLayer() {
  const { windows, dockVisible, focusedId, closeConsole, minimizeConsole, toggleMaximizeConsole, focusConsole, updateConsoleRect } =
    useClientConsole();

  const dockOffset = dockVisible ? CONSOLE_DOCK_HEIGHT + 12 : 0;
  const visibleWindows = windows.filter((window) => window.activated && window.state !== "minimized");

  return (
    <>
      {visibleWindows.map((window) => (
        <ClientConsoleWindowShell
          key={window.id}
          window={window}
          focused={window.id === focusedId}
          dockOffset={dockOffset}
          onClose={() => closeConsole(window.id)}
          onMinimize={() => minimizeConsole(window.id)}
          onToggleMaximize={() => toggleMaximizeConsole(window.id)}
          onFocus={() => focusConsole(window.id)}
          onRectChange={(rect) => updateConsoleRect(window.id, rect)}
        />
      ))}
    </>
  );
}

function ClientConsoleWindowShell({
  window,
  focused,
  dockOffset,
  onClose,
  onMinimize,
  onToggleMaximize,
  onFocus,
  onRectChange,
}: {
  window: ConsoleWindow;
  focused: boolean;
  dockOffset: number;
  onClose: () => void;
  onMinimize: () => void;
  onToggleMaximize: () => void;
  onFocus: () => void;
  onRectChange: (rect: NormalRect) => void;
}) {
  const { t } = useLocaleText();
  const host = frameHost(window.url);
  const maximized = window.state === "maximized";
  const dragRef = React.useRef<{ startX: number; startY: number; origin: NormalRect } | null>(null);
  const resizeRef = React.useRef<{ startX: number; startY: number; origin: NormalRect } | null>(null);

  const onDragPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (maximized) return;
    if ((event.target as HTMLElement).closest("[data-no-drag]")) return;
    event.preventDefault();
    onFocus();
    dragRef.current = { startX: event.clientX, startY: event.clientY, origin: window.normalRect };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onDragPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return;
    const dx = event.clientX - dragRef.current.startX;
    const dy = event.clientY - dragRef.current.startY;
    onRectChange(
      clampRect({
        ...dragRef.current.origin,
        x: dragRef.current.origin.x + dx,
        y: dragRef.current.origin.y + dy,
      }),
    );
  };

  const onDragPointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return;
    dragRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
  };

  const onResizePointerDown = (event: React.PointerEvent<HTMLButtonElement>) => {
    if (maximized) return;
    event.preventDefault();
    event.stopPropagation();
    onFocus();
    resizeRef.current = { startX: event.clientX, startY: event.clientY, origin: window.normalRect };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onResizePointerMove = (event: React.PointerEvent<HTMLButtonElement>) => {
    if (!resizeRef.current) return;
    const dx = event.clientX - resizeRef.current.startX;
    const dy = event.clientY - resizeRef.current.startY;
    onRectChange(
      clampRect({
        ...resizeRef.current.origin,
        width: resizeRef.current.origin.width + dx,
        height: resizeRef.current.origin.height + dy,
      }),
    );
  };

  const onResizePointerUp = (event: React.PointerEvent<HTMLButtonElement>) => {
    if (!resizeRef.current) return;
    resizeRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
  };

  const shellClass =
    "light flex flex-col overflow-hidden rounded-2xl border border-slate-200/80 bg-white text-slate-900 shadow-[0_24px_60px_rgba(15,23,42,0.16)] [--foreground:rgb(15,23,42)] [--muted:rgb(100,116,139)]";

  const style: React.CSSProperties = maximized
    ? {
        position: "fixed",
        top: "0.75rem",
        left: "0.75rem",
        right: "0.75rem",
        bottom: `calc(0.75rem + ${dockOffset}px)`,
        zIndex: window.zIndex,
      }
    : {
        position: "fixed",
        top: window.normalRect.y,
        left: window.normalRect.x,
        width: window.normalRect.width,
        height: window.normalRect.height,
        zIndex: window.zIndex,
      };

  return (
    <div
      className={`${shellClass} ${focused ? "ring-2 ring-primary/20" : ""}`}
      style={style}
      onPointerDown={onFocus}
      role="dialog"
      aria-label={`${t("dashboard.clientConsole")} ${window.title}`}
      aria-modal={maximized ? "true" : "false"}
    >
      <div
        className="flex cursor-default items-center gap-3 border-b border-slate-100 bg-slate-50/90 px-3 py-2.5"
        onPointerDown={onDragPointerDown}
        onPointerMove={onDragPointerMove}
        onPointerUp={onDragPointerUp}
        onPointerCancel={onDragPointerUp}
      >
        <ClientConsoleTrafficLights maximized={maximized} onClose={onClose} onMinimize={onMinimize} onToggleMaximize={onToggleMaximize} />
        <div className="min-w-0 flex-1 text-center">
          <p className="truncate text-[12px] font-medium text-slate-700">{window.title}</p>
          <a
            href={window.url}
            target="_blank"
            rel="noopener noreferrer"
            data-no-drag
            className="mt-0.5 inline-flex max-w-full items-center justify-center gap-1 truncate text-[10px] font-mono text-muted-foreground underline-offset-4 transition-colors hover:text-primary hover:underline"
            title={window.url}
            onClick={(event) => event.stopPropagation()}
          >
            <span className="min-w-0 truncate">{host}</span>
            <ExternalLink className="h-2.5 w-2.5 shrink-0" />
          </a>
        </div>
        <div className="w-[52px] shrink-0" aria-hidden />
      </div>

      <div className="relative min-h-0 flex-1 bg-slate-50 p-3">
        <ClientConsoleIframeSlot
          windowId={window.id}
          className="h-full overflow-hidden rounded-xl border border-slate-200/80 bg-transparent shadow-[inset_0_1px_0_rgba(255,255,255,0.7)]"
        />
        {!maximized ? (
          <button
            type="button"
            aria-label={t("dashboard.clientConsole.resize")}
            className="absolute bottom-1 right-1 z-10 cursor-se-resize rounded-sm bg-transparent"
            style={{ width: RESIZE_HANDLE, height: RESIZE_HANDLE }}
            onPointerDown={onResizePointerDown}
            onPointerMove={onResizePointerMove}
            onPointerUp={onResizePointerUp}
            onPointerCancel={onResizePointerUp}
          />
        ) : null}
      </div>
    </div>
  );
}
