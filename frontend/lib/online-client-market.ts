import { parseJson } from "@/lib/api";
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
  events: { kind: string; at: number }[];
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

export async function onlineRequest<T>(
  path: string,
  method = "GET",
  body?: unknown,
  signal?: AbortSignal,
) {
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
