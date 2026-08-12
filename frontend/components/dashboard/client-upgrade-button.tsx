"use client";

import { toast } from "@heroui/react";
import { CircleX, Clock3, Loader2, Rocket } from "lucide-react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { clientOwnerEmail, clientTunnelDisplayUrl } from "@/components/dashboard/data-tables";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  ApiError,
  type ClientInstallationUpgradeLog,
  getClientInstallationUpgradeStatus,
  upgradeClientInstallation,
} from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import type { DashboardClient } from "@/lib/types";

type ClientUpgradePhase = "idle" | "starting" | "recovering" | "running" | "failed";
type ClientUpgradeRecoveryReason = "discovery" | "start";

type ClientUpgradeState = {
  phase: ClientUpgradePhase;
  startedAt: number;
  taskId?: string;
  errorMessage?: string;
  recoveryReason?: ClientUpgradeRecoveryReason;
  statusUnavailable?: boolean;
};

const CLIENT_UPGRADE_START_TIMEOUT_MS = 35_000;
const CLIENT_UPGRADE_START_RECOVERY_TIMEOUT_MS = 60_000;
const CLIENT_UPGRADE_STATUS_REQUEST_TIMEOUT_MS = 10_000;
const CLIENT_UPGRADE_POLL_INTERVAL_MS = 2_000;
const CLIENT_UPGRADE_STATE_EVENT = "cc-switch-router-client-upgrade-state";
const IDLE_CLIENT_UPGRADE_STATE: ClientUpgradeState = { phase: "idle", startedAt: 0 };

function storageKey(installationId: string) {
  return `cc_switch_router_client_upgrade_v2:${installationId}`;
}

function isClientUpgradeActive(state: ClientUpgradeState) {
  return ["starting", "recovering", "running"].includes(state.phase);
}

function isStoredClientUpgradeState(value: unknown): value is ClientUpgradeState {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<ClientUpgradeState>;
  if (!["idle", "starting", "recovering", "running", "failed"].includes(candidate.phase || "")) {
    return false;
  }
  if (candidate.phase === "idle") return candidate.startedAt === 0;
  if (typeof candidate.startedAt !== "number" || !Number.isFinite(candidate.startedAt) || candidate.startedAt <= 0) {
    return false;
  }
  if (candidate.taskId != null && typeof candidate.taskId !== "string") return false;
  if (candidate.errorMessage != null && typeof candidate.errorMessage !== "string") return false;
  if (candidate.statusUnavailable != null && typeof candidate.statusUnavailable !== "boolean") return false;
  if (candidate.phase === "running" && !candidate.taskId?.trim()) return false;
  if (
    candidate.phase === "recovering"
    && candidate.recoveryReason !== "discovery"
    && candidate.recoveryReason !== "start"
  ) {
    return false;
  }
  return true;
}

function upgradeFailureMessage(logs: ClientInstallationUpgradeLog[]) {
  let message = "";
  for (let index = logs.length - 1; index >= 0; index -= 1) {
    const entry = logs[index];
    if (entry.level === "error" && entry.message.trim()) {
      message = entry.message.trim();
      break;
    }
  }
  if (!message) return undefined;
  return message.length > 800 ? `${message.slice(0, 797)}...` : message;
}

function readStoredState(installationId: string) {
  try {
    const parsed = JSON.parse(window.sessionStorage.getItem(storageKey(installationId)) || "null") as unknown;
    if (!isStoredClientUpgradeState(parsed)) return IDLE_CLIENT_UPGRADE_STATE;
    if (parsed.phase === "starting") {
      return {
        ...parsed,
        phase: "recovering" as const,
        recoveryReason: "start" as const,
        statusUnavailable: true,
      };
    }
    return parsed;
  } catch {
    return IDLE_CLIENT_UPGRADE_STATE;
  }
}

