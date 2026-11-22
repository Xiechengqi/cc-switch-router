import assert from "node:assert/strict";
import test from "node:test";
import { isOpenTopup, unitsToDollarInput } from "./wallet-view";
import type { MarketWalletTopup } from "./market-wallet-api";

test("send-back prefill converts ledger units to a dollar amount", () => {
  assert.equal(unitsToDollarInput("8640000"), "1.00");
  assert.equal(unitsToDollarInput(String(86_400n * 1234n)), "12.34");
  assert.equal(unitsToDollarInput("86399"), "0.00");
});

test("only top-ups that need the buyer or are still confirming are open", () => {
  const base = { id: "o", status: "pending", buyerAbandonedAt: null } as unknown as MarketWalletTopup;
  const view = (state: string) => ({ ...base, buyerView: { state, canPay: false, canClose: false, payUntil: null } }) as MarketWalletTopup;
  assert.equal(isOpenTopup(view("awaiting_payment")), true);
  assert.equal(isOpenTopup(view("confirming")), true);
  assert.equal(isOpenTopup({ ...view("confirming"), buyerAbandonedAt: "2026-11-20T00:00:00Z" }), false, "a stopped top-up stays out of the way");
  assert.equal(isOpenTopup(view("credited")), false);
  assert.equal(isOpenTopup(view("closed")), false);
});
