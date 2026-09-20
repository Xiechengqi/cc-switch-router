import { ApiError } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { MarketFundingSummary } from "@/lib/types";

export type MarketFundingConflict = {
  supplierUserId: string;
  requiredTopupMinor: number;
  prepaidAvailableMinor?: number;
  creditAvailableMinor?: number;
};

export const MAX_MARKET_FUNDING_TOPUP_MINOR = 100_000_000;

function finiteNonNegative(value: unknown) {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : undefined;
}

export function marketFundingConflictFromError(reason: unknown): MarketFundingConflict | null {
  if (!(reason instanceof ApiError) || reason.code !== "MARKET_PREPAID_REQUIRED") return null;
  const requiredTopupMinor = finiteNonNegative(reason.details?.requiredTopupMinor);
  const supplierUserId = typeof reason.details?.supplierUserId === "string"
    ? reason.details.supplierUserId
    : "";
  if (requiredTopupMinor == null || !supplierUserId) return null;
  return {
    supplierUserId,
    requiredTopupMinor,
    prepaidAvailableMinor: finiteNonNegative(reason.details?.prepaidAvailableMinor),
    creditAvailableMinor: finiteNonNegative(reason.details?.creditAvailableMinor),
  };
}

export function applyMarketFundingConflict(
  funding: MarketFundingSummary,
  conflict: MarketFundingConflict,
) {
  if (conflict.supplierUserId !== funding.supplierUserId) {
    return funding;
  }
  return {
    ...funding,
    requiredTopupMinor: conflict.requiredTopupMinor,
    prepaidAvailableMinor: conflict.prepaidAvailableMinor ?? funding.prepaidAvailableMinor,
    creditAvailableMinor: conflict.creditAvailableMinor ?? funding.creditAvailableMinor,
  };
}

const TOPUP_UNAVAILABLE_KEYS: Record<string, MessageKey> = {
  router_disabled: "marketFunding.topup.unavailable.routerDisabled",
  router_shadow: "marketFunding.topup.unavailable.routerShadow",
  region_restricted: "marketFunding.topup.unavailable.regionRestricted",
  temporarily_unavailable: "marketFunding.topup.unavailable.temporary",
  credential_storage_unavailable: "marketFunding.topup.unavailable.credentialStorage",
  supplier_unavailable: "marketFunding.topup.unavailable.supplier",
  settlement_required: "marketFunding.topup.unavailable.settlementRequired",
  relationship_closed: "marketFunding.topup.unavailable.relationshipClosed",
};

export function marketFundingTopupUnavailableKey(reason?: string): MessageKey {
  return reason
    ? TOPUP_UNAVAILABLE_KEYS[reason] || "marketFunding.topup.unavailable"
    : "marketFunding.topup.unavailable";
}

export function marketFundingTopupErrorKey(reason: unknown): MessageKey | undefined {
  if (!(reason instanceof ApiError)) return undefined;
  switch (reason.code) {
    case "MARKET_SETTLEMENT_REQUIRED":
      return marketFundingTopupUnavailableKey("settlement_required");
    case "MARKET_RELATIONSHIP_CLOSED":
      return marketFundingTopupUnavailableKey("relationship_closed");
    case "BINANCE_REGION_RESTRICTED":
      return marketFundingTopupUnavailableKey("region_restricted");
    case "BINANCE_TEMPORARILY_UNAVAILABLE":
      return marketFundingTopupUnavailableKey("temporarily_unavailable");
    default:
      return undefined;
  }
}

export function defaultMarketFundingTopupMinor(funding: MarketFundingSummary) {
  const quotedTarget = Math.max(
    funding.recommendedTopupMinor,
    funding.requiredTopupMinor,
  );
  const preferred = quotedTarget > 0
    ? quotedTarget
    : funding.projectedDailyRateMinor > 0
      ? funding.projectedDailyRateMinor
      : 1_000;
  return Math.min(preferred, MAX_MARKET_FUNDING_TOPUP_MINOR);
}

export function marketFundingUsesCredit(funding: MarketFundingSummary) {
  return funding.creditCoverageMinor > 0;
}

export function formatFundingRunway(
  seconds: number | undefined,
  locale: string,
  unlimitedLabel: string,
  unavailableLabel = "-",
) {
  if (seconds == null) return unlimitedLabel;
  if (!Number.isFinite(seconds) || seconds < 0) return unavailableLabel;
  if (seconds < 3_600) {
    const minutes = Math.max(0, Math.floor(seconds / 60));
    return new Intl.NumberFormat(locale, { style: "unit", unit: "minute", unitDisplay: "short" }).format(minutes);
  }
  if (seconds < 86_400) {
    const hours = Math.floor(seconds / 3_600);
    return new Intl.NumberFormat(locale, { style: "unit", unit: "hour", unitDisplay: "short" }).format(hours);
  }
  const days = seconds / 86_400;
  return new Intl.NumberFormat(locale, {
    maximumFractionDigits: days < 10 ? 1 : 0,
    style: "unit",
    unit: "day",
    unitDisplay: "short",
  }).format(days);
}
