"use client";

import * as React from "react";
import { CalendarHeatmapGrid, heatmapRateColor } from "@/components/dashboard/calendar-heatmap";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientMarketHostHealthCalendar } from "@/lib/api";
import type { HostHealthCalendar, HostHealthCalendarDay } from "@/lib/types";

export function hostHealthCalendarColor(day?: HostHealthCalendarDay) {
  if (!day || day.observableHours === 0 || day.receivedRate == null) return "bg-slate-200";
  return heatmapRateColor(day.receivedRate);
}

export function HostHealthHeatmap({ hostId }: { hostId: string }) {
  const { locale, t } = useLocaleText();
  const [calendar, setCalendar] = React.useState<HostHealthCalendar | null>(null);
  const [error, setError] = React.useState(false);

  React.useEffect(() => {
    const controller = new AbortController();
    setCalendar(null);
    setError(false);
    getClientMarketHostHealthCalendar(hostId, controller.signal)
      .then((response) => {
        if (!controller.signal.aborted) setCalendar(response);
      })
      .catch((cause) => {
        if (!controller.signal.aborted) {
          setError(true);
          console.warn("Host health calendar load failed", cause);
        }
      });
    return () => controller.abort();
  }, [hostId]);

  const dayFormatter = React.useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: "UTC" }),
    [locale],
  );
  const observedDays = calendar?.days.filter((day) => day.observableHours > 0).length ?? 0;

  return (
    <section className="grid min-w-0 gap-2" aria-label={t("hostHealth.calendar.title")}>
      <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-1">
        <div className="min-w-0">
          <h3 className="text-xs font-semibold text-slate-700">{t("hostHealth.calendar.title")}</h3>
          <p className="mt-1 text-[11px] leading-4 text-muted-foreground">{t("hostHealth.calendar.hint")}</p>
        </div>
        <span className="text-[11px] text-muted-foreground">
          {calendar
            ? t("hostHealth.calendar.schedule", { days: observedDays, timezone: calendar.timezone })
            : "UTC"}
        </span>
      </div>
      {error ? (
        <p className="border-l-2 border-amber-300 bg-amber-50 px-2 py-1.5 text-xs text-amber-800">
          {t("hostHealth.calendar.unavailable")}
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
          cellClassName={hostHealthCalendarColor}
          cellTitle={(cell) => {
            const day = cell.day;
            if (!day || day.observableHours === 0 || day.receivedRate == null) {
              return t("hostHealth.calendar.unobservedDay", { date: dayFormatter.format(cell.date) });
            }
            return t("hostHealth.calendar.day", {
              date: dayFormatter.format(cell.date),
              rate: day.receivedRate.toFixed(1),
              received: day.receivedHours,
              observed: day.observableHours,
            });
          }}
        />
      ) : (
        <div className="h-[104px] animate-pulse bg-slate-100" aria-label={t("common.loading")} />
      )}
    </section>
  );
}