function writeStoredState(installationId: string, state: ClientUpgradeState) {
  try {
    if (state.phase === "idle") {
      window.sessionStorage.removeItem(storageKey(installationId));
    } else {
      window.sessionStorage.setItem(storageKey(installationId), JSON.stringify(state));
    }
  } catch {
    // In-memory state still prevents duplicate clicks when session storage is unavailable.
  }
  window.dispatchEvent(new CustomEvent(CLIENT_UPGRADE_STATE_EVENT, {
    detail: { installationId, state },
  }));
}

function UpgradeStateIcon({ state }: { state: ClientUpgradeState }) {
  if (state.statusUnavailable || state.phase === "recovering") {
    return <Clock3 className="h-3 w-3 shrink-0" />;
  }
  if (state.phase === "starting" || state.phase === "running") {
    return <Loader2 className="h-3 w-3 shrink-0 animate-spin" />;
  }
  if (state.phase === "failed") return <CircleX className="h-3 w-3 shrink-0" />;
  return <Rocket className="h-3 w-3 shrink-0" />;
}

export function ClientUpgradeButton({ client }: { client: DashboardClient }) {
  const { t } = useLocaleText();
  const [state, setState] = React.useState<ClientUpgradeState>(IDLE_CLIENT_UPGRADE_STATE);
  const [stateReady, setStateReady] = React.useState(false);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const startGuardRef = React.useRef(false);
  const installationId = client.installation.id;
  const upgradeTarget = client.clientTunnel?.subdomain || installationId.slice(0, 8);
  const sessionEmail = readAuthState().email?.trim().toLowerCase();
  const ownerEmail = clientOwnerEmail(client)?.trim().toLowerCase();
  const clientTunnel = client.clientTunnel;
  const tunnelUrl = clientTunnelDisplayUrl(clientTunnel?.tunnelUrl);
  const delegateEnabled = client.installation.upgrade?.delegateUpgradeToRouterOwner !== false;
  const upgradeCapable = client.installation.upgrade?.upgradeCapable;
  const canInspect = !!sessionEmail && !!ownerEmail && sessionEmail === ownerEmail;
  const canUpgrade = canInspect
    && !!tunnelUrl
    && clientTunnel?.enabled === true
    && clientTunnel.online
    && delegateEnabled
    && upgradeCapable !== false;

  const patchState = React.useCallback((
    updater: ClientUpgradeState | ((prev: ClientUpgradeState) => ClientUpgradeState),
  ) => {
    setState((prev) => {
      const next = typeof updater === "function" ? updater(prev) : updater;
      if (next === prev) return prev;
      writeStoredState(installationId, next);
      return next;
    });
  }, [installationId]);

  const resetUpgradeState = React.useCallback(() => {
    startGuardRef.current = false;
    patchState(IDLE_CLIENT_UPGRADE_STATE);
  }, [patchState]);

  const markUpgradeFailed = React.useCallback((errorMessage?: string) => {
    startGuardRef.current = false;
    patchState((prev) => ({
      phase: "failed",
      startedAt: prev.startedAt || Date.now(),
      taskId: prev.taskId,
      errorMessage,
    }));
    toast.danger(t("dashboard.clientUpgradeFailed", { target: upgradeTarget }), errorMessage ? {
      description: errorMessage,
    } : undefined);
  }, [patchState, t, upgradeTarget]);

  React.useEffect(() => {
    setStateReady(false);
    if (!canInspect) {
      setState(IDLE_CLIENT_UPGRADE_STATE);
      startGuardRef.current = false;
      setStateReady(true);
      return;
    }
    const stored = readStoredState(installationId);
    const initial = stored.phase === "idle"
      ? {
          phase: "recovering" as const,
          startedAt: Date.now(),
          recoveryReason: "discovery" as const,
        }
      : stored;
    setState(initial);
    startGuardRef.current = isClientUpgradeActive(initial);
    setStateReady(true);
  }, [canInspect, installationId]);

  React.useEffect(() => {
    const syncState = (event: Event) => {
      const detail = (event as CustomEvent<{ installationId?: unknown; state?: unknown }>).detail;
      if (
        canInspect
        && detail?.installationId === installationId
        && isStoredClientUpgradeState(detail.state)
      ) {
        setState(detail.state);
        startGuardRef.current = isClientUpgradeActive(detail.state);
      }
    };
    window.addEventListener(CLIENT_UPGRADE_STATE_EVENT, syncState);
    return () => window.removeEventListener(CLIENT_UPGRADE_STATE_EVENT, syncState);
  }, [canInspect, installationId]);

  const upgrading = isClientUpgradeActive(state);
  const locked = upgrading;

  React.useEffect(() => {
    const isRunning = state.phase === "running" && !!state.taskId;
    const isRecovering = state.phase === "recovering";
    if (!isRunning && !isRecovering) return;

    const taskId = isRunning ? state.taskId : undefined;
    const recoveryReason = state.recoveryReason;
    const startedAt = state.startedAt;
    let cancelled = false;
    let finished = false;
    let pollTimer: number | undefined;
    let requestController: AbortController | undefined;

    const finishSuccess = () => {
      if (cancelled || finished) return;
      finished = true;
      toast.success(t("dashboard.clientUpgradeSucceeded", { target: upgradeTarget }));
      resetUpgradeState();
    };

    const retry = () => {
      if (!cancelled && !finished) {
        pollTimer = window.setTimeout(() => void poll(), CLIENT_UPGRADE_POLL_INTERVAL_MS);
      }
    };

    const poll = async () => {
      requestController = new AbortController();
      const requestTimeout = window.setTimeout(
        () => requestController?.abort(),
        CLIENT_UPGRADE_STATUS_REQUEST_TIMEOUT_MS,
      );
      try {
        const result = await getClientInstallationUpgradeStatus(
          installationId,
          taskId,
          requestController.signal,
        );
        if (cancelled) return;
        if (result.status === "success") {
          if (recoveryReason === "discovery") {
            finished = true;
            resetUpgradeState();
          } else {
            finishSuccess();
          }
          return;
        }
        if (result.status === "failed") {
          if (recoveryReason === "discovery") {
            finished = true;
            resetUpgradeState();
          } else {
            finished = true;
            markUpgradeFailed(upgradeFailureMessage(result.logs));
          }
          return;
        }
        if (result.statusSync === "lost") {
          finished = true;
          markUpgradeFailed(t("dashboard.clientUpgradeStatusLost"));
          return;
        }
        patchState((prev) => ({
          phase: "running",
          startedAt: prev.startedAt,
          taskId: result.taskId,
          statusUnavailable: result.statusSync !== "reported",
          errorMessage: result.statusSync === "reported"
            ? undefined
            : t("dashboard.clientUpgradeStatusUnavailable"),
        }));
        if (isRecovering && recoveryReason === "start") {
          toast.success(t("dashboard.clientUpgradeStarted", { taskId: result.taskId }));
        }
      } catch (error) {
        if (cancelled) return;
        const notFound = error instanceof ApiError && error.status === 404;
        if (notFound && recoveryReason === "discovery") {
          finished = true;
          resetUpgradeState();
          return;
        }
        if (
          notFound
          && recoveryReason === "start"
          && Date.now() - startedAt >= CLIENT_UPGRADE_START_RECOVERY_TIMEOUT_MS
        ) {
          finished = true;
          markUpgradeFailed(t("dashboard.clientUpgradeStartUnconfirmed"));
          return;
        }
        if (notFound && isRunning) {
          finished = true;
          markUpgradeFailed(t("dashboard.clientUpgradeTaskMissing"));
          return;
        }
        const details = error instanceof Error ? error.message : String(error);
        patchState((prev) => ({
          ...prev,
          statusUnavailable: true,
          errorMessage: `${t("dashboard.clientUpgradeStatusUnavailable")}: ${details}`,
        }));
      } finally {
        window.clearTimeout(requestTimeout);
      }
      retry();
    };

    void poll();
    return () => {
      cancelled = true;
      requestController?.abort();
      if (pollTimer != null) window.clearTimeout(pollTimer);
    };
  }, [
    installationId,
    markUpgradeFailed,
    patchState,
    resetUpgradeState,
    state.phase,
    state.recoveryReason,
    state.startedAt,
    state.taskId,
    t,
    upgradeTarget,
  ]);

  if (!canUpgrade && state.phase === "idle") return null;

  let buttonLabel = t("dashboard.clientUpgrade");
  if (state.phase === "starting" || state.phase === "running") {
    buttonLabel = t("dashboard.clientUpgrading");
  }
  if (state.phase === "recovering") buttonLabel = t("dashboard.clientUpgradeRecovering");
  if (state.phase === "failed") buttonLabel = t("dashboard.clientUpgradeFailedButton");
  if (state.statusUnavailable) buttonLabel = t("dashboard.clientUpgradeStatusUnavailableButton");

  let buttonAriaLabel = buttonLabel;
  if (state.phase === "failed") {
    buttonAriaLabel = t("dashboard.clientUpgradeFailed", { target: upgradeTarget });
  }
  if (state.statusUnavailable) {
    buttonAriaLabel = t("dashboard.clientUpgradeStatusUnavailable");
  }

  let buttonTone = "border-violet-200 bg-violet-50 text-violet-700";
  if (state.phase === "idle") buttonTone += " hover:border-violet-300 hover:bg-violet-100";
  if (state.phase === "failed") {
    buttonTone = "border-rose-200 bg-rose-50 text-rose-700 hover:border-rose-300 hover:bg-rose-100";
  }
  if (state.statusUnavailable || state.phase === "recovering") {
    buttonTone = "border-amber-200 bg-amber-50 text-amber-700";
  }
  if (locked) buttonTone += " pointer-events-none";

  async function runUpgrade(startedAt: number) {
    const controller = new AbortController();
    const requestTimeout = window.setTimeout(() => controller.abort(), CLIENT_UPGRADE_START_TIMEOUT_MS);
    try {
      const result = await upgradeClientInstallation(installationId, true, controller.signal);
      patchState({ phase: "running", startedAt, taskId: result.taskId });
      toast.success(t("dashboard.clientUpgradeStarted", { taskId: result.taskId }));
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      const resultIsUncertain = controller.signal.aborted
        || !(error instanceof ApiError)
        || error.status >= 500;
      if (resultIsUncertain) {
        patchState({
          phase: "recovering",
          startedAt,
          recoveryReason: "start",
          statusUnavailable: true,
          errorMessage,
        });
        toast.warning(t("dashboard.clientUpgradeStartUncertain"));
      } else {
        markUpgradeFailed(errorMessage);
      }
    } finally {
      window.clearTimeout(requestTimeout);
    }
  }

  function beginUpgrade() {
    if (startGuardRef.current || locked || !canUpgrade) return;
    startGuardRef.current = true;
    const startedAt = Date.now();
    patchState({ phase: "starting", startedAt });
    setConfirmOpen(false);
    void runUpgrade(startedAt);
  }

  const buttonDisabled = !stateReady || locked || confirmOpen || !canUpgrade;

  return (
    <>
      <button
        type="button"
        data-no-row-drawer
        aria-label={buttonAriaLabel}
        aria-busy={upgrading || undefined}
        title={state.errorMessage}
        disabled={buttonDisabled}
        onClick={(event) => {
          event.stopPropagation();
          if (buttonDisabled) return;
          setConfirmOpen(true);
        }}
        className={`inline-flex h-6 shrink-0 items-center gap-1 rounded-full border px-2.5 text-[11px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:cursor-not-allowed disabled:opacity-65 ${buttonTone}`}
      >
        <UpgradeStateIcon state={state} />
        <span>{buttonLabel}</span>
      </button>
      <ConfirmAlertDialog
        open={confirmOpen}
        title={t("dashboard.clientUpgradeConfirmTitle")}
        description={t("dashboard.clientUpgradeConfirm", { target: upgradeTarget })}
        confirmLabel={t("common.upgrade")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={upgrading}
        onConfirm={beginUpgrade}
        onOpenChange={(open) => {
          if (!locked) setConfirmOpen(open);
        }}
      />
    </>
  );
}
