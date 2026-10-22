import test from "node:test";
import assert from "node:assert/strict";
import { commissionPreview } from "./market-commission";

test("service fee preview conserves gross ledger units at all rates", () => {
  for (const price of ["0.01", "1", "10", "19.99", "999999.99"]) {
    for (const rate of [0, 1, 500, 1000, 1234, 9999, 10000]) {
      const result = commissionPreview(price, rate)!;
      assert.equal(BigInt(result.grossUnits), BigInt(result.feeUnits) + BigInt(result.netUnits));
    }
  }
  assert.deepEqual(commissionPreview("100", 1000), { grossUnits: "864000000", feeUnits: "86400000", netUnits: "777600000" });
  assert.equal(commissionPreview("0.01", 10000)?.netUnits, "0");
});
test("fee preview rejects invalid amounts/rates without floating point money", () => {
  for (const price of ["", "0", "-1", "1.001", "1e2", "Infinity", "9007199254740992"]) assert.equal(commissionPreview(price, 1000), null);
  for (const rate of [-1, 10001, 1.5, NaN]) assert.equal(commissionPreview("10", rate), null);
});
