"use client";

import { Button } from "@heroui/react";
import { ArrowDown, CheckCircle2, CircleStop, Loader2, WifiOff, XCircle } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";
import { isRouterUpgradeTerminal } from "@/lib/router-upgrade";
import type { RouterUpgradePhase, RouterUpgradeTask } from "@/lib/types";

const PHASE_KEYS: Record<RouterUpgradePhase, MessageKey> = {
  preparing: "version.upgradePreparing", downloading: "version.upgradeDownloading",
  validating: "version.upgradeValidating", hashing: "version.upgradeHashing",
  installing: "version.upgradeInstalling", restarting: "version.upgradeRestarting",
  verifying: "version.upgradeVerifying", complete: "version.upgradeSucceeded",
};
const LOG_TONES = {
  info: "text-foreground", progress: "text-muted-foreground", success: "text-[var(--color-upgrade-positive)]",
  warn: "text-[var(--color-upgrade-warning)]", error: "text-destructive",
};

function downloadSize(bytes: number) { return `${(bytes / 1048576).toFixed(1)} MiB`; }

export function UpgradeProgressPanel({ task, creating, cancelling, reconnecting, unavailable, actionError, onCancel, onClose, onRetry }: {
  task: RouterUpgradeTask | null;
  creating: boolean;
  cancelling: boolean;
  reconnecting: boolean;
  unavailable: boolean;
  actionError: string;
  onCancel: () => void;
  onClose: () => void;
  onRetry: () => void;
}) {
  const { t, locale } = useLocaleText();
  const titleId = React.useId();
  const descriptionId = React.useId();
  const scroller = React.useRef<HTMLDivElement>(null);
  const following = React.useRef(true);
  const [atBottom, setAtBottom] = React.useState(true);
  const terminal = !!task && isRouterUpgradeTerminal(task.status);
  const failed = task?.status === "failed" || task?.status === "interrupted";
  const stopping = cancelling || task?.status === "cancelling";
  const disconnected = !terminal && (reconnecting || unavailable);
  const overdue = !!task && !terminal && ["restarting", "verifying"].includes(task.phase)
    && Date.now() - Date.parse(task.updatedAt) > 300_000;

  React.useEffect(() => {
    following.current = true;
    setAtBottom(true);
  }, [task?.taskId]);
  React.useEffect(() => {
    if (following.current && scroller.current) scroller.current.scrollTop = scroller.current.scrollHeight;
  }, [task?.logs, task?.taskId]);

  const status = creating ? t("version.upgradePreparing")
    : task?.status === "success" ? t("version.upgradeSucceeded")
    : task?.status === "cancelled" ? t("version.upgradeCancelled")
    : task?.status === "interrupted" ? t("version.upgradeInterrupted")
    : failed ? t("version.upgradeFailedTitle")
    : stopping ? t("version.upgradeCancelling")
    : task?.status === "restarting" && task.phase === "restarting" ? t("version.upgradeRestarting")
    : disconnected ? t("version.upgradeReconnecting")
    : task ? t(PHASE_KEYS[task.phase] || "version.upgradeProgress") : t("version.waitingLogs");
  const hint = task?.persistenceError ? t("version.upgradePersistenceFailed")
    : failed ? t(task.binaryReplaced ? "version.upgradeFailedAfterInstall" : "version.upgradeFailedBeforeInstall")
    : task?.status === "cancelled" ? t("version.upgradeCancelledDesc")
    : overdue ? t("version.upgradeRecoveryUnconfirmed")
    : disconnected ? t("version.upgradeReconnectDesc")
    : !terminal && task && !task.canCancel && !stopping ? t("version.upgradeCannotCancel")
    : t("version.upgradePersists");
  const Icon = task?.status === "success" ? CheckCircle2 : failed ? XCircle
    : task?.status === "cancelled" ? CircleStop : disconnected ? WifiOff : Loader2;

  return (
    <section aria-labelledby={titleId} aria-describedby={descriptionId}
      className="upgrade-progress-panel grid min-w-0 grid-rows-[auto_minmax(0,1fr)_auto] overflow-hidden rounded-lg border border-border bg-card text-card-foreground">
      <div className="min-w-0 border-b border-border px-4 py-3">
        <div className="flex min-w-0 items-center gap-2">
          <Icon aria-hidden="true" className={`h-4 w-4 shrink-0 ${!terminal && !disconnected ? "animate-spin motion-reduce:animate-none" : ""} ${failed ? "text-destructive" : task?.status === "success" ? "text-[var(--color-upgrade-positive)]" : "text-muted-foreground"}`} />
          <h3 id={titleId} className="shrink-0 text-sm font-semibold">{t("version.upgradeProgress")}</h3>
          <span className="min-w-0 truncate text-sm text-muted-foreground" role="status" aria-live="polite">{status}</span>
        </div>
        <p id={descriptionId} className="mt-1 text-xs text-muted-foreground">{hint}</p>
        {task?.phase === "downloading" && !terminal && !stopping ? (
          <div className="mt-2 flex items-center gap-3">
            {task.progress != null ? <progress className="upgrade-download-progress min-w-0 flex-1" max={100} value={task.progress} aria-label={t("version.upgradeDownloading")} /> : null}
            <span className="whitespace-nowrap font-mono text-xs text-muted-foreground">
              {downloadSize(task.downloadedBytes)}{task.totalBytes ? ` / ${downloadSize(task.totalBytes)}` : ""}{task.progress != null ? ` · ${task.progress}%` : ""}
            </span>
          </div>
        ) : null}
      </div>
      <div ref={scroller} role="log" aria-live="off" aria-label={t("version.upgradeLog")} tabIndex={0}
        onScroll={() => {
          const element = scroller.current;
          if (!element) return;
          following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 40;
          setAtBottom(following.current);
        }}
        className="min-h-0 min-w-0 overflow-y-auto overscroll-contain bg-muted/25 px-4 py-3 font-mono text-xs leading-6 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring">
        {task?.logsTruncated ? <p className="text-muted-foreground">{t("version.upgradeLogsTruncated")}</p> : null}
        {task?.logs.length ? task.logs.map((line) => (
          <div key={line.seq} className="flex min-w-0 items-start gap-3">
            <time dateTime={line.at} className="shrink-0 text-muted-foreground">{new Date(line.at).toLocaleTimeString(locale)}</time>
            <p className={`min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere] ${LOG_TONES[line.level] || "text-foreground"}`}>
              <span className="mr-2 text-[0.6875rem] uppercase">{line.level}</span>{line.message}
            </p>
          </div>
        )) : <p className="text-muted-foreground">{t("version.waitingLogs")}</p>}
      </div>
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-2 border-t border-border px-3 py-2">
        {actionError ? <p role="alert" className="max-h-12 w-full overflow-y-auto text-xs text-destructive [overflow-wrap:anywhere]">{actionError}</p> : null}
        {!atBottom ? <Button size="sm" variant="ghost" className="mr-auto h-9 whitespace-nowrap" onClick={() => {
          following.current = true;
          setAtBottom(true);
          if (scroller.current) scroller.current.scrollTop = scroller.current.scrollHeight;
        }}><ArrowDown aria-hidden="true" className="h-4 w-4" />{t("version.upgradeLatestLogs")}</Button> : null}
        {unavailable && !terminal ? <Button size="sm" variant="outline" className="h-9 whitespace-nowrap" onClick={onRetry}>{t("version.upgradeReconnect")}</Button> : null}
        {terminal ? <Button size="sm" variant="outline" className="h-9 whitespace-nowrap" onClick={onClose}>{t("common.close")}</Button> : (
          <Button size="sm" variant="outline" className="h-9 whitespace-nowrap text-destructive" onClick={onCancel}
            isDisabled={creating || stopping || unavailable || !task?.canCancel}>
            {stopping ? <Loader2 aria-hidden="true" className="h-4 w-4 animate-spin motion-reduce:animate-none" /> : <CircleStop aria-hidden="true" className="h-4 w-4" />}
            {t(stopping ? "version.upgradeCancelling" : "version.upgradeCancel")}
          </Button>
        )}
      </div>
    </section>
  );
}
