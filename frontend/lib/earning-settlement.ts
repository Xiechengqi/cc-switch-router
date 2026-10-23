import type { MarketProviderEarningLot } from "./market-wallet-api";

export type EarningHistory = { items: MarketProviderEarningLot[]; nextCursor: string | null };

// Refresh through the oldest visible record, not just the first overlapping
// page: previously loaded rows may have matured or been frozen since polling.
export async function refreshEarningHistory(
  previous: EarningHistory | null,
  firstPage: EarningHistory,
  loadPage: (cursor: string) => Promise<EarningHistory>,
  isCurrent: () => boolean = () => true,
): Promise<EarningHistory | null> {
  const anchor = previous?.items.at(-1)?.id;
  const items = [...firstPage.items];
  const cursors = new Set<string>();
  let page = firstPage;
  while (isCurrent() && anchor && page.nextCursor && !page.items.some(item => item.id === anchor)) {
    if (cursors.has(page.nextCursor)) throw new Error("Repeated earnings history cursor");
    cursors.add(page.nextCursor);
    page = await loadPage(page.nextCursor);
    items.push(...page.items);
  }
  if (!isCurrent()) return null;
  const seen = new Set<string>();
  return {
    items: items.filter(item => {
      if (seen.has(item.id)) return false;
      seen.add(item.id);
      return true;
    }),
    nextCursor: page.nextCursor,
  };
}

export function earningUnits(raw: string) { try { return BigInt(raw); } catch { return 0n; } }
export function earningLotState(lot: MarketProviderEarningLot) {
  if (lot.disputed || earningUnits(lot.heldUnits) > 0n) return { key: "marketProviderFinance.lot.held", tone: "warning" } as const;
  const net = earningUnits(lot.amountUnits) - earningUnits(lot.reversedUnits);
  if (net <= 0n) return earningUnits(lot.grossUnits) > earningUnits(lot.reversedUnits) + earningUnits(lot.commissionReversedUnits)
    ? { key: "commission.zeroNet", tone: "neutral" } as const
    : { key: "marketProviderFinance.lot.reversed", tone: "neutral" } as const;
  if (earningUnits(lot.creditedUnits) >= net) return { key: "earningSettlement.credited", tone: "success" } as const;
  // A deadline is not proof of journaled income. Never promise spendable
  // balance based on the browser clock alone.
  return earningUnits(lot.unlockedUnits) > 0n
    ? { key: "earningSettlement.ready", tone: "accent" } as const
    : { key: "marketProviderFinance.lot.pending", tone: "neutral" } as const;
}
