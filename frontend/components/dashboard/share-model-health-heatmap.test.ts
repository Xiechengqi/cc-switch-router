import assert from "node:assert/strict";
import test from "node:test";
import {
  buildHeatmapCells,
  healthColor,
} from "@/components/dashboard/share-model-health-heatmap";
import type { ShareModelHealthCalendar } from "@/lib/types";

function calendar(
  startDate: string,
  endDate: string,
  days: ShareModelHealthCalendar["days"] = [],
): ShareModelHealthCalendar {
  return {
    shareId: "share-test",
    timezone: "UTC",
    expectedChecksPerFullDay: 48,
    startDate,
    endDate,
    days,
    epochs: [],
    sharedProbe: false,
    evidenceVersion: 2,
  };
}

test("heatmap pads a UTC date range to complete Monday-first weeks", () => {
  const cells = buildHeatmapCells(calendar("2026-08-26", "2026-08-27"));

  assert.equal(cells.length, 7);
  assert.equal(cells[0]?.key, "2026-08-24");
  assert.equal(cells[6]?.key, "2026-08-30");
  assert.equal(cells[0]?.date.getUTCDay(), 1);
  assert.equal(cells[6]?.date.getUTCDay(), 0);
});

test("heatmap preserves the Router day aggregate on its UTC date cell", () => {
  const day = {
    date: "2026-08-27",
    active: true,
    expectedChecks: 48,
    completedChecks: 47,
    successfulChecks: 46,
    observedChecks: 47,
    upstreamFailureChecks: 1,
    monitoringGapChecks: 1,
    successRate: (46 / 48) * 100,
    coverageRate: (47 / 48) * 100,
    mixedEpoch: false,
    evidenceVersion: 2,
  };
  const cells = buildHeatmapCells(calendar("2026-08-27", "2026-08-27", [day]));

  assert.equal(cells.find((cell) => cell.key === day.date)?.day, day);
  assert.equal(cells.filter((cell) => cell.day).length, 1);
});

test("heatmap color uses successful eligible slots, including monitoring gaps in the denominator", () => {
  const day: ShareModelHealthCalendar["days"][number] = {
    date: "2026-08-27",
    active: true,
    expectedChecks: 48,
    completedChecks: 48,
    successfulChecks: 38,
    observedChecks: 40,
    upstreamFailureChecks: 2,
    monitoringGapChecks: 8,
    successRate: (38 / 48) * 100,
    coverageRate: (40 / 48) * 100,
    mixedEpoch: true,
    evidenceVersion: 2,
  };

  assert.equal(healthColor(day), "bg-amber-400");
  assert.equal(healthColor({ ...day, successRate: 80 }), "bg-emerald-300");
  assert.equal(healthColor({ ...day, successRate: 95 }), "bg-emerald-600");
  assert.equal(healthColor({ ...day, active: false }), "bg-slate-200");
});
