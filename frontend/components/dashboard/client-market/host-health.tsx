"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientMarketHostHealth, manageClientMarketHostHealth } from "@/lib/api";
import type { HostHealth, HostHealthDetail } from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";

const ConfirmAlertDialog = React.lazy(() => import("@/components/common/confirm-alert-dialog").then(module => ({ default: module.ConfirmAlertDialog })));

const RETRYABLE_STATUSES = new Set(["onboarding", "maintaining", "idle"]);
const DYNAMIC_HIDDEN = new Set(["private", "expired"]);

export function HostHealthSummary({ health, hostId, canManage = false, preparing = false, canReplace = false, hostStatus, viewerKey, hostLabel, onChanged }: {
  health?: HostHealth | null;
  hostId?: string;
  canManage?: boolean;
  preparing?: boolean;
  hostStatus?: string;
  canReplace?: boolean;
  viewerKey?: string;
  hostLabel?: string;
  onChanged?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  const [loaded, setLoaded] = React.useState<{ value: HostHealthDetail; viewerKey: string | undefined } | null>(null);
  const details = loaded?.viewerKey === viewerKey ? loaded?.value : null;
  const [error, setError] = React.useState(false);
  const [actionError, setActionError] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [queued, setQueued] = React.useState(false);
  // The replacement epoch is captured when the dialog opens so a refresh cannot blank it.
  const [confirmation, setConfirmation] = React.useState<{ kind: "replace" | "revoke"; machineEpoch?: number } | null>(null);
  React.useEffect(() => {
    setQueued(false);
    setActionError(false);
  }, [open, hostId, health?.lastReceivedAt]);
  React.useEffect(() => {
    setLoaded(null);
    setError(false);
    if (!open || !hostId) return;
    const controller = new AbortController();
    getClientMarketHostHealth(hostId, controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoaded({ value, viewerKey }); })
      .catch(() => { if (!controller.signal.aborted) setError(true); });
    return () => controller.abort();
  }, [open, hostId, health?.lastReceivedAt, health?.state, health?.nextDueAt, viewerKey]);

  const time = (value: string) => new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(new Date(value));
  const relative = (value: string) => {
    const minutes = Math.round((new Date(value).getTime() - Date.now()) / 60_000);
    const format = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
    return Math.abs(minutes) < 120 ? format.format(minutes, "minute") : format.format(Math.round(minutes / 60), "hour");
  };
  const number = (value: number | null | undefined, divisor = 1) => value == null
    ? t("hostHealth.unknown")
    : new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value / divisor);
  const snapshot = health?.snapshot;
  const specs = snapshot ? [
    snapshot.arch,
    snapshot.effectiveVcpu.value == null ? null : t("hostHealth.cpu", { value: number(snapshot.effectiveVcpu.value) }),
    snapshot.memoryTotalBytes.value == null ? null : t("hostHealth.memory", { value: number(snapshot.memoryTotalBytes.value, 2 ** 30) }),
    snapshot.dataVolumeTotalBytes.value == null ? null : t("hostHealth.disk", { value: number(snapshot.dataVolumeTotalBytes.value, 2 ** 30) }),
  ].filter(Boolean).join(" · ") : null;
  const unknownSpec = snapshot && [snapshot.effectiveVcpu, snapshot.memoryTotalBytes, snapshot.dataVolumeTotalBytes].some(value => value.value == null);

  const retryDeployment = preparing || health?.state === "awaiting_first" || health?.state === "first_overdue" || !health;
  // Router only accepts a retry for Hosts outside a rental and not disabled or quarantined.
  const canRetry = !hostStatus || RETRYABLE_STATUSES.has(hostStatus);
  const unitValue = (reading: { value: number | null }, unit: string, divisor = 1) =>
    reading.value == null ? t("hostHealth.unknown") : `${number(reading.value, divisor)} ${unit}`;
  const diagnostic = (code: string) => {
    switch (code) {
      case "hour_conflict": return t("hostHealth.conflict");
      case "rotation_requested": return t("hostHealth.queued");
      case "replacement_pending": return t("hostHealth.replacementPending");
      case "maintenance_failed": return t("hostHealth.maintenanceFailed");
      default: return t("hostHealth.deployFailed", { detail: code });
    }
  };

  const manage = async (action: "retry" | "rotate-key" | "replace", options: { compromised?: boolean; machineEpoch?: number } = {}) => {
    if (!hostId || busy) return;
    setBusy(true);
    setActionError(false);
    try {
      await manageClientMarketHostHealth(hostId, action, { expectedMachineEpoch: options.machineEpoch ?? details?.machineEpoch, compromised: options.compromised ?? false });
      setQueued(true);
      setConfirmation(null);
      onChanged?.();
    } catch {
      setActionError(true);
      setConfirmation(null);
    }
    finally { setBusy(false); }
  };
  const dynamic = details?.hostHealth?.snapshot;
  const dynamicReadings = dynamic ? [dynamic.memoryAvailableBytes, dynamic.dataVolumeAvailableBytes, dynamic.cpuUsagePct] : [];
  const dynamicHidden = dynamicReadings.length > 0 && dynamicReadings.every(reading => reading.value == null && DYNAMIC_HIDDEN.has(reading.unknownReason || ""));

  return (
    <section aria-label={[t("hostHealth.title"), hostLabel].filter(Boolean).join(" · ")} className="mt-2 grid gap-1 text-[11px] leading-4" onClick={event => event.stopPropagation()}>
      <span className="text-muted-foreground">{t("hostHealth.title")}</span>
      <span className={health?.state === "healthy" ? "text-emerald-700" : health?.state === "missed" || health?.state === "first_overdue" ? "text-amber-700" : "text-muted-foreground"}>
        {health ? t(("hostHealth.state." + health.state) as MessageKey) : t("hostHealth.notReady")}
      </span>
      {health?.lastReceivedAt ? <time dateTime={health.lastReceivedAt}>{t("hostHealth.received", { time: time(health.lastReceivedAt) })} {t("hostHealth.ago", { relative: relative(health.lastReceivedAt) })}</time> : null}
      {health?.state === "missed" && health.consecutiveMissed > 0 ? <span className="text-amber-700">{t("hostHealth.missedCount", { count: health.consecutiveMissed })}</span> : null}
      {health?.nextDueAt ? <time dateTime={health.nextDueAt} className="text-muted-foreground">{t("hostHealth.next", { time: time(health.nextDueAt) })}</time> : null}
      {specs ? <span>{specs}</span> : null}
      {health?.sampledAt ? <time dateTime={health.sampledAt} className="text-muted-foreground">{t("hostHealth.sampled", { time: time(health.sampledAt) })}</time> : null}
      {unknownSpec ? <span className="text-muted-foreground">{t("hostHealth.unknownSpec")}</span> : null}
      <span>{health?.observable24h ? t("hostHealth.record", { received: health.received24h, observed: health.observable24h }) : t("hostHealth.noSamples")}</span>
      {health?.supervision === "nohup" ? <span className="text-amber-700">{t("hostHealth.noAutostart")}</span> : null}
      {health?.clockSynchronized === false ? <span className="text-amber-700">{t("hostHealth.clockUnsynced")}</span> : null}
      {health?.diagnostic ? <span className="text-amber-700">{diagnostic(health.diagnostic)}</span> : null}
      {hostId ? <button type="button" className="min-h-11 rounded text-left text-primary underline underline-offset-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-background" aria-expanded={open} onClick={() => setOpen(value => !value)}>{t(open ? "hostHealth.close" : "hostHealth.details")}</button> : null}
      {open ? (
        <div className="max-h-72 overflow-auto rounded-md border border-border bg-background p-2">
          <p className="font-medium">{t("hostHealth.history")}</p>
          {error ? <p role="alert">{t("hostHealth.error")}</p> : !details ? <p role="status">{t("hostHealth.loading")}</p> : <>
            {dynamic ? <p>{dynamicHidden ? t("hostHealth.dynamicHidden") : t("hostHealth.available", {
              memory: unitValue(dynamic.memoryAvailableBytes, "GiB", 2 ** 30),
              disk: unitValue(dynamic.dataVolumeAvailableBytes, "GiB", 2 ** 30),
              cpu: unitValue(dynamic.cpuUsagePct, "%"),
            })}</p> : null}
            <ol className="mt-2 grid gap-1">
              {details.hours.map(hour => <li key={hour.utcHour} className="flex flex-wrap justify-between gap-2 border-t border-border py-1">
                <time dateTime={hour.dueAt}>{time(hour.dueAt)}</time>
                <span>{t(("hostHealth.state." + hour.state) as MessageKey)}</span>
              </li>)}
            </ol>
          </>}
          {canManage ? <div className="mt-2 grid gap-2">
            {!retryDeployment || canRetry ? <button type="button" disabled={busy} className="min-h-11 rounded text-left text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-background" onClick={() => void manage(retryDeployment ? "retry" : "rotate-key")}>
              {t(retryDeployment ? "hostHealth.retry" : "hostHealth.rotate")}
            </button> : null}
            <button type="button" disabled={busy} className="min-h-11 rounded text-left text-amber-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-background" onClick={() => setConfirmation({ kind: "revoke" })}>{t("hostHealth.revoke")}</button>
            {canReplace ? <button type="button" disabled={busy || !details?.machineEpoch} className="min-h-11 rounded text-left text-amber-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-background" onClick={() => setConfirmation({ kind: "replace", machineEpoch: details?.machineEpoch })}>{t("hostHealth.replace")}</button> : null}
            {queued ? <p role="status">{t("hostHealth.queued")}</p> : null}
            {actionError ? <p role="alert" className="text-rose-600">{t("hostHealth.actionFailed")}</p> : null}
          </div> : null}
        </div>
      ) : null}
      {confirmation ? <React.Suspense fallback={null}><ConfirmAlertDialog open title={t(confirmation.kind === "replace" ? "hostHealth.replace" : "hostHealth.revoke")} description={t(confirmation.kind === "replace" ? "hostHealth.replaceDescription" : "hostHealth.revokeDescription")} confirmLabel={t("hostHealth.confirm")} cancelLabel={t("common.cancel")} busy={busy} tone="warning" onOpenChange={value => { if (!value && !busy) setConfirmation(null); }} onConfirm={() => void manage(confirmation.kind === "replace" ? "replace" : "rotate-key", confirmation.kind === "replace" ? { machineEpoch: confirmation.machineEpoch } : { compromised: true })} /></React.Suspense> : null}
    </section>
  );
}

