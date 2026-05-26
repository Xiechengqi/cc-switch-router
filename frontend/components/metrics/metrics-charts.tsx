"use client";

import { Card } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { fixed } from "@/lib/utils";
import { type ChartSeries, polyline } from "./metrics-utils";

const WIDTH = 600;
const HEIGHT = 160;

export function ResourceTrendChart({
  title,
  series,
  timestamps,
  maxMode = "percent",
}: {
  title: string;
  series: ChartSeries[];
  timestamps: number[];
  maxMode?: "percent" | "auto";
}) {
  const { t } = useLocaleText();
  const [hover, setHover] = React.useState<number | null>(null);
  const rafRef = React.useRef<number | null>(null);

  const max =
    maxMode === "percent" ? 100 : Math.max(1, ...series.flatMap((s) => s.values));
  const hasData = timestamps.length > 1 && series.some((s) => s.values.length > 1);

  const lines = React.useMemo(
    () =>
      series.map((item) => ({
        label: item.label,
        color: item.color,
        points: polyline(item.values, WIDTH, HEIGHT, max),
      })),
    [series, max],
  );

  const xLabels = React.useMemo(() => {
    if (timestamps.length < 2) return [];
    const last = timestamps.length - 1;
    return [0, 1 / 3, 2 / 3, 1].map((frac) => {
      const idx = Math.round(frac * last);
      return new Date(timestamps[idx] * 1000).toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
      });
    });
  }, [timestamps]);

  React.useEffect(
    () => () => {
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    },
    [],
  );

  const handleMove = (event: React.MouseEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const clientX = event.clientX;
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(() => {
      const idx = Math.round(((clientX - rect.left) / rect.width) * Math.max(timestamps.length - 1, 0));
      setHover(Math.max(0, Math.min(timestamps.length - 1, idx)));
    });
  };

  const handleLeave = () => {
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    setHover(null);
  };

  const hoverPct = hover !== null ? (hover / Math.max(timestamps.length - 1, 1)) * 100 : 0;
  const flip = hoverPct > 60;

  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{title}</Card.Title>
        <Card.Description>
          {hasData ? t("metrics.chart.samples", { count: timestamps.length }) : t("metrics.chart.sampling")}
        </Card.Description>
      </Card.Header>
      <Card.Content>
        <div className="mb-3 flex flex-wrap gap-3 text-xs text-muted-foreground">
          {series.map((item) => (
            <span key={item.label} className="flex items-center gap-1">
              <span className="h-2 w-2 rounded-full" style={{ background: item.color }} />
              {item.label}
            </span>
          ))}
        </div>
        {!hasData ? (
          <div className="rounded-lg border border-dashed py-10 text-center text-sm text-muted-foreground">
            {t("metrics.chart.noData")}
          </div>
        ) : (
          <>
            <div className="flex">
              <div className="flex h-40 w-10 flex-col justify-between pr-2 text-right text-[10px] text-muted-foreground">
                <span>{fixed(max)}</span>
                <span>{fixed(max / 2)}</span>
                <span>0</span>
              </div>
              <div className="relative flex-1" onMouseMove={handleMove} onMouseLeave={handleLeave}>
                <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" className="h-40 w-full">
                  <line x1={0} x2={WIDTH} y1={HEIGHT / 2} y2={HEIGHT / 2} stroke="#E2E8F0" strokeWidth="1" strokeDasharray="4 4" />
                  {lines.map((item) => (
                    <polyline key={item.label} fill="none" stroke={item.color} strokeWidth="2" points={item.points} />
                  ))}
                  {hover !== null ? (
                    <line x1={(hoverPct / 100) * WIDTH} x2={(hoverPct / 100) * WIDTH} y1={0} y2={HEIGHT} stroke="#CBD5E1" />
                  ) : null}
                </svg>
                {hover !== null && timestamps[hover] ? (
                  <div
                    className="pointer-events-none absolute top-2 z-10 rounded-lg border bg-background p-3 text-xs shadow"
                    style={flip ? { right: `${100 - hoverPct}%` } : { left: `${hoverPct}%` }}
                  >
                    <p className="font-medium">{new Date(timestamps[hover] * 1000).toLocaleTimeString()}</p>
                    {series.map((item) => (
                      <p key={item.label} style={{ color: item.color }}>
                        {item.label}: {fixed(item.values[hover])}
                      </p>
                    ))}
                  </div>
                ) : null}
              </div>
            </div>
            <div className="mt-1 flex justify-between pl-10 text-[10px] text-muted-foreground">
              {xLabels.map((label, index) => (
                <span key={`${label}-${index}`}>{label}</span>
              ))}
            </div>
          </>
        )}
      </Card.Content>
    </Card>
  );
}
