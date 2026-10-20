import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

export type MarketFinanceReadiness = {
  mode: "disabled" | "shadow" | "enabled";
  version: number;
  newOrdersPaused: boolean;
  newWithdrawalsPaused: boolean;
  codeIntegrated: boolean;
  legacyBlockers: { code: string; count: number }[];
  criticalCases: number;
  staleTopupReviews: number;
  verifiedLiveTopups: number;
  paymentChannelBlockers: { code: string; count: number }[];
  canEnable: boolean;
  webhookUrl?: string;
};

export type MarketWallet = {
  assetCode: "USD";
  postedUnits: string;
  heldUnits: string;
  availableUnits: string;
  version: number;
  mode: "disabled" | "shadow" | "enabled";
  canTopup: boolean;
  hasWalletActivity: boolean;
};

export type MarketWalletPage<T> = { items: T[]; nextCursor: string | null };

export type MarketWalletTransaction = {
  id: string;
  sourceKind: string;
  action: string;
  deltaUnits: string;
  heldDeltaUnits: string;
  createdAt: string;
};

export type MarketWalletTopup = {
  id: string;
  channelId: string;
  amountMinor: number;
  status: string;
  checkoutUrl: string | null;
  createdAt: string;
  autoCloseAt: string | null;
  buyerAbandonedAt: string | null;
  reviewReason: "terminal_payment_marker" | "paid_receipt_invalid" | null;
  buyerRecheckRequestedAt: string | null;
  buyerRecheckCompletedAt: string | null;
  buyerRecheckResult: "queued" | "retrying" | "pending" | "verifying" | "review_required" | "credited" | "expired" | "cancelled" | null;
  lastGatewayStatus: string | null;
  lastGatewayCheckedAt: string | null;
};

export type MarketTopupGatewayStatus = {
  gatewayStatus: string;
  canPay: boolean;
  canCancel: boolean;
  canAbandon: boolean;
  selectionDeadline: string | null;
  expiresAt: string | null;
  serverNow: string;
};

export type MarketPaymentChannel = {
  id: string;
  displayName: string;
  enabled: boolean;
  sortOrder: number;
  currentRevision: number;
  version: number;
  archivedAt: string | null;
  verified: boolean;
  webhookMode: "signed" | "poll_only";
  webhookVerifiedAt: string | null;
  webhookVerified: boolean;
  unresolvedTopups: number;
  verifiedLiveTopups: number;
  verifiedLiveTopupsCurrentRevision: number;
  gatewayKind: "dujiaopay";
};

export type MarketWalletChannel = Pick<MarketPaymentChannel, "id" | "displayName" | "gatewayKind">;

export type MarketProviderFinance = {
  isProvider: boolean;
  summary: {
    pendingUnits: string;
    availableUnits: string;
    withdrawableUnits: string;
    withdrawalHeldUnits: string;
    paidUnits: string;
  };
  destination: {
    id: string;
    version: number;
    assetCode: "USDT";
    network: "tron";
    address: string;
    usableAt: string;
  } | null;
};

export type MarketProviderWithdrawal = {
  id: string;
  amountUnits: string;
  status: string;
  network: "tron";
  address: string;
  version: number;
  createdAt: string;
  externalReference: string | null;
};

export type MarketWalletRenewal = {
  id: string;
  productKind: "share" | "client_host";
  billingModel: "calendar_month";
  cyclePriceMinor: number;
  renewalPolicy: "manual" | "automatic";
  status: string;
  periodEnd: string | null;
  trialSecondsRemaining: number;
};

export async function getMarketWalletRenewal(contractId: string) {
  return parseJson<MarketWalletRenewal>(
    await authFetch(`/v1/market-wallet/contracts/${encodeURIComponent(contractId)}/renewal`, { cache: "no-store" }),
  );
}

export async function updateMarketWalletRenewal(contractId: string, autoRenew: boolean, expectedPolicy: MarketWalletRenewal["renewalPolicy"]) {
  return parseJson<MarketWalletRenewal>(
    await authFetch(`/v1/market-wallet/contracts/${encodeURIComponent(contractId)}/renewal`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ autoRenew, expectedPolicy }),
    }),
  );
}

