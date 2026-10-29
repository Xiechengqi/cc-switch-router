import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync, existsSync } from "node:fs";
import { earningLotState, refreshEarningHistory, type EarningHistory } from "./earning-settlement";
import type { MarketProviderEarningLot } from "./market-wallet-api";

const lot: MarketProviderEarningLot = {
  id: "lot", amountUnits: "900", grossUnits: "1000", commissionUnits: "100", commissionReversedUnits: "0",
  commissionRateBps: 1000, commissionPolicyVersion: 1, reversedUnits: "0", unlockedUnits: "0", heldUnits: "0",
  creditedUnits: "0", fulfilledAt: "2026-01-01T00:00:00Z", unlockAt: "2026-01-08T00:00:00Z", disputed: false,
};
test("only journaled credits, not the browser date, say earnings reached the balance", () => {
  assert.equal(earningLotState(lot).key, "marketProviderFinance.lot.pending");
  assert.equal(earningLotState({ ...lot, unlockedUnits: "900" }).key, "earningSettlement.ready");
  assert.equal(earningLotState({ ...lot, unlockedUnits: "900", creditedUnits: "900" }).key, "earningSettlement.credited");
  assert.equal(earningLotState({ ...lot, unlockedUnits: "900", creditedUnits: "899" }).key, "earningSettlement.ready");
});
test("holds and reversals never claim spendable income", () => {
  assert.equal(earningLotState({ ...lot, disputed: true }).tone, "warning");
  assert.equal(earningLotState({ ...lot, heldUnits: "1" }).tone, "warning");
  assert.equal(earningLotState({ ...lot, reversedUnits: "900" }).tone, "neutral");
  assert.equal(earningLotState({ ...lot, reversedUnits: "900", commissionReversedUnits: "100" }).key, "marketProviderFinance.lot.reversed");
  assert.equal(earningLotState({ ...lot, amountUnits: "0", commissionUnits: "1000", commissionRateBps: 10000 }).key, "commission.zeroNet");
  assert.equal(earningLotState({ ...lot, reversedUnits: "100", unlockedUnits: "800", creditedUnits: "800" }).key, "earningSettlement.credited");
});
test("polling refreshes every loaded row and bridges multiple new history pages", async () => {
  const record = (id: string, creditedUnits = "0") => ({ ...lot, id, creditedUnits });
  const previous: EarningHistory = { items: [record("old-head"), record("old-tail")], nextCursor: "older" };
  const pages: Record<string, EarningHistory> = {
    new: { items: [record("new-2")], nextCursor: "head" },
    head: { items: [record("old-head", "900")], nextCursor: "tail" },
    tail: { items: [record("old-tail", "900")], nextCursor: "older" },
  };
  const calls: string[] = [];
  const refreshed = await refreshEarningHistory(previous, { items: [record("new-1")], nextCursor: "new" }, async cursor => {
    calls.push(cursor); return pages[cursor];
  });
  assert.deepEqual(calls, ["new", "head", "tail"]);
  assert.deepEqual(refreshed?.items.map(item => item.id), ["new-1", "new-2", "old-head", "old-tail"]);
  assert.equal(refreshed?.items.at(-1)?.creditedUnits, "900");
  assert.equal(refreshed?.nextCursor, "older");
});
test("history refresh does not leak prior accounts or loop on a malformed cursor", async () => {
  const page = { items: [lot], nextCursor: "next" };
  const neverLoad = async () => { throw new Error("unexpected history request"); };
  assert.deepEqual(await refreshEarningHistory(null, page, neverLoad), page);
  assert.equal(await refreshEarningHistory(page, page, neverLoad, () => false), null);
  await assert.rejects(refreshEarningHistory({ items: [{ ...lot, id: "missing" }], nextCursor: null }, page, async () => page), /Repeated/);
});
test("current UI has no external payout or manual earning-credit actions", () => {
  const source = (p: string) => readFileSync(new URL(p, import.meta.url), "utf8");
  for (const p of ["../components/dashboard/provider-finance-panel.tsx", "../components/dashboard/platform-wallet-panel.tsx",
    "../components/settings/market-payment-channels-panel.tsx", "./market-wallet-api.ts"]) {
    assert.doesNotMatch(source(p), /EarningWalletConversion|requestMarketProviderWithdrawal|saveMarketProviderDestination|MarketFinanceTransfersPanel|canWithdraw/);
  }
  assert.equal(existsSync(new URL("../components/dashboard/earning-wallet-conversion.tsx", import.meta.url)), false);
  assert.match(source("../components/dashboard/provider-finance-panel.tsx"), /href="\/account\/wallet\/"/);
  assert.match(source("../app/(dashboard)/account/provider-settings/page.tsx"), /<AccountProviderCenter/);
  assert.match(source("../components/dashboard/account-provider-center.tsx"), /tab === "income" \? <ProviderFinancePanel/);
  assert.doesNotMatch(source("../components/dashboard/share-market/paid-offer-readiness.tsx"), /finance\.destination|usableAt/);
});
