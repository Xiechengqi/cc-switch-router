import assert from "node:assert/strict";
import test from "node:test";

import {
  clientUpgradeStorageKey,
  clientUpgradeStorageKeyV2,
  IDLE_CLIENT_UPGRADE_STATE,
  parsePersistedClientUpgradeState,
  persistedClientUpgradeState,
  readClientUpgradeSessionState,
  shouldClearFailedUpgradeLatch,
  upgradeCommitsMatch,
  writeClientUpgradeSessionState,
  type ClientUpgradeState,
} from "./client-upgrade-state";

const failed: ClientUpgradeState = {
  phase: "failed",
  startedAt: 1_700_000_000_000,
  taskId: "task-1",
  errorMessage: "[resource_preflight/resource_preflight_failed] cgroup memory headroom is 0 MiB",
  observedCommitId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
};

test("upgrade commits match full SHA against a 12-char prefix", () => {
  assert.equal(
    upgradeCommitsMatch(
      "bead3d3fb28c879925d2b2316304fafb8be49e87",
      "bead3d3fb28c",
    ),
    true,
  );
  assert.equal(upgradeCommitsMatch("gpt5mini", "gpt5mini"), false);
  assert.equal(upgradeCommitsMatch("aaaaaaa", "bbbbbbb"), false);
});

test("failed and idle upgrade states are not persisted across refresh", () => {
  assert.equal(persistedClientUpgradeState(failed), null);
  assert.equal(persistedClientUpgradeState(IDLE_CLIENT_UPGRADE_STATE), null);
  assert.deepEqual(
    persistedClientUpgradeState({
      phase: "running",
      startedAt: 1,
      taskId: "task-1",
      observedCommitId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    }),
    {
      phase: "running",
      startedAt: 1,
      taskId: "task-1",
      observedCommitId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    },
  );
});

test("stored starting is recovered as start-recovery; failed storage is ignored", () => {
  const recovered = parsePersistedClientUpgradeState({
    phase: "starting",
    startedAt: 12,
    observedCommitId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  });
  assert.equal(recovered?.phase, "recovering");
  assert.equal(recovered?.recoveryReason, "start");
  assert.equal(recovered?.statusUnavailable, true);
  assert.equal(parsePersistedClientUpgradeState(failed), null);
  assert.equal(parsePersistedClientUpgradeState(IDLE_CLIENT_UPGRADE_STATE), null);
});

test("legacy v2 failed latches are dropped on read; only in-flight v3 states persist", () => {
  const memory = new Map<string, string>();
  const storage = {
    getItem(key: string) {
      return memory.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      memory.set(key, value);
    },
    removeItem(key: string) {
      memory.delete(key);
    },
  };
  const installationId = "inst-1";
  storage.setItem(clientUpgradeStorageKeyV2(installationId), JSON.stringify(failed));
  assert.deepEqual(readClientUpgradeSessionState(installationId, storage), IDLE_CLIENT_UPGRADE_STATE);
  assert.equal(storage.getItem(clientUpgradeStorageKeyV2(installationId)), null);
  assert.equal(storage.getItem(clientUpgradeStorageKey(installationId)), null);

  const legacyRunning: ClientUpgradeState = {
    phase: "running",
    startedAt: 12,
    taskId: "task-legacy",
  };
  storage.setItem(clientUpgradeStorageKeyV2(installationId), JSON.stringify(legacyRunning));
  assert.deepEqual(readClientUpgradeSessionState(installationId, storage), legacyRunning);
  assert.equal(storage.getItem(clientUpgradeStorageKeyV2(installationId)), null);
  assert.equal(
    storage.getItem(clientUpgradeStorageKey(installationId)),
    JSON.stringify(legacyRunning),
  );

  writeClientUpgradeSessionState(installationId, failed, storage);
  assert.equal(storage.getItem(clientUpgradeStorageKey(installationId)), null);

  const running: ClientUpgradeState = {
    phase: "running",
    startedAt: 12,
    taskId: "task-1",
    observedCommitId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  };
  writeClientUpgradeSessionState(installationId, running, storage);
  assert.deepEqual(readClientUpgradeSessionState(installationId, storage), running);
});

test("same-tab failed latch clears only after a different live commit while online", () => {
  assert.equal(
    shouldClearFailedUpgradeLatch(failed, {
      observedCommitId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      tunnelOnline: true,
    }),
    false,
    "already-latest preflight failure must keep the diagnostic",
  );
  assert.equal(
    shouldClearFailedUpgradeLatch(failed, {
      observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
      tunnelOnline: true,
    }),
    true,
  );
  assert.equal(
    shouldClearFailedUpgradeLatch(failed, {
      observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
      tunnelOnline: false,
    }),
    false,
  );
  assert.equal(
    shouldClearFailedUpgradeLatch(
      { ...failed, retryBlocked: true },
      {
        observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
        tunnelOnline: true,
      },
    ),
    false,
  );
  assert.equal(
    shouldClearFailedUpgradeLatch(
      { ...failed, observedCommitId: undefined },
      {
        observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
        tunnelOnline: true,
      },
    ),
    false,
  );
  assert.equal(
    shouldClearFailedUpgradeLatch(
      { ...failed, phase: "running" },
      {
        observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
        tunnelOnline: true,
      },
    ),
    false,
  );
});
