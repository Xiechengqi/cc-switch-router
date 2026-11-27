"use client";

import { Loader2, RefreshCw, Rocket, RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useAuth } from "@/components/auth/auth-provider";
import { useRouterUpgrade } from "./use-router-upgrade";
import { UpgradeProgressPanel } from "./upgrade-progress-panel";
import { getVersion, restartService, rollbackService } from "@/lib/api";
import type { VersionResponse } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";

function formatUptime(secs: number) {
  if (!secs || secs < 0) return "--";
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const minutes = Math.floor((secs % 3600) / 60);
  return [days ? `${days}d` : "", hours || days ? `${hours}h` : "", `${minutes}m`].filter(Boolean).join(" ");
}

function formatBytes(bytes?: number | null) {
  if (!bytes) return "--";
  const mib = bytes / 1024 / 1024;
  return mib >= 1 ? `${mib.toFixed(1)} MiB` : `${(bytes / 1024).toFixed(1)} KiB`;
}

type ConfirmAction = "restart" | "upgrade" | "rollback" | "rollbackAgain";

export function VersionPanel({ isAdmin }: { isAdmin: boolean }) {
  const { t } = useLocaleText();
  const [info, setInfo] = React.useState<VersionResponse | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState<string | null>(null);
  const { session } = useAuth();
  const upgradeState = useRouterUpgrade(isAdmin, session?.user?.id || "", session?.user?.email || "");
  const serviceLocked = !!busy || upgradeState.locked;
  const [serviceNotice, setServiceNotice] = React.useState("");
  const completedTask = React.useRef("");
  const [confirmAction, setConfirmAction] = React.useState<ConfirmAction | null>(null);

  const refresh = React.useCallback(async () => {
    try {
      setInfo(await getVersion());
      setError("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  React.useEffect(() => {
    refresh().catch(console.error);
  }, [refresh]);

  async function restart() {
    setConfirmAction(null);
    setBusy("restart");
    setError("");
    try {
      await restartService();
      setServiceNotice(t("version.restartScheduledHealth"));
      pollHealthAndReload().catch(console.error);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(null);
    }
  }

  function upgrade() {
    setConfirmAction(null);
    setError("");
    upgradeState.start();
  }

  async function rollback() {
    setConfirmAction(null);
    setBusy("rollback");
    setError("");
    try {
      await rollbackService();
      setServiceNotice(t("version.restartScheduledHealth"));
      pollHealthAndReload().catch(console.error);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(null);
    }
  }

  function confirmCurrentAction() {
    if (confirmAction === "restart") {
      restart().catch(console.error);
      return;
    }
    if (confirmAction === "upgrade") {
      upgrade();
      return;
    }
    if (confirmAction === "rollback") {
      setConfirmAction("rollbackAgain");
      return;
    }
    if (confirmAction === "rollbackAgain") {
      rollback().catch(console.error);
    }
  }

  const confirmCopy = (() => {
    switch (confirmAction) {
      case "restart":
        return {
          title: t("common.restart"),
          description: t("version.confirmRestart"),
          confirmLabel: t("common.restart"),
          tone: "primary" as const,
        };
      case "upgrade":
        return {
          title: t("common.upgrade"),
          description: t("version.confirmUpgrade"),
          confirmLabel: t("common.upgrade"),
          tone: "primary" as const,
        };
      case "rollback":
        return {
          title: t("common.rollback"),
          description: t("version.confirmRollback"),
          confirmLabel: t("common.rollback"),
          tone: "warning" as const,
        };
      case "rollbackAgain":
        return {
          title: t("common.rollback"),
          description: t("version.confirmRollbackAgain"),
          confirmLabel: t("common.rollback"),
          tone: "warning" as const,
        };
      default:
        return null;
    }
  })();

  React.useEffect(() => {
    const task = upgradeState.task;
    if (task && !["success", "failed", "cancelled", "interrupted"].includes(task.status)) setConfirmAction(null);
    if (task?.status === "success" && task.restartConfirmed && completedTask.current !== task.taskId) {
      completedTask.current = task.taskId;
      void refresh();
    }
  }, [upgradeState.task, refresh]);

  return (
    <div className="grid min-w-0 gap-4">
      <Card className="rounded-lg">
        <Card.Header className="flex-row items-start justify-between gap-4 space-y-0">
          <div>
            <Card.Title>{t("settings.version")}</Card.Title>
            <Card.Description>{t("version.description")}</Card.Description>
          </div>
          <Button variant="outline" isIconOnly onClick={() => { void refresh(); upgradeState.retry(); }} aria-label={t("version.refresh")}>
            <RefreshCw className="h-4 w-4" />
          </Button>
        </Card.Header>
        <Card.Content className="grid gap-4">
          {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
          {serviceNotice ? <p role="status" className="text-sm text-muted-foreground">{serviceNotice}</p> : null}
          {upgradeState.unavailable && !upgradeState.task ? <Alert status="warning">
            <div><p>{t("version.upgradeStatusUnavailable")}</p>
              {upgradeState.statusError ? <p className="mt-1 text-xs [overflow-wrap:anywhere]">{upgradeState.statusError}</p> : null}
            </div>
          </Alert> : null}
          {upgradeState.actionError && !upgradeState.visible ? <Alert status="danger">{upgradeState.actionError}</Alert> : null}
          {upgradeState.task?.persistenceError && !upgradeState.visible ? <Alert status="danger">{t("version.upgradePersistenceFailed")}</Alert> : null}
          <div className="grid gap-3 sm:grid-cols-2">
            <Info label={t("version.current")} value={`${info?.version || "--"} (${info?.commit || "--"})`} />
            <Info label={t("version.built")} value={formatDateTime(info?.buildTime)} />
            <Info label={t("version.uptime")} value={formatUptime(info?.uptimeSecs || 0)} />
            <Info label={t("version.service")} value={<Chip color={info?.service.active ? "success" : "default"} size="sm" variant={info?.service.active ? "soft" : "tertiary"}>{info?.service.manager || "--"} / {info?.service.activeState || (info?.service.active ? "active" : "inactive")}</Chip>} />
            <Info label={t("version.latestBinary")} value={info?.latest.available ? t("version.available", { size: formatBytes(info.latest.contentLength) }) : info?.latest.error || t("version.unknown")} />
            <Info label={t("version.binaryPath")} value={isAdmin ? info?.binaryPath || "--" : t("version.adminOnly")} />
            <Info label={t("common.rollback")} value={isAdmin ? (info?.rollbackAvailable ? info.rollbackPath : t("version.rollbackUnavailable")) : t("version.adminOnly")} />
          </div>
          {isAdmin ? (
            <div className="flex flex-wrap gap-2">
              <Button variant="outline" onClick={() => setConfirmAction("restart")} isDisabled={serviceLocked}>
                {busy === "restart" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
                {t("common.restart")}
              </Button>
              <Button variant="primary" onClick={() => setConfirmAction("upgrade")} isDisabled={serviceLocked}>
                {upgradeState.active || upgradeState.creating ? <Loader2 className="h-4 w-4 animate-spin" /> : <Rocket className="h-4 w-4" />}
                {t("common.upgrade")}
              </Button>
              <Button variant="outline" onClick={() => setConfirmAction("rollback")} isDisabled={serviceLocked || !info?.rollbackAvailable} className="border-amber-300 text-amber-700 hover:bg-amber-50">
                {busy === "rollback" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
                {t("common.rollback")}
              </Button>
            </div>
          ) : null}
        </Card.Content>
        {confirmCopy ? (
          <ConfirmAlertDialog
            open={!!confirmAction}
            title={confirmCopy.title}
            description={confirmCopy.description}
            confirmLabel={confirmCopy.confirmLabel}
            cancelLabel={t("common.cancel")}
            tone={confirmCopy.tone}
            // Only this card's own request makes the dialog busy. A status
            // blip must not trap it; a real conflict disables Confirm alone.
            busy={!!busy}
            confirmDisabled={upgradeState.active || upgradeState.creating || !!upgradeState.task?.persistenceError}
            onConfirm={confirmCurrentAction}
            onOpenChange={(open) => !open && setConfirmAction(null)}
          />
        ) : null}
      </Card>
      {upgradeState.visible ? <UpgradeProgressPanel
        task={upgradeState.creating ? null : upgradeState.task}
        creating={upgradeState.creating}
        cancelling={upgradeState.cancelling}
        reconnecting={upgradeState.reconnecting}
        unavailable={upgradeState.unavailable}
        actionError={upgradeState.actionError}
        onCancel={upgradeState.cancel}
        onClose={upgradeState.dismiss}
        onRetry={upgradeState.retry}
      /> : null}
    </div>
  );
}

function Info({ label, value }: { label: string; value?: React.ReactNode }) {
  return (
    <Card className="rounded-lg border bg-muted/30 p-0 shadow-none">
      <Card.Content className="p-3">
        <div className="mono-label text-muted-foreground">{label}</div>
        <div className="mt-2 break-words text-sm font-medium">{value || "--"}</div>
      </Card.Content>
    </Card>
  );
}

async function pollHealthAndReload(maxAttempts = 60) {
  for (let i = 0; i < maxAttempts; i += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 1000));
    try {
      const res = await fetch("/v1/healthz", { cache: "no-store" });
      if (res.ok) {
        window.location.reload();
        return;
      }
    } catch {
      // service may be restarting
    }
  }
}
