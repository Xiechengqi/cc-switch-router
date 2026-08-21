import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_CATALOG_AVAILABILITY,
  catalogSeatPreview,
  initialCatalogSeat,
  preserveCatalogSeat,
} from "./buyer-catalog-utils";

const seat = (id: string, status = "available", readOnly = false) => ({ id, status, readOnly });

test("catalog defaults to idle seats", () => {
  assert.equal(DEFAULT_CATALOG_AVAILABILITY, "idle");
});

test("multiple idle seats require an explicit selection", () => {
  assert.equal(initialCatalogSeat([seat("a"), seat("b")]), undefined);
  assert.equal(initialCatalogSeat([seat("a")])?.id, "a");
});

test("a selected seat is preserved by id without falling back", () => {
  assert.equal(preserveCatalogSeat([seat("b")], "a"), undefined);
  assert.equal(preserveCatalogSeat([seat("a"), seat("b")], "b")?.id, "b");
});

test("compact cards preview at most two idle seats", () => {
  assert.deepEqual(
    catalogSeatPreview([seat("a"), seat("busy", "occupied"), seat("b"), seat("c")]).map((item) => item.id),
    ["a", "b"],
  );
});
