import assert from "node:assert/strict";
import test from "node:test";
import { parseTopupMethod, topupMethodOptions } from "./topup-methods";

test("only an explicit supported Router category can become a top-up method", () => {
  assert.equal(parseTopupMethod("crypto"), "crypto");
  for (const value of [undefined, null, "dujiaopay", "unknown", "toString", {}]) {
    assert.equal(parseTopupMethod(value), null);
  }
});

test("each payment method appears once, represented by the first listed channel", () => {
  const options = topupMethodOptions([
    { id: "first", methodCategory: "crypto" },
    { id: "legacy-duplicate", methodCategory: "crypto" },
    { id: "unsupported", methodCategory: "unknown" },
    { id: "missing-category" },
  ]);
  assert.deepEqual(options.map((option) => [option.method, option.channel.id]), [["crypto", "first"]]);
});
