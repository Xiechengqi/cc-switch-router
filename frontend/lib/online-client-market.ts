import { ApiError, parseJson } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { authFetch } from "@/lib/auth";
import type { MarketWalletQuoteFunding } from "@/lib/types";

export type ListingAvailability =
  | "available" | "occupied" | "paused" | "upgrade_required" | "offline" | "upgrading"
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
  /** Last 30 UTC days; older Routers omit these. */
  onlineRate30d?: number | null;
  observedMinutes30d?: number;
  observedDays30d?: number;
  /** Seller's public title; the card falls back to `label`. */
  title?: string | null;
  sellerEmail?: string;
};

export type OnlineListingSyncStatus = {
  state: string;
  hostSyncState: "confirmed" | "pending" | "waiting_reconnect" | "after_rental";
  unlistedAt: number | null;
  commandAttempts: number;
  lastError: string | null;
  lastAttemptAt: number | null;
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
  cutoffAt?: number | null;
  releasedAt?: number | null;
  cyclePriceMinor: number;
  country: string | null;
  autoRenew: boolean;
  periodEnd: number | null;
  holdStatus: "active" | "captured" | "released" | null;
  events: { kind: string; at: number; count?: number }[];
  title?: string | null;
  /** Full egress IP, only while the rental runs. */
  egressIp?: string | null;
  reclaimEffectiveAt?: number | null;
  reclaimMode?: "notice" | "immediate" | null;
  reclaimReason?: ReclaimReason | null;
  contractId?: string | null;
  afterTrial?: "continue" | "end" | null;
  trialSecondsRemaining?: number | null;
  contractStatus?: string | null;
  paidStarted?: boolean;
  /** First Share created in this workspace, for the setup checklist. */
  shareUrl?: string | null;
  selfRental?: boolean;
  sellerEmail?: string;
  /** The seller's explanation, before and after the reclaim. */
  reclaimNote?: string | null;
  /** The rental ended because the seller reclaimed it. */
  reclaimed?: boolean;
  egressDiffers?: boolean;
};
export type ReclaimReason = "need_machine" | "tenant_violation" | "other";
export type OnlineSellerItem = {
  terminationId: string | null;
  id: string;
  listingId: string | null;
  label: string;
  state: string;
  cyclePriceMinor: number;
  tenantEmail: string | null;
  expiresAt: number | null;
  title?: string | null;
  paused?: boolean;
  reclaimEffectiveAt?: number | null;
  reclaimMode?: "notice" | "immediate" | null;
  reclaimable?: boolean;
  startedAt?: number | null;
  endedAt?: number | null;
  endReason?: string | null;
  /** Service time of the current period, as settlement recorded it. */
  availableSeconds?: number | null;
  unavailableSeconds?: number | null;
  settledThrough?: string | null;
  /** Earned so far (wallet units): rent net of the platform fee plus compensation. */
  incomeUnits?: string | null;
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
  ["CLIENT_TRIAL_ONLY_LIMIT", "clientMarket.checkout.trialOnlyLimit"],
  ["CLIENT_TRIAL_ONLY_UNAVAILABLE", "clientMarket.checkout.trialOnlyUnavailable"],
  ["CLIENT_TRIAL_ENDED", "clientMarket.checkout.trialEnded"],
  ["CLIENT_RECLAIM_PENDING", "clientMarket.reclaim.pending"],
  ["CLIENT_RECLAIM_EXPLANATION_REQUIRED", "clientMarket.reclaim.explanationRequired"],
  ["CLIENT_RENTAL_ENDING", "clientMarket.reclaim.ending"],
  ["CLIENT_LISTING_TITLE_INVALID", "clientMarket.listing.titleError"],
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

export type ListingTermsPayload = {
  cyclePriceMinor: number;
  freeDays: number | null;
  trialDays: number;
  title: string | null;
};

/** New terms apply to the next order; signed rentals keep theirs. */
export function updateOnlineListing(installationId: string, terms: ListingTermsPayload) {
  return onlineRequest<{ termsChanged: boolean }>(`installations/${encodeURIComponent(installationId)}/listing`, "PATCH", terms);
}

export function setOnlineListingPaused(installationId: string, paused: boolean) {
  return onlineRequest<{ paused: boolean }>(
    `installations/${encodeURIComponent(installationId)}/listing/${paused ? "pause" : "resume"}`,
    "POST",
  );
}

export type ReclaimRequest = {
  mode: "notice" | "immediate";
  reason: ReclaimReason;
  note: string;
  denyFutureAccess: boolean;
  unlistAfter: boolean;
};

export function reclaimOnlineLease(leaseId: string, request: ReclaimRequest) {
  return onlineRequest<{ leaseId: string; reclaimEffectiveAt: number; state: string }>(
    `leases/${encodeURIComponent(leaseId)}/reclaim`,
    "POST",
    request,
  );
}

export function cancelOnlineReclaim(leaseId: string) {
  return onlineRequest<{ cancelled: boolean; renewalRestored?: boolean }>(`leases/${encodeURIComponent(leaseId)}/reclaim/cancel`, "POST");
}

export function getCurrentOnlineLeases(signal?: AbortSignal) {
  return onlineRequest<{ leases: OnlineLease[] }>("leases?current=true", "GET", undefined, signal);
}

/** The public card name: the seller's title, else the neutral label. */
export function clientDisplayName(item: { title?: string | null; label: string }) {
  return item.title?.trim() || item.label;
}

/** Same rules as the Router: 1–30 characters, one line, no link or contact. */
export function validListingTitle(title: string) {
  const value = title.trim();
  if (!value) return true;
  // Same as the Router's `char::is_control`: C0, DEL and C1 controls.
  if ([...value].length > 30 || /\p{Cc}/u.test(value)) return false;
  const lower = value.toLowerCase();
  if (lower.includes("://") || lower.includes("www.") || lower.includes("@")) return false;
  return !value.split(/\s+/).some((token) => /^.+\.[a-z]{2,10}$/i.test(token));
}
