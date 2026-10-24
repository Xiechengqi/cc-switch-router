import assert from "node:assert/strict";
import test from "node:test";
import { ApiError } from "./api";
import { C2C_PENDING_PREFIX, C2CClock, c2cDescriptionParts, validC2CDescription, c2cDefinitelyRejected, c2cPaymentWindow, c2cPayLabel, matchC2COrder, matchC2CPendingOrder, parseC2CMinor, readC2CPending, writeC2CPending, validC2CEvent, validC2COrder, validC2CPolicy, validC2CQuote, validC2CSeller, type C2CEvent, type C2COrder, type C2CPending, type C2CSeller } from "./market-c2c";
import { marketC2CMessagesEn, marketC2CMessagesZh } from "./market-c2c-messages";
import { C2C_BLOCKERS, c2cBlockerKey, validC2CCapability } from "./market-c2c";

test("capabilities give distinct actionable blockers and never depend on official recharge mode", () => {
  assert.doesNotThrow(() => validC2CCapability([], true));
  for (const code of C2C_BLOCKERS) {
    assert.doesNotThrow(() => validC2CCapability([code], false));
    const key = c2cBlockerKey(code) as keyof typeof marketC2CMessagesEn;
    assert.ok(marketC2CMessagesEn[key], code);
    assert.ok(marketC2CMessagesZh[key], code);
    assert.equal(c2cDefinitelyRejected(new ApiError(409, "blocked", code)), true);
  }
  assert.throws(() => validC2CCapability([], false));
  assert.throws(() => validC2CCapability(["C2C_DISABLED"], true));
  assert.throws(() => validC2CCapability(["C2C_DISABLED", "C2C_DISABLED"], false));
  assert.throws(() => validC2CCapability(["TOPUP_SHADOW" as typeof C2C_BLOCKERS[number]], false));
});
const pending: C2CPending = { actor: "buyer", key: "original-key", quote: { id: "quote", sellerId: "seller", sellerName: "C2C-seller", sellerVersion: 1, creditMinor: 1000, payMinor: 1000, currency: "USD", fxMicros: 1_000_000, priceBps: 10000, description: "Bank transfer, contact @seller", expiresAt: "2026-10-22T12:05:00Z", flowVersion: 3, paymentMinutes: 30 } };
const order: C2COrder = { id: "order", quoteId: "quote", buyerId: "buyer", sellerId: "seller", amountUnits: "86400000", status: "awaiting_payment", version: 2, paymentDeadline: "2026-10-22T12:30:00Z", flowVersion: 3, acceptanceDeadline: null, acceptedAt: null, paidAt: null, releasedAt: null, latePaymentReported: false, latePaymentResolved: false, createdAt: "2026-10-22T12:00:00Z", updatedAt: "2026-10-22T12:00:00Z", sellerName: "C2C-seller", payMinor: 1000, currency: "USD", fxMicros: 1_000_000, priceBps: 10000, description: pending.quote.description, legacyPayment: null };
const legacyOrder: C2COrder = { ...order, flowVersion: 2, acceptanceDeadline: "2026-10-22T12:10:00Z", acceptedAt: order.createdAt };
function storage() { const data = new Map<string, string>(); return { getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => { data.set(key, value); } }; }
test("order recovery preserves user, quote and original key across reload and expired previews", () => {
  const s = storage(); writeC2CPending(s, pending); assert.deepEqual(readC2CPending(s, "buyer"), pending); assert.equal(readC2CPending(s, "other"), null);
});
test("malformed recovery cannot create money actions or cross accounts", () => {
  const s = storage();
  for (const p of [null, {}, { ...pending, actor: "other" }, { ...pending, key: "bad key" }, { ...pending, quote: { ...pending.quote, creditMinor: 1.1 } }, { ...pending, quote: { ...pending.quote, currency: "BTC" } }]) {
    s.setItem(C2C_PENDING_PREFIX + "buyer", JSON.stringify(p)); assert.throws(() => readC2CPending(s, "buyer"));
  }
  assert.throws(() => writeC2CPending({ getItem: () => null, setItem() {} }, pending));
});
test("timeouts, authentication failures and unknown conflicts retain recovery metadata", () => {
  for (const status of [401, 403, 408, 429, 500, 502, 503, 504]) assert.equal(c2cDefinitelyRejected(new ApiError(status, "unknown")), false);
  assert.equal(c2cDefinitelyRejected(new ApiError(409, "unknown")), false); assert.equal(c2cDefinitelyRejected(new Error("network lost")), false);
  assert.equal(c2cDefinitelyRejected(new ApiError(409, "stock", "C2C_STOCK")), true);
  assert.equal(c2cDefinitelyRejected(new ApiError(409, "idempotency", "C2C_IDEMPOTENCY")), false);
});
test("paying stops at deadline while declaring an existing payment has an explicit two minute grace", () => {
  assert.deepEqual(c2cPaymentWindow(order, Date.parse("2026-10-22T12:29:59Z")), { mayStartPayment: true, mayClaimPayment: true });
  assert.deepEqual(c2cPaymentWindow(order, Date.parse("2026-10-22T12:30:00Z")), { mayStartPayment: false, mayClaimPayment: true });
  assert.deepEqual(c2cPaymentWindow(order, Date.parse("2026-10-22T12:32:00Z")), { mayStartPayment: false, mayClaimPayment: false });
  for (const status of ["paid", "disputed", "released", "cancelled", "expired"] as const) assert.deepEqual(c2cPaymentWindow({ ...order, status }, Date.parse(order.createdAt)), { mayStartPayment: false, mayClaimPayment: false });
});
test("financial projections reject unsafe units, invalid status, self trades and incomplete release evidence", () => {
  assert.deepEqual(validC2COrder(order), order); assert.deepEqual(validC2CQuote(pending.quote), pending.quote);
  for (const changes of [{ amountUnits: "1.1" }, { amountUnits: "0" }, { amountUnits: "9223372036854775808" }, { amountUnits: "86401" }, { status: "paid_by_gateway" }, { sellerId: "buyer" }, { status: "released", releasedAt: null }, { payMinor: -1 }, { paymentDeadline: "unknown" }]) assert.throws(() => validC2COrder({ ...order, ...changes } as C2COrder));
});
test("payment currency labels distinguish offline payable from USD credit", () => {
  assert.equal(c2cPayLabel(7000, "CNY"), "CN¥70.00"); assert.equal(c2cPayLabel(1000, "USD"), "US$10.00");
});
test("C2C amounts have their own cent-exact range rather than the official top-up cap", () => {
  for (const [input, expected] of [["0.01", 1], ["1", 100], ["150", 15000], ["5000", 500000], ["100000", 10000000]] as const) assert.equal(parseC2CMinor(input), expected);
  for (const input of ["0", "0.001", "-1", "1e3", "100000.01", "NaN", "1,000"]) assert.equal(parseC2CMinor(input), null);
});
test("recovery must match the exact parties and frozen money terms", () => {
  assert.deepEqual(matchC2CPendingOrder(pending, order), order);
  assert.throws(() => matchC2CPendingOrder({ ...pending, actor: "another-user" }, order));
  assert.throws(() => matchC2CPendingOrder({ ...pending, quote: { ...pending.quote, id: "other-quote" } }, order));
  assert.throws(() => matchC2CPendingOrder({ ...pending, quote: { ...pending.quote, payMinor: 1005, priceBps: 10050 } }, order));
});
test("mutation responses cannot switch orders, rewrite money or roll back a version", () => {
  assert.deepEqual(matchC2COrder(order, { ...order, version: 2 }), { ...order, version: 2 });
  for (const changes of [{ id: "another-order" }, { quoteId: "other-quote" }, { buyerId: "owner" }, { version: 1 }]) assert.throws(() => matchC2COrder({ ...order, version: 2 }, { ...order, ...changes }));
});
test("server-anchored countdowns ignore client wall-clock skew and never rewind on stale replies", () => {
  const clock = new C2CClock(); const server = Date.parse(order.createdAt);
  assert.equal(clock.now(100, server + 86400000), server + 86400000);
  clock.update(order.createdAt, 100, 200);
  assert.equal(clock.now(1200, server + 86400000), server + 1050);
  assert.equal(clock.now(2200, server - 86400000), server + 2050);
  clock.update(order.createdAt, 2200, 2300);
  assert.equal(clock.now(3300, 0), server + 3150);
  clock.update("invalid", 3300, 3400);
  assert.equal(clock.now(4300, 0), server + 4150);
});
test("financial projections bind payable amount to currency, rate and price using integer rounding", () => {
  const cny = { ...pending.quote, currency: "CNY" as const, fxMicros: 7123456, payMinor: 7123 };
  assert.deepEqual(validC2CQuote(cny), cny);
  for (const changes of [{ payMinor: 7124 }, { priceBps: 4999 }, { fxMicros: 0 }, { creditMinor: 10_000_001 }]) assert.throws(() => validC2CQuote({ ...cny, ...changes }));
  assert.throws(() => validC2CQuote({ ...pending.quote, fxMicros: 7000000 }));
  assert.throws(() => validC2COrder({ ...order, status: "paid", paidAt: null }));
  assert.throws(() => validC2COrder({ ...order, latePaymentResolved: true }));
});
test("seller, event and policy projections reject incomplete or unsafe operational data", () => {
  const seller: C2CSeller = { userId: "seller", version: 1, enabled: true, suspended: false, displayName: "C2C-seller", description: "Contact @seller", currency: "USD", priceBps: 10000, minMinor: 1, maxMinor: 500000, autoPauseReason: null, autoPausedAt: null, autoPauseOrderId: null, autoAcceptTermsVersion: 3, autoAcceptConsentedAt: order.createdAt };
  assert.deepEqual(validC2CSeller(seller), seller);
  for (const changes of [{ enabled: "false" }, { displayName: "" }, { description: null }, { description: "" }, { minMinor: 0 }, { maxMinor: 10000001 }, { autoPauseReason: "missing" }, { autoAcceptTermsVersion: 2 }, { autoAcceptTermsVersion: 0 }, { autoAcceptConsentedAt: null }, { autoAcceptConsentedAt: "invalid" }]) assert.throws(() => validC2CSeller({ ...seller, ...changes } as C2CSeller));
  assert.doesNotThrow(() => validC2CSeller({ ...seller, autoAcceptTermsVersion: 0, autoAcceptConsentedAt: null }));
  assert.doesNotThrow(() => validC2CSeller({ ...seller, enabled: false, description: "" }));
  const event: C2CEvent = { id: "event", actorId: "system", action: "release_reminder", note: "", createdAt: order.createdAt };
  assert.deepEqual(validC2CEvent(event), event);
  assert.throws(() => validC2CEvent({ ...event, note: null } as unknown as C2CEvent));
  assert.throws(() => validC2CPolicy({ version: 1, enabled: false, admissionMinor: 99, minMinor: 1, maxMinor: 500000, paymentMinutes: 30 }));
});
test("every C2C UI and mandatory notification message has matching English and Chinese interpolation keys", () => {
  assert.deepEqual(Object.keys(marketC2CMessagesEn).sort(), Object.keys(marketC2CMessagesZh).sort());
  for (const k of Object.keys(marketC2CMessagesEn) as (keyof typeof marketC2CMessagesEn)[]) assert.deepEqual([...marketC2CMessagesEn[k].matchAll(/\{(\w+)\}/g)].map(m => m[1]).sort(), [...marketC2CMessagesZh[k].matchAll(/\{(\w+)\}/g)].map(m => m[1]).sort(), k);
});
test("public descriptions are bounded plain text and only explicit HTTP links become anchors", () => {
  assert.equal(validC2CDescription("支持微信 · @seller"), true);
  for (const value of ["", " ", "a\nb", "bad\0text", "字".repeat(301)]) assert.equal(validC2CDescription(value), false);
  assert.equal(validC2CDescription("字".repeat(300)), true);
  const source = '联系 @seller · https://example.test/path?q=1。 javascript:alert(1) <img onerror="bad"> https://user:pass@example.test/pay';
  const parts = c2cDescriptionParts(source);
  assert.equal(parts.map(p => p.text).join(""), source);
  assert.deepEqual(parts.filter(p => p.href).map(p => p.href), ["https://example.test/path?q=1"]);
});
test("recovery binds the public description and legacy private instructions stay separate", () => {
  assert.throws(() => matchC2CPendingOrder(pending, { ...order, description: "A different account" }));
  assert.throws(() => matchC2COrder(order, { ...order, description: "Changed" }));
  assert.doesNotThrow(() => validC2COrder({ ...order, description: null, legacyPayment: { contact: "Original contact", paymentMethod: "Bank", paymentDetails: "Original account" } }));
  assert.throws(() => validC2COrder({ ...order, legacyPayment: { contact: "Contact", paymentMethod: "Bank", paymentDetails: "Account" } }));
});

