import assert from "node:assert/strict";
import test from "node:test";
import {
  formatSeatQuota,
  formatSeatValueDiscount,
  formatSeatValueRatio,
  listingBestValueRatio,
  seatValueDiscount,
} from "@/components/dashboard/share-market/market-utils";
import { sortSeatRows, type SeatRowLike } from "@/components/dashboard/share-market/seat-table-utils";
import type { ShareValueQuota } from "@/lib/types";

const usd = (limitUsdMicros: string): ShareValueQuota => ({ limitUsdMicros, tariffPolicyId: "api-equivalent-v1" });
const period = (value: string) => value === "day" ? "Daily" : value;
const value = (amount: string) => `${amount} API value`;

test("a USD seat reads as API value, never as a price", () => {
  assert.equal(
    formatSeatQuota({ tokenPeriod: "day", valueQuota: usd("10500000") }, "en", "Unlimited", period, value),
    "US$10.50 API value · Daily",
  );
  assert.equal(
    formatSeatQuota({ tokenPeriod: "day", tokenLimit: 5_000_000 }, "en", "Unlimited", period, value),
    "5 M · Daily",
  );
  assert.equal(formatSeatQuota({ tokenPeriod: "lifetime" }, "en", "Unlimited", period, value), "Unlimited");
});

function row(position: number, seat: Partial<SeatRowLike["seat"]>): SeatRowLike {
  return {
    listing: { shareName: "Share", shareOnline: true, ownerEmail: "owner@example.com" },
    seat: { position, isFree: true, status: "available", readOnly: false, ...seat },
    statusKey: "available",
    searchText: "",
  };
}

test("quota sort groups Tokens, then USD, then unlimited, each by amount", () => {
  const rows = [
    row(1, {}),
    row(2, { valueQuota: usd("20000000") }),
    row(3, { tokenLimit: 9_000 }),
    row(4, { valueQuota: usd("5000000") }),
    row(5, { tokenLimit: 1_000 }),
  ];
  assert.deepEqual(
    sortSeatRows(rows, { key: "tokens", dir: "asc" }).map((item) => item.seat.position),
    [5, 3, 4, 2, 1],
  );
});

test("a USD seat's nominal discount compares 30 days of rent with 30 days of full use", () => {
  const seat = { isFree: false, valueQuota: usd("10000000"), tokenPeriod: "day" as const, cyclePriceMinor: 2_000 };
  const discount = seatValueDiscount(seat);
  assert.deepEqual(discount, { ratioBps: 667, monthlyValueMicros: "300000000", monthlyPriceMinor: 2_000 });
  assert.equal(formatSeatValueDiscount(discount!, "en"), "93% off");
  assert.equal(formatSeatValueDiscount(discount!, "zh-CN"), "0.67 折");
  assert.equal(formatSeatValueRatio(discount!, "en"), "6.67%");

  const weekly = seatValueDiscount({ ...seat, tokenPeriod: "week", cyclePriceMinor: 3_000 });
  // $30 against $10 × 30/7 ≈ $42.86 → 70%.
  assert.equal(weekly?.ratioBps, 7_000);
  assert.equal(formatSeatValueDiscount(weekly!, "zh-CN"), "7 折");

  const legacyDaily = seatValueDiscount({ ...seat, cyclePriceMinor: undefined, dailyRateMinor: 100 });
  assert.equal(legacyDaily?.ratioBps, 1_000);
});

test("no discount for free, Token, cumulative or over-priced seats", () => {
  const seat = { isFree: false, valueQuota: usd("10000000"), tokenPeriod: "calendarMonth" as const, cyclePriceMinor: 2_000 };
  assert.equal(seatValueDiscount({ ...seat, isFree: true }), null);
  assert.equal(seatValueDiscount({ ...seat, valueQuota: undefined }), null);
  assert.equal(seatValueDiscount({ ...seat, tokenPeriod: "lifetime" }), null);
  const above = seatValueDiscount({ ...seat, cyclePriceMinor: 1_200 });
  assert.equal(above?.ratioBps, 12_000);
  assert.equal(formatSeatValueDiscount(above!, "en"), null);
  assert.equal(
    listingBestValueRatio({ seats: [
      { ...seat, status: "available", readOnly: false },
      { ...seat, status: "available", readOnly: false, cyclePriceMinor: 500 },
      { ...seat, status: "occupied", readOnly: false, cyclePriceMinor: 100 },
    ] }),
    5_000,
  );
});

test("nominal discounts reject malformed money and never advertise paid rent as free", () => {
  const seat = { isFree: false, valueQuota: usd("1000000000000000"), tokenPeriod: "day" as const, cyclePriceMinor: 1 };
  const small = seatValueDiscount(seat)!;
  assert.equal(small.ratioBps, 1);
  assert.equal(formatSeatValueDiscount(small, "en"), "99% off");
  for (const cyclePriceMinor of [NaN, Infinity, -1, 0, 1.2, Number.MAX_SAFE_INTEGER + 1]) {
    assert.equal(seatValueDiscount({ ...seat, cyclePriceMinor }), null);
  }
  for (const amount of ["0", "01", "1e6", "1000000000000001"]) {
    assert.equal(seatValueDiscount({ ...seat, valueQuota: usd(amount) }), null);
  }
  assert.equal(formatSeatValueDiscount({ ...small, ratioBps: 0 }, "en"), null);
  assert.equal(formatSeatValueDiscount({ ...small, ratioBps: NaN }, "en"), null);
  assert.equal(formatSeatValueDiscount({ ...small, ratioBps: -1 }, "zh-CN"), null);
});
