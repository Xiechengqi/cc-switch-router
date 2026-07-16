"use client";

import { toast } from "@heroui/react";
import { CircleX, Clock3, Loader2, Rocket } from "lucide-react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { clientOwnerEmail, clientTunnelDisplayUrl } from "@/components/dashboard/data-tables";
import { clientOperationalSummary } from "@/components/dashboard/operational-status";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientInstallationUpgradeStatus, upgradeClientInstallation } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import type { DashboardClient } from "@/lib/types";

type ClientUpgradePhase = "idle" | "starting" | "running" | "failed" | "timeout";

type ClientUpgradeState = {
  phase: ClientUpgradePhase;
  startedAt: number;
  taskId?: string;
};

const CLIENT_UPGRADE_START_TIMEOUT_MS = 30_000;
const CLIENT_UPGRADE_STATUS_REQUEST_TIMEOUT_MS = 10_000;
const CLIENT_UPGRADE_TOTAL_TIMEOUT_MS = 6 * 60_000;
const CLIENT_UPGRADE_POLL_INTERVAL_MS = 2_000;
const CLIENT_UPGRADE_STATE_EVENT = "cc-switch-router-client-upgrade-state";
const IDLE_CLIENT_UPGRADE_STATE: ClientUpgradeState = { phase: "idle", startedAt: 0 };

function storageKey(installationId: string) {
  return `cc_switch_router_client_upgrade_v1:${installationId}`;
}

function isStoredClientUpgradeState(value: unknown): value is ClientUpgradeState {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<ClientUpgradeState>;
  if (!["starting", "running", "failed", "timeout"].includes(candidate.phase || "")) return false;
  if (typeof candidate.startedAt !== "number" || !Number.isFinite(candidate.startedAt) || candidate.startedAt <= 0) return false;
  if (candidate.taskId != null && typeof candidate.taskId !== "string") return false;
  return candidate.phase !== "running" || !!candidate.taskId?.trim();
}

