import assert from "node:assert/strict";
import test from "node:test";
import { parseResourceRate, resourceRateInput, RESOURCE_UNITS } from "./resource-prices";
test("resource prices use exact native units, preserve zero and reject lost precision", () => {
  assert.equal(parseResourceRate("0.0125", RESOURCE_UNITS["image.output"]), "12500000000");
  assert.equal(parseResourceRate("0.05", RESOURCE_UNITS["video.output_millisecond"]), "50000000");
  assert.equal(parseResourceRate("2.50", RESOURCE_UNITS["audio.input_token"]), "2500000");
  assert.equal(resourceRateInput("2500000", 1000000n), "2.5");
  assert.equal(parseResourceRate("0", 1n), "0");
  for (const invalid of ["", "-1", "1e3", "NaN", "0.0000000000001", "9223372036854775808"]) assert.equal(parseResourceRate(invalid, 1n), undefined);
  assert.equal(parseResourceRate("0.000000000001", 1000n), undefined);
});
