// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
"use client";
import * as React from "react";
import { Input } from "@heroui/react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { InfoHint } from "@/components/common/info-hint";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";
import {
  formatQuotaPicos, formatQuotaTime, renderValueQuotaMessage, valuePreviewBlocker, valuePreviewRowOutcome, valueQuotaErrorMessage,
  type ValuePreviewInvalidRow, type ValuePreviewRow,
} from "@/lib/share-value-quota";
import type { ValuePreviewState } from "@/lib/use-value-quota-preview";

export type QuotaUnit = "tokens" | "usdEquivalent";

export function QuotaUnitSwitch({value, onChange, usdDisabled, disabled, readinessId, t}: {
  value: QuotaUnit; onChange: (unit: QuotaUnit) => void; usdDisabled?: boolean; disabled?: boolean;
  /** Id of the visible "USD quota unavailable" line, read with the disabled `$`. */
  readinessId?: string; t: TFn;
}) {
  const reasonId = React.useId();
  const usdOff = !!usdDisabled && value !== "usdEquivalent";
  // `title` alone is not announced reliably; the reason is real text, linked by id.
  const describedBy = usdOff ? [reasonId, readinessId].filter(Boolean).join(" ") : undefined;
  return <>
  <SegmentedControl<QuotaUnit>
    ariaLabel={t("dashboard.userLimit.value.unit")}
    ariaDescribedBy={describedBy}
    value={value}
    onChange={onChange}
    disabled={disabled}
    size="md"
    fullWidth
    items={[
      {id: "tokens", label: t("dashboard.userLimit.value.tokens")},
      {id: "usdEquivalent", label: t("dashboard.userLimit.value.unitUsdShort"), ariaLabel: t("dashboard.userLimit.value.usd"),
        title: t("dashboard.userLimit.value.usd"), disabled: usdOff, describedBy},
    ]}
  />
  {usdOff ? <span id={reasonId} className="sr-only">{t("dashboard.userLimit.value.usdDisabledReason")}</span> : null}
  </>;
}

