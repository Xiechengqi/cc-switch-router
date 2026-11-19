import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
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

test("normalized LiteLLM keys share one entry when their derived prices agree", (t) => {
  const directory = mkdtempSync(join(tmpdir(), "router-price-keys-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = join(directory, "source.json");
  const output = join(directory, "prices.json");
  const script = fileURLToPath(new URL("./derive-model-prices.mjs", import.meta.url));
  const derive = (canonical) => {
    writeFileSync(source, JSON.stringify({ "provider/Model": standard, "provider/model": canonical }));
    return spawnSync(process.execPath, [script, "--source", source, "--out", output], { encoding: "utf8" });
  };

  const matching = derive({ ...standard, metadata: { note: "case alias" } });
  assert.equal(matching.status, 0, matching.stderr);
  const catalog = JSON.parse(readFileSync(output, "utf8"));
  assert.deepEqual(catalog.models.map((model) => model.priceKey), ["provider/model"]);
  assert.match(catalog.models[0].sourceNote, /entry `provider\/model`/);
  assert.deepEqual(catalog.aliases.map((alias) => alias.priceKey), ["provider/model"]);

  const conflicting = derive({ ...standard, output_cost_per_token: 0.000003 });
  assert.equal(conflicting.status, 1);
  assert.match(conflicting.stderr, /conflicting LiteLLM prices for provider\/model/);
});
