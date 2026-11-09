"use client";

import type { ModelRouteRecentResult, UserModelRoutingResponse } from "@/lib/types";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";

export function currentModelRouteResult(profile: UserModelRoutingResponse | null, routeId: string | undefined, _now?: number): ModelRouteRecentResult | undefined {
  return profile?.routeRecentResults?.find(result => result.routeId === routeId
    && result.revision === profile.revision && profile.routes.some(route => route.id === routeId)
    && Number.isFinite(Date.parse(result.observedAt)));
}

/** One line per rule. The caveats live in tooltips; the panel states the scope once. */
export function ModelHubRecentResult({ profile, routeId, locale, t }: {
  profile: UserModelRoutingResponse | null; routeId?: string; locale: AppLocale;
  t: (key: MessageKey, params?: Record<string, string | number>) => string;
}) {
  const result = currentModelRouteResult(profile, routeId);
  if (!result) {
    return <p className="text-xs text-muted-foreground">{t("modelHub.recent.title")} · {t("modelHub.recent.unknown")}</p>;
  }
  const status = result.statusCode;
  const ok = !!status && status >= 200 && status < 300 && result.phase !== "body_incomplete";
  const chip = "inline-flex h-5 items-center rounded-full px-1.5";
  const target = result.targetShareId
    ? profile?.eligibleShares.find(share => share.shareId === result.targetShareId)?.subdomain || result.targetShareId
    : "";
  return (
    <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
      <span>{t("modelHub.recent.title")}</span>
      <span className={cn(chip, "font-mono font-semibold tabular-nums", ok ? "bg-emerald-50 text-emerald-700" : "bg-red-50 text-red-700")}>
        HTTP {status ?? "—"}
      </span>
      <span>{t(`modelHub.recent.${result.source}`)} · {t(`modelHub.recent.${result.phase}`)}</span>
      {target ? <span className="min-w-0 max-w-[12rem] truncate font-mono text-foreground" title={target}>{target}</span> : null}
      {result.usedBackup ? <span className={cn(chip, "bg-amber-50 text-amber-800")} title={t("modelHub.recent.backup")}>{t("modelHub.recent.backupShort")}</span> : null}
      {result.continuedSearch ? <span className={cn(chip, "bg-secondary")} title={t("modelHub.recent.continued")}>{t("modelHub.recent.continuedShort")}</span> : null}
      <time dateTime={result.observedAt} className="tabular-nums">{new Date(result.observedAt).toLocaleString(locale)}</time>
    </p>
  );
}
