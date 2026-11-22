import type { MarketWalletTopup } from "@/lib/market-wallet-api";

/** A top-up that still needs the buyer or is still being confirmed. Stopped
 * top-ups stay out of the way: a late payment still credits on its own. */
export function isOpenTopup(order: MarketWalletTopup) {
  const state = order.buyerView?.state;
  if (state) return state === "awaiting_payment" || (state === "confirming" && !order.buyerAbandonedAt);
  return !["credited", "expired", "cancelled"].includes(order.status) && !order.buyerAbandonedAt;
}

/** Dollars and cents from ledger units (86,400 per cent), for an amount field. */
export function unitsToDollarInput(raw: string) {
  let units = 0n;
  try { units = BigInt(raw); } catch { /* invalid input is zero */ }
  const cents = units / 86_400n;
  return `${cents / 100n}.${(cents % 100n).toString().padStart(2, "0")}`;
}