export type MarketBuyerRefund = {
  id: string;
  amountUnits: string;
  status: string;
  network: "tron";
  address: string;
  version: number;
  createdAt: string;
  externalReference: string | null;
};

export type MarketFinanceTransfer = {
  id: string;
  userId?: string;
  providerUserId?: string;
  amountUnits: string;
  status: string;
  version: number;
  createdAt: string;
  network: "tron";
  address: string;
  destinationVersion?: number;
};

export type MarketFinanceTransferAction =
  "approve" | "begin_transfer" | "mark_paid" | "reject" | "mark_failed" | "review";

export type MarketServiceDispute = {
  id: string;
  contractId: string;
  buyerUserId: string;
  providerUserId: string;
  reasonCode: string;
  status: "open" | "resolved";
  resolutionCode: string | null;
  refundedUnits: string;
  refundableUnits: string;
  version: number;
  createdAt: string;
  resolvedAt: string | null;
};

export type MarketFinanceCase = {
  id: string;
  code: string;
  objectKind: string;
  objectId: string;
  status: "open" | "resolved";
  detailCode: string | null;
  createdAt: string;
  resolvedAt: string | null;
};

export type MarketFinanceTopup = {
  id: string;
  userId: string;
  channelId: string;
  merchantOrderId: string;
  gatewayOrderId: string | null;
  amountMinor: number;
  status: string;
  createdAt: string;
  updatedAt: string;
  buyerAbandonedAt: string | null;
};

export type MarketFinanceTopupRecovery = {
  orderId: string;
  gatewayStatus: string;
  selectionDeadline: string | null;
  expiresAt: string | null;
  canRecover: boolean;
  canCancel: boolean;
  recovered: boolean;
  requiresManualReference: boolean;
  blockers: string[];
  reviewCaseCodes: string[];
};

export type MarketTopupGatewayObservation = {
  id: string;
  source: "webhook" | "reconcile" | "owner_review";
  gatewayStatus: string;
  evidenceFlags: string[];
  evidenceSha256: string;
  firstObservedAt: string;
  lastObservedAt: string;
  observationCount: number;
};

export type MarketPaymentChannelInput = {
  displayName: string;
  sortOrder: number;
  apiKeyId: string;
  apiSecret: string;
  webhookSecret?: string;
  webhookMode?: "signed" | "poll_only";
  allowedMethods: string[];
  expectedVersion?: number;
};

export async function getMarketWallet() {
  return parseJson<MarketWallet>(
    await authFetch("/v1/market-wallet", { cache: "no-store" }),
  );
}

export async function getMarketProviderFinance() {
  return parseJson<MarketProviderFinance>(
    await authFetch("/v1/market-provider-finance/me", { cache: "no-store" }),
  );
}

export async function getMarketProviderWithdrawals(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketProviderWithdrawal>>(
    await authFetch(`/v1/market-provider-finance/withdrawals${query}`, { cache: "no-store" }),
  );
}

export async function saveMarketProviderDestination(address: string) {
  return parseJson<NonNullable<MarketProviderFinance["destination"]>>(
    await authFetch("/v1/market-provider-finance/destination", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ network: "tron", address, confirmation: "CHANGE_WITHDRAWAL_ADDRESS" }),
    }),
  );
}

export async function requestMarketProviderWithdrawal(amountMinor: number, idempotencyKey: string) {
  return parseJson<MarketProviderWithdrawal>(
    await authFetch("/v1/market-provider-finance/withdrawals", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ amountMinor }),
    }),
  );
}

export async function getMarketBuyerRefunds(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketBuyerRefund>>(
    await authFetch(`/v1/market-wallet/refund-requests${query}`, { cache: "no-store" }),
  );
}

export async function requestMarketBuyerRefund(amountMinor: number, address: string, idempotencyKey: string) {
  return parseJson<MarketBuyerRefund>(
    await authFetch("/v1/market-wallet/refund-requests", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ amountMinor, network: "tron", address }),
    }),
  );
}

export async function getMarketFinanceTransfers(kind: "withdrawals" | "refunds") {
  return parseJson<MarketFinanceTransfer[]>(
    await authFetch(`/v1/admin/market-finance/${kind}`, { cache: "no-store" }),
  );
}

