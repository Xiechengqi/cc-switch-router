import assert from "node:assert/strict";
import test from "node:test";
import { messages } from "@/lib/i18n";
import {
  formatModelPriceEffectiveFrom,
  formatModelPriceHistoryRange,
} from "@/lib/model-price-effectivity";

const locale = "en-US";
const derivedLabel = "Baseline (retroactive)";
const adminStart = 1_760_000_000;
const adminEnd = adminStart + 86_400;

test("derived model prices display a retroactive baseline, not the Unix epoch", () => {
  assert.equal(formatModelPriceEffectiveFrom("derived", 0, locale, derivedLabel), derivedLabel);
  assert.equal(
    formatModelPriceHistoryRange({ source: "derived", effectiveFrom: 0, effectiveTo: undefined }, locale, derivedLabel),
    derivedLabel,
  );
});

test("admin generations keep their real effective timestamps and history bounds", () => {
  const start = new Date(adminStart * 1000).toLocaleString(locale);
  const end = new Date(adminEnd * 1000).toLocaleString(locale);
  assert.equal(formatModelPriceEffectiveFrom("admin", adminStart, locale, derivedLabel), start);
  assert.equal(
    formatModelPriceHistoryRange({ source: "admin", effectiveFrom: adminStart, effectiveTo: adminEnd }, locale, derivedLabel),
    `${start} → ${end}`,
  );
  assert.equal(
    formatModelPriceHistoryRange({ source: "admin", effectiveFrom: adminStart, effectiveTo: undefined }, locale, derivedLabel),
    `${start} → —`,
  );
});

test("a future time-bounded derived generation keeps its timestamp", () => {
  const start = new Date(adminStart * 1000).toLocaleString(locale);
  assert.equal(formatModelPriceEffectiveFrom("derived", adminStart, locale, derivedLabel), start);
  assert.equal(
    formatModelPriceHistoryRange({ source: "derived", effectiveFrom: adminStart, effectiveTo: null }, locale, derivedLabel),
    `${start} → —`,
  );
});

test("unpriced models have no effective timestamp", () => {
  assert.equal(formatModelPriceEffectiveFrom(null, null, locale, derivedLabel), "—");
});

test("both supported locales explain the retroactive baseline", () => {
  for (const copy of [messages.en, messages["zh-CN"]]) {
    assert.ok(copy["settings.modelPrices.effective.derived"]);
    assert.ok(copy["settings.modelPrices.effective.derivedHint"]);
  }
});
