import { cn } from "@/lib/utils";
import type { ShareMarketProviderHealthState } from "@/lib/types";

export type ShareProviderStatusPanelView = {
  primaryLine: string;
  identityLine: string;
  modelsLine: string;
  primaryTitle?: string;
  identityTitle?: string;
  modelsTitle?: string;
  panelTitle?: string;
  primaryMonospace?: boolean;
  toneClassName: string;
};

export function marketProviderHealthTone(
  state: ShareMarketProviderHealthState,
) {
  if (state === "unavailable") {
    return "border-red-200 bg-red-50 text-red-700";
  }
  if (state === "degraded") {
    return "border-amber-200 bg-amber-50 text-amber-700";
  }
  if (state === "healthy") {
    return "border-emerald-200 bg-emerald-50 text-emerald-700";
  }
  return "border-slate-200 bg-slate-50 text-slate-600";
}

export function ShareProviderStatusPanel({
  view,
  className,
  wrapPrimaryLine = false,
}: {
  view: ShareProviderStatusPanelView;
  className?: string;
  wrapPrimaryLine?: boolean;
}) {
  return (
    <div
      className={cn(
        "grid min-h-[4.125rem] min-w-0 content-center gap-1 rounded-md border px-2 py-1.5 text-[11px]",
        view.toneClassName,
        className,
      )}
      title={view.panelTitle}
    >
      <span
        className={cn(
          "min-w-0 font-semibold leading-4",
          wrapPrimaryLine ? "whitespace-normal break-words" : "truncate",
          view.primaryMonospace && "font-mono text-[10px]",
        )}
        title={view.primaryTitle || view.primaryLine}
      >
        {view.primaryLine}
      </span>
      <span
        className="min-w-0 truncate opacity-80"
        title={view.identityTitle || view.identityLine}
      >
        {view.identityLine}
      </span>
      <span
        className="min-w-0 truncate opacity-80"
        title={view.modelsTitle || view.modelsLine}
      >
        {view.modelsLine}
      </span>
    </div>
  );
}
