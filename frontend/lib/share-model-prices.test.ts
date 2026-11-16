import assert from "node:assert/strict";
import test from "node:test";
import {
  EMPTY_PRICE_DRAFT, comparePriceRates, modelPriceErrorKey, modelPriceErrorMessage, normalizePriceRate, priceDraftFromRates,
  sharePriceAccess, validatePriceDraft,
} from "./share-model-prices";
import { shareModelPriceMessagesEn, shareModelPriceMessagesZh } from "./share-model-price-messages";

test("rates are exact decimals: non-negative, at most six places, no float round-trip", () => {
  assert.deepEqual(normalizePriceRate(" 2.50 "), { ok: true, value: "2.5" });
  assert.deepEqual(normalizePriceRate("$0.000001"), { ok: true, value: "0.000001" });
  assert.deepEqual(normalizePriceRate(".5"), { ok: true, value: "0.5" });
  assert.deepEqual(normalizePriceRate("007."), { ok: true, value: "7" });
  assert.deepEqual(normalizePriceRate("0"), { ok: true, value: "0" });
  assert.deepEqual(normalizePriceRate(""), { ok: false, error: "required" });
  for (const bad of ["-1", "1e3", "1.0000001", "1,000", ".", "abc", "+2"]) {
    assert.deepEqual(normalizePriceRate(bad), { ok: false, error: "format" }, bad);
  }
});

test("a draft needs every required rate, an optional 1h rate, and input or output above zero", () => {
  const draft = { input: "3", output: "15", cacheRead: "0.3", cacheWrite5m: "3.75", cacheWrite1h: "" };
  assert.deepEqual(validatePriceDraft(draft).rates, {
    inputUsdPer1m: "3", outputUsdPer1m: "15", cacheReadUsdPer1m: "0.3", cacheWrite5mUsdPer1m: "3.75",
  });
  assert.equal(validatePriceDraft({ ...draft, cacheWrite1h: "6" }).rates?.cacheWrite1hUsdPer1m, "6");
  assert.deepEqual(validatePriceDraft({ ...draft, cacheWrite1h: "x" }).errors, { cacheWrite1h: "format" });
  assert.deepEqual(validatePriceDraft(EMPTY_PRICE_DRAFT).errors,
    { input: "required", output: "required", cacheRead: "required", cacheWrite5m: "required" });
  const free = validatePriceDraft({ ...draft, input: "0", output: "0.000" });
  assert.equal(free.allZero, true);
  assert.equal(free.rates, undefined, "a model can't be made free by accident");
  assert.equal(validatePriceDraft({ ...draft, input: "0" }).rates?.inputUsdPer1m, "0", "output alone is enough");
  assert.deepEqual(validatePriceDraft({ ...draft, cacheWrite1h: "3.7" }).errors, { cacheWrite1h: "below5m" });
  assert.equal(validatePriceDraft({ ...draft, cacheWrite1h: "3.750" }).rates?.cacheWrite1hUsdPer1m, "3.75", "equal to 5m is allowed");
});

test("rates compare as exact decimals", () => {
  assert.equal(comparePriceRates("10", "9.999999"), 1);
  assert.equal(comparePriceRates("0.1", "0.09"), 1);
  assert.equal(comparePriceRates("3.75", "3.75"), 0);
  assert.equal(comparePriceRates("0.000001", "0.00001"), -1);
});

test("suggestions and owner prices fill the editor exactly as sent", () => {
  assert.deepEqual(priceDraftFromRates({ inputUsdPer1m: "1.25", outputUsdPer1m: "10", cacheReadUsdPer1m: "0.125", cacheWrite5mUsdPer1m: "1.5625" }),
    { input: "1.25", output: "10", cacheRead: "0.125", cacheWrite5m: "1.5625", cacheWrite1h: "" });
  assert.deepEqual(priceDraftFromRates(undefined), EMPTY_PRICE_DRAFT);
});

