"use client";
// Development-only eight-state catalogue; not imported by production routes.
import { C2CAction } from "./c2c-primitives";
export function C2CControlsPreview() {
  return <section className="grid max-w-md gap-4 bg-card p-4 text-foreground" aria-label="C2C action states">
    {(["default", "hover", "focus", "active", "disabled", "loading", "error", "success"] as const).map(state => <div key={state} className="flex flex-wrap items-center justify-between gap-3"><span>{state}</span><C2CAction type="button" disabled={state === "disabled"} phase={state === "loading" || state === "error" || state === "success" ? state : "default"} previewState={state === "hover" || state === "focus" || state === "active" ? state : undefined}>{state === "loading" ? "处理中…" : state === "success" ? "余额已到账" : state === "error" ? "重试" : "确认购入"}</C2CAction></div>)}
  </section>;
}
