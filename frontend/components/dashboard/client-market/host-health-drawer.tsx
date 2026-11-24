"use client";

import * as React from "react";
import { Button, Drawer } from "@heroui/react";
import { Loader2, RefreshCw, X } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientMarketHostHealth, manageClientMarketHostHealth } from "@/lib/api";
import { formatHostHealthTime, hostHealthDisplayState } from "@/lib/host-health-view";
import type { ClientMarketHost, HostHealthDetail, HostHealthReading } from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";
import { HostHardwareSummary, HostHealthSummary } from "./host-health";

type Action = "bootstrap" | "retry" | "rotate-key" | "replace";

export function HostHealthDrawer({ host, viewerKey, onClose, onChanged }: {
  host: ClientMarketHost;
  viewerKey: string;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { locale, t } = useLocaleText();
  const scope = [viewerKey, host.id, host.installationId, host.hostHealth?.machineEpoch, host.isHostOwner, host.isClientOwner, host.status,
    host.hostHealth?.snapshot?.cpuUsagePct.unknownReason || "known"].join(":");
  const currentScope = React.useRef(scope);
  currentScope.current = scope;
  const [loaded, setLoaded] = React.useState<{ scope: string; value: HostHealthDetail } | null>(null);
  const [readError, setReadError] = React.useState(false);
  const [actionError, setActionError] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [queued, setQueued] = React.useState(false);
  const [revision, setRevision] = React.useState(0);
  const [confirmation, setConfirmation] = React.useState<{ kind: "replace" | "revoke"; epoch?: number } | null>(null);
  const details = loaded?.scope === scope ? loaded.value : null;
  const health = details?.hostHealth || host.hostHealth;
  const label = host.hostname || host.clientSubdomain || host.ip || host.id.slice(0, 8);
  const epoch = health?.machineEpoch || details?.machineEpoch;

  React.useEffect(() => {
    setLoaded(null); setReadError(false);
    const controller = new AbortController();
    getClientMarketHostHealth(host.id, controller.signal)
      .then(value => { if (!controller.signal.aborted && currentScope.current === scope) setLoaded({ scope, value }); })
      .catch(() => { if (!controller.signal.aborted && currentScope.current === scope) setReadError(true); });
    return () => controller.abort();
  }, [scope, host.id, host.hostHealth?.lastReceivedAt, host.hostHealth?.state, host.hostHealth?.deployment?.state, revision]);
  React.useEffect(() => { setQueued(false); setBusy(false); setActionError(false); setConfirmation(null); }, [scope]);
  React.useEffect(() => { setQueued(false); }, [host.hostHealth?.deployment?.state]);

  const manage = async (action: Action, options: { compromised?: boolean; epoch?: number } = {}) => {
    if (busy) return;
    const initiatingScope = scope;
    setBusy(true); setActionError(false);
    try {
      await manageClientMarketHostHealth(host.id, action, {
        expectedMachineEpoch: options.epoch ?? epoch,
        expectedInstallationId: host.installationId,
        compromised: options.compromised,
      });
      if (currentScope.current !== initiatingScope) return;
      setQueued(true); setConfirmation(null); setRevision(value => value + 1); onChanged();
    } catch {
      if (currentScope.current === initiatingScope) { setActionError(true); setConfirmation(null); }
    } finally { if (currentScope.current === initiatingScope) setBusy(false); }
  };
  const value = (reading: HostHealthReading, divisor: number, unit: string) => reading.value == null
    ? t("hostHealth.unknown")
    : `${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(reading.value / divisor)} ${unit}`;
  const snapshot = details?.hostHealth?.snapshot;
  const dynamics = snapshot ? [snapshot.memoryAvailableBytes, snapshot.dataVolumeAvailableBytes, snapshot.cpuUsagePct] : [];
  const hidden = dynamics.length > 0 && dynamics.every(reading => reading.value == null && ["private", "expired"].includes(reading.unknownReason || ""));
  const canRetry = ["idle", "onboarding", "maintaining"].includes(host.status);
  const canReplace = !host.installationId && ["idle", "onboarding", "disabled", "abnormal"].includes(host.status);
  const canBootstrap = !!health?.deployment?.canBootstrap && !!host.installationId && !!epoch;
  const monitorEnrolled = !!health?.lastReceivedAt || ["awaiting_first", "first_overdue", "revoked"].includes(hostHealthDisplayState(health));
  const time = (date: string) => formatHostHealthTime(date, locale);
  const diagnostic = (code: string) => {
    const known: Record<string, MessageKey> = { hour_conflict: "hostHealth.conflict", rotation_requested: "hostHealth.queued", replacement_pending: "hostHealth.replacementPending", maintenance_failed: "hostHealth.maintenanceFailed", monitor_bootstrap_interrupted: "hostHealth.bootstrapInterrupted" };
    return known[code] ? t(known[code]) : t("hostHealth.deployFailed", { detail: code });
  };

  return <>
    <Drawer.Backdrop isOpen onOpenChange={open => { if (!open) onClose(); }}>
      <Drawer.Content placement="right">
        <Drawer.Dialog className="host-health-drawer light">
          <Drawer.CloseTrigger className="host-health-close" aria-label={t("common.close")}><X className="h-4 w-4" aria-hidden="true" /></Drawer.CloseTrigger>
          <Drawer.Header className="host-health-drawer-header">
            <Drawer.Heading className="text-lg font-semibold">{t("hostHealth.title")}</Drawer.Heading>
            <p className="break-all text-sm text-muted-foreground">{label}</p>
          </Drawer.Header>
          <Drawer.Body className="host-health-drawer-body">
            <HostHealthSummary health={health} hostLabel={label} variant="table" />
            {health?.deployment?.reason === "rental_not_enrolled" ? <p className="text-sm text-muted-foreground">{t("hostHealth.bootstrapInfo")}</p> : null}
            <section className="host-health-detail-section" aria-label={t("hostHealth.specifications")}>
              <h3>{t("hostHealth.specifications")}</h3>
              <HostHardwareSummary health={health} />
              {health?.sampledAt ? <p className="text-xs text-muted-foreground">{t("hostHealth.sampled", { time: time(health.sampledAt) })}</p> : null}
              {snapshot ? <p className="text-sm text-muted-foreground">{hidden ? t("hostHealth.dynamicHidden") : t("hostHealth.available", {
                memory: value(snapshot.memoryAvailableBytes, 2 ** 30, "GiB"), disk: value(snapshot.dataVolumeAvailableBytes, 1000 ** 3, "GB"), cpu: value(snapshot.cpuUsagePct, 1, "%"),
              })}</p> : null}
              {health?.clockSynchronized === false ? <p className="host-health-limitation">{t("hostHealth.clockUnsynced")}</p> : null}
            </section>
            <section className="host-health-detail-section" aria-label={t("hostHealth.reporting")}>
              <div className="flex items-center justify-between gap-3"><h3>{t("hostHealth.history")}</h3>
                <Button variant="ghost" isIconOnly size="sm" className="min-h-11 min-w-11" aria-label={t("hostHealth.retryRead")} onPress={() => setRevision(value => value + 1)}><RefreshCw className="h-4 w-4" /></Button>
              </div>
              {readError ? <p role="alert">{t("hostHealth.error")}</p> : !details ? <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("hostHealth.loading")}</p>
                : details.hours.length === 0 ? <p className="text-sm text-muted-foreground">{t("hostHealth.emptyHistory")}</p> : <>
                  <ol className="host-health-hour-strip" aria-label={t("hostHealth.reporting")}>
                    {details.hours.slice(0, 24).reverse().map(hour => <li key={hour.utcHour} data-state={hour.state} title={`${time(hour.dueAt)} · ${t(("hostHealth.state." + hour.state) as MessageKey)}`}><span className="sr-only">{time(hour.dueAt)} · {t(("hostHealth.state." + hour.state) as MessageKey)}</span></li>)}
                  </ol>
                  <ol className="host-health-hour-list">
                    {details.hours.map(hour => <li key={hour.utcHour}><time dateTime={hour.dueAt}>{time(hour.dueAt)}</time><span className="host-health-hour-state" data-state={hour.state}>{t(("hostHealth.state." + hour.state) as MessageKey)}</span></li>)}
                  </ol>
                </>}
            </section>
            {host.isHostOwner ? <section className="host-health-detail-section" aria-label={t("hostHealth.management")}>
              <h3>{t("hostHealth.management")}</h3>
              {health?.diagnostic ? <p className="host-health-limitation break-words">{diagnostic(health.diagnostic)}</p> : null}
              {canBootstrap ? <Button variant="primary" className="min-h-11" isDisabled={busy || queued} onPress={() => void manage("bootstrap")}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("hostHealth.bootstrap")}</Button> : null}
              {!canBootstrap && canRetry && !monitorEnrolled ? <Button variant="outline" className="min-h-11" isDisabled={busy || queued} onPress={() => void manage("retry")}>{t("hostHealth.retry")}</Button> : null}
              {monitorEnrolled ? <>
                <Button variant="outline" className="min-h-11" aria-label={t("hostHealth.rotate")} isDisabled={busy} onPress={() => void manage("rotate-key")}><span title={t("hostHealth.rotate")}>{t("hostHealth.rotateShort")}</span></Button>
                <Button variant="ghost" className="min-h-11 text-destructive" aria-label={t("hostHealth.revoke")} isDisabled={busy} onPress={() => setConfirmation({ kind: "revoke" })}><span title={t("hostHealth.revoke")}>{t("hostHealth.revokeShort")}</span></Button>
              </> : null}
              {canReplace ? <Button variant="ghost" className="min-h-11 text-destructive" aria-label={t("hostHealth.replace")} isDisabled={busy || !epoch} onPress={() => setConfirmation({ kind: "replace", epoch })}><span title={t("hostHealth.replace")}>{t("hostHealth.replaceShort")}</span></Button> : null}
              {queued ? <p role="status" className="text-sm text-muted-foreground">{t("hostHealth.queued")}</p> : null}
              {actionError ? <p role="alert" className="text-sm text-destructive">{t("hostHealth.actionFailed")}</p> : null}
            </section> : null}
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
    {confirmation ? <ConfirmAlertDialog open title={t(confirmation.kind === "replace" ? "hostHealth.replace" : "hostHealth.revoke")} description={t(confirmation.kind === "replace" ? "hostHealth.replaceDescription" : "hostHealth.revokeDescription")} confirmLabel={t("hostHealth.confirm")} cancelLabel={t("common.cancel")} busy={busy} tone="warning" onOpenChange={open => { if (!open && !busy) setConfirmation(null); }} onConfirm={() => void manage(confirmation.kind === "replace" ? "replace" : "rotate-key", { epoch: confirmation.epoch, compromised: confirmation.kind === "revoke" })} /> : null}
  </>;
}
