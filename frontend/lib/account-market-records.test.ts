import assert from "node:assert/strict";
import test from "node:test";
import { compareAccountRecords, type AccountRecordOrder } from "./account-market-records";

test("account records prioritize work, upcoming expiry, then newest history", () => {
  const rows: AccountRecordOrder[] = [
    { id: "old", ended: true, updatedAt: "2026-01-01" },
    { id: "permanent", expiresAt: null },
    { id: "later", expiresAt: "2026-03-01" },
    { id: "urgent", attention: true, ended: true },
    { id: "soon", expiresAt: "2026-02-01" },
    { id: "recent", ended: true, updatedAt: "2026-01-02" },
  ];
  assert.deepEqual(rows.sort(compareAccountRecords).map(row => row.id), ["urgent", "soon", "later", "permanent", "recent", "old"]);
});
test("incomplete dates keep deterministic ID order instead of NaN", () => {
  assert.ok(compareAccountRecords({ id: "a", expiresAt: "bad" }, { id: "b" }) < 0);
  assert.equal(compareAccountRecords({ id: "a" }, { id: "a" }), 0);
});
