import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

export type MarketFinanceReadiness = {
  mode: "disabled" | "shadow" | "enabled";
  version: number;
  newOrdersPaused: boolean;
  newWithdrawalsPaused: boolean;
  newTopupsPaused: boolean;
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
  /** Balance already due to running services (renewals, metered budgets). */
  committedUnits?: string;
  version: number;
  mode: "disabled" | "shadow" | "enabled";
  canTopup: boolean;
  hasWalletActivity: boolean;
  earningsConversion: MarketEarningConversionCapacity;
};

export type MarketEarningConversionCapacity = {
  convertibleUnits: string;
  canConvert: boolean;
  conversionBlockers: string[];
};

export type MarketWalletPage<T> = { items: T[]; nextCursor: string | null };

export type MarketWalletTransaction = {
  id: string;
  sourceKind: string;
  action: string;
  deltaUnits: string;
  heldDeltaUnits: string;
  createdAt: string;
  transferId?: string | null;
  counterpartyEmail?: string | null;
  c2cOrderId?: string | null;
};

export type MarketTopupOrderMode = "cashier" | "direct";

export type MarketWalletTopup = {
  id: string;
  channelId: string;
  amountMinor: number;
  creditAmountMinor: number;
  platformFeeMinor: number;
  platformFeeRateBps: number;
  policyVersion: number;
  status: string;
  checkoutUrl: string | null;
  createdAt: string;
  autoCloseAt: string | null;
  buyerAbandonedAt: string | null;
  reviewReason: "terminal_payment_marker" | "paid_receipt_invalid" | "paid_terms_review" | null;
  buyerRecheckRequestedAt: string | null;
  buyerRecheckCompletedAt: string | null;
  buyerRecheckResult: "queued" | "retrying" | "pending" | "verifying" | "review_required" | "credited" | "expired" | "cancelled" | null;
  lastGatewayStatus: string | null;
  lastGatewayCheckedAt: string | null;
  /** Older API responses omit these; treat a missing mode as "cashier". */
  orderMode?: MarketTopupOrderMode;
  paymentTokenId?: string | null;
  hasPaymentInstructions?: boolean;
  localCloseReason?: "never_created" | null;
  paidAt?: string | null;
};

export type MarketTopupPayabilityReason = "payable" | "buyer_stopped" | "cancel_pending" | "order_changed"
  | "gateway_closed" | "payment_processing" | "gateway_unknown" | "payment_evidence"
  | "deadline_passed" | "checkout_unavailable";

/** In-site (direct mode) payment instructions. `payAddress` and
 * `payableAmount` are present only while `canPay` is true. */
export type MarketTopupPaymentInstructions = {
  gatewayStatus: string;
  canPay: boolean;
  payabilityReason: MarketTopupPayabilityReason | string;
  canCancel: boolean;
  chain: string | null;
  chainName: string | null;
  tokenId: string | null;
  tokenSymbol: string | null;
  minConfirmations: number | null;
  confirmations: number | null;
  payAddress?: string | null;
  payableAmount?: string | null;
  fiatAmountMinor: number;
  expiresAt: string | null;
  serverNow: string;
};

export type MarketTopupGatewayStatus = {
  gatewayStatus: string;
  canPay: boolean;
  payabilityReason?: MarketTopupPayabilityReason;
  canCancel: boolean;
  canAbandon: boolean;
  selectionDeadline: string | null;
  expiresAt: string | null;
  serverNow: string;
};

export const MARKET_PAYMENT_TOKEN_IDS = [
  "tron-trx", "tron-usdt", "ethereum-eth", "ethereum-usdt", "ethereum-usdc",
  "bsc-bnb", "bsc-usdt", "bsc-usdc", "polygon-usdc", "polygon-usdt0", "base-usdc",
  "arbitrum-usdc", "arbitrum-usdt0", "plasma-usdt0", "x-layer-usdt0",
  "solana-usdc", "solana-usdt", "aptos-usdc", "aptos-usdt",
] as const;

export type MarketPaymentTokenId = (typeof MARKET_PAYMENT_TOKEN_IDS)[number];

export type MarketPaymentChannelRollout = "off" | "canary" | "live";

export type MarketPaymentChannelSettings = {
  orderMode: MarketTopupOrderMode;
  allowedMethods: string[];
  minTopupMinor: number | null;
  maxTopupMinor: number | null;
  feeRateBps: number;
  fixedFeeMinor: number;
  version: number;
};

