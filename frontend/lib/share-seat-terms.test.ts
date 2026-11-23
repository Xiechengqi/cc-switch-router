import assert from "node:assert/strict";
import test from "node:test";
import { seatTerminationDraft, seatTerminationRateBps } from "./share-seat-terms";

test("an empty rate follows the platform and a typed rate is capped at 50%", () => {
  assert.equal(seatTerminationRateBps(""), null);
  assert.equal(seatTerminationRateBps("  "), null);
  assert.equal(seatTerminationRateBps("0"), 0);
  assert.equal(seatTerminationRateBps("12.5"), 1250);
  assert.equal(seatTerminationRateBps("50"), 5000);
  for (const invalid of ["50.01", "51", "-1", "1e1", "abc", "10.123", "100"]) {
    assert.equal(seatTerminationRateBps(invalid), undefined, invalid);
  }
});

test("a seat's draft shows only a seller-set rate; platform rates stay a placeholder", () => {
  assert.deepEqual(seatTerminationDraft(), { earlyTermination: "allowed", terminationRate: "" });
  assert.deepEqual(
    seatTerminationDraft({ terminationTerms: { version: 1, rateBps: 1000, earlyTermination: "allowed", rateSource: "router" } }),
    { earlyTermination: "allowed", terminationRate: "" },
  );
  assert.deepEqual(
    seatTerminationDraft({ terminationTerms: { version: 1, rateBps: 2550, earlyTermination: "forbidden", rateSource: "seat" } }),
    { earlyTermination: "forbidden", terminationRate: "25.5" },
  );
});
