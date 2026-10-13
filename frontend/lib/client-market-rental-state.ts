import type { ClientMarketRental } from "./types";
import type { MessageKey } from "./i18n";

export function clientMarketCleanupReasonKey(kind?: string): MessageKey {
  if (kind === "cleanup_ssh_unreachable") return "clientMarket.release.reason.unreachable";
  if (kind === "cleanup_ssh_timeout") return "clientMarket.release.reason.timeout";
  if (kind === "cleanup_fingerprint_mismatch" || kind === "cleanup_host_binding_mismatch") {
    return "clientMarket.release.reason.safety";
  }
  if (kind === "cleanup_purge_failed") return "clientMarket.release.reason.purge";
  return "clientMarket.release.reason.remote";
}

/** Show ended rentals only while a quarantined Host still needs follow-up. */
export function clientMarketRentalHasBanner(rental?: ClientMarketRental | null): boolean {
  if (!rental || !rental.isClientOwner) return false;
  if (rental.status === "released") return rental.hostCleanupPending;
  if (rental.status === "releasing" || rental.status === "release_failed") return true;
  if (rental.pricingModel === "prepaid_calendar_month" || rental.recurring) return true;
  if (rental.pricingModel === "free" || (rental.dailyRateMinor == null && rental.cyclePriceMinor == null)) {
    if (!rental.activatedAt) return true;
    if (rental.expiresAt) return true;
    return false;
  }
  return true;
}
