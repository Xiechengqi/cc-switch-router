"use client";

import { ExternalLink, RotateCw } from "lucide-react";
import { usePathname } from "next/navigation";
import * as React from "react";
import { ClientConsoleIframe } from "@/components/dashboard/client-console/client-console-iframe";
import { ClientConsoleTrafficLights } from "@/components/dashboard/client-console/client-console-traffic-lights";
import {
  CONSOLE_DOCK_HEIGHT,
  CONSOLE_DOCK_RESERVED_HEIGHT,
  type ConsoleWindow,
  type NormalRect,
  useClientConsole,
} from "@/components/dashboard/client-console/client-console-manager";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_CLIENTS_PATH, isClientsRoute } from "@/lib/dashboard-nav";

const MIN_WIDTH = 420;
const MIN_HEIGHT = 280;
const RESIZE_HANDLE = 14;

function clampRect(rect: NormalRect): NormalRect {
  if (typeof globalThis.window === "undefined") return rect;
  const maxW = globalThis.window.innerWidth - 24;
  const maxH = globalThis.window.innerHeight - CONSOLE_DOCK_RESERVED_HEIGHT - 24;
  const width = Math.min(Math.max(rect.width, MIN_WIDTH), maxW);
  const height = Math.min(Math.max(rect.height, MIN_HEIGHT), maxH);
  const x = Math.min(Math.max(rect.x, 8), Math.max(8, globalThis.window.innerWidth - width - 8));
  const y = Math.min(
    Math.max(rect.y, 8),
    Math.max(8, globalThis.window.innerHeight - height - CONSOLE_DOCK_RESERVED_HEIGHT - 8),
  );
  return { x, y, width, height };
}

function useConsoleClickOutsideMinimize({
  enabled,
  windows,
  focusedId,
  minimizeConsole,
}: {
  enabled: boolean;
  windows: ConsoleWindow[];
  focusedId: string | null;
  minimizeConsole: (id: string) => void;
}) {
  React.useEffect(() => {
    if (!enabled) return;

    function shouldIgnoreClickOutside(target: Element) {
      if (target.closest("[data-console-window]")) return true;
      if (target.closest("[data-console-dock]")) return true;
      if (target.closest("[data-board-dock]")) return true;
      if (target.closest("[data-rac]")) return true;
      if (target.closest("[role='dialog']")) return true;
      if (target.closest("[role='alertdialog']")) return true;
      return false;
    }

    function handlePointerDown(event: PointerEvent) {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (shouldIgnoreClickOutside(target)) return;

      const visible = windows.filter((window) => window.activated && window.state !== "minimized");
      if (!visible.length) return;

      let id = focusedId;
      if (!id || !visible.some((window) => window.id === id)) {
        id = visible.reduce((best, window) => (!best || window.zIndex > best.zIndex ? window : best)).id;
      }
      minimizeConsole(id);
    }

    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [enabled, focusedId, minimizeConsole, windows]);
}

export function ClientConsoleWindowLayer() {
  const pathname = usePathname() || DASHBOARD_CLIENTS_PATH;
  const onClientsPage = isClientsRoute(pathname);
  const {
    windows,
    dockVisible,
    focusedId,
    closeConsole,
    minimizeConsole,
    toggleMaximizeConsole,
    focusConsole,
    refreshConsole,
    updateConsoleRect,
  } = useClientConsole();

  const dockOffset = dockVisible ? CONSOLE_DOCK_RESERVED_HEIGHT + 12 : 0;
  const mountedWindows = windows.filter((window) => window.activated);

  useConsoleClickOutsideMinimize({
    enabled: onClientsPage,
    windows,
    focusedId,
    minimizeConsole,
  });

  return (
    <>
      {mountedWindows.map((window) => (
        <ClientConsoleWindowShell
          key={window.id}
          window={window}
          minimized={window.state === "minimized" || !onClientsPage}
          focused={onClientsPage && window.id === focusedId}
          dockOffset={dockOffset}
          onClose={() => closeConsole(window.id)}
          onMinimize={() => minimizeConsole(window.id)}
          onToggleMaximize={() => toggleMaximizeConsole(window.id)}
          onRefresh={() => refreshConsole(window.id)}
          onFocus={() => focusConsole(window.id)}
          onRectChange={(rect) => updateConsoleRect(window.id, rect)}
        />
      ))}
    </>
  );
}

function ClientConsoleWindowShell({
  window,
  minimized,
  focused,
  dockOffset,
  onClose,
  onMinimize,
  onToggleMaximize,
  onRefresh,
  onFocus,
  onRectChange,
}: {
  window: ConsoleWindow;
  minimized: boolean;
  focused: boolean;
  dockOffset: number;
  onClose: () => void;
  onMinimize: () => void;
  onToggleMaximize: () => void;
  onRefresh: () => void;
  onFocus: () => void;
  onRectChange: (rect: NormalRect) => void;
}) {
  const { t } = useLocaleText();
  const maximized = window.state === "maximized";
  const dragRef = React.useRef<{ startX: number; startY: number; origin: NormalRect } | null>(null);
  const resizeRef = React.useRef<{ startX: number; startY: number; origin: NormalRect } | null>(null);

  const onDragPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (minimized || maximized) return;
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

  const style: React.CSSProperties = minimized
    ? {
        position: "fixed",
        left: -window.normalRect.width - 100,
        top: 0,
        width: window.normalRect.width,
        height: window.normalRect.height,
        zIndex: -1,
        opacity: 0,
        pointerEvents: "none",
      }
    : maximized
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
      data-console-window
      className={`${shellClass} ${focused && !minimized ? "ring-2 ring-primary/20" : ""}`}
      style={style}
      onPointerDown={minimized ? undefined : onFocus}
      role="dialog"
      aria-label={`${t("dashboard.clientConsole")} ${window.title}`}
      aria-modal={maximized && !minimized ? "true" : "false"}
      aria-hidden={minimized}
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
          <div className="inline-flex max-w-full items-center justify-center gap-1">
            <p className="truncate text-[12px] font-medium text-slate-700">{window.title}</p>
            <a
              href={window.url}
              target="_blank"
              rel="noopener noreferrer"
              data-no-drag
              className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-md text-slate-500 transition-colors hover:bg-slate-200/80 hover:text-slate-800"
              title={t("dashboard.clientFrame.openNewTab")}
              aria-label={t("dashboard.clientFrame.openNewTab")}
              onClick={(event) => event.stopPropagation()}
            >
              <ExternalLink className="h-3 w-3" />
            </a>
          </div>
        </div>
        <button
          type="button"
          data-no-drag
          onClick={(event) => {
            event.stopPropagation();
            onRefresh();
          }}
          className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-slate-500 transition-colors hover:bg-slate-200/80 hover:text-slate-800"
          aria-label={t("dashboard.clientConsole.refresh")}
          title={t("dashboard.clientConsole.refresh")}
        >
          <RotateCw className="h-3.5 w-3.5" />
        </button>
      </div>

      <div className="relative min-h-0 flex-1 bg-slate-50 p-3">
        <div className="h-full overflow-hidden rounded-xl border border-slate-200/80 bg-white shadow-[inset_0_1px_0_rgba(255,255,255,0.7)]">
          <ClientConsoleIframe window={window} paused={minimized} />
        </div>
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
