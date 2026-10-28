"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { trialDuration } from "@/lib/client-checkout";
import type { ClientTrialState } from "@/lib/types";

/** Future eligibility is deliberately separate from the current trial timer. */
export function ClientNextTrialNotice({ trial, className = "" }: {
  trial?: ClientTrialState;
  className?: string;
}) {
  const { locale, t } = useLocaleText();
  if (!trial || !Number.isSafeInteger(trial.epoch) || trial.epoch <= 0
    || !trial.lastResetAt || !Number.isFinite(Date.parse(trial.lastResetAt))
    || !Number.isSafeInteger(trial.allowanceSeconds) || trial.allowanceSeconds < 0) return null;
  return <span data-next-rental-trial className={`block text-xs leading-5 text-muted-foreground ${className}`}>
    {trial.allowanceSeconds === 0 ? t("clientTrial.restoredDisabled")
      : t("clientTrial.restored", { duration: trialDuration(trial.allowanceSeconds, locale) })}
  </span>;
}
