import assert from "node:assert/strict";
import test from "node:test";

import {
  formatPercent,
  formatUsdMicros,
  formatUsdMicrosPerMillion,
  formatUsdMicrosRange,
  parseUsdMicros,
} from "./usd-micros";

test("parses micro-USD strings without going through Number", () => {
  assert.equal(parseUsdMicros("0"), 0n);
  assert.equal(parseUsdMicros("12400000"), 12_400_000n);
  assert.equal(parseUsdMicros("-500"), -500n);
  assert.equal(parseUsdMicros(" 42 "), 42n);
  assert.equal(parseUsdMicros("1.5"), null);
  assert.equal(parseUsdMicros(""), null);
  assert.equal(parseUsdMicros(null), null);
});

test("keeps full precision past the float-safe integer range", () => {
  // 2^53 micro-USD is only about $9.0e9, which a long window can exceed.
  const huge = "9007199254740993";
  assert.equal(parseUsdMicros(huge), 9_007_199_254_740_993n);
  assert.equal(formatUsdMicros(huge, "en-US"), "$9,007,199,254.74");
});

test("formats amounts at two decimals by default", () => {
  assert.equal(formatUsdMicros("12400000", "en-US"), "$12.40");
  assert.equal(formatUsdMicros("0", "en-US"), "$0.00");
  assert.equal(formatUsdMicros("-1500000", "en-US"), "-$1.50");
});

test("widens small amounts instead of rendering them as zero", () => {
  assert.equal(formatUsdMicros("300", "en-US"), "$0.0003");
  assert.equal(formatUsdMicros("1", "en-US"), "$0.000001");
  assert.equal(formatUsdMicros("5000", "en-US"), "$0.005");
  // A value that legitimately rounds to two decimals stays at two.
  assert.equal(formatUsdMicros("120000", "en-US"), "$0.12");
});

test("rounds half away from zero at the display scale only", () => {
  assert.equal(formatUsdMicros("1005000", "en-US"), "$1.01");
  assert.equal(formatUsdMicros("1004999", "en-US"), "$1.00");
  assert.equal(formatUsdMicros("999995000", "en-US"), "$1,000.00");
});

test("collapses a degenerate interval to a single value", () => {
  assert.equal(formatUsdMicrosRange("12400000", "12400000", "en-US"), "$12.40");
  assert.equal(formatUsdMicrosRange("12400000", null, "en-US"), "$12.40");
  // An upper bound below the point estimate is nonsense; never render it.
  assert.equal(formatUsdMicrosRange("12400000", "12000000", "en-US"), "$12.40");
  assert.equal(
    formatUsdMicrosRange("12400000", "13280000", "en-US"),
    "$12.40 ~ $13.28",
  );
  assert.equal(formatUsdMicrosRange(null, "13280000", "en-US"), null);
});

test("formats per-1M rates through the same rounding as amounts", () => {
  assert.equal(formatUsdMicrosPerMillion(3_000_000, "en-US"), "$3.00");
  assert.equal(formatUsdMicrosPerMillion("300000", "en-US"), "$0.30");
  assert.equal(formatUsdMicrosPerMillion(null, "en-US"), null);
});

test("formats percentages with one decimal", () => {
  assert.equal(formatPercent(97.3, "en-US"), "97.3");
  assert.equal(formatPercent(100, "en-US"), "100.0");
  assert.equal(formatPercent(Number.NaN, "en-US"), "-");
});
