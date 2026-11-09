import * as React from "react";
import { cn } from "@/lib/utils";

/** Design-system section label (mono, uppercase, wide tracking) in an accent pill;
 * the dot pulses while the section is live. */
export function SectionHeading({ id, label, hint, count, live, action, as: Heading = "h3" }: {
  id: string; label: string; hint?: string; count?: number; live: boolean; action?: React.ReactNode;
  as?: "h2" | "h3";
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-2">
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
        <Heading id={id} className="inline-flex items-center gap-2 rounded-full border border-primary/30 bg-primary/5 px-3 py-1 font-mono text-xs uppercase tracking-[0.15em] text-primary">
          <span className={cn("model-route-dot", live ? "bg-primary model-route-dot-live" : "bg-primary/40")} aria-hidden />
          {label}
          {count ? <span className="font-mono text-[11px] tabular-nums text-primary/70">{count}</span> : null}
        </Heading>
        {hint ? <p className="min-w-0 text-xs text-muted-foreground">{hint}</p> : null}
      </div>
      {action}
    </div>
  );
}
