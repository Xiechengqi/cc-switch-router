// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
"use client";
import { ProgressBar } from "@heroui/react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { InfoHint } from "@/components/common/info-hint";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  VALUE_STATUS_STALE_MS, formatQuotaPicos, formatQuotaTime, formatUsdLimit, renderValueQuotaMessage, valueMeteringState,
  valueQuotaAvailability, valueStatusReason, valueStatusTextKey, type ValueQuotaView,
} from "@/lib/share-value-quota";

/** A thin bar like the token rows', fed by exact BigInt basis points. */
function ValueBar({basisPoints, t}: {basisPoints: number; t: TFn}) {
  const pct = Math.min(100, Math.max(0, basisPoints / 100));
  const fill = pct >= 100 ? "rounded bg-danger" : pct >= 80 ? "rounded bg-warning" : "rounded bg-primary";
  return <ProgressBar aria-label={t("progress.usage")} value={pct} minValue={0} maxValue={100} size="sm" className="mt-1 w-full gap-0">
    <ProgressBar.Track className="h-1 rounded bg-muted"><ProgressBar.Fill className={fill} /></ProgressBar.Track>
  </ProgressBar>;
}

/** "→ US$10 from Nov 1" under the still-enforced policy, with the rule behind ⓘ. */
export function ScheduledValueQuotaLine({scheduled, t}: {scheduled: NonNullable<ValueQuotaView["scheduled"]>; t: TFn}) {
  const {locale} = useLocaleText();
  return <div className="mt-0.5 flex min-w-0 items-center gap-1 text-[10px] text-muted-foreground">
    <span className="min-w-0 break-words font-mono">
      {t("dashboard.userLimit.value.scheduledLine", {amount: formatUsdLimit(scheduled.limitMicros), date: formatQuotaTime(scheduled.effectiveFromMs, locale, "short")})}
    </span>
    <InfoHint label={t("dashboard.userLimit.value.details")}>{t("dashboard.userLimit.value.scheduledHint")}</InfoHint>
  </div>;
}

/**
 * The enforced USD row: one line of used / limit, a bar, and only the state
 * that needs action. Metering state, sync time and the soft-cap rule sit
 * behind the ⓘ.
 */
export function EnforcedValueQuotaCell({enforced, periodLabel, nowMs, stale, onReconcile, t}: {
  enforced: NonNullable<ValueQuotaView["enforced"]>; periodLabel: string; nowMs: number; stale?: boolean;
  onReconcile?: () => void; t: TFn;
}) {
  const {locale} = useLocaleText();
  const status = enforced.status;
  const reason = valueStatusReason(status);
  const availability = valueQuotaAvailability(status?.usedPicos, enforced.limitMicros);
  const blocked = !!status && valueMeteringState(status) === "blocked";
  const delayed = !!status && (stale || nowMs - status.syncedAtMs > VALUE_STATUS_STALE_MS);
  const risk = status?.pendingRiskPicos && /^[1-9]\d*$/.test(status.pendingRiskPicos) ? status.pendingRiskPicos : undefined;
  return <>
    <div className="flex min-w-0 items-center gap-0.5 font-mono leading-4">
      <span className="min-w-0">
        <strong>{status ? formatQuotaPicos(status.usedPicos) : "—"}{" / "}{formatUsdLimit(enforced.limitMicros)}</strong>
        <span className="text-muted-foreground">{" · "}{periodLabel}</span>
      </span>
      <InfoHint label={t("dashboard.userLimit.value.details")}>
        <span className="grid gap-1">
          <span className="font-medium">{t(valueStatusTextKey(status, nowMs))}</span>
          {status ? <>
            {availability ? <span>{t("dashboard.userLimit.value.remaining")}: {formatQuotaPicos(availability.remainingPicos)}</span> : null}
            {risk ? <span>{t("dashboard.userLimit.value.pendingRisk", {amount: formatQuotaPicos(risk)})}</span> : null}
            <span>{t("dashboard.userLimit.value.meteredSince", {time: formatQuotaTime(status.meteredFromMs, locale)})}</span>
            <span>{t("dashboard.userLimit.value.lastSync", {time: formatQuotaTime(status.syncedAtMs, locale)})}</span>
          </> : null}
          <span className="text-muted-foreground">{t("dashboard.userLimit.value.hint")}</span>
        </span>
      </InfoHint>
    </div>
    {availability ? <ValueBar basisPoints={availability.basisPoints} t={t} /> : null}
    {blocked ? <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-[10px] font-medium text-danger">
      <span className="inline-flex items-center gap-1"><span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-danger" />{t("dashboard.userLimit.value.needsReconcile")}</span>
      {onReconcile ? <button type="button" onClick={onReconcile}
        className="rounded px-0.5 underline underline-offset-2 transition-colors hover:text-foreground focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary">
        {t("dashboard.userLimit.value.reconcile")}</button> : null}
    </div> : !status ? <div className="mt-0.5 text-[10px] text-muted-foreground">{t("dashboard.userLimit.value.unsynced")}</div>
      : availability?.exhausted ? <div className="mt-0.5 text-[10px] text-danger">{t("dashboard.userLimit.atLimit")}</div>
        : availability?.nearLimit ? <div className="mt-0.5 text-[10px] text-warning">{t("dashboard.userLimit.nearLimit")}</div> : null}
    {reason ? <div className="mt-0.5 break-words text-[10px] text-muted-foreground">{renderValueQuotaMessage(reason, t)}</div> : null}
    {delayed ? <div className="mt-0.5 text-[10px] italic text-muted-foreground">{t("dashboard.userLimit.value.syncDelayed")}</div> : null}
  </>;
}
