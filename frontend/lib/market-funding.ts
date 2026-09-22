import { ApiError } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { MarketFundingSummary, MarketRecurringFundingSummary } from "@/lib/types";

export type MarketTopupFunding = MarketFundingSummary | MarketRecurringFundingSummary;

export function isRecurringFunding(
  funding: MarketTopupFunding,
): funding is MarketRecurringFundingSummary {
  return "totalRequiredHoldMinor" in funding;
}

export function recurringFundingForRenewal(
  funding: MarketRecurringFundingSummary,
  autoRenew: boolean,
  renewalHoldMinor = funding.cyclePriceMinor,
): MarketRecurringFundingSummary {
  const renewal = autoRenew ? renewalHoldMinor : 0;
  const total = funding.initialHoldMinor + renewal;
  return {
    ...funding,
    renewalPolicy: autoRenew ? "automatic" : "manual",
    renewalHoldMinor: renewal,
    totalRequiredHoldMinor: total,
    requiredTopupMinor: Math.max(0, total - funding.prepaidAvailableMinor),
  };
}

/**
 * Project a legacy metered funding summary after calendar-month holds against
 * the same supplier account. Monthly contracts cannot use credit, so they must
 * consume prepaid funds before the metered reservation chooses prepaid/credit.
 */
export function marketFundingAfterRecurringHolds(
  funding: MarketFundingSummary,
  recurringHoldMinor: number,
): MarketFundingSummary {
  const requestedHold = Number.isSafeInteger(recurringHoldMinor)
    ? Math.max(0, recurringHoldMinor)
    : 0;
  const plannedHold = Math.min(funding.prepaidAvailableMinor, requestedHold);
  if (plannedHold === 0) return funding;

  const prepaidAvailableMinor = funding.prepaidAvailableMinor - plannedHold;
  const requiredCoverageMinor = Math.max(0, funding.requiredCoverageMinor);
  const prepaidCoverageMinor = Math.min(prepaidAvailableMinor, requiredCoverageMinor);
  const uncoveredAfterPrepaid = requiredCoverageMinor - prepaidCoverageMinor;
  const unlimitedCredit = funding.creditKind === "unlimited";
  const creditAvailableMinor = unlimitedCredit
    ? uncoveredAfterPrepaid
    : Math.max(0, funding.creditAvailableMinor ?? 0);
  const creditCoverageMinor = Math.min(uncoveredAfterPrepaid, creditAvailableMinor);
  const requiredTopupMinor = Math.max(
    0,
    uncoveredAfterPrepaid - creditCoverageMinor,
  );
  const recommendedTargetMinor = Math.max(
    0,
    funding.projectedDailyRateMinor * funding.recommendedCoverageDays,
  );
  const recommendedTopupMinor = Math.max(
    requiredTopupMinor,
    recommendedTargetMinor - prepaidAvailableMinor,
  );
  const prepaidRunwaySeconds = funding.projectedDailyRateMinor > 0
    ? Math.floor(
      prepaidAvailableMinor * 86_400 / funding.projectedDailyRateMinor,
    )
    : undefined;
  const estimatedRunwaySeconds = funding.projectedDailyRateMinor <= 0
    || unlimitedCredit
    ? undefined
    : Math.floor(
      (prepaidAvailableMinor + creditAvailableMinor) * 86_400
        / funding.projectedDailyRateMinor,
    );

  return {
    ...funding,
    prepaidHeldMinor: funding.prepaidHeldMinor + plannedHold,
    prepaidAvailableMinor,
    prepaidCoverageMinor,
    creditCoverageMinor,
    requiredTopupMinor,
    recommendedTopupMinor,
    prepaidRunwaySeconds,
    estimatedRunwaySeconds,
  };
}

export type MarketFundingConflict = {
  supplierUserId: string;
  pricingModel?: string;
  requiredTopupMinor: number;
  prepaidAvailableMinor?: number;
  creditAvailableMinor?: number;
};

export type MarketFundingDecisionState =
  | "ready"
  | "low_runway"
  | "uses_credit"
  | "shortfall";

const MARKET_FUNDING_LOW_RUNWAY_SECONDS = 86_400;

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
  const pricingModel = typeof reason.details?.pricingModel === "string"
    ? reason.details.pricingModel
    : undefined;
  if (requiredTopupMinor == null || !supplierUserId) return null;
  return {
    supplierUserId,
    ...(pricingModel ? { pricingModel } : {}),
    requiredTopupMinor,
    prepaidAvailableMinor: finiteNonNegative(reason.details?.prepaidAvailableMinor),
    creditAvailableMinor: finiteNonNegative(reason.details?.creditAvailableMinor),
  };
}

export function applyMarketFundingConflict(
  funding: MarketFundingSummary,
  conflict: MarketFundingConflict,
  recurringHoldMinor = 0,
) {
  if (
    conflict.supplierUserId !== funding.supplierUserId
    || conflict.pricingModel === "prepaid_calendar_month"
  ) {
    return funding;
  }
  return {
    ...funding,
    requiredTopupMinor: conflict.requiredTopupMinor,
    prepaidAvailableMinor: conflict.prepaidAvailableMinor == null
      ? funding.prepaidAvailableMinor
      : conflict.prepaidAvailableMinor + Math.max(0, recurringHoldMinor),
    creditAvailableMinor: conflict.creditAvailableMinor ?? funding.creditAvailableMinor,
  };
}

export function applyRecurringFundingConflict(
  funding: MarketRecurringFundingSummary,
  conflict: MarketFundingConflict,
) {
  if (
    conflict.supplierUserId !== funding.supplierUserId
    || conflict.pricingModel === "legacy_metered_daily"
  ) return funding;
  return {
    ...funding,
    requiredTopupMinor: conflict.requiredTopupMinor,
    prepaidAvailableMinor: conflict.prepaidAvailableMinor ?? funding.prepaidAvailableMinor,
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

export function defaultMarketFundingTopupMinor(funding: MarketTopupFunding) {
  if (isRecurringFunding(funding)) {
    const preferred = funding.requiredTopupMinor > 0
      ? funding.requiredTopupMinor
      : funding.cyclePriceMinor;
    return Math.min(preferred, MAX_MARKET_FUNDING_TOPUP_MINOR);
  }
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

export function marketFundingDecisionState(
  funding: MarketFundingSummary,
): MarketFundingDecisionState {
  if (funding.requiredTopupMinor > 0) return "shortfall";
  if (marketFundingUsesCredit(funding)) return "uses_credit";
  if (
    funding.estimatedRunwaySeconds != null
    && funding.estimatedRunwaySeconds <= MARKET_FUNDING_LOW_RUNWAY_SECONDS
  ) {
    return "low_runway";
  }
  return "ready";
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
