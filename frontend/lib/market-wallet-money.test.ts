import assert from "node:assert/strict";
import test from "node:test";
import {
  formatWalletUnits,
  formatWalletUnitsAsCny,
  parseUsdMinor,
  parseWithdrawalMinor,
  parseNonNegativeUsdMinor,
  payableCheckoutUrl,
  safeCheckoutUrl,
  safeMarketReturnPath,
  topupDisplayStatus,
  walletTopupHref,
} from "./market-wallet-money";

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

test("Provider withdrawal parses exact cents beyond the top-up per-order limit", () => {
  assert.equal(parseWithdrawalMinor("1.00"), 100);
  assert.equal(parseWithdrawalMinor("10000.75"), 1_000_075);
  for (const invalid of ["0.99", "1.001", "1e5", "-1", "90071992547409.92"]) {
    assert.equal(parseWithdrawalMinor(invalid), null, invalid);
  }
});

test("service dispute resolution can record zero or exact cent refunds", () => {
  assert.equal(parseNonNegativeUsdMinor("0.00"), 0);
  assert.equal(parseNonNegativeUsdMinor("0.01"), 1);
  assert.equal(parseNonNegativeUsdMinor("100.25"), 10_025);
  for (const invalid of ["-0.01", "0.001", "1e2", "90071992547409.92"]) {
    assert.equal(parseNonNegativeUsdMinor(invalid), null, invalid);
  }
});

test("only the official DujiaoPay checkout origin is followed", () => {
  assert.equal(safeCheckoutUrl("https://www.dujiaopay.com/checkout/order"), "https://www.dujiaopay.com/checkout/order");
  for (const invalid of ["http://www.dujiaopay.com/checkout/order", "https://www.dujiaopay.com.evil/checkout/order", "https://dujiaopay.com/checkout/order", "https://pay.dujiaopay.com/checkout/order", "https://evil@www.dujiaopay.com/checkout/order", "https://www.dujiaopay.com:444/checkout/order", "https://www.dujiaopay.com/", "https://www.dujiaopay.com/c/order", "https://www.dujiaopay.com/checkout/order#redirect", "https://www.dujiaopay.com/checkout/order?next=evil", "https://www.dujiaopay.com/checkout/order/other", "https://www.dujiaopay.com/checkout/%2f%2fevil.example", "https://www.dujiaopay.com\\checkout\\order", "javascript:alert(1)"]) {
    assert.equal(safeCheckoutUrl(invalid), null, invalid);
  }
});

test("only a pending order exposes its checkout link", () => {
  const url = "https://www.dujiaopay.com/checkout/ct_once";
  assert.equal(payableCheckoutUrl("pending", url), url);
  for (const status of ["creating", "create_unknown", "verifying", "credited", "expired", "cancelled", "review_required"]) {
    assert.equal(payableCheckoutUrl(status, url), null, status);
  }
  for (const gatewayStatus of ["confirming", "paid", "expired", "canceled", "cancelled", "unknown_gateway_state"]) {
    assert.equal(payableCheckoutUrl("pending", url, gatewayStatus), null, gatewayStatus);
  }
  assert.equal(payableCheckoutUrl("pending", url, "awaiting_payment"), url);
  assert.equal(payableCheckoutUrl("pending", url, "pending"), url);
  assert.equal(payableCheckoutUrl("pending", url, null), null);
});

test("gateway payment states do not collapse to a cancelable local pending label", () => {
  assert.equal(topupDisplayStatus("pending", "awaiting_payment"), "awaiting_payment");
  assert.equal(topupDisplayStatus("pending", "pending"), "gateway_pending");
  assert.equal(topupDisplayStatus("pending", "confirming"), "gateway_confirming");
  assert.equal(topupDisplayStatus("pending", "canceled"), "gateway_canceled");
  assert.equal(topupDisplayStatus("review_required", "pending"), "review_required");
});

test("wallet recharge links preserve only local Market context", () => {
  assert.equal(
    walletTopupHref("86400", "/share-market/?focus=share-1"),
    "/account/wallet/?amount=1.00&returnTo=%2Fshare-market%2F%3Ffocus%3Dshare-1",
  );
  assert.equal(safeMarketReturnPath("//evil.example/share-market/"), null);
  assert.equal(safeMarketReturnPath("https://evil.example/client-market/"), null);
  assert.equal(safeMarketReturnPath("/admin/"), null);
  assert.equal(safeMarketReturnPath("/client-market/?focusHost=host-1"), "/client-market/?focusHost=host-1");
});

test("wallet recharge amounts round up to cents and respect merchant limits", () => {
  assert.match(walletTopupHref("8640001", "/client-market/"), /amount=1\.01/);
  assert.match(walletTopupHref("864000000000000", "/client-market/"), /amount=1000\.00/);
});
