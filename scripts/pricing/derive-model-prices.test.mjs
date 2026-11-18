import assert from "node:assert/strict";
import test from "node:test";
import { buildRateRow } from "./derive-model-prices.mjs";

const standard = {
  input_cost_per_token: 0.000001,
  output_cost_per_token: 0.000002,
  cache_read_input_token_cost: 0.0000001,
};
test("ultrafast never derives a row from standard or a partial tier price", () => {
  assert.equal(buildRateRow(standard, "_ultrafast", null), null);
  assert.equal(buildRateRow({ ...standard, input_cost_per_token_ultrafast: 0.00001 }, "_ultrafast", null), null);
  assert.equal(buildRateRow({ ...standard, input_cost_per_token_ultrafast: 0.00001, output_cost_per_token_ultrafast: 0.00002 }, "_ultrafast", null), null);
});
test("explicit ultrafast dimensions use exact decimal pricing and their own cache rates", () => {
  const built = buildRateRow({
    ...standard,
    input_cost_per_token_ultrafast: 0.00001,
    output_cost_per_token_ultrafast: 0.00002,
    cache_read_input_token_cost_ultrafast: 0.000001,
    cache_creation_input_token_cost_above_1hr: 0.0002,
  }, "_ultrafast", null);
  assert.equal(built.row.input, 10_000_000n);
  assert.equal(built.row.output, 20_000_000n);
  assert.equal(built.row.cacheRead, 1_000_000n);
  assert.equal(built.row.cacheWrite1h, null);
  assert.equal(built.tierExplicit, true);
});
