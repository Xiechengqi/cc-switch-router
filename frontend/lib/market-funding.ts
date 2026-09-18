import { ApiError } from "@/lib/api";
import type { MarketFundingSummary } from "@/lib/types";

export type MarketFundingConflict = {
  supplierUserId: string;
  requiredTopupMinor: number;
  prepaidAvailableMinor?: number;
  creditAvailableMinor?: number;
};

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
