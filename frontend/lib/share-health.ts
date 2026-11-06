import { messages, type AppLocale } from "@/lib/i18n";
import type { ShareModelHealthStatus } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";

const tones = {
  healthy: "border-emerald-200 bg-emerald-50 text-emerald-700",
  degraded: "border-amber-200 bg-amber-50 text-amber-700",
  unavailable: "border-red-200 bg-red-50 text-red-700",
  unknown: "border-slate-200 bg-slate-50 text-slate-600",
};

/** The Share account panel is a Share-level observation, not an assertion that
 * every protocol was tested. Preserve the observed App/source in the title. */
export function shareAccountHealthPresentation(
  status: ShareModelHealthStatus | undefined,
  currentFingerprint: string | undefined,
  locale: AppLocale = "en",
  now = Date.now(),
) {
  return modelHealthPresentation(
    !currentFingerprint || currentFingerprint === status?.healthFingerprint ? status : undefined,
    locale,
    now,
  );
}

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
  const manual = state !== "unknown" && (status?.source === "server_manual" || status?.source === "router_connection_test");
  const slow = state === "degraded" && (status?.reasonCode === "probe_slow" || status?.reasonCode === "probe_degraded");
  const recovering = state === "degraded" && status?.reasonCode === "probe_recovering";
  const manualFailed = manual && (status?.reasonCode === "manual_probe_failed" || status?.reasonCode === "upstream_rate_limited");
  const detailLabel = state === "unknown"
    ? status?.reasonCode === "awaiting_probe" && !stale ? copy["dashboard.modelHealth.awaiting"] : label
    : [manual ? copy[manualFailed ? "dashboard.modelHealth.manualFailed" : "dashboard.modelHealth.manual"] : "", slow ? copy["dashboard.modelHealth.slow"] : "",
        recovering ? copy["dashboard.modelHealth.recoveringShort"] : ""].filter(Boolean).join(" · ");
  const title = [
    label,
    [status?.appType, status?.model].filter(Boolean).join(" · "),
    status?.statusCode ? `HTTP ${status.statusCode}` : "",
    status?.checkedAt ? formatDateTime(status.checkedAt * 1000, locale) : "",
    status?.reasonCode === "upstream_rate_limited" ? copy["dashboard.modelHealth.rateLimited"] : "",
    status?.reasonCode === "probe_recovering" ? copy["dashboard.modelHealth.recovering"] : "",
    manual ? copy[`dashboard.modelHealth.source.${status!.source as "server_manual" | "router_connection_test"}`] : "",
    slow ? copy["dashboard.modelHealth.slow"] : "",
    Number.isSafeInteger(status?.latencyMs) && Number(status?.latencyMs) >= 0
      ? `${(Number(status?.latencyMs) / 1000).toFixed(2)}s` : "",
  ].filter(Boolean).join(" · ");
  return { className: tones[state], state: state as keyof typeof tones, label, title, detailLabel };
}