export type MarketPaymentChannelSettingsInput = {
  orderMode: MarketTopupOrderMode;
  allowedMethods: string[];
  minTopupMinor: number | null;
  maxTopupMinor: number | null;
  feeRateBps: number;
  fixedFeeMinor: number;
  expectedVersion: number;
};

export type MarketPaymentChannel = {
  id: string;
  displayName: string;
  enabled: boolean;
  rolloutState?: MarketPaymentChannelRollout;
  promotedAt?: string | null;
  settings?: MarketPaymentChannelSettings;
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

export type MarketWalletPaymentMethod = {
  tokenId: string;
  chain: string;
  chainName: string;
  symbol: string;
  minConfirmations: number;
};

export type MarketWalletChannel = Pick<MarketPaymentChannel, "id" | "displayName" | "gatewayKind"> & {
  orderMode?: MarketTopupOrderMode;
  methods?: MarketWalletPaymentMethod[];
  minTopupMinor?: number;
  maxTopupMinor?: number;
  /** Visible only to the Owner while the channel is in canary rollout. */
  canary?: boolean;
};

export type MarketProviderFinance = MarketEarningConversionCapacity & {
  isProvider: boolean;
  hasFinanceActivity: boolean;
  canWithdraw: boolean;
  summary: {
    grossEarnedUnits: string;
    commissionUnits: string;
    netEarnedUnits: string;
    pendingUnits: string;
    availableUnits: string;
    withdrawableUnits: string;
    withdrawalHeldUnits: string;
    paidUnits: string;
    convertedUnits: string;
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
  externalReference?: string | null;
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
  userEmail?: string | null;
  orderMode?: MarketTopupOrderMode;
  createLastErrorCode?: string | null;
  reconcileFailures?: number;
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

export async function getMarketWallet(signal?: AbortSignal) {
  return parseJson<MarketWallet>(
    await authFetch("/v1/market-wallet", { cache: "no-store", signal }),
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

/** Step 1 of an address change: emails a 6-digit code to the account. */
export async function requestMarketProviderDestinationChallenge(address: string) {
  return parseJson<{ challengeId: string; expiresInSeconds: number }>(
    await authFetch("/v1/market-provider-finance/destination/challenge", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ network: "tron", address }),
    }),
  );
}

/** Step 2 of an address change: confirm with the emailed code. */
export async function saveMarketProviderDestination(address: string, challengeId: string, code: string) {
  return parseJson<NonNullable<MarketProviderFinance["destination"]>>(
    await authFetch("/v1/market-provider-finance/destination", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        network: "tron", address, confirmation: "CHANGE_WITHDRAWAL_ADDRESS", challengeId, code,
      }),
    }),
  );
}

export async function cancelMarketProviderWithdrawal(id: string, expectedVersion: number) {
  return parseJson<MarketProviderWithdrawal>(
    await authFetch(`/v1/market-provider-finance/withdrawals/${encodeURIComponent(id)}/cancel`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedVersion }),
    }),
  );
}

export type MarketEarningHoldReason = "dispute" | "cleanup_uncertain" | "service_missing" | "manual" | "legacy_unknown";

export type MarketEarningHold = {
  reasonKind: MarketEarningHoldReason | string;
  reasonRef: string;
  providerUserId: string;
  lots: number;
  heldUnits: string;
  createdAt: string;
};

export type MarketProviderEarningLot = {
  id: string;
  amountUnits: string;
  grossUnits: string;
  commissionUnits: string;
  commissionReversedUnits: string;
  commissionRateBps: number;
  commissionPolicyVersion: number;
  reversedUnits: string;
  unlockedUnits: string;
  heldUnits: string;
  paidUnits: string;
  convertedUnits: string;
  fulfilledAt: string;
  unlockAt: string;
  disputed: boolean;
};

export async function getMarketProviderHolds() {
  return parseJson<MarketEarningHold[]>(
    await authFetch("/v1/market-provider-finance/holds", { cache: "no-store" }),
  );
}

