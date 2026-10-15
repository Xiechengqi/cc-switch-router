import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

export type MarketFinanceReadiness = {
  mode: "disabled" | "shadow" | "enabled";
  version: number;
  codeIntegrated: boolean;
  legacyBlockers: { code: string; count: number }[];
  criticalCases: number;
  verifiedLiveTopups: number;
  canEnable: boolean;
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
};

export type MarketPaymentChannel = {
  id: string;
  displayName: string;
  enabled: boolean;
  sortOrder: number;
  currentRevision: number;
  verified: boolean;
  gatewayKind: "dujiaopay";
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

export type MarketPaymentChannelInput = {
  displayName: string;
  sortOrder: number;
  apiKeyId: string;
  apiSecret: string;
  webhookSecret: string;
  allowedMethods: string[];
};

export async function getMarketWallet() {
  return parseJson<MarketWallet>(
    await authFetch("/v1/market-wallet", { cache: "no-store" }),
  );
}

export async function getMarketWalletTransactions(cursor?: string) {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  return parseJson<MarketWalletPage<MarketWalletTransaction>>(
    await authFetch(`/v1/market-wallet/transactions${query}`, { cache: "no-store" }),
  );
}

export async function getMarketWalletChannels() {
  return parseJson<MarketPaymentChannel[]>(
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

export async function getMarketFinanceCases() {
  return parseJson<MarketFinanceCase[]>(
    await authFetch("/v1/admin/market-finance/cases", { cache: "no-store" }),
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

export async function createMarketPaymentChannel(input: MarketPaymentChannelInput) {
  return parseJson<MarketPaymentChannel>(
    await authFetch("/v1/admin/market-payment-channels", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    }),
  );
}

export async function setMarketPaymentChannelEnabled(id: string, enabled: boolean) {
  return parseJson<{ enabled: boolean }>(
    await authFetch(`/v1/admin/market-payment-channels/${encodeURIComponent(id)}/enabled`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled }),
    }),
  );
}
