"use client";

import * as React from "react";
import { CalendarHeatmapGrid, heatmapRateColor } from "@/components/dashboard/calendar-heatmap";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientOnlineCalendar } from "@/lib/api";
import type { ClientOnlineCalendar, ClientOnlineCalendarDay } from "@/lib/types";

export function clientOnlineColor(day?: ClientOnlineCalendarDay) {
  if (!day || day.observedMinutes === 0 || day.onlineRate == null) {
    return "bg-slate-200";
  }
  return heatmapRateColor(day.onlineRate);
}

export function ClientOnlineHeatmap({ installationId }: { installationId: string }) {
  const { locale, t } = useLocaleText();
  const [calendar, setCalendar] = React.useState<ClientOnlineCalendar | null>(null);
  const [error, setError] = React.useState(false);

  React.useEffect(() => {
    const controller = new AbortController();
    setCalendar(null);
    setError(false);
    getClientOnlineCalendar(installationId, 365, controller.signal)
      .then((response) => {
        if (!controller.signal.aborted) setCalendar(response);
      })
      .catch((cause) => {
        if (!controller.signal.aborted) {
          setError(true);
          console.warn("Client online calendar load failed", cause);
        }
      });
    return () => controller.abort();
  }, [installationId]);

  const dayFormatter = React.useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: "UTC" }),
    [locale],
  );
  const observedDays = calendar?.days.filter((day) => day.observedMinutes > 0).length ?? 0;

  return (
    <section className="grid min-w-0 gap-2" aria-label={t("dashboard.clientOnlineCalendar.title")}>
      <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-1">
        <div className="min-w-0">
          <h3 className="text-xs font-semibold text-slate-700">
            {t("dashboard.clientOnlineCalendar.title")}
          </h3>
          <p className="mt-1 min-w-0 text-[10px] leading-4 text-slate-500">
            {t("dashboard.clientOnlineCalendar.hint")}
          </p>
        </div>
        <span className="text-[10px] text-slate-400">
          {calendar
            ? t("dashboard.clientOnlineCalendar.schedule", {
                days: observedDays,
                timezone: calendar.timezone,
              })
            : "UTC"}
        </span>
      </div>

      {error ? (
        <p className="border-l-2 border-amber-300 bg-amber-50 px-2 py-1.5 text-xs text-amber-800">
          {t("dashboard.clientOnlineCalendar.unavailable")}
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
          cellClassName={clientOnlineColor}
          cellTitle={(cell) => {
            const day = cell.day;
            if (!day) return undefined;
            if (day.observedMinutes === 0 || day.onlineRate == null) {
              return t("dashboard.clientOnlineCalendar.unobservedDay", {
                date: dayFormatter.format(cell.date),
              });
            }
            return t("dashboard.clientOnlineCalendar.day", {
              date: dayFormatter.format(cell.date),
              rate: day.onlineRate.toFixed(1),
              online: day.onlineMinutes,
              observed: day.observedMinutes,
            });
          }}
        />
      ) : (
        <div className="h-[104px] animate-pulse bg-slate-100" aria-label={t("common.loading")} />
      )}
    </section>
  );
}
