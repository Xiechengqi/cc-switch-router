// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
"use client";
import { Button } from "@heroui/react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import type { useValueQuotaReadiness } from "@/lib/use-value-quota-readiness";

export function ValueQuotaReadiness({state, t}: {state:ReturnType<typeof useValueQuotaReadiness>;t:TFn}) {
  const known = {router_protocol:"router_protocol",sqlite_authority:"sqlite_authority",metering_storage:"metering_storage",
    acknowledged_tariff:"acknowledged_tariff",tariff_published:"tariff_published",text_surface:"text_surface"} as const;
  const failed = state.preview?.checks?.filter(check => !check.ready) ?? [];
  const pricing = state.preview?.pricing;
  return <div className="grid min-w-0 gap-2 rounded-md border bg-background p-3 text-xs" aria-busy={state.loading}>
    <div className="flex flex-wrap items-center justify-between gap-2">
      <span role="status">{t(`dashboard.userLimit.value.readiness.${state.loading ? "loading" : state.ready ? "ready" : "blocked"}`)}</span>
      <Button size="sm" variant="secondary" isDisabled={state.loading} onClick={state.retry}>{t("dashboard.userLimit.value.readiness.retry")}</Button>
    </div>
    {state.error ? <p role="alert" className="text-danger">{t(`dashboard.userLimit.value.readiness.${state.error}`)}</p> : null}
    {failed.length ? <ul className="grid gap-1 pl-4 list-disc">{failed.map((check,index) => <li key={`${check.code}:${index}`}>
      {t(`dashboard.userLimit.value.readiness.${known[check.code as keyof typeof known] ?? "failed"}`)}
      {check.code === "sqlite_authority" && check.state ? ` (${check.state})` : null}
      {check.code === "text_surface" ? <span className="break-all font-mono"> {check.app} · {check.profileId ?? "—"} / {check.driverId ?? "—"}</span> : null}
    </li>)}</ul> : null}
    {!state.ready && !state.loading && pricing?.publishedRevision && !state.preview?.activePricingRevision ? <p>{t(`dashboard.userLimit.value.readiness.${pricing.acknowledgedAtMs == null ? "pendingAck" : "pendingEffective"}`)}</p> : null}
  </div>;
}
