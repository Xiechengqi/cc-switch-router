"use client";

import { X } from "lucide-react";
import * as React from "react";
import { CONSOLE_DOCK_HEIGHT, useClientConsole } from "@/components/dashboard/client-console/client-console-manager";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function ClientConsoleDock() {
  const { windows, dockVisible, restoreConsole, closeConsole, focusConsole } = useClientConsole();
  const { t } = useLocaleText();

  const docked = windows.filter((window) => window.state === "minimized" || !window.activated);
  if (!dockVisible || docked.length === 0) return null;

  return (
    <div
      className="fixed bottom-4 left-1/2 z-50 -translate-x-1/2"
      style={{ maxWidth: "min(calc(100vw - 2rem), 720px)" }}
      role="toolbar"
      aria-label={t("dashboard.clientConsole.dockLabel")}
    >
      <div
        className="flex items-center gap-2 overflow-x-auto rounded-2xl border border-slate-200/80 bg-white/95 px-3 py-2 shadow-[0_12px_40px_rgba(15,23,42,0.14)] backdrop-blur-md"
        style={{ minHeight: CONSOLE_DOCK_HEIGHT - 16 }}
      >
        {docked.map((window) => {
          const suspended = !window.activated;
          return (
            <div key={window.id} className="group relative flex shrink-0 items-center">
              <button
                type="button"
                onClick={() => {
                  restoreConsole(window.id);
                  focusConsole(window.id);
                }}
                className="inline-flex h-9 max-w-[200px] items-center gap-2 rounded-xl border border-slate-200/80 bg-slate-50 px-3 text-left text-[11px] font-medium text-slate-700 transition-colors hover:border-sky-200 hover:bg-sky-50 hover:text-sky-800"
                title={suspended ? t("dashboard.clientConsole.resumeHint") : window.url}
                aria-label={
                  suspended
                    ? t("dashboard.clientConsole.resumeNamed", { name: window.title })
                    : t("dashboard.clientConsole.restoreNamed", { name: window.title })
                }
              >
                <span
                  className={`h-2 w-2 shrink-0 rounded-full ${suspended ? "bg-amber-400" : "bg-emerald-500"}`}
                  aria-hidden
                />
                <span className="min-w-0 truncate font-mono">{window.title}</span>
                {suspended ? (
                  <span className="shrink-0 rounded bg-amber-100 px-1.5 py-0.5 text-[9px] font-semibold text-amber-800">
                    {t("dashboard.clientConsole.suspended")}
                  </span>
                ) : null}
              </button>
              <button
                type="button"
                onClick={(event) => {
                  event.stopPropagation();
                  closeConsole(window.id);
                }}
                className="absolute -right-1 -top-1 inline-flex h-4 w-4 items-center justify-center rounded-full border border-slate-200 bg-white text-slate-500 opacity-0 shadow-sm transition-opacity hover:text-rose-600 group-hover:opacity-100"
                aria-label={t("dashboard.clientConsole.closeNamed", { name: window.title })}
              >
                <X className="h-2.5 w-2.5" />
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
