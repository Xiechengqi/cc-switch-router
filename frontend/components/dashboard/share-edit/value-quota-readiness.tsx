// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
"use client";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { InfoHint } from "@/components/common/info-hint";
import { knownReadinessCode, valueReadinessItems, valueReadinessReasonKey, valueReadinessSummary, valueReadinessWarnings, type ValueReadinessCheck } from "@/lib/share-value-quota";
import type { useValueQuotaReadiness } from "@/lib/use-value-quota-readiness";

const LINK = "rounded px-1 py-0.5 font-medium text-primary underline-offset-2 transition-colors hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary";

const shortPhrase = (code: string, t: TFn) => {
  const known = knownReadinessCode(code);
  return known ? t(`dashboard.userLimit.value.readiness.short.${known}`) : t("dashboard.userLimit.value.readiness.short.unknown", {code});
};
const actionSentence = (check: ValueReadinessCheck, t: TFn) => {
  const reasonKey = valueReadinessReasonKey(check);
  if (reasonKey) return t(reasonKey);
  const known = knownReadinessCode(check.code);
  return known ? t(`dashboard.userLimit.value.readiness.${known}`) : t("dashboard.userLimit.value.readiness.unknown", {code: check.code});
};

const bindingLabel = (check: ValueReadinessCheck) => [check.app, check.providerId, check.model, check.profileId, check.driverId].filter(Boolean).join(" · ");

/** Passed checks whose model has no price: one line per model, each with why it counts as $0. */
function UnpricedModelList({checks, t}: {checks: ValueReadinessCheck[]; t: TFn}) {
  return <span className="grid gap-1 pl-3">{checks.map((check, index) => <span key={`${check.model ?? check.code}:${index}`} className="-indent-3">
    {"· "}
    {check.model ? <span className="break-all font-mono">{check.model} </span> : null}
    <span>— {t(valueReadinessReasonKey(check) ?? "dashboard.userLimit.value.unpriced.reason.model_unpriced")}</span>
    {bindingLabel(check) && bindingLabel(check) !== check.model ? <span className="block break-all font-mono text-muted-foreground">{bindingLabel(check)}</span> : null}
  </span>)}</span>;
}

/**
 * Silent when ready or still unknown. When a completed check says no, one
 * line names the root cause (or the request failure); the ⓘ lists every
 * failed check, causes first with what to do, consequences with what they
 * wait on.
 */
export function ValueQuotaReadiness({state, t, id}: {state:ReturnType<typeof useValueQuotaReadiness>;t:TFn;
  /** Lets the disabled `$` option point at this line. */
  id?: string}) {
  const checks = state.preview?.checks;
  const warnings = valueReadinessWarnings(checks);
  if (!state.unavailable) {
    // Ready, but some models are unpriced: a warning, never a blocker.
    if (!state.preview || !warnings.length) return null;
    return <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-amber-800" role="status">
      <span id={id} className="inline-flex min-w-0 items-center gap-1.5">
        <span aria-hidden="true" className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning" />
        <span className="min-w-0">{t("dashboard.userLimit.value.unpriced.readinessLine", {count: new Set(warnings.map(check => check.model ?? check.code)).size})}</span>
      </span>
      <InfoHint label={t("dashboard.userLimit.value.unpriced.readinessLabel")}>
        <span className="grid gap-1.5">
          <span>{t("dashboard.userLimit.value.unpriced.readinessIntro")}</span>
          <UnpricedModelList checks={warnings} t={t} />
          <span className="text-muted-foreground">{t("dashboard.userLimit.value.unpriced.reprice")}</span>
        </span>
      </InfoHint>
    </div>;
  }
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
            {check.code === "text_surface" ? <span className="break-all font-mono"> {check.app} · {check.providerId ?? "—"} · {check.profileId ?? "—"} / {check.driverId ?? "—"}</span> : null}
            {check.code === "model_price_coverage" && check.model ? <span className="break-all font-mono"> {check.app} · {check.providerId ?? "—"} · {check.model} · {check.profileId ?? "—"} / {check.driverId ?? "—"}</span> : null}
          </span>)}</span>
        </> : null}
        {warnings.length ? <>
          <span>{t("dashboard.userLimit.value.unpriced.readinessLabel")}</span>
          <UnpricedModelList checks={warnings} t={t} />
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
