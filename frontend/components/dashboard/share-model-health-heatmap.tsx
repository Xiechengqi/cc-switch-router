"use client";

import * as React from "react";
import {
  CalendarHeatmapGrid,
  buildCalendarHeatmapCells,
  heatmapRateColor,
} from "@/components/dashboard/calendar-heatmap";
import { getShareModelHealthCalendar } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type {
  ShareModelHealthCalendar,
  ShareModelHealthCalendarDay,
} from "@/lib/types";
import { SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";

export function buildHeatmapCells(calendar: ShareModelHealthCalendar) {
  return buildCalendarHeatmapCells(calendar.startDate, calendar.endDate, calendar.days);
}

export function healthColor(day?: ShareModelHealthCalendarDay) {
  if (!day || !day.active || day.expectedChecks === 0 || day.successRate == null) {
    return "bg-slate-200";
  }
  return heatmapRateColor(day.successRate);
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

  const dayFormatter = React.useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: "UTC" }),
    [locale],
  );

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
        <CalendarHeatmapGrid
          startDate={calendar.startDate}
          endDate={calendar.endDate}
          days={calendar.days}
          locale={locale}
          loadingLabel={t("common.loading")}
          lowerLabel={t("dashboard.healthCalendar.lower")}
          higherLabel={t("dashboard.healthCalendar.higher")}
          cellClassName={healthColor}
          cellTitle={(cell) => {
            const day = cell.day;
            if (!day) return undefined;
            if (!day.active) {
              return t("dashboard.healthCalendar.inactiveDay", {
                date: dayFormatter.format(cell.date),
              });
            }
            return [
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
              .join("\n");
          }}
        />
      ) : (
        <div className="h-[104px] animate-pulse bg-slate-100" aria-label={t("common.loading")} />
      )}
    </section>
  );
}
