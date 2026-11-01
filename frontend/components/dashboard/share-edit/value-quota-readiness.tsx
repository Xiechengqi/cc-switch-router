// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
"use client";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { InfoHint } from "@/components/common/info-hint";
import { knownReadinessCode, valueReadinessItems, valueReadinessSummary, type ValueReadinessCheck } from "@/lib/share-value-quota";
import type { useValueQuotaReadiness } from "@/lib/use-value-quota-readiness";

const LINK = "rounded px-1 py-0.5 font-medium text-primary underline-offset-2 transition-colors hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary";

const shortPhrase = (code: string, t: TFn) => {
  const known = knownReadinessCode(code);
  return known ? t(`dashboard.userLimit.value.readiness.short.${known}`) : t("dashboard.userLimit.value.readiness.short.unknown", {code});
};
const actionSentence = (check: ValueReadinessCheck, t: TFn) => {
  const known = knownReadinessCode(check.code);
  return known ? t(`dashboard.userLimit.value.readiness.${known}`) : t("dashboard.userLimit.value.readiness.unknown", {code: check.code});
};

/**
 * Silent when ready or still unknown. When a completed check says no, one
 * line names the root cause (or the request failure); the ⓘ lists every
 * failed check, causes first with what to do, consequences with what they
 * wait on.
 */
export function ValueQuotaReadiness({state, t, id}: {state:ReturnType<typeof useValueQuotaReadiness>;t:TFn;
  /** Lets the disabled `$` option point at this line. */
  id?: string}) {
  if (!state.unavailable) return null;
  const checks = state.preview?.checks;
  const items = valueReadinessItems(checks);
  const {root, more} = valueReadinessSummary(checks);
  const reason = state.error ? t(`dashboard.userLimit.value.readiness.short.${state.error}`) : root ? shortPhrase(root.code, t) : undefined;
  const pricing = state.preview?.pricing;
  return <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-muted-foreground" role="status" aria-busy={state.loading}>
    <span id={id} className="inline-flex min-w-0 items-center gap-1.5">
      <span aria-hidden="true" className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning" />
      <span className="min-w-0">{reason ? t("dashboard.userLimit.value.readiness.line", {reason}) : t("dashboard.userLimit.value.readiness.blocked")}</span>
      {!state.error && more > 0 ? <>
        <span aria-hidden="true" className="rounded-full bg-muted px-1.5 font-mono text-[10px] tabular-nums text-foreground/70">
          {t("dashboard.userLimit.value.readiness.more", {count: more})}
        </span>
        <span className="sr-only">{t("dashboard.userLimit.value.readiness.moreLabel", {count: more})}</span>
      </> : null}
    </span>
    <InfoHint label={t("dashboard.userLimit.value.readiness.blocked")}>
      <span className="grid gap-1.5">
        {state.error ? <span className="text-danger">{t(`dashboard.userLimit.value.readiness.${state.error}`)}</span> : null}
        {items.length ? <>
          <span>{t("dashboard.userLimit.value.unavailableHint")}</span>
          <span className="grid gap-1 pl-3">{items.map(({check, waitingOn}, index) => <span key={`${check.code}:${index}`} className="-indent-3">
            {"· "}
            {waitingOn ? <>
              {shortPhrase(check.code, t)}
              <span className="text-muted-foreground"> — {t("dashboard.userLimit.value.readiness.waitingOn", {root: shortPhrase(waitingOn, t)})}</span>
            </> : actionSentence(check, t)}
            {check.code === "sqlite_authority" && check.state ? <span className="font-mono"> ({check.state})</span> : null}
            {check.code === "text_surface" ? <span className="break-all font-mono"> {check.app} · {check.profileId ?? "—"} / {check.driverId ?? "—"}</span> : null}
          </span>)}</span>
        </> : null}
        {pricing?.publishedRevision && !state.preview?.activePricingRevision
          ? <span>{t(`dashboard.userLimit.value.readiness.${pricing.acknowledgedAtMs == null ? "pendingAck" : "pendingEffective"}`)}</span> : null}
      </span>
    </InfoHint>
    <span aria-hidden="true">·</span>
    <button type="button" className={`${LINK} disabled:opacity-50`} disabled={state.loading} onClick={state.retry}>
      {t("dashboard.userLimit.value.readiness.retry")}
    </button>
  </div>;
}