export async function transitionMarketFinanceTransfer(
  kind: "withdrawals" | "refunds",
  id: string,
  action: MarketFinanceTransferAction,
  expectedVersion: number,
  options: { externalReference?: string; reason?: string; actualAmountMinor?: number } = {},
) {
  return parseJson<MarketProviderWithdrawal | MarketBuyerRefund>(
    await authFetch(`/v1/admin/market-finance/${kind}/${encodeURIComponent(id)}/transition`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ action, expectedVersion, ...options }),
    }),
  );
}

export async function openMarketServiceDispute(contractId: string, reasonCode: string) {
  return parseJson<MarketServiceDispute>(await authFetch(
    `/v1/market-wallet/contracts/${encodeURIComponent(contractId)}/disputes`, {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ reasonCode }),
    },
  ));
}

export async function getMarketServiceDisputes() {
  return parseJson<MarketServiceDispute[]>(
    await authFetch("/v1/market-wallet/service-disputes", { cache: "no-store" }),
  );
}

export async function getAdminMarketServiceDisputes() {
  return parseJson<MarketServiceDispute[]>(
    await authFetch("/v1/admin/market-finance/service-disputes", { cache: "no-store" }),
  );
}

export async function resolveMarketServiceDispute(id: string, expectedVersion: number,
  resolutionCode: string, refundAmountMinor: number) {
  return parseJson<MarketServiceDispute>(await authFetch(
    `/v1/admin/market-finance/service-disputes/${encodeURIComponent(id)}/resolve`, {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedVersion, resolutionCode, refundAmountMinor }),
    },
  ));
}

export async function getMarketWalletTransactions(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketWalletTransaction>>(
    await authFetch(`/v1/market-wallet/transactions${query}`, { cache: "no-store" }),
  );
}

export async function getMarketWalletChannels() {
  return parseJson<MarketWalletChannel[]>(
    await authFetch("/v1/market-wallet/channels", { cache: "no-store" }),
  );
}

export async function getMarketWalletTopups(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketWalletTopup>>(
    await authFetch(`/v1/market-wallet/topups${query}`, { cache: "no-store" }),
  );
}

export async function getMarketWalletTopup(id: string) {
  return parseJson<MarketWalletTopup>(
    await authFetch(`/v1/market-wallet/topups/${encodeURIComponent(id)}`, { cache: "no-store" }),
  );
}

export async function getMarketWalletTopupGatewayStatus(id: string) {
  return parseJson<MarketTopupGatewayStatus>(
    await authFetch(`/v1/market-wallet/topups/${encodeURIComponent(id)}/gateway-status`, { cache: "no-store" }),
  );
}

export async function recheckMarketWalletTopup(id: string) {
  return parseJson<{ queued: boolean; order: MarketWalletTopup }>(
    await authFetch(`/v1/market-wallet/topups/${encodeURIComponent(id)}/recheck`, { method: "POST" }),
  );
}

export async function stopMarketWalletTopup(id: string) {
  return parseJson<MarketWalletTopup>(
    await authFetch(`/v1/market-wallet/topups/${encodeURIComponent(id)}/stop`, { method: "POST" }),
  );
}

export async function cancelMarketWalletTopup(id: string) {
  return parseJson<MarketWalletTopup>(
    await authFetch(`/v1/market-wallet/topups/${encodeURIComponent(id)}/cancel`, { method: "POST" }),
  );
}

export async function abandonMarketWalletTopup(id: string) {
  return parseJson<MarketWalletTopup>(
    await authFetch(`/v1/market-wallet/topups/${encodeURIComponent(id)}/abandon`, { method: "POST" }),
  );
}

export async function createMarketWalletTopup(channelId: string, amountMinor: number, idempotencyKey: string) {
  return parseJson<MarketWalletTopup>(
    await authFetch("/v1/market-wallet/topups", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ channelId, amountMinor }),
    }),
  );
}

export async function getMarketFinanceReadiness() {
  return parseJson<MarketFinanceReadiness>(
    await authFetch("/v1/admin/market-finance/readiness", { cache: "no-store" }),
  );
}

export async function getMarketPaymentChannels() {
  return parseJson<MarketPaymentChannel[]>(
    await authFetch("/v1/admin/market-payment-channels", { cache: "no-store" }),
  );
}

