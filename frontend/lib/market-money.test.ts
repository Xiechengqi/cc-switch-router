import assert from "node:assert/strict";
import test from "node:test";
import { formatMarketPrice, formatUsdCnyMoney, formatUsdMoney, formatUsdMoneyInCurrency, setMarketMoneyDisplay, usdMinorToCnyMinor } from "./market-money";

test("currency switch converts display only with the verified quote", () => {
  assert.equal(usdMinorToCnyMinor(100, 6_693_100), 669);
  assert.equal(formatUsdMoneyInCurrency(100, "en-US", "USD", 6_693_100), "$1.00");
  assert.equal(formatUsdMoneyInCurrency(100, "en-US", "CNY", 6_693_100), "≈CN¥6.69");
});

test("currency preference does not rewrite frozen invoice amounts", () => {
  setMarketMoneyDisplay("CNY", 6_693_100, true);
  assert.equal(formatUsdCnyMoney(100, "en-US", 675), "$1.00 / CN¥6.75");
  setMarketMoneyDisplay("USD", 6_693_100, true);
});

test("stale quotes cannot activate CNY estimates", () => {
  setMarketMoneyDisplay("CNY", 7_000_000, false);
  assert.equal(formatMarketPrice(100, "en-US"), "$1.00");
});

test("currency preference never changes a USD payment amount", () => {
  setMarketMoneyDisplay("CNY", 6_693_100, true);
  assert.equal(formatMarketPrice(100, "en-US"), "≈CN¥6.69");
  assert.equal(formatUsdMoney(100, "en-US"), "$1.00");
  setMarketMoneyDisplay("USD", 6_693_100, true);
});
