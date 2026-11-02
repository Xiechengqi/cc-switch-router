import type { AppLocale, MessageKey } from "./i18n";

const STAGES: Record<string, MessageKey> = {
  awaiting_revocation: "shareMarket.controlProgress.revocation",
  awaiting_final_usage: "shareMarket.controlProgress.finalUsage",
  awaiting_settlement: "shareMarket.controlProgress.settlement",
  retry_scheduled: "shareMarket.controlProgress.retry",
  manual_intervention: "shareMarket.controlProgress.manual",
};

export function controlProgressMessageKey(stage: string): MessageKey | null {
  return Object.hasOwn(STAGES, stage) ? STAGES[stage] : null;
}

export function controlRetryTime(value: string | undefined, locale: AppLocale): string | null {
  if (!value) return null;
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" }).format(timestamp)
    : null;
}
