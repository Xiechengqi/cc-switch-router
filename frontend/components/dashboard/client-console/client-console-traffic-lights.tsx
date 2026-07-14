"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";

type TrafficLightsProps = {
  maximized: boolean;
  onClose: () => void;
  onMinimize: () => void;
  onToggleMaximize: () => void;
};

export function ClientConsoleTrafficLights({ maximized, onClose, onMinimize, onToggleMaximize }: TrafficLightsProps) {
  const { t } = useLocaleText();

  return (
    <div className="flex items-center gap-1.5" data-no-drag>
      <button
        type="button"
        onClick={onClose}
        aria-label={t("dashboard.clientConsole.close")}
        className="group relative flex h-[13px] w-[13px] items-center justify-center rounded-full bg-[#ff5f57] transition-opacity hover:opacity-90"
      >
        <span className="pointer-events-none text-[9px] font-bold leading-none text-[#4d0000]/0 transition-colors group-hover:text-[#4d0000]/80">
          ×
        </span>
      </button>
      <button
        type="button"
        onClick={onMinimize}
        aria-label={t("dashboard.clientConsole.minimize")}
        className="group relative flex h-[13px] w-[13px] items-center justify-center rounded-full bg-[#febc2e] transition-opacity hover:opacity-90"
      >
        <span className="pointer-events-none text-[10px] font-bold leading-none text-[#5a4200]/0 transition-colors group-hover:text-[#5a4200]/80">
          −
        </span>
      </button>
      <button
        type="button"
        onClick={onToggleMaximize}
        aria-label={maximized ? t("dashboard.clientConsole.restore") : t("dashboard.clientConsole.maximize")}
        className="group relative flex h-[13px] w-[13px] items-center justify-center rounded-full bg-[#28c840] transition-opacity hover:opacity-90"
      >
        <span className="pointer-events-none text-[9px] font-bold leading-none text-[#004d00]/0 transition-colors group-hover:text-[#004d00]/80">
          {maximized ? "↙" : "+"}
        </span>
      </button>
    </div>
  );
}
