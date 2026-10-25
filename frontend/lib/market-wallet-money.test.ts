import assert from "node:assert/strict";
import test from "node:test";
import {
  formatWalletUnits,
  formatWalletUnitsAsCny,
  parseUsdMinor,
  parseNonNegativeUsdMinor,
  parseTopupMinor,
  topupBounds,
  payableCheckoutUrl,
  safeCheckoutUrl,
  safeMarketReturnPath,
  shouldResetTopupIdempotency,
  topupDisplayStatus,
  topupCancelAction,
  verifiedPayableCheckoutUrl,
  walletTopupCheckoutHref,
  topupAutoCheckCountdown,
  formatTopupCountdown,
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

test("top-up input uses authoritative fee-adjusted credit limits, not payment limits", () => {
  const bounds = topupBounds({ minCreditMinor: 100, maxCreditMinor: 95_238 });
  assert.deepEqual(bounds, { min: 100, max: 95_238 });
  assert.equal(parseTopupMinor("952.38", bounds!), 95_238);
  assert.equal(parseTopupMinor("$1.00", bounds!), 100);
  for (const invalid of ["952.39", "960", "1000", "0.99", "1.001", "1e2", "-1", ".50", "01.00"]) {
    assert.equal(parseTopupMinor(invalid, bounds!), null, invalid);
  }
  assert.equal(parseTopupMinor("1000", topupBounds({ minCreditMinor: 100, maxCreditMinor: 100_000 })!), 100_000);
  const raisedMinimum = topupBounds({ minCreditMinor: 952, maxCreditMinor: 4762 })!;
  assert.equal(parseTopupMinor("9.51", raisedMinimum), null);
  assert.equal(parseTopupMinor("9.52", raisedMinimum), 952);
  assert.equal(parseTopupMinor("47.62", raisedMinimum), 4762);
  assert.equal(parseTopupMinor("47.63", raisedMinimum), null);
});

test("missing, empty and malformed credit ranges fail closed", () => {
  for (const channel of [null, undefined, {},
    { minCreditMinor: null, maxCreditMinor: null },
    { minCreditMinor: 100 }, { maxCreditMinor: 100_000 },
    { minCreditMinor: "100", maxCreditMinor: 100_000 },
    { minCreditMinor: 100, maxCreditMinor: NaN },
    { minCreditMinor: 100, maxCreditMinor: Infinity },
    { minCreditMinor: 100.1, maxCreditMinor: 100_000 },
    { minCreditMinor: 99, maxCreditMinor: 100_000 },
    { minCreditMinor: 100, maxCreditMinor: 100_001 },
    { minCreditMinor: 200, maxCreditMinor: 100 },
  ]) assert.equal(topupBounds(channel), null);
  // An exactly one-cent-wide feasible interval is not an empty range.
  assert.deepEqual(topupBounds({ minCreditMinor: 100, maxCreditMinor: 100 }), { min: 100, max: 100 });
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
  assert.equal(payableCheckoutUrl("pending", url, "awaiting_payment", true), null);
  assert.equal(payableCheckoutUrl("pending", url, "pending", true), null);
});

test("checkout requires a fresh authenticated payable capability", () => {
  const checkout = "https://www.dujiaopay.com/checkout/ct_valid";
  const now = Date.parse("2026-10-20T00:00:00Z");
  const gateway = { gatewayStatus: "pending", canPay: true, selectionDeadline: null, expiresAt: "2026-10-20T00:01:00Z" };
  assert.equal(verifiedPayableCheckoutUrl("pending", checkout, gateway, false, now), checkout);
  assert.equal(verifiedPayableCheckoutUrl("pending", checkout, { ...gateway, canPay: false }, false, now), null);
  assert.equal(verifiedPayableCheckoutUrl("pending", checkout, null, false, now), null);
  assert.equal(verifiedPayableCheckoutUrl("pending", checkout, gateway, false, now + 60_000), null);
  assert.equal(verifiedPayableCheckoutUrl("review_required", checkout, gateway, false, now), null);
  assert.equal(verifiedPayableCheckoutUrl("pending", checkout, gateway, true, now), null);
});

test("checkout navigation stays local and encodes the order ID", () => {
  assert.equal(walletTopupCheckoutHref("order/one?two"), "/wallet-checkout/?topup=order%2Fone%3Ftwo");
});

test("a closed or nonpayable attempt uses a new browser idempotency key", () => {
  const order = { status: "pending", checkoutUrl: "https://www.dujiaopay.com/checkout/ct_old", buyerAbandonedAt: null };
  assert.equal(shouldResetTopupIdempotency(order), false);
  assert.equal(shouldResetTopupIdempotency({ ...order, status: "creating", checkoutUrl: null }), false);
  assert.equal(shouldResetTopupIdempotency({ ...order, checkoutUrl: null }), true);
  assert.equal(shouldResetTopupIdempotency({ ...order, buyerAbandonedAt: "2026-10-20T00:00:00Z" }), true);
  for (const status of ["verifying", "review_required", "credited", "expired", "cancelled"]) {
    assert.equal(shouldResetTopupIdempotency({ ...order, status }), true, status);
  }
});

test("gateway payment states do not collapse to a cancelable local pending label", () => {
  assert.equal(topupDisplayStatus("pending", "awaiting_payment"), "awaiting_payment");
  assert.equal(topupDisplayStatus("pending", "pending"), "gateway_pending");
  assert.equal(topupDisplayStatus("pending", "confirming"), "gateway_confirming");
  assert.equal(topupDisplayStatus("pending", "canceled"), "gateway_canceled");
  assert.equal(topupDisplayStatus("review_required", "pending"), "review_required");
  assert.equal(topupDisplayStatus("pending", "awaiting_payment", true), "buyer_abandoned");
  assert.equal(topupDisplayStatus("pending", "paid", true), "gateway_paid");
});

test("cancel action chooses a verified gateway cancel, local abandon, or an explanation", () => {
  assert.equal(topupCancelAction("pending", { canCancel: true, canAbandon: false }), "gateway_cancel");
  assert.equal(topupCancelAction("pending", { canCancel: false, canAbandon: true }), "abandon");
  assert.equal(topupCancelAction("pending", { canCancel: false, canAbandon: true }, true), "unavailable");
  assert.equal(topupCancelAction("pending", { canCancel: false, canAbandon: false }), "unavailable");
  assert.equal(topupCancelAction("pending", null), "unavailable");
  assert.equal(topupCancelAction("review_required", { canCancel: true, canAbandon: true }), "unavailable");
});

test("auto-check countdown warns before the durable deadline without claiming closure", () => {
  const deadline = "2026-10-20T00:30:00Z";
  const start = Date.parse("2026-10-20T00:00:00Z");
  assert.deepEqual(topupAutoCheckCountdown(deadline, start), { remainingSeconds: 1800, phase: "active" });
  assert.deepEqual(topupAutoCheckCountdown(deadline, start + 25 * 60_000), { remainingSeconds: 300, phase: "soon" });
  assert.deepEqual(topupAutoCheckCountdown(deadline, start + 30 * 60_000), { remainingSeconds: 0, phase: "checking" });
  assert.equal(formatTopupCountdown(300), "05:00");
  assert.equal(formatTopupCountdown(0), "00:00");
  assert.equal(topupAutoCheckCountdown(null, start), null);
  assert.equal(topupAutoCheckCountdown("invalid", start), null);
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
