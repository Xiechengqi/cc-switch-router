"use client";

import { Loader2, RefreshCw, Rocket, RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip, Modal, ScrollShadow } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { readAuthState } from "@/lib/auth";
import { getVersion, restartService, rollbackService, startUpgrade } from "@/lib/api";
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
  const [upgradeOpen, setUpgradeOpen] = React.useState(false);
  const [logs, setLogs] = React.useState<string[]>([]);
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
      setLogs((prev) => [...prev, t("version.restartScheduledHealth")]);
      pollHealthAndReload().catch(console.error);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(null);
    }
  }

  async function upgrade() {
    setConfirmAction(null);
    setBusy("upgrade");
    setError("");
    setLogs([]);
    setUpgradeOpen(true);
    try {
      const { taskId } = await startUpgrade();
      streamUpgrade(taskId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(null);
    }
  }

  async function rollback() {
    setConfirmAction(null);
    setBusy("rollback");
    setError("");
    try {
      await rollbackService();
      setLogs((prev) => [...prev, t("version.restartScheduledHealth")]);
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
      upgrade().catch(console.error);
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

  function streamUpgrade(taskId: string) {
    const token = readAuthState().accessToken;
    const params = new URLSearchParams({ taskId });
    if (token) params.set("accessToken", token);
    const source = new EventSource(`/v1/admin/upgrade/stream?${params}`);
    source.addEventListener("log", (event) => {
      try {
        const data = JSON.parse((event as MessageEvent).data);
        setLogs((prev) => [...prev, `${data.ts || ""} ${data.level || "info"} ${data.message || ""}`.trim()]);
      } catch {
        setLogs((prev) => [...prev, (event as MessageEvent).data]);
      }
    });
    source.addEventListener("done", (event) => {
      setLogs((prev) => [...prev, `done ${(event as MessageEvent).data}`]);
      source.close();
      pollHealthAndReload().catch(console.error);
    });
    source.onerror = () => {
      setLogs((prev) => [...prev, t("version.streamDisconnected")]);
      source.close();
      setBusy(null);
    };
  }

  return (
    <Card className="rounded-lg">
      <Card.Header className="flex-row items-start justify-between gap-4 space-y-0">
        <div>
          <Card.Title>{t("settings.version")}</Card.Title>
          <Card.Description>{t("version.description")}</Card.Description>
        </div>
        <Button variant="outline" isIconOnly onClick={() => refresh()} aria-label={t("version.refresh")}>
          <RefreshCw className="h-4 w-4" />
        </Button>
      </Card.Header>
      <Card.Content className="grid gap-4">
        {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
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
            <Button variant="outline" onClick={() => setConfirmAction("restart")} isDisabled={!!busy}>
              {busy === "restart" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
              {t("common.restart")}
            </Button>
            <Button variant="primary" onClick={() => setConfirmAction("upgrade")} isDisabled={!!busy}>
              {busy === "upgrade" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Rocket className="h-4 w-4" />}
              {t("common.upgrade")}
            </Button>
            <Button variant="outline" onClick={() => setConfirmAction("rollback")} isDisabled={!!busy || !info?.rollbackAvailable} className="border-amber-300 text-amber-700 hover:bg-amber-50">
              {busy === "rollback" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
              {t("common.rollback")}
            </Button>
          </div>
        ) : null}
      </Card.Content>
      <Modal isOpen={upgradeOpen} onOpenChange={setUpgradeOpen}>
        <Modal.Backdrop>
          <Modal.Container placement="center" size="lg">
            <Modal.Dialog className="!bg-white !text-slate-900">
              <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Modal.Header>
                <div>
                  <Modal.Heading>{t("version.upgradeLog")}</Modal.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">{t("version.upgradeLogDesc")}</p>
                </div>
              </Modal.Header>
              <Modal.Body>
                <ScrollShadow className="h-96 rounded-lg border bg-slate-950 p-4 font-mono text-xs text-slate-100">
                  <div className="grid gap-2 pr-3">
                    {logs.length ? logs.map((line, index) => <div key={`${index}-${line}`}>{line}</div>) : <div>{t("version.waitingLogs")}</div>}
                  </div>
                </ScrollShadow>
              </Modal.Body>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>
      {confirmCopy ? (
        <ConfirmAlertDialog
          open={!!confirmAction}
          title={confirmCopy.title}
          description={confirmCopy.description}
          confirmLabel={confirmCopy.confirmLabel}
          cancelLabel={t("common.cancel")}
          tone={confirmCopy.tone}
          busy={!!busy}
          onConfirm={confirmCurrentAction}
          onOpenChange={(open) => !open && setConfirmAction(null)}
        />
      ) : null}
    </Card>
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
