import { ApiError, parseJson } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { authFetch } from "@/lib/auth";
import type { MarketWalletQuoteFunding } from "@/lib/types";

export type ListingAvailability =
  | "available" | "occupied" | "upgrade_required" | "offline" | "upgrading"
  | "switching" | "unavailable" | "locking" | "unlocking" | "unlisted";

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
  registeredAt?: string;
  availability?: ListingAvailability;
  exclusiveManagement?: boolean;
  ownerRental?: boolean;
  onlineRate24h: number | null;
  observedMinutes24h: number;
  observationCoverage24h: number;
};

export type ListingBlocker =
  | "already_listed" | "listing_changed" | "owner_unverified" | "inactive" | "upgrade_required"
  | "offline" | "share_in_use" | "workspace_busy" | "control_unavailable"
  | "egress_unverified" | "hardware_unavailable" | "provider_verification"
  | "email_verification" | "provider_paused" | "provider_conflict";

export type ListingPreflight = {
  ready: boolean;
  paidReady: boolean;
  paidBlocker?: ListingBlocker | null;
  blocker?: ListingBlocker | null;
  probeStatus?: "skipped" | "unreachable" | "completed";
  countryCode?: string | null;
  resource?: OnlineListing["hardware"];
  // Earlier Routers only returned these individual checks.
  checks: Record<"ownerVerified" | "activated" | "capable" | "shareClear" | "unlisted", boolean>;
  online: boolean;
  exclusiveManagement?: boolean;
  workspace: boolean;
  egress: boolean;
  hardware: boolean;
};

export function listingBlocker(check: ListingPreflight): ListingBlocker | null {
  if (check.blocker) return check.blocker;
  if (check.ready === true && typeof check.paidReady === "boolean") return null;
  if (!check.checks) return "control_unavailable";
  if (check.checks.unlisted === false) return "already_listed";
  if (check.checks.ownerVerified === false) return "owner_unverified";
  if (check.checks.activated === false) return "inactive";
  if (check.checks.capable === false) return "upgrade_required";
  if (check.checks.shareClear === false) return "share_in_use";
  if (check.online === false) return "offline";
  // A legacy response initializes the remote flags to false even when the
  // signed status never arrived. Workspace or hardware success confirms receipt.
  const receivedStatus = check.probeStatus === "completed" || typeof check.exclusiveManagement === "boolean"
    || check.workspace === true || check.hardware === true;
  if (!receivedStatus) return "control_unavailable";
  if (check.exclusiveManagement === false) return "upgrade_required";
  if (check.workspace === false) return "workspace_busy";
  if (check.hardware === false) return "hardware_unavailable";
  if (check.egress === false) return "egress_unverified";
  return "control_unavailable";
}

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
    selfRental?: boolean;
    listingPriceMinor?: number;
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

export function getOnlineListings(signal?: AbortSignal) {
  return onlineRequest<{ listings: OnlineListing[] }>("listings?includeAvailableHours24=false", "GET", undefined, signal);
}