test("historical v2 applications keep their manual acceptance window", () => {
  const application: C2COrder = { ...legacyOrder, status: "awaiting_seller", version: 1, acceptedAt: null, paymentDeadline: null };
  assert.deepEqual(validC2COrder(application), application);
  assert.deepEqual(c2cPaymentWindow(application, Date.parse(order.createdAt)), { mayStartPayment: false, mayClaimPayment: false });
  assert.deepEqual(matchC2COrder(application, legacyOrder), legacyOrder);
  for (const changes of [{ paymentDeadline: order.paymentDeadline }, { acceptedAt: legacyOrder.acceptedAt }, { acceptanceDeadline: null }, { flowVersion: 1 }, { flowVersion: 3 }, { status: "paid", paidAt: order.createdAt }]) {
    assert.throws(() => validC2COrder({ ...application, ...changes } as C2COrder));
  }
  const earlyClaim = { ...application, status: "disputed" as const, paidAt: order.createdAt, latePaymentReported: true };
  assert.deepEqual(validC2COrder(earlyClaim), earlyClaim);
});

test("accepted timestamps and flow version stay frozen across mutations", () => {
  for (const changes of [{ acceptedAt: "2026-10-22T12:01:00Z" }, { acceptanceDeadline: "2026-10-22T12:11:00Z" }, { paymentDeadline: "2026-10-22T12:31:00Z" }]) {
    assert.throws(() => matchC2COrder(legacyOrder, { ...legacyOrder, ...changes }));
  }
  assert.throws(() => validC2COrder({ ...legacyOrder, acceptedAt: legacyOrder.acceptanceDeadline }));
  assert.throws(() => validC2COrder({ ...legacyOrder, paymentDeadline: legacyOrder.acceptedAt }));
  assert.throws(() => matchC2CPendingOrder(pending, { ...order, flowVersion: 1, acceptanceDeadline: null, acceptedAt: null }));
});

