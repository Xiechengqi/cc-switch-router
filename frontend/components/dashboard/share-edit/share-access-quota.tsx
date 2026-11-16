"use client";

import * as React from "react";
import { Globe2, Lock } from "lucide-react";
import { ChoiceCard } from "@/components/common/choice-card";
import { InfoHint } from "@/components/common/info-hint";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  accountCycleEnded,
  accountResetBasisKey,
  accountResetBucketKey,
  accountResetReasonKey,
  accountResetStatusKey,
} from "@/lib/account-reset-display";
import type { AccountQuotaCycle, AccountResetCapability } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";
import { ShareEditSection, ShareEditSubheading } from "./share-edit-section";

/**
 * Private or public, as one radio choice. Read-only callers get the same two
 * words and the one line that explains the current mode.
 */
export function ShareAccessModeField({
  t,
  freeAccess,
  disabled,
  onChange,
}: {
  t: TFn;
  freeAccess: boolean;
  disabled?: boolean;
  onChange?: (freeAccess: boolean) => void;
}) {
  const name = React.useId();
  const headingId = `${name}-heading`;
  if (!onChange) {
    return (
      <div className="grid gap-1">
        <ShareEditSubheading>{t("dashboard.access.title")}</ShareEditSubheading>
        <p className="text-sm text-foreground">
          <span className="font-medium">{t(freeAccess ? "dashboard.access.public" : "dashboard.access.private")}</span>
          <span className="text-muted-foreground">
            {" · "}
            {t(freeAccess ? "dashboard.access.publicHint" : "dashboard.access.privateHint")}
          </span>
        </p>
      </div>
    );
  }
  return (
    <div className="grid gap-2">
      <ShareEditSubheading id={headingId}>{t("dashboard.access.title")}</ShareEditSubheading>
      <div role="radiogroup" aria-labelledby={headingId} className="grid gap-2 sm:grid-cols-2">
        <ChoiceCard
          name={name}
          value="private"
          checked={!freeAccess}
          disabled={disabled}
          onChange={() => onChange(false)}
          icon={Lock}
          title={t("dashboard.access.private")}
          description={t("dashboard.access.privateHint")}
          selectedLabel={t("dashboard.access.selected")}
        />
        <ChoiceCard
          name={name}
          value="public"
          checked={freeAccess}
          disabled={disabled}
          onChange={() => onChange(true)}
          icon={Globe2}
          title={t("dashboard.access.public")}
          description={t("dashboard.access.publicHint")}
          selectedLabel={t("dashboard.access.selected")}
        />
      </div>
    </div>
  );
}

/**
 * "Sharing & quotas" in one fixed order, outside in: who may call, the
 * Share-wide ceiling, then each user's quota inside it.
 */
export function ShareAccessQuotaSection({
  t,
  access,
  ceiling,
  users,
}: {
  t: TFn;
  access: React.ReactNode;
  ceiling: React.ReactNode;
  users: React.ReactNode;
}) {
  return (
    <ShareEditSection title={t("dashboard.shareEdit.section.access")}>
      <div className="grid grid-cols-[minmax(0,1fr)] gap-5 [&>*+*]:border-t [&>*+*]:border-border [&>*+*]:pt-5">
        {access}
        {ceiling}
        {users}
      </div>
    </ShareEditSection>
  );
}

/**
 * The account reset cycle in words. Raw Server codes never reach the page:
 * status, reason, bucket and basis read as copy; the code itself stays in the
 * detail tooltip for support. Times are local.
 */
export function AccountResetStatus({
  t,
  capability,
  cycle,
  capacityWarning,
}: {
  t: TFn;
  capability?: AccountResetCapability;
  cycle?: AccountQuotaCycle;
  capacityWarning?: { count: number; limit: number };
}) {
  const { locale } = useLocaleText();
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const endsAtMs = cycle?.endsAtMs;
  // Re-read the clock when the cycle ends, so an open dialog does not keep
  // calling an ended cycle ready.
  React.useEffect(() => {
    if (endsAtMs == null || endsAtMs <= nowMs) return;
    const timer = window.setTimeout(() => setNowMs(Date.now()), Math.min(endsAtMs - nowMs + 1_000, 2_000_000_000));
    return () => window.clearTimeout(timer);
  }, [endsAtMs, nowMs]);
  const ended = accountCycleEnded(cycle, nowMs);
  // An ended cycle is no longer ready, whatever an older Server still says;
  // a worse reported state (error, needs reconcile) is kept as reported.
  const status = ended && capability?.syncStatus === "ready" ? "stale" : capability?.syncStatus;
  const reason = capability?.reason ?? (ended ? "cycle_expired" : null);
  const ready = status === "ready";
  const basisKey = accountResetBasisKey(cycle?.basis);
  const details = [
    cycle ? t("dashboard.userLimit.accountReset.basis", { basis: basisKey ? t(basisKey) : cycle.basis }) : null,
    cycle ? `${t("dashboard.userLimit.accountReset.observed")} ${formatDateTime(cycle.observedAtMs, locale)}` : null,
    capability?.reason ? `(${capability.reason})` : null,
    t("dashboard.userLimit.accountReset.caveat"),
  ].filter(Boolean).join(" · ");

  return (
    <div className="grid gap-1 rounded-xl bg-muted/40 px-3 py-2.5 text-xs leading-relaxed">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
        <span className="font-medium text-foreground">{t("dashboard.userLimit.accountReset.label")}</span>
        <span role="status" className={ready ? "text-success" : "text-warning"}>
          {t(accountResetStatusKey(status))}
        </span>
        <InfoHint label={t("dashboard.userLimit.accountReset.details")}>{details}</InfoHint>
      </div>
      {!ready && reason ? <p className="text-muted-foreground">{t(accountResetReasonKey(reason))}</p> : null}
      {cycle ? (
        <p className="tabular-nums text-muted-foreground">
          {t("dashboard.userLimit.accountReset.range", {
            bucket: t(accountResetBucketKey(cycle.bucketId)),
            start: formatDateTime(cycle.startsAtMs, locale),
            end: cycle.endsAtMs
              ? ended
                ? t("dashboard.userLimit.accountReset.ended", { time: formatDateTime(cycle.endsAtMs, locale) })
                : formatDateTime(cycle.endsAtMs, locale)
              : t("dashboard.userLimit.accountReset.noEnd"),
          })}
        </p>
      ) : null}
      {capacityWarning ? (
        <p className="text-warning">
          {t("dashboard.userLimit.accountReset.capacityWarning", capacityWarning)}
        </p>
      ) : null}
    </div>
  );
}
