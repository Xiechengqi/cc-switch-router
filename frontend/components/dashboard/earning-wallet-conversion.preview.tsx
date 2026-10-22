"use client";
// Development-only state catalogue; never mounted in the production route tree.
import { ConversionAction } from "./earning-wallet-conversion";
export function EarningConversionPreview() {
  return <section className="grid max-w-md gap-4 bg-card p-5 text-foreground" aria-label="Conversion action states">
    {(["default", "hover", "focus", "active", "disabled", "loading", "error", "success"] as const).map((state) => <div key={state} className="flex flex-wrap items-center justify-between gap-3">
      <span>{state}</span><ConversionAction type="button" disabled={state === "disabled"}
        phase={state === "loading" || state === "error" || state === "success" ? state : "default"}
        previewState={state === "hover" || state === "focus" || state === "active" ? state : undefined}>
        {state === "loading" ? "处理中…" : state === "success" ? "已转入" : state === "error" ? "重试转入" : "转入余额"}
      </ConversionAction>
    </div>)}
  </section>;
}
