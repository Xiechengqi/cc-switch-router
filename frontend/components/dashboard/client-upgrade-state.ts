export type ClientUpgradePhase = "idle" | "starting" | "recovering" | "running" | "failed";
export type ClientUpgradeRecoveryReason = "discovery" | "start";

export type ClientUpgradeState = {
  phase: ClientUpgradePhase;
  startedAt: number;
  taskId?: string;
  errorMessage?: string;
  recoveryReason?: ClientUpgradeRecoveryReason;
  statusUnavailable?: boolean;
  retryBlocked?: boolean;
  /** Commit observed when this attempt started; used to drop a same-tab failed latch. */
  observedCommitId?: string;
};

export type ClientUpgradeObservedFacts = {
  observedCommitId?: string | null;
  tunnelOnline: boolean;
};

export const CLIENT_UPGRADE_STORAGE_PREFIX_V2 = "cc_switch_router_client_upgrade_v2:";
export const CLIENT_UPGRADE_STORAGE_PREFIX = "cc_switch_router_client_upgrade_v3:";
export const IDLE_CLIENT_UPGRADE_STATE: ClientUpgradeState = { phase: "idle", startedAt: 0 };

export function clientUpgradeStorageKey(installationId: string) {
  return `${CLIENT_UPGRADE_STORAGE_PREFIX}${installationId}`;
}

export function clientUpgradeStorageKeyV2(installationId: string) {
  return `${CLIENT_UPGRADE_STORAGE_PREFIX_V2}${installationId}`;
}

export function isClientUpgradeActive(state: ClientUpgradeState) {
  return ["starting", "recovering", "running"].includes(state.phase);
}

export function isPersistedClientUpgradePhase(phase: ClientUpgradePhase) {
  return phase === "starting" || phase === "recovering" || phase === "running";
}

export function persistedClientUpgradeState(state: ClientUpgradeState): ClientUpgradeState | null {
  return isPersistedClientUpgradePhase(state.phase) ? state : null;
}

export function upgradeCommitsMatch(
  left: string | null | undefined,
  right: string | null | undefined,
): boolean {
  const leftNorm = left?.trim().toLowerCase() ?? "";
  const rightNorm = right?.trim().toLowerCase() ?? "";
  if (
    leftNorm.length < 7
    || leftNorm.length > 40
    || rightNorm.length < 7
    || rightNorm.length > 40
    || ![...leftNorm].every((char) => isAsciiHex(char))
    || ![...rightNorm].every((char) => isAsciiHex(char))
  ) {
    return false;
  }
  return leftNorm.startsWith(rightNorm) || rightNorm.startsWith(leftNorm);
}

function isAsciiHex(char: string) {
  return (char >= "0" && char <= "9") || (char >= "a" && char <= "f");
}

export function isClientUpgradeState(value: unknown): value is ClientUpgradeState {
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
  if (candidate.retryBlocked != null && typeof candidate.retryBlocked !== "boolean") return false;
  if (candidate.observedCommitId != null && typeof candidate.observedCommitId !== "string") return false;
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

export function parsePersistedClientUpgradeState(value: unknown): ClientUpgradeState | null {
  if (!isClientUpgradeState(value) || !isPersistedClientUpgradePhase(value.phase)) return null;
  if (value.phase === "starting") {
    return {
      ...value,
      phase: "recovering",
      recoveryReason: "start",
      statusUnavailable: true,
    };
  }
  return value;
}

export function readClientUpgradeSessionState(
  installationId: string,
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem">,
): ClientUpgradeState {
  try {
    const current = storage.getItem(clientUpgradeStorageKey(installationId));
    const legacy = storage.getItem(clientUpgradeStorageKeyV2(installationId));
    storage.removeItem(clientUpgradeStorageKeyV2(installationId));
    const parsed = parsePersistedClientUpgradeState(JSON.parse((current ?? legacy) || "null"));
    const state = parsed ?? IDLE_CLIENT_UPGRADE_STATE;
    if (legacy != null && parsed) {
      writeClientUpgradeSessionState(installationId, parsed, storage);
    }
    return state;
  } catch {
    storage.removeItem(clientUpgradeStorageKeyV2(installationId));
    return IDLE_CLIENT_UPGRADE_STATE;
  }
}

export function writeClientUpgradeSessionState(
  installationId: string,
  state: ClientUpgradeState,
  storage: Pick<Storage, "setItem" | "removeItem">,
) {
  storage.removeItem(clientUpgradeStorageKeyV2(installationId));
  const persisted = persistedClientUpgradeState(state);
  if (!persisted) {
    storage.removeItem(clientUpgradeStorageKey(installationId));
    return;
  }
  storage.setItem(clientUpgradeStorageKey(installationId), JSON.stringify(persisted));
}

/**
 * Drop a same-tab failed latch only when the live Client reports a different
 * commit than the attempt that failed, and the tunnel is online. Does not use
 * updateAvailable (already-latest + preflight fail would wipe the diagnostic).
 * Circuit-open retryBlocked stays until the operator leaves the tab.
 */
export function shouldClearFailedUpgradeLatch(
  state: ClientUpgradeState,
  facts: ClientUpgradeObservedFacts,
): boolean {
  if (state.phase !== "failed" || state.retryBlocked) return false;
  if (!facts.tunnelOnline) return false;
  const baseline = state.observedCommitId?.trim();
  const observed = facts.observedCommitId?.trim();
  if (!baseline || !observed) return false;
  return !upgradeCommitsMatch(baseline, observed);
}
