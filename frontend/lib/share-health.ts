import { messages, type AppLocale } from "@/lib/i18n";
import type { ShareModelHealthStatus } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";

const tones = {
  healthy: "border-emerald-200 bg-emerald-50 text-emerald-700",
  degraded: "border-amber-200 bg-amber-50 text-amber-700",
  unavailable: "border-red-200 bg-red-50 text-red-700",
  unknown: "border-slate-200 bg-slate-50 text-slate-600",
};

export function modelHealthPresentation(
  status?: ShareModelHealthStatus,
  locale: AppLocale = "en",
  now = Date.now(),
) {
  const observed =
    Number.isSafeInteger(status?.checkedAt) &&
    Number.isSafeInteger(status?.staleAt) &&
    Number(status?.checkedAt) > 0 &&
    Number(status?.checkedAt) * 1000 <= now &&
    Number(status?.staleAt) > Number(status?.checkedAt);
  const stale = observed && Number(status?.staleAt) * 1000 <= now;
  const state = observed && !stale && status && Object.hasOwn(tones, status.state)
    ? status.state : "unknown";
  const copy = messages[locale];
  const label = stale ? copy["dashboard.modelHealth.stale"] : copy[`dashboard.modelHealth.${state}`];
  const title = [
    label,
    [status?.appType, status?.model].filter(Boolean).join(" · "),
    status?.statusCode ? `HTTP ${status.statusCode}` : "",
    status?.checkedAt ? formatDateTime(status.checkedAt * 1000, locale) : "",
    status?.reasonCode === "upstream_rate_limited" ? copy["dashboard.modelHealth.rateLimited"] : "",
    status?.reasonCode === "probe_recovering" ? copy["dashboard.modelHealth.recovering"] : "",
  ].filter(Boolean).join(" · ");
  return { className: tones[state], label, title };
}