/** The amount field for either unit; USD shows a `$` prefix and an inline, specific error. */
export function QuotaLimitField({id, unit, value, onChange, error, disabled, labelHidden, t}: {
  id: string; unit: QuotaUnit; value: string; onChange: (value: string) => void; error?: string; disabled?: boolean;
  /** Keep the accessible name when a checkbox already labels the row visually. */
  labelHidden?: boolean; t: TFn;
}) {
  const usd = unit === "usdEquivalent";
  const errorId = `${id}-error`;
  return <div className="grid gap-1.5">
    <span className={cn("flex items-center gap-1", labelHidden && !usd && "sr-only")}>
      <label htmlFor={id} className={cn("mono-label text-muted-foreground", labelHidden && "sr-only")}>
        {t(usd ? "dashboard.userLimit.value.limit" : "dashboard.field.tokenLimit")}
      </label>
      {usd ? <InfoHint label={t("dashboard.userLimit.value.details")}>
        <span className="grid gap-1.5"><span>{t("dashboard.userLimit.value.hint")}</span><span>{t("dashboard.userLimit.value.start")}</span></span>
      </InfoHint> : null}
    </span>
    <div className="relative">
      {usd ? <span aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 z-10 -translate-y-1/2 text-sm text-muted-foreground">$</span> : null}
      <Input id={id} type="text" inputMode="decimal" autoComplete="off" disabled={disabled}
        className={cn("w-full", usd && "pl-6")}
        placeholder={usd ? t("dashboard.userLimit.value.amountPlaceholder") : t("common.unlimited")}
        aria-invalid={error ? true : undefined} aria-describedby={error ? errorId : undefined}
        value={value} onChange={event => onChange(event.target.value)} />
    </div>
    {error ? <p id={errorId} className="text-xs text-red-600">{error}</p> : null}
  </div>;
}

function RowOutcome({row, t, locale, withEmail}: {row: ValuePreviewRow; t: TFn; locale: string; withEmail?: boolean}) {
  const outcome = valuePreviewRowOutcome(row);
  const used = formatQuotaPicos(row.usedPicos ?? "0");
  const text = outcome === "scheduled"
    ? t("dashboard.userLimit.value.previewStarts", {date: formatQuotaTime(row.effectiveFromMs, locale, "short"), used})
    : outcome === "meteredFromNow" ? t("dashboard.userLimit.value.previewMeteredFromNow")
      : t("dashboard.userLimit.value.previewApplies", {used});
  const hint = outcome === "scheduled" ? "dashboard.userLimit.value.previewHint.scheduled"
    : outcome === "meteredFromNow" ? "dashboard.userLimit.value.previewHint.meteredFromNow" : "dashboard.userLimit.value.previewHint.ready";
  return <span className="inline-flex min-w-0 items-center gap-1">
    <span className="min-w-0 break-words">{withEmail ? <span className="font-mono">{row.email} · </span> : null}{text}</span>
    {withEmail ? null : <InfoHint label={t("dashboard.userLimit.value.preview")}>
      <span className="grid gap-1">
        <span>{t(hint)}</span>
        <span className="text-muted-foreground">{t("dashboard.userLimit.value.meteredSince", {time: formatQuotaTime(row.meteredFromMs, locale)})}</span>
      </span>
    </InfoHint>}
  </span>;
}

function invalidRowReason(row: ValuePreviewInvalidRow, t: TFn) {
  const blocker = valuePreviewBlocker({ready: false, configRevision: 0, rows: [], invalidRows: [row]});
  return blocker ? renderValueQuotaMessage(blocker.message, t) : "";
}

/**
 * One line under the amount: when the policy starts and what it carries over.
 * Batches add a compact per-user list. Blocking reasons are translated per row.
 */
export function ValuePreviewLine({state, t}: {state: ValuePreviewState; t: TFn}) {
  const {locale} = useLocaleText();
  if (!state.intent) return null;
  if (state.loading) return <p className="text-xs text-muted-foreground" role="status">{t("dashboard.userLimit.value.previewLoading")}</p>;
  if (state.error) return <p className="text-xs text-red-600" role="alert">{renderValueQuotaMessage(valueQuotaErrorMessage(state.error), t)}</p>;
  const preview = state.preview;
  if (!preview) return null;
  const blocker = valuePreviewBlocker(preview);
  const invalid = preview.invalidRows ?? [];
  if (invalid.length > 1 || (invalid.length && preview.rows.length)) {
    // Batches list every rejected user, so one fix-up pass is enough.
    return <div className="grid gap-1 text-xs text-red-600" role="alert">
      <span>{t("dashboard.userLimit.value.invalidRows", {count: invalid.length})}</span>
      <ul className="grid max-h-28 gap-0.5 overflow-auto rounded-md border border-red-200 bg-red-50/40 px-2 py-1.5">
        {invalid.map((row, index) => <li key={`${row.email}:${index}`} className="min-w-0 break-words">
          <span className="font-mono">{row.email}</span> · {invalidRowReason(row, t)}
        </li>)}
      </ul>
    </div>;
  }
  if (blocker) {
    const reason = renderValueQuotaMessage(blocker.message, t);
    return <p className="text-xs text-red-600" role="alert">
      {blocker.email && (preview.rows.length + invalid.length) > 1 ? t("dashboard.userLimit.value.rowReason", {email: blocker.email, reason}) : reason}
    </p>;
  }
  if (preview.rows.length <= 1) {
    return preview.rows[0] ? <p className="text-xs text-muted-foreground" role="status"><RowOutcome row={preview.rows[0]} t={t} locale={locale} /></p> : null;
  }
  const scheduled = preview.rows.filter(row => valuePreviewRowOutcome(row) === "scheduled").length;
  const fromNow = preview.rows.filter(row => valuePreviewRowOutcome(row) === "meteredFromNow").length;
  return <div className="grid gap-1 text-xs text-muted-foreground" role="status">
    <span className="inline-flex items-center gap-1">
      {t("dashboard.userLimit.value.previewSummary", {count: preview.rows.length, scheduled, now: fromNow})}
      <InfoHint label={t("dashboard.userLimit.value.preview")}>
        <span className="grid gap-1.5">
          <span>{t("dashboard.userLimit.value.previewHint.scheduled")}</span>
          <span>{t("dashboard.userLimit.value.previewHint.meteredFromNow")}</span>
        </span>
      </InfoHint>
    </span>
    <ul className="grid max-h-28 gap-0.5 overflow-auto rounded-md border bg-muted/20 px-2 py-1.5">
      {preview.rows.map(row => <li key={row.email} className="min-w-0"><RowOutcome row={row} t={t} locale={locale} withEmail /></li>)}
    </ul>
  </div>;
}
