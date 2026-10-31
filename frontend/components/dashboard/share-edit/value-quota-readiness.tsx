// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
"use client";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { InfoHint } from "@/components/common/info-hint";
import type { useValueQuotaReadiness } from "@/lib/use-value-quota-readiness";

const KNOWN_CHECKS = new Set(["router_protocol", "sqlite_authority", "metering_storage", "acknowledged_tariff", "tariff_published", "text_surface"]);

/**
 * Silent when ready or still unknown. When a completed check says no, one
 * line names the state; the checklist sits behind the ⓘ.
 */
export function ValueQuotaReadiness({state, t, id}: {state:ReturnType<typeof useValueQuotaReadiness>;t:TFn;
  /** Lets the disabled `$` option point at this line. */
  id?: string}) {
  if (!state.unavailable) return null;
  const failed = state.preview?.checks?.filter(check => !check.ready) ?? [];
  const pricing = state.preview?.pricing;
  return <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground" role="status" aria-busy={state.loading}>
    <span id={id} className="inline-flex items-center gap-1.5">
      <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-warning" />
      {t("dashboard.userLimit.value.unavailable")}
    </span>
    <InfoHint label={t("dashboard.userLimit.value.readiness.blocked")}>
      <span className="grid gap-1.5">
        {state.error ? <span className="text-danger">{t(`dashboard.userLimit.value.readiness.${state.error}`)}</span> : null}
        {failed.length ? <>
          <span>{t("dashboard.userLimit.value.unavailableHint")}</span>
          <span className="grid gap-1 pl-3">{failed.map((check, index) => <span key={`${check.code}:${index}`} className="-indent-3">
            {"· "}{t(`dashboard.userLimit.value.readiness.${KNOWN_CHECKS.has(check.code) ? check.code as "router_protocol" : "failed"}`)}
            {check.code === "sqlite_authority" && check.state ? ` (${check.state})` : null}
            {check.code === "text_surface" ? <span className="break-all font-mono"> {check.app} · {check.profileId ?? "—"} / {check.driverId ?? "—"}</span> : null}
          </span>)}</span>
        </> : null}
        {pricing?.publishedRevision && !state.preview?.activePricingRevision
          ? <span>{t(`dashboard.userLimit.value.readiness.${pricing.acknowledgedAtMs == null ? "pendingAck" : "pendingEffective"}`)}</span> : null}
      </span>
    </InfoHint>
    <span aria-hidden="true">·</span>
    <button type="button" className="rounded px-1 py-0.5 font-medium text-primary underline-offset-2 transition-colors hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary disabled:opacity-50"
      disabled={state.loading} onClick={state.retry}>
      {t("dashboard.userLimit.value.readiness.retry")}
    </button>
  </div>;
}
