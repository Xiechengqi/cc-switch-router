import assert from "node:assert/strict";
import test from "node:test";
import { formatWalletUnits, formatWalletUnitsAsCny, parseUsdMinor, safeCheckoutUrl } from "./market-wallet-money";

test("wallet money display uses integer units even above the JS safe-integer range", () => {
  assert.equal(formatWalletUnits("8640000"), "$1.00");
  assert.equal(formatWalletUnits("17280000"), "$2.00");
  assert.equal(formatWalletUnits("1"), "<$0.000001");
  assert.equal(formatWalletUnits("-8640000"), "−$1.00");
  assert.equal(formatWalletUnits("90071992547409930"), "$10,424,999,137.431704");
  assert.equal(formatWalletUnitsAsCny("8640000", 6_693_100), "≈¥6.6931");
});

test("top-up amount accepts only exact supported cents", () => {
  assert.equal(parseUsdMinor("1.00"), 100);
  assert.equal(parseUsdMinor("1000"), 100_000);
  for (const invalid of ["0.99", "1000.01", "1.001", "1e2", "-1", ".50", "01.00"]) {
    assert.equal(parseUsdMinor(invalid), null, invalid);
  }
});

test("only the official DujiaoPay checkout origin is followed", () => {
  assert.equal(safeCheckoutUrl("https://www.dujiaopay.com/c/order"), "https://www.dujiaopay.com/c/order");
  for (const invalid of ["http://www.dujiaopay.com/c/order", "https://www.dujiaopay.com.evil/c/order", "https://evil@www.dujiaopay.com/c/order", "javascript:alert(1)"]) {
    assert.equal(safeCheckoutUrl(invalid), null, invalid);
  }
});