test("pricing permissions come from the Router, with a fallback for older Routers", () => {
  assert.deepEqual(sharePriceAccess({ canSetPrice: true, canSetGlobalPrice: false }, { isAdmin: true, canManage: false }), { client: true, global: false });
  assert.deepEqual(sharePriceAccess({ canSetPrice: false }, { canManage: true }), { client: false, global: false });
  assert.deepEqual(sharePriceAccess({}, { canManage: true, isAdmin: false }), { client: true, global: false });
  assert.deepEqual(sharePriceAccess(undefined, { isAdmin: true }), { client: false, global: true });
});

test("save failures map to one translated sentence per scope", () => {
  assert.equal(modelPriceErrorKey({ status: 409, code: "MODEL_PRICE_VERSION_CONFLICT" }, "client"), "dashboard.modelPrice.conflict.client");
  assert.equal(modelPriceErrorKey({ status: 409 }, "global"), "dashboard.modelPrice.conflict.global");
  assert.equal(modelPriceErrorKey({ status: 400, code: "MODEL_PRICE_INVALID" }, "client"), "dashboard.modelPrice.error.invalid");
  assert.equal(modelPriceErrorKey({ status: 403, code: "SHARE_FORBIDDEN" }, "client"), "dashboard.modelPrice.error.forbidden");
  assert.equal(modelPriceErrorKey({ status: 404, code: "SHARE_NOT_FOUND" }, "client"), "dashboard.modelPrice.error.notFound");
  assert.equal(modelPriceErrorKey({ status: 404 }, "client"), "dashboard.modelPrice.error.unsupported", "an older Router has no route");
  assert.equal(modelPriceErrorKey({ status: 404 }, "global"), "dashboard.modelPrice.error.failed");
  assert.equal(modelPriceErrorKey(new Error("boom"), "client"), "dashboard.modelPrice.error.failed");
  assert.equal(modelPriceErrorKey({ status: 401 }, "client"), "dashboard.modelPrice.error.failed");
  for (const messages of [shareModelPriceMessagesEn, shareModelPriceMessagesZh]) {
    for (const value of Object.values(messages)) assert.ok(value.trim());
  }
});

test("validation reasons name the rate and the allowed maximum", () => {
  const invalid = (details: Record<string, unknown>, message = "The model price is not valid") =>
    Object.assign(new Error(message), { status: 400, code: "MODEL_PRICE_INVALID", details });
  assert.deepEqual(
    modelPriceErrorMessage(invalid({ reason: "rate_above_catalog_max", field: "outputUsdPer1m", maxUsdPer1m: "150" }), "client"),
    { key: "dashboard.modelPrice.reason.rateAboveMax", params: { max: "150" }, field: "output" },
  );
  assert.deepEqual(modelPriceErrorMessage(invalid({ reason: "too_many_prices", max: 200 }), "client"),
    { key: "dashboard.modelPrice.reason.tooMany", params: { max: 200 }, field: undefined });
  assert.equal(modelPriceErrorMessage(invalid({ reason: "catalog_priced" }), "client").key, "dashboard.modelPrice.reason.catalogPriced");
  assert.equal(modelPriceErrorMessage(invalid({ reason: "cache_write_1h_below_5m", field: "cacheWrite1hUsdPer1m" }), "client").key,
    "dashboard.modelPrice.reason.below5m");
  // A sentence with a placeholder the Router didn't fill falls back to the generic one.
  assert.equal(modelPriceErrorMessage(invalid({ reason: "rate_above_catalog_max" }), "client").key, "dashboard.modelPrice.error.invalid");
  assert.deepEqual(modelPriceErrorMessage(invalid({ reason: "something_new", detail: "  parser said no " }), "client"),
    { key: "dashboard.modelPrice.error.invalid", detail: "parser said no" });
  assert.deepEqual(modelPriceErrorMessage({ status: 409, code: "MODEL_PRICE_VERSION_CONFLICT" }, "client"),
    { key: "dashboard.modelPrice.conflict.client" });
  for (const messages of [shareModelPriceMessagesEn, shareModelPriceMessagesZh]) {
    assert.match(messages["dashboard.modelPrice.reason.rateAboveMax"], /\{field\}[\s\S]*\{max\}/);
    assert.ok(!messages["dashboard.modelPrice.summaryUnpricedSingle"].includes("{count}"));
  }
});
