import assert from "node:assert/strict";
import test from "node:test";

import {
  clientUpgradeStorageKey,
  clientUpgradeStorageKeyV2,
  IDLE_CLIENT_UPGRADE_STATE,
  isClientUpgradeActive,
  isClientUpgradeLocked,
  parsePersistedClientUpgradeState,
  persistedClientUpgradeState,
  readClientUpgradeSessionState,
  shouldClearClientUpgradeLatch,
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

test("unavailable tasks remain tracked but permit an explicit retry", () => {
  const running: ClientUpgradeState = {
    phase: "running", startedAt: 12, taskId: "task-1", statusUnavailable: true,
  };
  const recovering: ClientUpgradeState = {
    phase: "recovering", startedAt: 12, recoveryReason: "start", statusUnavailable: true,
  };
  for (const state of [running, recovering]) {
    assert.equal(isClientUpgradeActive(state), true, "keep polling the uncertain attempt");
    assert.deepEqual(persistedClientUpgradeState(state), state, "refresh must retain recovery");
    assert.equal(isClientUpgradeLocked(state), false, "a status outage must not disable retry");
    assert.equal(isClientUpgradeLocked({ ...state, statusUnavailable: false }), true);
    assert.equal(isClientUpgradeLocked({ ...state, retryBlocked: true }), true);
  }
  assert.equal(isClientUpgradeLocked({ phase: "starting", startedAt: 12 }), true);
  assert.equal(isClientUpgradeLocked({
    phase: "starting", startedAt: 12, statusUnavailable: true,
  }), true, "an in-flight POST must never permit another click");
  assert.equal(isClientUpgradeLocked(failed), false);
  assert.equal(isClientUpgradeLocked(IDLE_CLIENT_UPGRADE_STATE), false);
});

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
  assert.equal(isClientUpgradeLocked(recovered!), false, "a restored POST can be retried safely");
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
    shouldClearClientUpgradeLatch(failed, {
      observedCommitId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      tunnelOnline: true,
    }),
    false,
    "already-latest preflight failure must keep the diagnostic",
  );
  assert.equal(
    shouldClearClientUpgradeLatch(failed, {
      observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
      tunnelOnline: true,
    }),
    true,
  );
  assert.equal(
    shouldClearClientUpgradeLatch(failed, {
      observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
      tunnelOnline: false,
    }),
    false,
  );
  assert.equal(
    shouldClearClientUpgradeLatch(
      { ...failed, retryBlocked: true },
      {
        observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
        tunnelOnline: true,
      },
    ),
    false,
  );
  assert.equal(
    shouldClearClientUpgradeLatch(
      { ...failed, observedCommitId: undefined },
      {
        observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
        tunnelOnline: true,
      },
    ),
    false,
  );
  assert.equal(
    shouldClearClientUpgradeLatch(
      { ...failed, phase: "running" },
      {
        observedCommitId: "bead3d3fb28c879925d2b2316304fafb8be49e87",
        tunnelOnline: true,
      },
    ),
    false,
  );
});

test("a different online Client commit clears an unavailable attempt without unlocking confirmed work", () => {
  const baseline = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
  const observed = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
  const running: ClientUpgradeState = {
    phase: "running", startedAt: 12, taskId: "task-1", statusUnavailable: true,
    observedCommitId: baseline,
  };
  const recovering: ClientUpgradeState = {
    phase: "recovering", startedAt: 12, recoveryReason: "start", statusUnavailable: true,
    observedCommitId: baseline,
  };
  for (const state of [running, recovering]) {
    assert.equal(shouldClearClientUpgradeLatch(state, {
      observedCommitId: observed, tunnelOnline: true,
    }), true);
    assert.equal(shouldClearClientUpgradeLatch(state, {
      observedCommitId: baseline.slice(0, 12), tunnelOnline: true,
    }), false, "a matching abbreviated commit is not a new binary");
    assert.equal(shouldClearClientUpgradeLatch(state, {
      observedCommitId: observed, tunnelOnline: false,
    }), false);
    for (const invalid of [undefined, "unknown", "gpt5mini", "123", "g".repeat(40)]) {
      assert.equal(shouldClearClientUpgradeLatch(state, {
        observedCommitId: invalid, tunnelOnline: true,
      }), false);
      assert.equal(shouldClearClientUpgradeLatch({ ...state, observedCommitId: invalid }, {
        observedCommitId: observed, tunnelOnline: true,
      }), false);
    }
    assert.equal(shouldClearClientUpgradeLatch({ ...state, retryBlocked: true }, {
      observedCommitId: observed, tunnelOnline: true,
    }), false);
    assert.equal(shouldClearClientUpgradeLatch({ ...state, statusUnavailable: false }, {
      observedCommitId: observed, tunnelOnline: true,
    }), false, "keep following a confirmed active task through replacement checks");
  }
  assert.equal(shouldClearClientUpgradeLatch({
    phase: "starting", startedAt: 12, statusUnavailable: true, observedCommitId: baseline,
  }, { observedCommitId: observed, tunnelOnline: true }), false);
});
