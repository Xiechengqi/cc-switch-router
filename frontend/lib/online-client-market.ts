import { ApiError, parseJson } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { authFetch } from "@/lib/auth";
import type { MarketWalletQuoteFunding } from "@/lib/types";

export type OnlineListing = {
  installationId: string;
  providerId: string | null;
  label: string;
  revision: number;
  state: string;
  owned: boolean;
  cyclePriceMinor: number;
  freeDays: number | null;
  trialDays: number;
  country: string | null;
  egressIp: string | null;
  egressDiffers: boolean;
  hardware?: {
    arch: string;
    cpu: number | null;
    memoryBytes: number | null;
    diskBytes: number | null;
  };
  clientUrl: string | null;
  lastError: string | null;
  attempts: number | null;
  healthy: boolean;
  occupied: boolean;
  observedAt: number | null;
  availableHours24: number;
};
export type OnlineLease = {
  leaseId: string;
  terminationId: string | null;
  installationId: string;
  alias: string;
  clientUrl: string;
  state: string;
  expiresAt: number;
  createdAt: number;
  activatedAt: number | null;
  cyclePriceMinor: number;
  country: string | null;
  autoRenew: boolean;
  periodEnd: number | null;
  holdStatus: "active" | "captured" | "released" | null;
  events: { kind: string; at: number; count?: number }[];
};
export type OnlineSellerItem = {
  terminationId: string | null;
  id: string;
  listingId: string | null;
  label: string;
  state: string;
  cyclePriceMinor: number;
  tenantEmail: string | null;
  expiresAt: number | null;
};

export type OnlineQuote = {
  id: string;
  walletFunding: MarketWalletQuoteFunding | null;
  terms: {
    cyclePriceMinor: number;
    autoRenew: boolean;
    trialSeconds: number;
    freeDays: number | null;
    terminationPolicy: { version: number; rateBps: number };
  };
  expiresAt: number;
};

const workspaceErrorMessages = new Map<string, MessageKey>([
  ["workspace_leased", "lease.locked"],
  ["unlist_first", "lease.unlistFirst"],
  ["owner_proof_required", "lease.ownerProof"],
  ["tenant_session_required", "lease.tenantLogin"],
  ["lease_ended", "lease.ended"],
  ["lease_starting", "lease.awaiting"],
  ["MARKET_ACCESS_REQUIRED", "clientMarket.shop.accessRequired"],
]);

export function onlineMarketError(error: unknown, t: (key: MessageKey) => string) {
  if (error instanceof ApiError) {
    const known = workspaceErrorMessages.get(error.code || "");
    if (known) return t(known);
    if (error.status === 401) return t("lease.tenantLogin");
    if (error.status === 409) return t("lease.offerChanged");
    if (error.status >= 500) return t("lease.controlUnavailable");
  }
  return error instanceof Error ? error.message : String(error);
}

export async function onlineRequest<T>(path: string, method = "GET", body?: unknown, signal?: AbortSignal) {
  const response = await authFetch(`/v1/client-market/online/${path}`, {
    method,
    cache: "no-store",
    signal,
    ...(body === undefined
      ? {}
      : {
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }),
  });
  return parseJson<T>(response);
}