test("old local submissions recover only their original version-one order", () => {
  const s = storage(); const quote: Record<string, unknown> = { ...pending.quote };
  delete quote.flowVersion; delete quote.acceptanceMinutes;
  s.setItem(C2C_PENDING_PREFIX + "buyer", JSON.stringify({ ...pending, quote }));
  const recovered = readC2CPending(s, "buyer")!;
  assert.equal(recovered.quote.flowVersion, 1);
  assert.deepEqual(matchC2CPendingOrder(recovered, { ...order, flowVersion: 1, acceptanceDeadline: null, acceptedAt: null }).flowVersion, 1);
  assert.throws(() => matchC2CPendingOrder(recovered, order));
});

test("automatic policy and quote payment windows fail closed on missing or out-of-range values", () => {
  const policy = { version: 1, enabled: true, admissionMinor: 10000, minMinor: 100, maxMinor: 500000, paymentMinutes: 30 };
  assert.doesNotThrow(() => validC2CPolicy(policy));
  for (const paymentMinutes of [undefined, 0, 4, 61, 10.5]) {
    assert.throws(() => validC2CPolicy({ ...policy, paymentMinutes } as typeof policy));
    assert.throws(() => validC2CQuote({ ...pending.quote, paymentMinutes }));
  }
  for (const acceptanceMinutes of [undefined, 0, 4, 31, 10.5]) {
    assert.throws(() => validC2CQuote({ ...pending.quote, flowVersion: 2, acceptanceMinutes }));
  }
});

test("automatic orders are immediately payable without faking manual acceptance", () => {
  assert.equal(validC2COrder(order).acceptedAt, null);
  assert.equal(c2cPaymentWindow(order, Date.parse(order.createdAt)).mayStartPayment, true);
  for (const changes of [{ acceptedAt: order.createdAt }, { acceptanceDeadline: order.paymentDeadline }, { paymentDeadline: null }, { status: "awaiting_seller" }]) {
    assert.throws(() => validC2COrder({ ...order, ...changes } as C2COrder));
  }
  assert.throws(() => matchC2CPendingOrder(pending, { ...order, paymentDeadline: "2026-10-22T12:31:00Z" }));
  assert.throws(() => matchC2COrder(order, { ...order, paymentDeadline: "2026-10-22T12:31:00Z" }));
});

test("v2 local submissions can still recover their original frozen order", () => {
  const old: C2CPending = { ...pending, quote: { ...pending.quote, flowVersion: 2, acceptanceMinutes: 10 } };
  const s=storage(); writeC2CPending(s,old);
  assert.deepEqual(matchC2CPendingOrder(readC2CPending(s,"buyer")!,legacyOrder),legacyOrder);
  assert.throws(() => matchC2CPendingOrder(old,order));
});
