"use client";

import type { ModelRouteRecentResult, UserModelRoutingResponse } from "@/lib/types";
import type { AppLocale, MessageKey } from "@/lib/i18n";

export function currentModelRouteResult(profile: UserModelRoutingResponse | null, routeId: string | undefined, _now?: number): ModelRouteRecentResult | undefined {
  return profile?.routeRecentResults?.find(result => result.routeId === routeId
    && result.revision === profile.revision && profile.routes.some(route => route.id === routeId)
    && Number.isFinite(Date.parse(result.observedAt)));
}

export function ModelHubRecentResult({ profile, routeId, locale, t }: {
  profile: UserModelRoutingResponse | null; routeId?: string; locale: AppLocale;
  t: (key: MessageKey, params?: Record<string, string | number>) => string;
}) {
  const result = currentModelRouteResult(profile, routeId);
  return <div className="grid gap-1 text-xs text-muted-foreground">
    <p>{t("modelHub.recent.title")}: {result ? `HTTP ${result.statusCode ?? "—"}` : t("modelHub.recent.unknown")}</p>
    {result ? <>
      <p>{t(`modelHub.recent.${result.source}`)} · {t(`modelHub.recent.${result.phase}`)} · <time dateTime={result.observedAt}>{new Date(result.observedAt).toLocaleString(locale)}</time></p>
      {result.targetShareId ? <p className="break-all">{profile?.eligibleShares.find(share => share.shareId === result.targetShareId)?.subdomain || result.targetShareId}</p> : null}
      {result.continuedSearch ? <p>{t("modelHub.recent.continued")}</p> : null}
      {result.usedBackup ? <p>{t("modelHub.recent.backup")}</p> : null}
    </> : null}
    <p>{t("modelHub.recent.scope")}</p>
  </div>;
}
