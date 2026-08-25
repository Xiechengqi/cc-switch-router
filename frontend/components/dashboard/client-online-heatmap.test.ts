import assert from "node:assert/strict";
import test from "node:test";
import { buildCalendarHeatmapCells } from "@/components/dashboard/calendar-heatmap";
import { clientOnlineColor } from "@/components/dashboard/client-online-heatmap";
import type { ClientOnlineCalendarDay } from "@/lib/types";

test("client online heatmap keeps observed days on their UTC cells", () => {
  const day: ClientOnlineCalendarDay = {
    date: "2026-08-27",
    onlineMinutes: 80,
    observedMinutes: 100,
    onlineRate: 80,
  };
  const cells = buildCalendarHeatmapCells("2026-08-27", "2026-08-27", [day]);
  assert.equal(cells.find((cell) => cell.key === day.date)?.day, day);
});

test("client online heatmap colors follow observed uptime", () => {
  const day: ClientOnlineCalendarDay = {
    date: "2026-08-27",
    onlineMinutes: 40,
    observedMinutes: 100,
    onlineRate: 40,
  };
  assert.equal(clientOnlineColor(day), "bg-rose-500");
  assert.equal(clientOnlineColor({ ...day, onlineRate: 80, onlineMinutes: 80 }), "bg-emerald-300");
  assert.equal(clientOnlineColor({ ...day, observedMinutes: 0, onlineRate: undefined }), "bg-slate-200");
});