function readStoredState(installationId: string) {
  try {
    const parsed = JSON.parse(window.sessionStorage.getItem(storageKey(installationId)) || "null") as unknown;
    if (!isStoredClientUpgradeState(parsed)) return IDLE_CLIENT_UPGRADE_STATE;
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

function UpgradeStateIcon({ phase }: { phase: ClientUpgradePhase }) {
  if (phase === "starting" || phase === "running") {
    return <Loader2 className="h-3 w-3 shrink-0 animate-spin" />;
  }
  if (phase === "failed") return <CircleX className="h-3 w-3 shrink-0" />;
  if (phase === "timeout") return <Clock3 className="h-3 w-3 shrink-0" />;
  return <Rocket className="h-3 w-3 shrink-0" />;
}

function isClientUpgradeRecoverable(client: DashboardClient) {
  const upgrade = client.installation.upgrade;
  if (upgrade?.delegateUpgradeToRouterOwner === false) return false;
  if (upgrade?.upgradeCapable === false) return false;

  const tunnel = client.clientTunnel;
  if (!tunnel?.tunnelUrl?.trim() || !tunnel.enabled || !tunnel.online) return false;

  const state = clientOperationalSummary(client).state;
  return state !== "offline" && state !== "disabled";
}

export function ClientUpgradeButton({ client }: { client: DashboardClient }) {
  const { t } = useLocaleText();
  const [state, setState] = React.useState<ClientUpgradeState>(IDLE_CLIENT_UPGRADE_STATE);
  const [stateReady, setStateReady] = React.useState(false);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const startGuardRef = React.useRef(false);
  const lastRecoverableRef = React.useRef<boolean | null>(null);
  const installationId = client.installation.id;
  const upgradeTarget = client.clientTunnel?.subdomain || installationId.slice(0, 8);

  const patchState = React.useCallback((
    updater: ClientUpgradeState | ((prev: ClientUpgradeState) => ClientUpgradeState),
  ) => {
    setState((prev) => {
      const next = typeof updater === "function" ? updater(prev) : updater;
      writeStoredState(installationId, next);
      return next;
    });
  }, [installationId]);

  const resetUpgradeState = React.useCallback(() => {
    startGuardRef.current = false;
    patchState(IDLE_CLIENT_UPGRADE_STATE);
  }, [patchState]);

  React.useEffect(() => {
    const stored = readStoredState(installationId);
    setState(stored);
    startGuardRef.current = stored.phase !== "idle";
    lastRecoverableRef.current = null;
    setStateReady(true);
  }, [installationId]);

  React.useEffect(() => {
    const syncState = (event: Event) => {
      const detail = (event as CustomEvent<{ installationId?: unknown; state?: unknown }>).detail;
      if (detail?.installationId === installationId && isStoredClientUpgradeState(detail.state)) {
        setState(detail.state);
        startGuardRef.current = detail.state.phase !== "idle";
      }
    };
    window.addEventListener(CLIENT_UPGRADE_STATE_EVENT, syncState);
    return () => window.removeEventListener(CLIENT_UPGRADE_STATE_EVENT, syncState);
  }, [installationId]);

  const upgrading = state.phase === "starting" || state.phase === "running";
  const locked = state.phase !== "idle";

  React.useEffect(() => {
    if (!stateReady) return;

    const recoverable = isClientUpgradeRecoverable(client);
    const previous = lastRecoverableRef.current;
    lastRecoverableRef.current = recoverable;

    if (state.phase !== "failed" && state.phase !== "timeout") return;
    if (!recoverable) return;
    if (previous !== null && previous) return;

    resetUpgradeState();
  }, [
    client,
    resetUpgradeState,
    state.phase,
    stateReady,
  ]);

  React.useEffect(() => {
    if (state.phase !== "starting") return;
    const startedAt = state.startedAt;
    const remaining = CLIENT_UPGRADE_START_TIMEOUT_MS - (Date.now() - startedAt);
    const markTimeout = () => {
      patchState((prev) => (prev.phase === "starting" ? { ...prev, phase: "timeout" } : prev));
      toast.warning(t("dashboard.clientUpgradeTimedOut", { target: upgradeTarget }));
    };
    if (remaining <= 0) {
      markTimeout();
      return;
    }
    const timer = window.setTimeout(markTimeout, remaining);
    return () => window.clearTimeout(timer);
  }, [patchState, state.phase, state.startedAt, t, upgradeTarget]);

  React.useEffect(() => {
    if (state.phase !== "running" || !state.taskId) return;
    const taskId = state.taskId;
    const startedAt = state.startedAt;
    let cancelled = false;
    let finished = false;
    let pollTimer: number | undefined;
    let requestController: AbortController | undefined;

    const finish = (phase: "success" | "failed" | "timeout") => {
      if (cancelled || finished) return;
      finished = true;
      if (phase === "success") {
        toast.success(t("dashboard.clientUpgradeSucceeded", { target: upgradeTarget }));
        resetUpgradeState();
        return;
      }
      patchState((prev) => ({ ...prev, phase }));
      if (phase === "failed") toast.danger(t("dashboard.clientUpgradeFailed", { target: upgradeTarget }));
      if (phase === "timeout") toast.warning(t("dashboard.clientUpgradeTimedOut", { target: upgradeTarget }));
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
        if (result.status === "success" || result.status === "failed") {
          finish(result.status);
          return;
        }
      } catch {
        if (cancelled) return;
      } finally {
        window.clearTimeout(requestTimeout);
      }

      if (Date.now() - startedAt >= CLIENT_UPGRADE_TOTAL_TIMEOUT_MS) {
        finish("timeout");
        return;
      }
      if (!cancelled && !finished) {
        pollTimer = window.setTimeout(() => void poll(), CLIENT_UPGRADE_POLL_INTERVAL_MS);
      }
    };

    void poll();
    return () => {
      cancelled = true;
      requestController?.abort();
      if (pollTimer != null) window.clearTimeout(pollTimer);
    };
  }, [installationId, patchState, resetUpgradeState, state.phase, state.startedAt, state.taskId, t, upgradeTarget]);

  const sessionEmail = readAuthState().email?.trim().toLowerCase();
  const ownerEmail = clientOwnerEmail(client)?.trim().toLowerCase();
  const tunnelUrl = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  const delegateEnabled = client.installation.upgrade?.delegateUpgradeToRouterOwner !== false;
  const upgradeCapable = client.installation.upgrade?.upgradeCapable;
  const canUpgrade =
    !!sessionEmail &&
    !!ownerEmail &&
    sessionEmail === ownerEmail &&
    !!tunnelUrl &&
    delegateEnabled &&
    upgradeCapable !== false;

  if (!canUpgrade && !locked) return null;

  let buttonLabel = t("dashboard.clientUpgrade");
  if (upgrading) buttonLabel = t("dashboard.clientUpgrading");
  if (state.phase === "failed") buttonLabel = t("dashboard.clientUpgradeFailed");
  if (state.phase === "timeout") buttonLabel = t("dashboard.clientUpgradeTimedOut");

  let buttonTone = "border-violet-200 bg-violet-50 text-violet-700";
  if (state.phase === "idle") buttonTone += " hover:border-violet-300 hover:bg-violet-100";
  if (state.phase === "failed") buttonTone = "border-rose-200 bg-rose-50 text-rose-700";
  if (state.phase === "timeout") buttonTone = "border-amber-200 bg-amber-50 text-amber-700";
  if (locked) buttonTone += " pointer-events-none";

  async function runUpgrade(startedAt: number) {
    const controller = new AbortController();
    const requestTimeout = window.setTimeout(() => controller.abort(), CLIENT_UPGRADE_START_TIMEOUT_MS);
    try {
      const result = await upgradeClientInstallation(installationId, true, controller.signal);
      patchState({ phase: "running", startedAt, taskId: result.taskId });
      toast.success(t("dashboard.clientUpgradeStarted", { taskId: result.taskId }));
    } catch (error) {
      if (controller.signal.aborted) {
        patchState({ phase: "timeout", startedAt });
        toast.warning(t("dashboard.clientUpgradeTimedOut", { target: upgradeTarget }));
      } else {
        patchState({ phase: "failed", startedAt });
        toast.danger(error instanceof Error ? error.message : String(error));
      }
    } finally {
      window.clearTimeout(requestTimeout);
    }
  }

  function beginUpgrade() {
    if (startGuardRef.current || locked) return;
    startGuardRef.current = true;
    const startedAt = Date.now();
    patchState({ phase: "starting", startedAt });
    setConfirmOpen(false);
    void runUpgrade(startedAt);
  }

  const buttonDisabled = !stateReady || locked || confirmOpen;

  return (
    <>
      <button
        type="button"
        data-no-row-drawer
        aria-label={buttonLabel}
        aria-busy={upgrading || undefined}
        disabled={buttonDisabled}
        onClick={(event) => {
          event.stopPropagation();
          if (buttonDisabled) return;
          setConfirmOpen(true);
        }}
        className={`inline-flex h-6 shrink-0 items-center gap-1 rounded-full border px-2.5 text-[11px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:cursor-not-allowed disabled:opacity-65 ${buttonTone}`}
      >
        <UpgradeStateIcon phase={state.phase} />
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