export async function getMarketFinanceCases(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketFinanceCase>>(
    await authFetch(`/v1/admin/market-finance/cases${query}`, { cache: "no-store" }),
  );
}

export async function getMarketFinanceTopups(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketFinanceTopup>>(
    await authFetch(`/v1/admin/market-finance/topups${query}`, { cache: "no-store" }),
  );
}

export async function getMarketFinanceTopupObservations(id: string, cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketTopupGatewayObservation>>(
    await authFetch(`/v1/admin/market-finance/topups/${encodeURIComponent(id)}/observations${query}`, { cache: "no-store" }),
  );
}

export async function recoverMarketFinanceTopup(
  id: string,
  input: {
    expectedStatus: string;
    expectedUpdatedAt: string;
    gatewayOrderId?: string;
    reasonCode: string;
    manualReference?: string;
    manualConfirmed?: boolean;
    dryRun: boolean;
  },
) {
  return parseJson<MarketFinanceTopupRecovery>(
    await authFetch(`/v1/admin/market-finance/topups/${encodeURIComponent(id)}/recover`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    }),
  );
}

export async function cancelMarketFinanceTopup(id: string) {
  return parseJson<MarketWalletTopup>(
    await authFetch(`/v1/admin/market-finance/topups/${encodeURIComponent(id)}/cancel`, { method: "POST" }),
  );
}

export async function setMarketFinanceMode(mode: MarketFinanceReadiness["mode"], expectedVersion: number) {
  return parseJson<{ mode: MarketFinanceReadiness["mode"] }>(
    await authFetch("/v1/admin/market-finance/mode", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ mode, expectedVersion }),
    }),
  );
}

export async function setMarketFinancePause(
  newOrdersPaused: boolean,
  newWithdrawalsPaused: boolean,
  expectedVersion: number,
) {
  return parseJson<{ newOrdersPaused: boolean; newWithdrawalsPaused: boolean }>(
    await authFetch("/v1/admin/market-finance/pause", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ newOrdersPaused, newWithdrawalsPaused, expectedVersion }),
    }),
  );
}

export async function createMarketPaymentChannel(input: MarketPaymentChannelInput) {
  return parseJson<MarketPaymentChannel>(
    await authFetch("/v1/admin/market-payment-channels", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    }),
  );
}

export async function setMarketPaymentChannelEnabled(id: string, enabled: boolean, expectedVersion: number) {
  return parseJson<{ enabled: boolean }>(
    await authFetch(`/v1/admin/market-payment-channels/${encodeURIComponent(id)}/enabled`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled, expectedVersion }),
    }),
  );
}

function channelUrl(id: string, action: string) {
  return `/v1/admin/market-payment-channels/${encodeURIComponent(id)}/${action}`;
}

export async function updateMarketPaymentChannelDetails(id: string, input: {
  displayName: string; sortOrder: number; expectedVersion: number;
}) {
  return parseJson<MarketPaymentChannel>(await authFetch(channelUrl(id, "details"), {
    method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
  }));
}

export async function verifyMarketPaymentChannelApi(id: string, expectedVersion: number) {
  return parseJson<MarketPaymentChannel>(await authFetch(channelUrl(id, "verify"), {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ expectedVersion }),
  }));
}

export async function rotateMarketPaymentChannelApi(id: string, input: {
  apiKeyId: string; apiSecret: string; expectedVersion: number;
}) {
  return parseJson<MarketPaymentChannel>(await authFetch(channelUrl(id, "api-credentials"), {
    method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
  }));
}

export async function rotateMarketPaymentChannelWebhook(id: string, input: {
  webhookMode: "signed" | "poll_only"; webhookSecret?: string;
  emergencyRevokePrevious?: boolean; expectedVersion: number;
}) {
  return parseJson<MarketPaymentChannel>(await authFetch(channelUrl(id, "webhook-secret"), {
    method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
  }));
}

export async function setMarketPaymentChannelArchived(id: string, archived: boolean, expectedVersion: number) {
  return parseJson<MarketPaymentChannel>(await authFetch(channelUrl(id, "archive"), {
    method: "PUT", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ archived, expectedVersion }),
  }));
}