export async function getMarketProviderEarnings(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketProviderEarningLot>>(
    await authFetch(`/v1/market-provider-finance/earnings${query}`, { cache: "no-store" }),
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

export async function getMarketFinanceTransfers(kind: "withdrawals") {
  return parseJson<MarketFinanceTransfer[]>(
    await authFetch(`/v1/admin/market-finance/${kind}`, { cache: "no-store" }),
  );
}

export async function transitionMarketFinanceTransfer(
  kind: "withdrawals",
  id: string,
  action: MarketFinanceTransferAction,
  expectedVersion: number,
  options: {
    externalReference?: string;
    reason?: string;
    /** mark_paid only: typed by the Owner, never prefilled. */
    actualAmountMinor?: number;
    /** mark_paid only: RFC 3339 timestamp of the on-chain transfer. */
    transferredAt?: string;
    networkFeeMinor?: number;
    /** Required reason code when the actual amount differs from the request. */
    varianceReason?: string;
  } = {},
) {
  return parseJson<MarketProviderWithdrawal>(
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

export async function getMarketWalletTopupPaymentInstructions(id: string) {
  return parseJson<MarketTopupPaymentInstructions>(
    await authFetch(`/v1/market-wallet/topups/${encodeURIComponent(id)}/payment-instructions`, { cache: "no-store" }),
  );
}

/** `tokenId` is required for a direct channel and must be omitted for a cashier one. */
export async function createMarketWalletTopup(channelId: string, amountMinor: number, idempotencyKey: string, quoteId: string, tokenId?: string) {
  return parseJson<MarketWalletTopup>(
    await authFetch("/v1/market-wallet/topups", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ channelId, amountMinor, quoteId, ...(tokenId ? { tokenId } : {}) }),
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
  newTopupsPaused?: boolean,
) {
  return parseJson<{ newOrdersPaused: boolean; newWithdrawalsPaused: boolean; newTopupsPaused?: boolean }>(
    await authFetch("/v1/admin/market-finance/pause", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(newTopupsPaused === undefined
        ? { newOrdersPaused, newWithdrawalsPaused, expectedVersion }
        : { newOrdersPaused, newWithdrawalsPaused, newTopupsPaused, expectedVersion }),
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


export async function updateMarketPaymentChannelSettings(id: string, input: MarketPaymentChannelSettingsInput) {
  return parseJson<MarketPaymentChannel>(await authFetch(channelUrl(id, "settings"), {
    method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
  }));
}

export async function rewrapMarketPaymentChannelSecrets() {
  return parseJson<{ rewrapped: number; keyVersion: number }>(
    await authFetch("/v1/admin/market-payment-channels/rewrap", { method: "POST" }),
  );
}

// ---------------------------------------------------------------------------
// Owner finance console

export const MARKET_CRITICAL_CASE_CODES = [
  "wallet_projection_mismatch", "provider_liability_mismatch", "period_unfulfilled_mismatch",
  "journal_unsealed", "credited_topup_missing_evidence", "paid_payout_missing_evidence",
  "wallet_transfer_missing_evidence", "wallet_transfer_orphan_journal",
  "commission_evidence_mismatch", "commission_orphan_journal",
  "earning_conversion_missing_evidence", "earning_conversion_allocation_mismatch", "earning_conversion_orphan_journal",
] as const;

export function isCriticalMarketFinanceCase(code: string) {
  return (MARKET_CRITICAL_CASE_CODES as readonly string[]).includes(code);
}

/** Owner audit reason codes: 3–64 characters of [A-Za-z0-9_-]. */
export function validMarketReasonCode(value: string) {
  return /^[A-Za-z0-9_-]{3,64}$/.test(value.trim());
}

export async function resolveMarketFinanceCase(id: string, reasonCode: string) {
  return parseJson<MarketFinanceCase>(await authFetch(
    `/v1/admin/market-finance/cases/${encodeURIComponent(id)}/resolve`, {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ reasonCode }),
    },
  ));
}

export type MarketFinanceOverview = {
  walletPostedUnits: string;
  walletHeldUnits: string;
  unfulfilledUnits: string;
  providerPendingUnits: string;
  providerAvailableUnits: string;
  providerHeldUnits: string;
  buyerRefundHeldUnits: string;
  ownerLossUnits: string;
  estimatedGatewayCostMinor: number;
  creditedTopupMinor: number;
  grossTopupMinor: number;
  maintenanceRevenueUnits: string;
  commissionPendingUnits: string;
  commissionRevenueUnits: string;
  commissionReversedUnits: string;
  earningConvertedUnits: string;
  openCases: number;
  openCriticalCases: number;
  heldEarningLots: number;
};

export async function getMarketFinanceOverview() {
  return parseJson<MarketFinanceOverview>(
    await authFetch("/v1/admin/market-finance/overview", { cache: "no-store" }),
  );
}

export type MarketFinanceUserLookup = {
  userId: string;
  email: string;
  wallet: { assetCode: string; postedUnits: string; heldUnits: string; availableUnits: string; version: number };
  provider: MarketProviderFinance["summary"];
  transactions: MarketWalletTransaction[];
  topups: MarketWalletTopup[];
  earningHolds: MarketEarningHold[];
};

export async function lookupMarketFinanceUser(email: string) {
  return parseJson<MarketFinanceUserLookup>(
    await authFetch(`/v1/admin/market-finance/users?email=${encodeURIComponent(email.trim())}`, { cache: "no-store" }),
  );
}

export async function getMarketFinanceEarningHolds() {
  return parseJson<MarketEarningHold[]>(
    await authFetch("/v1/admin/market-finance/earning-holds", { cache: "no-store" }),
  );
}

export async function placeMarketFinanceEarningHold(contractId: string, reasonCode: string) {
  return parseJson<{ held: boolean }>(await authFetch("/v1/admin/market-finance/earning-holds", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ contractId, reasonCode }),
  }));
}

export async function releaseMarketFinanceEarningHold(input: {
  reasonKind: "manual" | "legacy_unknown"; reasonRef: string; reasonCode: string;
}) {
  return parseJson<Record<string, unknown>>(await authFetch("/v1/admin/market-finance/earning-holds/release", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
  }));
}

export type MarketTopupManualCreditResult = {
  orderId: string;
  gatewayStatus: string;
  paidSource: string | null;
  amountMinor: number;
  creditMinor: number;
  feeMinor: number;
  tokenId: string;
  credited: boolean;
};

export async function manualCreditMarketFinanceTopup(id: string, input: {
  expectedUpdatedAt: string;
  txHash: string;
  tokenId?: string;
  evidenceReference: string;
  reasonCode: string;
  dryRun: boolean;
}) {
  return parseJson<MarketTopupManualCreditResult>(await authFetch(
    `/v1/admin/market-finance/topups/${encodeURIComponent(id)}/manual-credit`, {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
    },
  ));
}

export type MarketTopupReverseResult = {
  orderId: string;
  amountMinor: number;
  principalMinor: number;
  feeMinor: number;
  fromWalletMinor: number;
  shortfallMinor: number;
  reversed: boolean;
};

export async function reverseMarketFinanceTopup(id: string, input: {
  amountMinor: number;
  evidenceReference: string;
  reasonCode: string;
  dryRun: boolean;
  /** Required to commit: the shortfall the dry run showed. */
  expectedShortfallMinor?: number;
}) {
  return parseJson<MarketTopupReverseResult>(await authFetch(
    `/v1/admin/market-finance/topups/${encodeURIComponent(id)}/reverse`, {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
    },
  ));
}

// ---------------------------------------------------------------------------
// Buyer / Provider notifications

export const MARKET_NOTIFICATION_KINDS = [
  "topup_credited", "topup_closed_unpaid", "topup_review_required", "topup_reversed",
  "wallet_low_runway", "renewal_upcoming_shortfall", "contract_suspended", "contract_resumed",
  "contract_terminating_soon", "contract_terminated", "refund_status", "dispute_resolved",
  "payout_status", "earnings_held", "earnings_released", "earnings_converted", "payout_address_changed",
  "wallet_transfer_sent", "wallet_transfer_received",
  "commission_policy_changed",
] as const;

export type MarketNotificationKind = (typeof MARKET_NOTIFICATION_KINDS)[number];

export type MarketNotification = {
  id: string;
  audience: "buyer" | "provider" | string;
  eventKind: MarketNotificationKind | string;
  objectKind: string;
  objectId: string;
  payload: Record<string, unknown>;
  mandatory: boolean;
  readAt: string | null;
  createdAt: string;
};

export type MarketNotificationPreference = {
  eventKind: MarketNotificationKind | string;
  mandatory: boolean;
  enabled: boolean;
};

export async function getMarketNotifications(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketNotification> & { unread: number }>(
    await authFetch(`/v1/market-notifications${query}`, { cache: "no-store" }),
  );
}

/** Pass "all" to mark every notification read. */
export async function markMarketNotificationRead(id: string | "all") {
  return parseJson<{ ok: boolean }>(
    await authFetch(`/v1/market-notifications/${encodeURIComponent(id)}/read`, { method: "POST" }),
  );
}

export async function getMarketNotificationPreferences() {
  return parseJson<MarketNotificationPreference[]>(
    await authFetch("/v1/market-notifications/preferences", { cache: "no-store" }),
  );
}

export async function updateMarketNotificationPreferences(updates: { eventKind: string; enabled: boolean }[]) {
  return parseJson<MarketNotificationPreference[]>(
    await authFetch("/v1/market-notifications/preferences", {
      method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(updates),
    }),
  );
}
