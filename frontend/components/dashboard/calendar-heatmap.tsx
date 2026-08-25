"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

export const HEATMAP_CELL_SIZE = 9;
export const HEATMAP_CELL_GAP = 3;
export const HEATMAP_WEEK_WIDTH = HEATMAP_CELL_SIZE + HEATMAP_CELL_GAP;

export type CalendarHeatmapDay = {
  date: string;
};

export type CalendarHeatmapCell<T extends CalendarHeatmapDay> = {
  date: Date;
  key: string;
  day?: T;
};

type MonthLabel = {
  key: string;
  week: number;
  label: string;
};

export function utcDate(value: string) {
  return new Date(`${value}T00:00:00Z`);
}

export function dateKey(value: Date) {
  return value.toISOString().slice(0, 10);
}

function mondayIndex(value: Date) {
  return (value.getUTCDay() + 6) % 7;
}

export function shiftUtcDate(value: Date, days: number) {
  const next = new Date(value);
  next.setUTCDate(next.getUTCDate() + days);
  return next;
}

export function buildCalendarHeatmapCells<T extends CalendarHeatmapDay>(
  startDate: string,
  endDate: string,
  days: T[],
) {
  const byDate = new Map(days.map((day) => [day.date, day]));
  const first = utcDate(startDate);
  const last = utcDate(endDate);
  const gridStart = shiftUtcDate(first, -mondayIndex(first));
  const gridEnd = shiftUtcDate(last, 6 - mondayIndex(last));
  const cells: CalendarHeatmapCell<T>[] = [];
  for (let date = gridStart; date <= gridEnd; date = shiftUtcDate(date, 1)) {
    const key = dateKey(date);
    cells.push({ date, key, day: byDate.get(key) });
  }
  return cells;
}

function monthLabels(cells: Array<{ date: Date }>, locale: string) {
  const formatter = new Intl.DateTimeFormat(locale, { month: "short", timeZone: "UTC" });
  const starts = new Map<number, Date>();
  if (cells[0]) starts.set(0, cells[0].date);
  cells.forEach((cell, index) => {
    if (cell.date.getUTCDate() === 1) starts.set(Math.floor(index / 7), cell.date);
  });
  return [...starts.entries()]
    .sort(([left], [right]) => left - right)
    .map<MonthLabel>(([week, date]) => ({
      key: `${week}:${date.getUTCFullYear()}-${date.getUTCMonth()}`,
      week,
      label: formatter.format(date),
    }));
}

export function heatmapRateColor(rate?: number | null) {
  if (rate == null) return "bg-slate-200";
  if (rate < 50) return "bg-rose-500";
  if (rate < 80) return "bg-amber-400";
  if (rate < 95) return "bg-emerald-300";
  return "bg-emerald-600";
}

export function CalendarHeatmapGrid<T extends CalendarHeatmapDay>({
  startDate,
  endDate,
  days,
  locale,
  loadingLabel,
  lowerLabel,
  higherLabel,
  cellClassName,
  cellTitle,
}: {
  startDate: string;
  endDate: string;
  days: T[];
  locale: string;
  loadingLabel: string;
  lowerLabel: string;
  higherLabel: string;
  cellClassName: (day?: T) => string;
  cellTitle: (cell: CalendarHeatmapCell<T>) => string | undefined;
}) {
  const cells = React.useMemo(
    () => buildCalendarHeatmapCells(startDate, endDate, days),
    [startDate, endDate, days],
  );
  const labels = React.useMemo(() => monthLabels(cells, locale), [cells, locale]);
  const weekCount = Math.ceil(cells.length / 7);
  const weekdayFormatter = React.useMemo(
    () => new Intl.DateTimeFormat(locale, { weekday: "short", timeZone: "UTC" }),
    [locale],
  );
  const weekdayLabels = React.useMemo(() => {
    const monday = new Date("2026-08-24T00:00:00Z");
    return Array.from({ length: 7 }, (_, index) =>
      weekdayFormatter.format(shiftUtcDate(monday, index)),
    );
  }, [weekdayFormatter]);

  if (!cells.length) {
    return <div className="h-[104px] animate-pulse bg-slate-100" aria-label={loadingLabel} />;
  }

  return (
    <>
      <div className="overflow-x-auto pb-1">
        <div style={{ width: 28 + weekCount * HEATMAP_WEEK_WIDTH }}>
          <div className="relative ml-7 h-4" style={{ width: weekCount * HEATMAP_WEEK_WIDTH }}>
            {labels.map((label) => (
              <span
                key={label.key}
                className="absolute top-0 whitespace-nowrap text-[9px] leading-3 text-slate-500"
                style={{ left: label.week * HEATMAP_WEEK_WIDTH }}
              >
                {label.label}
              </span>
            ))}
          </div>
          <div className="flex gap-0">
            <div
              className="grid w-7 shrink-0 pr-1 text-right text-[8px] leading-[9px] text-slate-400"
              style={{ gridTemplateRows: `repeat(7, ${HEATMAP_CELL_SIZE}px)`, rowGap: HEATMAP_CELL_GAP }}
              aria-hidden
            >
              {weekdayLabels.map((label, index) => (
                <span key={label}>{index % 2 === 0 ? label : ""}</span>
              ))}
            </div>
            <div
              className="grid grid-flow-col"
              style={{
                gridTemplateRows: `repeat(7, ${HEATMAP_CELL_SIZE}px)`,
                gridAutoColumns: HEATMAP_CELL_SIZE,
                gap: HEATMAP_CELL_GAP,
              }}
            >
              {cells.map((cell) => {
                const title = cellTitle(cell);
                return (
                  <span
                    key={cell.key}
                    title={title}
                    role={title ? "img" : undefined}
                    aria-label={title}
                    className={cn("block rounded-[2px]", cell.day ? cellClassName(cell.day) : "bg-transparent")}
                    style={{ width: HEATMAP_CELL_SIZE, height: HEATMAP_CELL_SIZE }}
                  />
                );
              })}
            </div>
          </div>
        </div>
      </div>
      <div className="flex items-center justify-end gap-1.5 text-[9px] text-slate-400" aria-hidden>
        <span>{lowerLabel}</span>
        {["bg-rose-500", "bg-amber-400", "bg-emerald-300", "bg-emerald-600"].map((color) => (
          <span
            key={color}
            className={cn("block rounded-[2px]", color)}
            style={{ width: HEATMAP_CELL_SIZE, height: HEATMAP_CELL_SIZE }}
          />
        ))}
        <span>{higherLabel}</span>
      </div>
    </>
  );
}
