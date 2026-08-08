"use client";

import * as React from "react";
import type { ConsoleWindow } from "@/components/dashboard/client-console/client-console-manager";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function ClientConsoleIframe({ window, paused = false }: { window: ConsoleWindow; paused?: boolean }) {
  const { t } = useLocaleText();

  return (
    <iframe
      key={`${window.id}-${window.reloadKey}`}
      src={window.url}
      title={`${t("dashboard.clientConsole")} ${window.title}`}
      className="h-full w-full rounded-xl border-0 bg-white"
      allow="clipboard-read; clipboard-write"
      sandbox="allow-downloads allow-forms allow-modals allow-popups allow-same-origin allow-scripts"
      referrerPolicy="same-origin"
      aria-hidden={paused}
      tabIndex={paused ? -1 : undefined}
      style={paused ? { contentVisibility: "hidden" } : undefined}
    />
  );
}
