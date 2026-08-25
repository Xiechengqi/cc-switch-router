import assert from "node:assert/strict";
import test from "node:test";

import {
  formatTokenMillions,
  millionsInputToTokens,
  tokensToMillionsInput,
  validTokenMillionsInput,
} from "./token-units";

test("formats raw token counts as exact editable million values", () => {
  assert.equal(tokensToMillionsInput(1), "0.000001");
  assert.equal(tokensToMillionsInput(100_000), "0.1");
  assert.equal(tokensToMillionsInput(1_250_000), "1.25");
  assert.equal(tokensToMillionsInput(12_000_001), "12.000001");
  assert.equal(tokensToMillionsInput(-1), "");
});

test("parses million inputs without floating point rounding", () => {
  assert.equal(millionsInputToTokens(".000001"), 1);
  assert.equal(millionsInputToTokens("0.1"), 100_000);
  assert.equal(millionsInputToTokens("1.25"), 1_250_000);
  assert.equal(millionsInputToTokens("12.000001"), 12_000_001);
  assert.equal(millionsInputToTokens("1."), 1_000_000);
  assert.equal(
    millionsInputToTokens(tokensToMillionsInput(Number.MAX_SAFE_INTEGER)),
    Number.MAX_SAFE_INTEGER,
  );
});

test("rejects values that cannot represent a whole safe token count", () => {
  assert.equal(millionsInputToTokens(""), null);
  assert.equal(millionsInputToTokens("-1"), null);
  assert.equal(millionsInputToTokens("0.0000001"), null);
  assert.equal(millionsInputToTokens("1,000"), null);
  assert.equal(millionsInputToTokens("9007199254.740992"), null);
  assert.equal(validTokenMillionsInput("0"), false);
  assert.equal(validTokenMillionsInput("0", { allowZero: true }), true);
});

test("renders configured quantities with an explicit M suffix", () => {
  assert.equal(formatTokenMillions(1, "en"), "0.000001 M");
  assert.equal(formatTokenMillions(1_250_000, "en"), "1.25 M");
  assert.equal(
    formatTokenMillions(Number.MAX_SAFE_INTEGER, "en"),
    "9,007,199,254.740991 M",
  );
  assert.equal(formatTokenMillions(1.5, "en"), "-");
});

test("renders Chinese quantities with 万 and 亿", () => {
  assert.equal(formatTokenMillions(0, "zh-CN"), "0万");
  assert.equal(formatTokenMillions(1, "zh-CN"), "0.0001万");
  assert.equal(formatTokenMillions(12_500, "zh-CN"), "1.25万");
  assert.equal(formatTokenMillions(1_250_000, "zh-CN"), "125万");
  assert.equal(formatTokenMillions(99_999_999, "zh-CN"), "9,999.9999万");
  assert.equal(formatTokenMillions(100_000_000, "zh-CN"), "1亿");
  assert.equal(formatTokenMillions(150_000_000, "zh-CN"), "1.5亿");
  assert.equal(
    formatTokenMillions(Number.MAX_SAFE_INTEGER, "zh-CN"),
    "90,071,992.54740991亿",
  );
});
