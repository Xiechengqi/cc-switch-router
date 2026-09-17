import assert from "node:assert/strict";
import test from "node:test";
import { binanceAutoSettlementUiState } from "./binance-auto-settlement";
import type { BinanceAutoSettlementStatus } from "./types";

function status(
  overrides: Partial<BinanceAutoSettlementStatus> = {},
): BinanceAutoSettlementStatus {
  return {
    globalMode: "enabled",
    credentialStorageConfigured: true,
    paymentHomeRegion: "region-a",
    ...overrides,
  };
}

function account(overrides: Partial<NonNullable<BinanceAutoSettlementStatus["account"]>> = {}) {
  return {
    binanceUid: "123456789",
    maskedApiKey: "abcd…wxyz",
    status: "verified",
    automationMode: "enabled",
    paymentHomeRegion: "region-a",
    uidConfirmed: true,
    consecutiveFailures: 0,
    credentialRevision: 1,
    createdAt: "2026-09-16T00:00:00Z",
    updatedAt: "2026-09-17T00:00:00Z",
    ...overrides,
  };
}

test("unavailable state collapses disabled and missing credential storage", () => {
  assert.equal(binanceAutoSettlementUiState(null), "unavailable");
  assert.equal(
    binanceAutoSettlementUiState(status({ credentialStorageConfigured: false })),
    "unavailable",
  );
  assert.equal(
    binanceAutoSettlementUiState(status({ globalMode: "disabled" })),
    "unavailable",
  );
  assert.equal(
    binanceAutoSettlementUiState(status({ globalMode: "future-mode" })),
    "unavailable",
  );
});

test("account failures take precedence over rollout state", () => {
  assert.equal(
    binanceAutoSettlementUiState(status({
      globalMode: "shadow",
      account: account({ status: "degraded" }),
    })),
    "degraded",
  );
  assert.equal(
    binanceAutoSettlementUiState(status({ account: account({ status: "disabled" }) })),
    "accountDisabled",
  );
});

test("enabled router distinguishes new, shadow, and active bindings", () => {
  assert.equal(binanceAutoSettlementUiState(status()), "ready");
  assert.equal(
    binanceAutoSettlementUiState(status({ account: account({ automationMode: "shadow" }) })),
    "actionRequired",
  );
  assert.equal(
    binanceAutoSettlementUiState(status({ account: account({ status: "pending" }) })),
    "actionRequired",
  );
  assert.equal(binanceAutoSettlementUiState(status({ account: account() })), "active");
});

test("shadow router is presented as a trial", () => {
  assert.equal(binanceAutoSettlementUiState(status({ globalMode: "shadow" })), "trial");
});
