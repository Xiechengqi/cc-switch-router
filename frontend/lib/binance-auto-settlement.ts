import type { BinanceAutoSettlementStatus } from "@/lib/types";

export type BinanceAutoSettlementUiState =
  | "unavailable"
  | "regionRestricted"
  | "trial"
  | "ready"
  | "actionRequired"
  | "active"
  | "degraded"
  | "accountDisabled";

export function binanceAutoSettlementUiState(
  status?: BinanceAutoSettlementStatus | null,
): BinanceAutoSettlementUiState {
  if (
    !status?.credentialStorageConfigured
    || !["shadow", "enabled"].includes(status.globalMode)
  ) {
    return "unavailable";
  }
  if (status.serviceAvailability === "region_restricted") return "regionRestricted";
  if (status.account?.status === "degraded") return "degraded";
  if (status.account?.status === "disabled") return "accountDisabled";
  if (status.globalMode === "shadow") return "trial";
  if (!status.account) return "ready";
  if (status.account.automationMode !== "enabled") return "actionRequired";
  if (status.account.status !== "verified") return "actionRequired";
  return "active";
}
