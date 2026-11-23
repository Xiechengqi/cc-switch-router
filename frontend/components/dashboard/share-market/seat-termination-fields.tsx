"use client";
/*
 * Seller control over how a paid seat can end early. Changes apply to new
 * rentals only; each rental keeps the terms it was quoted with.
 */
import * as React from "react";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getPublicShareTerminationPolicy } from "@/lib/share-termination";
import { MAX_SEAT_TERMINATION_RATE_PERCENT, type SeatTerminationDraft } from "@/lib/share-seat-terms";
import { cn } from "@/lib/utils";

let platformRate: Promise<number | null> | null = null;
function usePlatformRatePercent() {
  const [rate, setRate] = React.useState<number | null>(null);
  React.useEffect(() => {
    let live = true;
    platformRate ??= getPublicShareTerminationPolicy().then((p) => p.rateBps / 100).catch(() => { platformRate = null; return null; });
    void platformRate.then((value) => { if (live) setRate(value); });
    return () => { live = false; };
  }, []);
  return rate;
}

export function SeatTerminationFields({
  value,
  disabled,
  invalid,
  describedBy,
  fieldClassName,
  onChange,
}: {
  value: SeatTerminationDraft;
  disabled?: boolean;
  invalid?: boolean;
  describedBy?: string;
  fieldClassName: string;
  onChange: (value: SeatTerminationDraft) => void;
}) {
  const { t } = useLocaleText();
  const platform = usePlatformRatePercent();
  const forbidden = value.earlyTermination === "forbidden";
  const inputId = React.useId();
  return (
    <fieldset className="grid min-w-0 gap-2 rounded-xl border border-border px-3 py-3">
      <legend className="px-1 text-xs font-medium text-foreground">{t("shareSeatTerms.title")}</legend>
      <SegmentedControl
        value={value.earlyTermination}
        onChange={(earlyTermination) => onChange({ ...value, earlyTermination })}
        ariaLabel={t("shareSeatTerms.title")}
        size="md"
        fullWidth
        disabled={disabled}
        items={[
          { id: "allowed", label: t("shareSeatTerms.allowed") },
          { id: "forbidden", label: t("shareSeatTerms.forbidden") },
        ]}
      />
      {forbidden ? (
        <p className="text-xs leading-5 text-muted-foreground">{t("shareSeatTerms.forbiddenHelp")}</p>
      ) : (
        <div className="grid gap-1">
          <label htmlFor={inputId} className="text-xs text-slate-500">{t("shareSeatTerms.rate")}</label>
          <span className="relative block sm:max-w-56">
            <input
              id={inputId}
              className={cn(fieldClassName, "w-full pr-8")}
              inputMode="decimal"
              autoComplete="off"
              disabled={disabled}
              aria-invalid={invalid}
              aria-describedby={describedBy}
              value={value.terminationRate}
              placeholder={platform == null ? t("shareSeatTerms.platformDefault") : t("shareSeatTerms.platformDefaultRate", { rate: platform })}
              onChange={(event) => onChange({ ...value, terminationRate: event.target.value })}
            />
            <span aria-hidden="true" className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-sm text-slate-400">%</span>
          </span>
          <p className="text-xs leading-5 text-muted-foreground">{t("shareSeatTerms.rateHelp", { max: MAX_SEAT_TERMINATION_RATE_PERCENT })}</p>
        </div>
      )}
      <p className="text-[11px] leading-5 text-muted-foreground">{t("shareSeatTerms.newRentalsOnly")}</p>
    </fieldset>
  );
}
