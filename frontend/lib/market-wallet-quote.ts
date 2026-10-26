import type { MarketWalletQuoteFunding } from "./types";

/** Check the complete funding projection before rendering or enabling purchase. */
export function validWalletQuoteFunding(funding?: MarketWalletQuoteFunding): boolean {
  if (!funding || funding.assetCode !== "USD") return false;
  const fields = [funding.availableUnits, funding.heldUnits, funding.committedUnits,
    funding.requiredUnits, funding.shortfallUnits];
  if (!fields.every(value => typeof value === "string" && /^\d{1,19}$/.test(value))) return false;
  const [available, , committed, required, shortfall] = fields.map(BigInt);
  const maximum = 9_223_372_036_854_775_807n;
  if (fields.some(value => BigInt(value) > maximum) || required + committed > maximum) return false;
  const due = required + committed;
  return shortfall === (due > available ? due - available : 0n);
}
