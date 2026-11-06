"use client";

import * as React from "react";
import { buildShareDependencies, type DraftModelRoute, type ModelRoutingProtocol, type ShareDependencySummary } from "@/lib/model-routing";
import type { UserModelRoutingShare } from "@/lib/types";
import type { AppLocale, MessageKey } from "@/lib/i18n";

const PROTOCOL_KEYS: Record<ModelRoutingProtocol, MessageKey> = {
  claude: "modelHub.protocol.claude", codex: "modelHub.protocol.codex", gemini: "modelHub.protocol.gemini",
};
type DependencyContext = { summaries: Map<string, ShareDependencySummary>; shares: UserModelRoutingShare[];
  view: "saved" | "draft"; onLocate: (clientId: string) => void };
export const RoutingDependencyContext = React.createContext<DependencyContext | null>(null);
/** `expanded` renders the list directly for a parent that already gates it
 * behind its own disclosure; otherwise the list mounts only when opened. */
export function RoutingShareDependencies({ shareId, expanded = false, t }: {
  shareId: string; expanded?: boolean; t: (key: MessageKey, params?: Record<string, string | number>) => string;
}) {
  const [opened, setOpened] = React.useState(false);
  const context = React.useContext(RoutingDependencyContext);
  const summary = context?.summaries.get(shareId);
  if (!summary || !context) return null;
  const list = expanded || opened ? <ul className="grid max-h-80 min-w-0 gap-1 overflow-y-auto">{summary.dependencies.map(dependency => {
      const next = context.shares.find(share => share.shareId === dependency.nextEligibleOnlineCandidateShareId);
      return <li key={`${dependency.routeClientId}:${dependency.position}`} className="grid min-w-0 gap-1">
        <button type="button" onClick={() => context.onLocate(dependency.routeClientId)}
          className="min-h-11 truncate rounded px-2 text-left text-primary underline underline-offset-4 hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring">
          {PROTOCOL_KEYS[dependency.appType as ModelRoutingProtocol] ? t(PROTOCOL_KEYS[dependency.appType as ModelRoutingProtocol]) : dependency.appType} · {dependency.requestedModel || "—"} · {dependency.position === 0 ? t("modelHub.candidatePrimary") : t("modelHub.candidateOrder", { order: dependency.position + 1 })}
        </button>
        <span className="break-words px-2 [overflow-wrap:anywhere]">{t("modelHub.dependencies.next")}: {next?.subdomain || next?.shareName || t("modelHub.dependencies.unknown")}</span>
      </li>;
    })}</ul> : null;
  return <div className="grid min-w-0 gap-1 text-xs text-muted-foreground">
    {expanded ? null : <p>{t("modelHub.dependencies.count", { count: summary.dependencies.length })}{context.view === "draft" ? ` · ${t("modelHub.dependencies.draft")}` : ""}</p>}
    {summary.noNextEligibleOnlineCandidateCount ? <p className="text-amber-800">{t("modelHub.dependencies.risk", { count: summary.noNextEligibleOnlineCandidateCount })}</p> : null}
    {expanded ? list : <details onToggle={event => setOpened(event.currentTarget.open)}><summary className="min-h-11 cursor-pointer py-3">{t("modelHub.dependencies.title")}</summary>{list}</details>}
  </div>;
}
export function ModelHubDependencyProvider({ saved, draft, shares, dirty, onLocate, locale, t, children }: {
  saved: DraftModelRoute[]; draft: DraftModelRoute[]; shares: UserModelRoutingShare[]; dirty: boolean;
  onLocate: (clientId: string) => void; locale: AppLocale;
  t: (key: MessageKey, params?: Record<string, string | number>) => string; children: React.ReactNode;
}) {
  const [open, setOpen] = React.useState(false);
  const [selectedView, setView] = React.useState<"saved" | "draft">("saved");
  const view = dirty ? selectedView : "saved";
  const summaries = React.useMemo(() => buildShareDependencies(view, view === "saved" ? saved : draft, shares), [view, saved, draft, shares]);
  void locale;
  return <RoutingDependencyContext.Provider value={{ summaries, shares, view, onLocate }}>
    <details onToggle={event => { if (event.target === event.currentTarget) setOpen(event.currentTarget.open); }} className="min-w-0 border-t border-border py-2 text-xs text-muted-foreground">
      <summary className="min-h-11 cursor-pointer py-3 font-medium text-foreground focus-visible:outline-2 focus-visible:outline-ring">{t("modelHub.dependencies.title")}</summary>
      <p className="pb-2 leading-relaxed">{t("modelHub.dependencies.hint")}</p>
      <div className="flex flex-wrap gap-2" role="group" aria-label={t("modelHub.dependencies.view")}>
        {(["saved", "draft"] as const).map(value => <button type="button" key={value} disabled={value === "draft" && !dirty}
          aria-pressed={view === value} onClick={() => setView(value)}
          className="min-h-11 whitespace-nowrap rounded border border-input px-3 hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50">
          {t(`modelHub.dependencies.${value}`)}</button>)}
      </div>
      <div className="grid gap-3 pt-3">{open ? [...summaries.values()].map(summary => <details key={summary.shareId}>
        <summary className="min-h-11 cursor-pointer truncate py-3 font-mono focus-visible:outline-2 focus-visible:outline-ring">{shares.find(share => share.shareId === summary.shareId)?.subdomain || summary.shareId} · {t("modelHub.dependencies.count", { count: summary.dependencies.length })}</summary>
        <RoutingShareDependencies shareId={summary.shareId} expanded t={t} />
      </details>) : null}</div>
    </details>
    {children}
  </RoutingDependencyContext.Provider>;
}
