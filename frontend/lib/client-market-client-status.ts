import { walletReleaseScheduled } from "./client-checkout";
import type { ClientMarketRental } from "@/lib/types";
import { clientMarketRentalNeedsFollowup } from "./client-market-rental-state";

export type MarketClientHint =
  | "unknown"
  | "free-no-expiry"
  | "long-free-no-expiry"
  | "free-timed"
  | "active"
  | "long-active"
  | "billing-suspended"
  | "released";

export type MarketClientContext = {
  hint: MarketClientHint | null;
  manageAs: "client" | "provider" | null;
};

const LONG_OFFLINE_MS = 24 * 60 * 60 * 1_000;

/** Keep the public origin badge independent of private rental lookup success. */
export function marketClientContext(
  provisionSource: string | undefined,
  rental: ClientMarketRental | undefined,
  offline: boolean,
  lastSeenAt: string,
  nowMs = Date.now(),
): MarketClientContext | null {
  if (provisionSource !== "router_market" && !rental) return null;

  const manageAs = !rental || (rental.status === "released" && !clientMarketRentalNeedsFollowup(rental))
    ? null
    : rental.isClientOwner ? "client" : "provider";
  if (!offline) return { hint: null, manageAs };

  const lastSeenMs = Date.parse(lastSeenAt);
  const longOffline = Number.isFinite(lastSeenMs)
    && nowMs >= lastSeenMs
    && nowMs - lastSeenMs >= LONG_OFFLINE_MS;

  if (!rental) return { hint: "unknown", manageAs };
  if (rental.status === "released") return { hint: "released", manageAs };
  if (rental.status === "releasing" || rental.status === "release_failed") {
    // The existing rental banner owns these progress and recovery states.
    return { hint: null, manageAs };
  }
  if (rental.status === "billing_suspended") return { hint: "billing-suspended", manageAs };
  if (walletReleaseScheduled(rental.walletContract, rental.pricingModel)) {
    // The existing rental banner shows the actual period-end date.
    return { hint: null, manageAs };
  }
  if (rental.status !== "active") return { hint: "unknown", manageAs };
  if (rental.pricingModel === "free") {
    if (rental.expiresAt || (rental.freeDurationDays != null && rental.freeDurationDays > 0)) {
      return { hint: "free-timed", manageAs };
    }
    return { hint: longOffline ? "long-free-no-expiry" : "free-no-expiry", manageAs };
  }
  return { hint: longOffline ? "long-active" : "active", manageAs };
}
