"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";

const POLL_MS = 60_000;

function loadedUiBuildId() {
  return document.querySelector('meta[name="ccr-ui-build"]')?.getAttribute("content")?.trim() || "";
}

/**
 * Asks before a reload when a newer Router binary is serving the UI.
 * The check is a public health read. A failed read leaves the page alone.
 * The button is the only reload; nothing reloads while the process is down.
 */
export function UiUpdateNotice() {
  const { t } = useLocaleText();
  const [stale, setStale] = React.useState(false);

  React.useEffect(() => {
    const loaded = loadedUiBuildId();
    if (!loaded) return;
    let stopped = false;
    const look = async () => {
      if (document.visibilityState === "hidden") return;
      try {
        const response = await fetch("/v1/healthz", { cache: "no-store" });
        if (response.status !== 200 && response.status !== 503) return;
        const body = await response.json() as { uiBuildId?: unknown };
        if (stopped || typeof body.uiBuildId !== "string" || !body.uiBuildId) return;
        if (body.uiBuildId !== loaded) setStale(true);
      } catch {
        // The process may be restarting. Leave the current page where it is.
      }
    };
    void look();
    const timer = window.setInterval(() => void look(), POLL_MS);
    document.addEventListener("visibilitychange", look);
    return () => {
      stopped = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", look);
    };
  }, []);

  if (!stale) return null;
  return (
    <div className="fixed inset-x-0 bottom-0 z-[80] px-3 pb-3" role="status">
      <div className="mx-auto flex max-w-3xl flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-card px-4 py-3 text-sm shadow-lg">
        <p>{t("common.uiUpdated")}</p>
        <button
          type="button"
          className="min-h-11 rounded-md bg-slate-900 px-3 text-sm font-medium text-white hover:bg-slate-800"
          onClick={() => window.location.reload()}
        >
          {t("common.uiUpdatedAction")}
        </button>
      </div>
    </div>
  );
}
