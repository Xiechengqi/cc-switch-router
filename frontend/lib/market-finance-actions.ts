import type { MarketFinanceTransferAction } from "@/lib/market-wallet-api";

const UNITS_PER_MINOR = 86_400n;

export function marketFinanceActions(status: string): MarketFinanceTransferAction[] {
  switch (status) {
    case "requested": return ["approve", "reject"];
    case "approved": return ["begin_transfer", "reject"];
    case "transfer_in_progress": return ["mark_paid", "mark_failed", "review"];
    case "transfer_failed": return ["approve", "reject"];
    case "review_required": return ["mark_paid", "mark_failed"];
    default: return [];
  }
}

export function marketFinanceActionsForMode(
  status: string,
  kind: "withdrawals",
  financeMode: "disabled" | "shadow" | "enabled",
  withdrawalsPaused = false,
): MarketFinanceTransferAction[] {
  const actions = marketFinanceActions(status);
  if (financeMode === "enabled" && !withdrawalsPaused) return actions;
  // Pausing finance stops new Provider payouts, but the Owner must still be
  // able to reject one or record the outcome of a transfer already started.
  return actions.filter((action) => action !== "approve" && action !== "begin_transfer");
}

export function exactTransferMinor(amountUnits: string): number | null {
  try {
    const units = BigInt(amountUnits);
    if (units <= 0n || units % UNITS_PER_MINOR !== 0n) return null;
    const minor = units / UNITS_PER_MINOR;
    return minor <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(minor) : null;
  } catch {
    return null;
  }
}

export function validTronTransactionId(value: string): boolean {
  return /^[a-fA-F0-9]{64}$/.test(value.trim());
}
