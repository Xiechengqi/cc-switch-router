import assert from "node:assert/strict";
import test from "node:test";
import {
  binanceAutoSettlementUiState,
  binanceReceiptBuyerEmail,
  binanceReceiptStatus,
} from "./binance-auto-settlement";
import type {
  BinanceAutoSettlementStatus,
  BinanceReceiptHistoryEntry,
} from "./types";

function status(
  overrides: Partial<BinanceAutoSettlementStatus> = {},
): BinanceAutoSettlementStatus {
  return {
    globalMode: "enabled",
    credentialStorageConfigured: true,
    paymentHomeRegion: "region-a",
    serviceAvailability: "available",
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

test("region restriction blocks configuration before account state", () => {
  assert.equal(
    binanceAutoSettlementUiState(status({
      serviceAvailability: "region_restricted",
      account: account({ status: "degraded" }),
    })),
    "regionRestricted",
  );
});

test("enabled router distinguishes new, activatable, invalid, and active bindings", () => {
  assert.equal(binanceAutoSettlementUiState(status()), "ready");
  assert.equal(
    binanceAutoSettlementUiState(status({ account: account({ automationMode: "shadow" }) })),
    "activationRequired",
  );
  assert.equal(
    binanceAutoSettlementUiState(status({
      account: account({ automationMode: "shadow", status: "verifying" }),
    })),
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

test("receipt presentation handles invoice payments and prepaid top-ups", () => {
  const common = {
    receiptId: "receipt-1",
    transactionId: "transaction-1",
    transactionAt: "2026-09-20T12:00:00Z",
    confirmedAt: "2026-09-20T12:00:01Z",
    source: "binance_auto",
    matchedBy: "exact_amount",
    asset: "USDT",
    expectedAmount: "5.0037",
    actualAmount: "5.0037",
  };
  const invoice = {
    ...common,
    kind: "invoice_payment",
    paymentIntentId: "payment-intent-1",
    invoice: {
      id: "invoice-1",
      sequence: 7,
      status: "paid",
      paidAt: "2026-09-20T12:00:01Z",
      buyerEmail: "invoice-buyer@example.com",
      amountUsdMinor: 500,
      amountCnyMinor: 3500,
      lines: [],
    },
  } satisfies BinanceReceiptHistoryEntry;
  const topup = {
    ...common,
    receiptId: "receipt-2",
    kind: "prepaid_topup",
    fundingIntentId: "funding-intent-1",
    topup: {
      prepaidAccountId: "prepaid-1",
      buyerEmail: "topup-buyer@example.com",
      status: "credited",
      creditedAt: "2026-09-20T12:00:01Z",
      currency: "USD",
      creditedAmountMinor: 500,
    },
  } satisfies BinanceReceiptHistoryEntry;

  assert.equal(binanceReceiptBuyerEmail(invoice), "invoice-buyer@example.com");
  assert.equal(binanceReceiptStatus(invoice), "paid");
  assert.equal(binanceReceiptBuyerEmail(topup), "topup-buyer@example.com");
  assert.equal(binanceReceiptStatus(topup), "credited");
  assert.equal(topup.topup.creditedAmountMinor, 500);
});
