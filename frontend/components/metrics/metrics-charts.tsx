"use client";

import { Card } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { compactNumber, fixed } from "@/lib/utils";
import { type ChartSeries, polyline } from "./metrics-utils";

const WIDTH = 600;
const HEIGHT = 192;

export type ChartState =
  | "loading"
  | "disabled"
  | "stale"
  | "no-data"
  | "no-traffic"
  | "ready";

/**
 * The chart treats "no data" as a first-class state. Callers pass the source
 * of "no traffic" (e.g. `series.flatMap(...).every(v => v === 0)`) so we can
 * tell "the bucket axis is full but every value is zero" (no traffic) apart
 * from "the bucket axis is empty" (no data) and "metrics are turned off in
 * config" (disabled). Each gets its own affordance so the user knows whether
 * to wait, generate traffic, or fix configuration.
 */
export function ResourceTrendChart({
  title,
  series,
  timestamps,
  maxMode = "percent",
  unit,
  state = "ready",
  hint,
}: {
  title: string;
  series: ChartSeries[];
  timestamps: number[];
  maxMode?: "percent" | "auto";
  unit?: string;
  state?: ChartState;
  hint?: string;
}) {
  const { locale, t } = useLocaleText();
  const [hover, setHover] = React.useState<number | null>(null);
  const [hidden, setHidden] = React.useState<Set<string>>(() => new Set());
  const rafRef = React.useRef<number | null>(null);

  const visibleSeries = series.filter((item) => !hidden.has(item.label));
  const max =
    maxMode === "percent"
      ? 100
      : Math.max(1, ...visibleSeries.flatMap((s) => s.values));
  const hasBuckets = timestamps.length > 1;
  const allZero = series.every((s) => s.values.every((v) => !Number.isFinite(v) || v === 0));
  const resolvedState: ChartState =
    state === "ready"
      ? hasBuckets
        ? allZero
          ? "no-traffic"
          : "ready"
        : "no-data"
      : state;

  const lines = React.useMemo(
    () =>
      visibleSeries.map((item) => ({
        label: item.label,
        color: item.color,
        points: polyline(item.values, WIDTH, HEIGHT, max),
      })),
    [visibleSeries, max],
  );

  const xLabels = React.useMemo(() => {
    if (timestamps.length < 2) return [];
    const last = timestamps.length - 1;
    return [0, 1 / 3, 2 / 3, 1].map((frac) => {
      const idx = Math.round(frac * last);
      return new Date(timestamps[idx] * 1000).toLocaleTimeString(locale, {
        hour: "2-digit",
        minute: "2-digit",
      });
    });
  }, [locale, timestamps]);

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

  const description = (() => {
    switch (resolvedState) {
      case "loading":
        return t("metrics.chart.sampling");
      case "disabled":
        return t("metrics.chart.disabled");
      case "stale":
        return t("metrics.chart.stale");
      case "no-data":
        return t("metrics.chart.noData");
      case "no-traffic":
        return t("metrics.chart.noTraffic");
      default:
        return t("metrics.chart.samples", { count: timestamps.length });
    }
  })();

  const showEmpty = resolvedState !== "ready" && resolvedState !== "no-traffic";

  return (
    <Card className="rounded-2xl">
      <Card.Header>
        <Card.Title className="text-base font-semibold">{title}</Card.Title>
        <Card.Description>
          <span className="text-xs text-muted-foreground">{description}</span>
          {hint ? <span className="ml-2 text-[10px] text-muted-foreground/70">{hint}</span> : null}
        </Card.Description>
      </Card.Header>
      <Card.Content>
        <Legend
          series={series}
          hidden={hidden}
          onToggle={(label) =>
            setHidden((prev) => {
              const next = new Set(prev);
              if (next.has(label)) next.delete(label);
              else next.add(label);
              return next;
            })
          }
        />
        {showEmpty ? (
          <EmptyState state={resolvedState} unit={unit} />
        ) : (
          <>
            <div className="flex">
              <div className="flex h-48 w-12 flex-col justify-between pr-2 text-right text-[10px] text-muted-foreground/80">
                <span>{formatAxis(max, unit)}</span>
                <span>{formatAxis(max * 0.75, unit)}</span>
                <span>{formatAxis(max / 2, unit)}</span>
                <span>{formatAxis(max * 0.25, unit)}</span>
                <span>{formatAxis(0, unit)}</span>
              </div>
              <div className="relative flex-1" onMouseMove={handleMove} onMouseLeave={handleLeave}>
                <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" className="h-48 w-full">
                  {[0.25, 0.5, 0.75].map((frac) => (
                    <line
                      key={frac}
                      x1={0}
                      x2={WIDTH}
                      y1={HEIGHT * frac}
                      y2={HEIGHT * frac}
                      stroke="#E2E8F0"
                      strokeWidth="1"
                      strokeDasharray={frac === 0.5 ? "0" : "3 5"}
                    />
                  ))}
                  <line x1={0} x2={WIDTH} y1={HEIGHT - 0.5} y2={HEIGHT - 0.5} stroke="#CBD5E1" strokeWidth="1" />
                  {resolvedState === "no-traffic" ? (
                    <text
                      x={WIDTH / 2}
                      y={HEIGHT / 2 + 4}
                      textAnchor="middle"
                      fontSize="11"
                      fill="#94A3B8"
                    >
                      {t("metrics.chart.noTraffic")}
                    </text>
                  ) : null}
                  {lines.map((item) => (
                    <polyline key={item.label} fill="none" stroke={item.color} strokeWidth="1.8" points={item.points} />
                  ))}
                  {hover !== null ? (
                    <line x1={(hoverPct / 100) * WIDTH} x2={(hoverPct / 100) * WIDTH} y1={0} y2={HEIGHT} stroke="#CBD5E1" strokeDasharray="2 2" />
                  ) : null}
                </svg>
                {hover !== null && timestamps[hover] ? (
                  <div
                    className="pointer-events-none absolute top-2 z-10 min-w-[140px] rounded-lg border bg-background/95 p-2 text-xs shadow-md backdrop-blur"
                    style={flip ? { right: `${100 - hoverPct}%` } : { left: `${hoverPct}%` }}
                  >
                    <p className="font-medium">
                      {new Date(timestamps[hover] * 1000).toLocaleTimeString(locale)}
                    </p>
                    {series.map((item) => (
                      <p
                        key={item.label}
                        className={hidden.has(item.label) ? "opacity-30" : ""}
                        style={{ color: item.color }}
                      >
                        {item.label}: {formatAxis(item.values[hover], unit)}
                      </p>
                    ))}
                  </div>
                ) : null}
              </div>
            </div>
            <div className="mt-1 flex justify-between pl-12 text-[10px] text-muted-foreground">
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

export function ClockOffsetChart({
  title,
  values,
  timestamps,
  state = "ready",
}: {
  title: string;
  values: Array<number | null | undefined>;
  timestamps: number[];
  state?: ChartState;
}) {
  const { locale, t } = useLocaleText();
  const [hover, setHover] = React.useState<number | null>(null);
  const finiteValues = values.filter((value): value is number => Number.isFinite(value));
  const resolvedState: ChartState =
    state === "ready" && finiteValues.length < 2 ? "no-data" : state;
  const minValue = finiteValues.length ? Math.min(0, ...finiteValues) : 0;
  const maxValue = finiteValues.length ? Math.max(0, ...finiteValues) : 0;
  const baseSpan = Math.max(maxValue - minValue, 1_000);
  const min = minValue - baseSpan * 0.12;
  const max = maxValue + baseSpan * 0.12;
  const zeroY = signedY(0, min, max);
  const path = signedPath(values, min, max);
  const lastIndex = timestamps.length - 1;
  const xLabels = lastIndex < 1
    ? []
    : [0, 1 / 3, 2 / 3, 1].map((fraction) => {
        const index = Math.round(lastIndex * fraction);
        return new Date(timestamps[index] * 1000).toLocaleTimeString(locale, {
          hour: "2-digit",
          minute: "2-digit",
        });
      });
  const hoverPct = hover === null ? 0 : (hover / Math.max(lastIndex, 1)) * 100;
  const description =
    resolvedState === "ready"
      ? t("metrics.chart.samples", { count: finiteValues.length })
      : resolvedState === "loading"
        ? t("metrics.chart.sampling")
        : resolvedState === "disabled"
          ? t("metrics.clock.disabled")
          : resolvedState === "stale"
            ? t("metrics.chart.stale")
            : t("metrics.clock.noData");

  const handleMove = (event: React.MouseEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const index = Math.round(
      ((event.clientX - rect.left) / rect.width) * Math.max(lastIndex, 0),
    );
    setHover(Math.max(0, Math.min(lastIndex, index)));
  };

  return (
    <Card className="rounded-2xl">
      <Card.Header>
        <Card.Title className="text-base font-semibold">{title}</Card.Title>
        <Card.Description>
          <span className="text-xs text-muted-foreground">{description}</span>
          <span className="ml-2 text-[10px] text-muted-foreground/70">
            {t("metrics.clock.thresholdHint")}
          </span>
        </Card.Description>
      </Card.Header>
      <Card.Content>
        {resolvedState !== "ready" ? (
          <div className="flex h-48 items-center justify-center rounded-xl border border-dashed bg-muted/30 px-6 text-center text-sm text-muted-foreground">
            {description}
          </div>
        ) : (
          <>
            <div className="flex">
              <div className="flex h-48 w-16 flex-col justify-between pr-2 text-right text-[10px] text-muted-foreground/80">
                <span>{formatClockOffset(max)}</span>
                <span>{formatClockOffset(0)}</span>
                <span>{formatClockOffset(min)}</span>
              </div>
              <div
                className="relative flex-1"
                onMouseMove={handleMove}
                onMouseLeave={() => setHover(null)}
              >
                <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" className="h-48 w-full">
                  {[0.25, 0.5, 0.75].map((fraction) => (
                    <line
                      key={fraction}
                      x1={0}
                      x2={WIDTH}
                      y1={HEIGHT * fraction}
                      y2={HEIGHT * fraction}
                      stroke="#E2E8F0"
                      strokeWidth="1"
                      strokeDasharray="3 5"
                    />
                  ))}
                  <line
                    x1={0}
                    x2={WIDTH}
                    y1={zeroY}
                    y2={zeroY}
                    stroke="#64748B"
                    strokeWidth="1.2"
                  />
                  <path d={path} fill="none" stroke="#DC2626" strokeWidth="1.8" />
                  {hover !== null ? (
                    <line
                      x1={(hoverPct / 100) * WIDTH}
                      x2={(hoverPct / 100) * WIDTH}
                      y1={0}
                      y2={HEIGHT}
                      stroke="#94A3B8"
                      strokeDasharray="2 2"
                    />
                  ) : null}
                </svg>
                {hover !== null && timestamps[hover] ? (
                  <div
                    className="pointer-events-none absolute top-2 z-10 min-w-[150px] rounded-lg border bg-background/95 p-2 text-xs shadow-md"
                    style={hoverPct > 60 ? { right: `${100 - hoverPct}%` } : { left: `${hoverPct}%` }}
                  >
                    <p className="font-medium">
                      {new Date(timestamps[hover] * 1000).toLocaleTimeString(locale)}
                    </p>
                    <p className="mt-1 text-red-600">
                      {t("metrics.clock.offset")}: {formatClockOffset(values[hover])}
                    </p>
                  </div>
                ) : null}
              </div>
            </div>
            <div className="mt-1 flex justify-between pl-16 text-[10px] text-muted-foreground">
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

function signedY(value: number, min: number, max: number) {
  return HEIGHT - ((value - min) / Math.max(max - min, 1)) * HEIGHT;
}

function signedPath(
  values: Array<number | null | undefined>,
  min: number,
  max: number,
) {
  let drawing = false;
  return values
    .map((value, index) => {
      if (!Number.isFinite(value)) {
        drawing = false;
        return "";
      }
      const x = (index / Math.max(values.length - 1, 1)) * WIDTH;
      const command = drawing ? "L" : "M";
      drawing = true;
      return `${command}${x},${signedY(value as number, min, max)}`;
    })
    .filter(Boolean)
    .join(" ");
}

function formatClockOffset(value: number | null | undefined) {
  if (!Number.isFinite(value)) return "-";
  const seconds = (value as number) / 1_000;
  return `${seconds > 0 ? "+" : ""}${seconds.toFixed(Math.abs(seconds) < 10 ? 2 : 1)} s`;
}

function Legend({
  series,
  hidden,
  onToggle,
}: {
  series: ChartSeries[];
  hidden: Set<string>;
  onToggle: (label: string) => void;
}) {
  return (
    <div className="mb-3 flex flex-wrap gap-3 text-[11px] text-muted-foreground">
      {series.map((item) => {
        const isHidden = hidden.has(item.label);
        const values = item.values.filter((v) => Number.isFinite(v));
        const min = values.length ? Math.min(...values) : 0;
        const max = values.length ? Math.max(...values) : 0;
        const cur = values.length ? values[values.length - 1] : 0;
        return (
          <button
            key={item.label}
            type="button"
            onClick={() => onToggle(item.label)}
            className={`group flex items-center gap-1.5 rounded-md border border-transparent px-1.5 py-0.5 transition-colors hover:border-border ${isHidden ? "opacity-40" : ""}`}
          >
            <span
              className="h-2 w-2 rounded-full"
              style={{ background: item.color }}
            />
            <span className="font-medium text-foreground/80">{item.label}</span>
            <span className="text-muted-foreground/70">
              {compactNumber(min)} · {compactNumber(cur)} · {compactNumber(max)}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function EmptyState({ state, unit }: { state: ChartState; unit?: string }) {
  const { t } = useLocaleText();
  const tone =
    state === "disabled" || state === "stale"
      ? "border-amber-200 bg-amber-50/50 text-amber-700"
      : "border-dashed border-border bg-muted/30 text-muted-foreground";
  const title = (() => {
    switch (state) {
      case "loading":
        return t("metrics.chart.sampling");
      case "disabled":
        return t("metrics.chart.disabledTitle");
      case "stale":
        return t("metrics.chart.staleTitle");
      case "no-traffic":
        return t("metrics.chart.noTrafficTitle");
      default:
        return t("metrics.chart.noDataTitle");
    }
  })();
  const detail = (() => {
    switch (state) {
      case "loading":
        return t("metrics.chart.samplingDetail");
      case "disabled":
        return t("metrics.chart.disabledDetail");
      case "stale":
        return t("metrics.chart.staleDetail");
      case "no-traffic":
        return t("metrics.chart.noTrafficDetail");
      default:
        return t("metrics.chart.noDataDetail");
    }
  })();
  return (
    <div className={`flex h-48 flex-col items-center justify-center rounded-xl border ${tone} px-6 text-center`}>
      <p className="font-display text-base">{title}</p>
      <p className="mt-1 max-w-md text-xs">{detail}</p>
      {unit ? <p className="mt-2 font-mono text-[10px] uppercase opacity-60">unit · {unit}</p> : null}
    </div>
  );
}

function formatAxis(value: number, unit?: string) {
  if (!Number.isFinite(value)) return "-";
  const formatted = Math.abs(value) >= 1000 ? compactNumber(value) : fixed(value);
  if (!unit) return formatted;
  return `${formatted}${unit.startsWith("/") ? unit : ` ${unit}`}`;
}
