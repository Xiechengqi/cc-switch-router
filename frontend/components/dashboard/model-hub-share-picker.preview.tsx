"use client";

import * as React from "react";
import { interpolate, messages, type MessageKey } from "@/lib/i18n";
import type { UserModelRoutingShare } from "@/lib/types";
import { SharePoolPicker } from "./model-hub-share-picker";

/** Hallmark · component: routing Transfer · genre: modern-minimal
 * Existing Router tokens, no new page theme. Development fixtures only;
 * not imported by a production route and never requests accounts or API keys.
 * Eight-state preview is supplied for optional human inspection, not a claim
 * of visual verification (browser automation and screenshots are excluded).
 */
const states = ["default", "hover", "focus", "active", "disabled", "loading", "error", "success"] as const;

const shares: UserModelRoutingShare[] = ["one", "two"].map(id => ({
  shareId: id,
  shareName: `Preview ${id}`,
  subdomain: `preview-${id}`,
  directApiUrl: "https://preview.example.test",
  access: "owner",
  freeAccess: false,
  apps: ["codex"],
  isOnline: true,
  appCapabilities: [{ app: "codex", kind: "official_oauth", accountEmail: "fixture-only@example.test" }],
}));
const t = (key: MessageKey, params?: Record<string, string | number>) => interpolate(messages.en[key], params);

function StatePreview({ state }: { state: (typeof states)[number] }) {
  const [selected, setSelected] = React.useState(["one"]);
  const busy = state === "loading" || state === "disabled";
  return (
    <section className={`route-state-preview is-${state} grid min-w-0 gap-3 border-t border-border py-4`} aria-busy={state === "loading"}>
      <h3 className="text-sm font-semibold">{state}</h3>
      <div className="min-h-5 text-xs text-muted-foreground" role="status">
        {state === "loading" ? "Loading fixture…" : state === "error" ? "Share status could not be refreshed. Draft retained."
          : state === "success" ? "Fixture saved." : ""}
      </div>
      <SharePoolPicker app="codex" routeKey={`preview-${state}`} shares={shares}
        selected={state === "error" ? ["missing-fixture"] : selected}
        disabled={busy} onChange={setSelected} locale="en" t={t} />
    </section>
  );
}

export function ModelHubSharePickerPreview() {
  return (
    <div className="grid min-w-0 gap-6 bg-background p-4 text-foreground">
      <h2 className="text-base font-semibold">Routing Transfer — development fixtures</h2>
      <style>{`
        .route-state-preview.is-hover button:not(:disabled),
        .route-state-preview.is-active button:not(:disabled) { background: var(--color-provider-paper); }
        .route-state-preview.is-focus button:not(:disabled) { outline: 2px solid var(--color-provider-focus); outline-offset: 1px; }
      `}</style>
      {states.map(state => <StatePreview key={state} state={state} />)}
    </div>
  );
}
