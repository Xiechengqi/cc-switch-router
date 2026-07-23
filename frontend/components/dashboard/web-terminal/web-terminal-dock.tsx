"use client";

import { Trash2, X } from "lucide-react";
import * as React from "react";
import {
  CONSOLE_DOCK_BOTTOM_INSET,
  CONSOLE_DOCK_HEIGHT,
  useClientConsole,
} from "@/components/dashboard/client-console/client-console-manager";
import { useWebTerminal } from "@/components/dashboard/web-terminal/web-terminal-manager";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";

export function WebTerminalDock() {
  const { windows, focusedId, dockVisible, restoreTerminal, closeTerminal, closeAllTerminals } =
    useWebTerminal();
  const { dockVisible: consoleDockVisible } = useClientConsole();
  const { t } = useLocaleText();

  if (!dockVisible || windows.length === 0) return null;

  // Sit above the client-console dock when both are on /clients.
  const bottom = consoleDockVisible
    ? CONSOLE_DOCK_BOTTOM_INSET + CONSOLE_DOCK_HEIGHT + 8
    : CONSOLE_DOCK_BOTTOM_INSET;

  return (
    <div
      data-web-terminal-dock
      className="fixed left-1/2 z-50 -translate-x-1/2"
      style={{ bottom, maxWidth: "min(calc(100vw - 2rem), 720px)" }}
      role="toolbar"
      aria-label={t("clientMarket.terminal.dockLabel")}
    >
      <div
        className="flex items-center gap-2 overflow-x-auto rounded-2xl border border-white/45 bg-white/22 px-3 py-2 shadow-[0_10px_40px_rgba(15,23,42,0.12),inset_0_1px_0_rgba(255,255,255,0.55)] backdrop-blur-2xl backdrop-saturate-150"
        style={{ minHeight: CONSOLE_DOCK_HEIGHT - 16 }}
      >
        {windows.map((window) => {
          const suspended = !window.activated;
          const isActive =
            window.id === focusedId && window.activated && window.state !== "minimized";
          return (
            <div key={window.id} className="group relative flex shrink-0 items-center">
              <button
                type="button"
                onClick={() => restoreTerminal(window.id)}
                className={cn(
                  "inline-flex h-9 max-w-[200px] items-center gap-2 rounded-xl border px-3 text-left text-[11px] font-medium transition-colors",
                  isActive
                    ? "border-sky-300/70 bg-white/45 text-sky-900 ring-2 ring-sky-200/70 backdrop-blur-md"
                    : "border-white/45 bg-white/28 text-slate-700 backdrop-blur-md hover:border-sky-200/70 hover:bg-white/40 hover:text-sky-800",
                )}
                title={suspended ? t("clientMarket.terminal.resumeHint") : window.title}
                aria-current={isActive ? "true" : undefined}
                aria-label={
                  suspended
                    ? t("clientMarket.terminal.resumeNamed", { name: window.title })
                    : isActive
                      ? t("clientMarket.terminal.activeNamed", { name: window.title })
                      : t("clientMarket.terminal.switchNamed", { name: window.title })
                }
              >
                <span
                  className={cn(
                    "h-2 w-2 shrink-0 rounded-full",
                    suspended ? "bg-amber-400" : isActive ? "bg-sky-500" : "bg-emerald-500",
                  )}
                  aria-hidden
                />
                <span className="min-w-0 truncate font-mono">{window.title}</span>
                {suspended ? (
                  <span className="shrink-0 rounded bg-amber-100 px-1.5 py-0.5 text-[9px] font-semibold text-amber-800">
                    {t("clientMarket.terminal.suspended")}
                  </span>
                ) : null}
              </button>
              <button
                type="button"
                onClick={(event) => {
                  event.stopPropagation();
                  closeTerminal(window.id);
                }}
                className="absolute -right-1 -top-1 inline-flex h-4 w-4 items-center justify-center rounded-full border border-slate-200 bg-white text-slate-500 opacity-0 shadow-sm transition-opacity hover:text-rose-600 group-hover:opacity-100"
                aria-label={t("clientMarket.terminal.closeNamed", { name: window.title })}
              >
                <X className="h-2.5 w-2.5" />
              </button>
            </div>
          );
        })}
        <div className="ml-1 flex shrink-0 items-center border-l border-white/35 pl-2">
          <button
            type="button"
            onClick={() => closeAllTerminals()}
            className="inline-flex h-9 w-9 items-center justify-center rounded-xl border border-white/45 bg-white/28 text-slate-600 backdrop-blur-md transition-colors hover:border-rose-200/70 hover:bg-rose-50/35 hover:text-rose-700"
            aria-label={t("clientMarket.terminal.cleanAll")}
            title={t("clientMarket.terminal.cleanAll")}
          >
            <Trash2 className="h-4 w-4 shrink-0" aria-hidden />
          </button>
        </div>
      </div>
    </div>
  );
}
