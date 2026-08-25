import assert from "node:assert/strict";
import test from "node:test";

import {
  isHistorySubscription,
  needsRentalAttention,
  partitionShareMarketSubscriptions,
  sortShareMarketSubscriptions,
} from "./subscription-utils";
import type { ShareMarketSubscription } from "@/lib/types";

const subscription = (
  id: string,
  status: string,
  extra: Partial<ShareMarketSubscription> = {},
) => ({
  id,
  shareName: extra.shareName || id,
  status,
  updatedAt: extra.updatedAt || "2026-01-01T00:00:00Z",
  integrityState: extra.integrityState || "compatible",
  ...extra,
}) as ShareMarketSubscription;

test("history is completed releases only", () => {
  assert.equal(isHistorySubscription("released"), true);
  assert.equal(isHistorySubscription("grant_failed"), false);
  assert.equal(isHistorySubscription("active_free"), false);
});

test("attention covers failures, integrity, and pending price changes", () => {
  assert.equal(needsRentalAttention(subscription("a", "grant_failed")), true);
  assert.equal(needsRentalAttention(subscription("b", "billing_suspended")), true);
  assert.equal(needsRentalAttention(subscription("c", "revoke_failed")), true);
  assert.equal(needsRentalAttention(subscription("d", "billing_control_failed")), true);
  assert.equal(needsRentalAttention(subscription("e", "active_free", { integrityState: "violated" })), true);
  assert.equal(needsRentalAttention(subscription("f", "active_postpaid", {
    priceChange: { id: "pc", status: "pending" } as ShareMarketSubscription["priceChange"],
  })), true);
  assert.equal(needsRentalAttention(subscription("g", "grant_pending")), false);
  assert.equal(needsRentalAttention(subscription("h", "active_free")), false);
  assert.equal(needsRentalAttention(subscription("i", "released")), false);
});

test("partitions grant_failed into attention and released into history", () => {
  const failed = subscription("failed", "grant_failed");
  const pendingPrice = subscription("price", "active_postpaid", {
    priceChange: { id: "pc", status: "pending" } as ShareMarketSubscription["priceChange"],
  });
  const healthy = subscription("live", "active_free");
  const granting = subscription("granting", "grant_pending");
  const released = subscription("done", "released");
  const partitioned = partitionShareMarketSubscriptions([
    released,
    healthy,
    failed,
    granting,
    pendingPrice,
  ]);

  assert.deepEqual(partitioned.attention.map((item) => item.id), ["failed", "price"]);
  assert.deepEqual(partitioned.active.map((item) => item.id), ["granting", "live"]);
  assert.deepEqual(partitioned.history.map((item) => item.id), ["done"]);
});

test("released rows stay in history even with leftover integrity flags", () => {
  const released = subscription("done", "released", { integrityState: "terminated" });
  const partitioned = partitionShareMarketSubscriptions([released]);
  assert.deepEqual(partitioned.history.map((item) => item.id), ["done"]);
  assert.deepEqual(partitioned.attention, []);
});

test("sorts failed statuses ahead of healthy rentals", () => {
  const healthy = subscription("live", "active_free", { updatedAt: "2026-02-01T00:00:00Z" });
  const failed = subscription("failed", "revoke_failed", { updatedAt: "2026-01-01T00:00:00Z" });
  assert.deepEqual(
    [healthy, failed].sort(sortShareMarketSubscriptions).map((item) => item.id),
    ["failed", "live"],
  );
});
