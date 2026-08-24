"use client";

import * as React from "react";
import { getShareModelHealthCalendar } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type {
  ShareModelHealthCalendar,
  ShareModelHealthCalendarDay,
} from "@/lib/types";
import { SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import { cn } from "@/lib/utils";

const CELL_SIZE = 9;
const CELL_GAP = 3;
const WEEK_WIDTH = CELL_SIZE + CELL_GAP;

type HeatmapCell = {
  date: Date;
  key: string;
  day?: ShareModelHealthCalendarDay;
};

type MonthLabel = {
  key: string;
  week: number;
  label: string;
};

function utcDate(value: string) {
  return new Date(`${value}T00:00:00Z`);
}

function dateKey(value: Date) {
  return value.toISOString().slice(0, 10);
}

function mondayIndex(value: Date) {
  return (value.getUTCDay() + 6) % 7;
}

function shiftUtcDate(value: Date, days: number) {
  const next = new Date(value);
  next.setUTCDate(next.getUTCDate() + days);
  return next;
}

export function buildHeatmapCells(calendar: ShareModelHealthCalendar) {
  const byDate = new Map(calendar.days.map((day) => [day.date, day]));
  const first = utcDate(calendar.startDate);
  const last = utcDate(calendar.endDate);
  const gridStart = shiftUtcDate(first, -mondayIndex(first));
  const gridEnd = shiftUtcDate(last, 6 - mondayIndex(last));
  const cells: HeatmapCell[] = [];
  for (let date = gridStart; date <= gridEnd; date = shiftUtcDate(date, 1)) {
    const key = dateKey(date);
    cells.push({ date, key, day: byDate.get(key) });
  }
  return cells;
}

function monthLabels(cells: HeatmapCell[], locale: string) {
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

export function healthColor(day?: ShareModelHealthCalendarDay) {
  if (!day || !day.active || day.expectedChecks === 0 || day.successRate == null) {
    return "bg-slate-200";
  }
  if (day.successRate < 50) return "bg-rose-500";
  if (day.successRate < 80) return "bg-amber-400";
  if (day.successRate < 95) return "bg-emerald-300";
  return "bg-emerald-600";
}

function appLabel(value: string) {
  return SHARE_APP_LABELS[value as CoreShareApp] || value;
}

export function ShareModelHealthHeatmap({ shareId }: { shareId: string }) {
  const { locale, t } = useLocaleText();
  const [calendar, setCalendar] = React.useState<ShareModelHealthCalendar | null>(null);
  const [error, setError] = React.useState(false);

  React.useEffect(() => {
    const controller = new AbortController();
    setCalendar(null);
    setError(false);
    getShareModelHealthCalendar(shareId, 365, controller.signal)
      .then((response) => {
        if (!controller.signal.aborted) setCalendar(response);
      })
      .catch((cause) => {
        if (!controller.signal.aborted) {
          setError(true);
          console.warn("Share model health calendar load failed", cause);
        }
      });
    return () => controller.abort();
  }, [shareId]);

  const cells = React.useMemo(() => (calendar ? buildHeatmapCells(calendar) : []), [calendar]);
  const labels = React.useMemo(() => monthLabels(cells, locale), [cells, locale]);
  const weekCount = Math.ceil(cells.length / 7);
  const dayFormatter = React.useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: "UTC" }),
    [locale],
  );
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

  return (
    <section className="grid min-w-0 gap-2" aria-label={t("dashboard.healthCalendar.title")}>
      <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-1">
        <div className="min-w-0">
          <h3 className="text-xs font-semibold text-slate-700">{t("dashboard.healthCalendar.title")}</h3>
          {calendar?.currentProbe ? (
            <p className="mt-1 min-w-0 text-[10px] leading-4 text-slate-500">
              {t("dashboard.healthCalendar.currentProbe", {
                app: appLabel(calendar.currentProbe.appType),
                model: calendar.currentProbe.requestedModel,
              })}
            </p>
          ) : calendar ? (
            <p className="mt-1 text-[10px] leading-4 text-slate-400">
              {t("dashboard.healthCalendar.inactiveNow")}
            </p>
          ) : null}
        </div>
        <span className="text-[10px] text-slate-400">
          {calendar
            ? t("dashboard.healthCalendar.schedule", {
                count: calendar.expectedChecksPerFullDay,
                timezone: calendar.timezone,
              })
            : "UTC"}
        </span>
      </div>

      {calendar?.sharedProbe ? (
        <p className="border-l-2 border-emerald-300 pl-2 text-[10px] leading-4 text-emerald-700">
          {t("dashboard.healthCalendar.sharedProbe")}
        </p>
      ) : null}
      {calendar && calendar.evidenceVersion < 2 ? (
        <p className="border-l-2 border-amber-300 pl-2 text-[10px] leading-4 text-amber-700">
          {t("dashboard.healthCalendar.legacyEvidence")}
        </p>
      ) : null}

      {error ? (
        <p className="border-l-2 border-amber-300 bg-amber-50 px-2 py-1.5 text-xs text-amber-800">
          {t("dashboard.healthCalendar.unavailable")}
        </p>
      ) : calendar ? (
        <div className="overflow-x-auto pb-1">
          <div style={{ width: 28 + weekCount * WEEK_WIDTH }}>
            <div className="relative ml-7 h-4" style={{ width: weekCount * WEEK_WIDTH }}>
              {labels.map((label) => (
                <span
                  key={label.key}
                  className="absolute top-0 whitespace-nowrap text-[9px] leading-3 text-slate-500"
                  style={{ left: label.week * WEEK_WIDTH }}
                >
                  {label.label}
                </span>
              ))}
            </div>
            <div className="flex gap-0">
              <div
                className="grid w-7 shrink-0 pr-1 text-right text-[8px] leading-[9px] text-slate-400"
                style={{ gridTemplateRows: `repeat(7, ${CELL_SIZE}px)`, rowGap: CELL_GAP }}
                aria-hidden
              >
                {weekdayLabels.map((label, index) => (
                  <span key={label}>{index % 2 === 0 ? label : ""}</span>
                ))}
              </div>
              <div
                className="grid grid-flow-col"
                style={{
                  gridTemplateRows: `repeat(7, ${CELL_SIZE}px)`,
                  gridAutoColumns: CELL_SIZE,
                  gap: CELL_GAP,
                }}
              >
                {cells.map((cell) => {
                  const day = cell.day;
                  const title = day
                    ? day.active
                      ? [
                          t("dashboard.healthCalendar.day", {
                            date: dayFormatter.format(cell.date),
                            rate: (day.successRate ?? 0).toFixed(1),
                            successful: day.successfulChecks,
                            expected: day.expectedChecks,
                            observed: day.observedChecks,
                            upstreamFailures: day.upstreamFailureChecks,
                            gaps: day.monitoringGapChecks,
                            coverage: (day.coverageRate ?? 0).toFixed(1),
                          }),
                          day.mixedEpoch ? t("dashboard.healthCalendar.mixedEpoch") : "",
                          day.evidenceVersion > 0 && day.evidenceVersion < 2
                            ? t("dashboard.healthCalendar.legacyDay")
                            : "",
                        ]
                          .filter(Boolean)
                          .join("\n")
                      : t("dashboard.healthCalendar.inactiveDay", {
                          date: dayFormatter.format(cell.date),
                        })
                    : undefined;
                  return (
                    <span
                      key={cell.key}
                      title={title}
                      role={title ? "img" : undefined}
                      aria-label={title}
                      className={cn(
                        "block rounded-[2px]",
                        day ? healthColor(day) : "bg-transparent",
                      )}
                      style={{ width: CELL_SIZE, height: CELL_SIZE }}
                    />
                  );
                })}
              </div>
            </div>
          </div>
        </div>
      ) : (
        <div className="h-[104px] animate-pulse bg-slate-100" aria-label={t("common.loading")} />
      )}

      {calendar ? (
        <div className="flex items-center justify-end gap-1.5 text-[9px] text-slate-400" aria-hidden>
          <span>{t("dashboard.healthCalendar.lower")}</span>
          {["bg-rose-500", "bg-amber-400", "bg-emerald-300", "bg-emerald-600"].map((color) => (
            <span key={color} className={cn("block rounded-[2px]", color)} style={{ width: CELL_SIZE, height: CELL_SIZE }} />
          ))}
          <span>{t("dashboard.healthCalendar.higher")}</span>
        </div>
      ) : null}
    </section>
  );
}
