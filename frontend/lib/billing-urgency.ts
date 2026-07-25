import type { ClientMarketBilling } from "@/lib/types";

export type BillingUrgencyTier = "silent" | "soft" | "urgent";

const SOFT_MAX_DAYS = 7;
const URGENT_MAX_DAYS = 3;

export function billingDeadlineTarget(billing: ClientMarketBilling): string | undefined {
  if (billing.status === "payment_due") return billing.paymentDeadline || billing.currentPeriodEnd;
  return billing.currentPeriodEnd;
}

export function billingRemainingMs(billing: ClientMarketBilling, now = Date.now()): number | null {
  const target = billingDeadlineTarget(billing);
  if (!target) return null;
  const parsed = Date.parse(target);
  if (!Number.isFinite(parsed)) return null;
  return parsed - now;
}

/** Paid active / payment_due only. Free forever and non-billable states return null. */
export function billingUrgencyTier(
  billing: ClientMarketBilling | undefined,
  now = Date.now(),
): BillingUrgencyTier | null {
  if (!billing || !billing.priceCents) return null;
  if (billing.status === "payment_due") return "urgent";
  if (billing.status !== "active") return null;
  const remaining = billingRemainingMs(billing, now);
  if (remaining == null) return null;
  const days = remaining / (24 * 60 * 60 * 1000);
  if (days <= URGENT_MAX_DAYS) return "urgent";
  if (days <= SOFT_MAX_DAYS) return "soft";
  return "silent";
}

export function formatBillingCountdown(target: string | undefined, locale: string, now = Date.now()): string {
  if (!target) return "";
  const milliseconds = Date.parse(target) - now;
  if (!Number.isFinite(milliseconds) || milliseconds <= 0) {
    return locale.startsWith("zh") ? "已逾期" : "overdue";
  }
  const minutes = Math.ceil(milliseconds / 60_000);
  const days = Math.floor(minutes / 1_440);
  const hours = Math.floor((minutes % 1_440) / 60);
  const remainingMinutes = minutes % 60;
  if (locale.startsWith("zh")) {
    if (days) return `${days}天 ${hours}小时`;
    if (hours) return `${hours}小时 ${remainingMinutes}分钟`;
    return `${remainingMinutes}分钟`;
  }
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${remainingMinutes}m`;
  return `${remainingMinutes}m`;
}

export function formatBillingAbsoluteDate(value: string | undefined, locale: string): string {
  if (!value) return "";
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return "";
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(parsed));
}
