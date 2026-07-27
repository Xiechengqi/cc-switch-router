"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type {
  AccountUsagePeriod,
  UsageDailyBucket,
  UsageModelRow,
  UsageTokenTotals,
} from "@/lib/types";
import { compactTokens, cn } from "@/lib/utils";

export const USAGE_PERIODS: readonly AccountUsagePeriod[] = ["24h", "7d", "30d"];

type Translate = ReturnType<typeof useLocaleText>["t"];

export function UsagePeriodChips({
  period,
  onChange,
  t,
}: {
  period: AccountUsagePeriod;
  onChange: (period: AccountUsagePeriod) => void;
  t: Translate;
}) {
  return (
    <div className="flex flex-wrap items-center gap-1">
      {USAGE_PERIODS.map((item) => (
        <button
          key={item}
          type="button"
          className={cn(
            "rounded-md border px-2.5 py-1 text-xs transition-colors",
            period === item
              ? "border-foreground/25 bg-background font-semibold text-foreground"
              : "border-transparent text-muted-foreground hover:border-border hover:bg-muted/40",
          )}
          onClick={() => onChange(item)}
        >
          {t(`account.usage.period.${item}`)}
        </button>
      ))}
    </div>
  );
}

export function UsageTotalsStrip({
  totals,
  t,
}: {
  totals: UsageTokenTotals;
  t: Translate;
}) {
  const items = [
    { label: t("account.usage.total"), value: totals.totalTokens },
    { label: t("account.usage.input"), value: totals.inputTokens },
    { label: t("account.usage.output"), value: totals.outputTokens },
    { label: t("account.usage.cacheRead"), value: totals.cacheReadTokens },
    { label: t("account.usage.cacheWrite"), value: totals.cacheCreationTokens },
  ];
  return (
    <div className="grid gap-2 rounded-xl border border-border bg-card p-3 sm:grid-cols-5">
      {items.map((item) => (
        <div key={item.label} className="min-w-0 rounded-lg bg-muted/30 px-3 py-2">
          <div className="text-[11px] uppercase tracking-[0.06em] text-muted-foreground">{item.label}</div>
          <div className="mt-0.5 font-mono text-sm font-semibold tabular-nums">{compactTokens(item.value)}</div>
        </div>
      ))}
    </div>
  );
}

export function UsageTokenMixBar({
  totals,
  t,
}: {
  totals: UsageTokenTotals;
  t: Translate;
}) {
  const cache = totals.cacheReadTokens + totals.cacheCreationTokens;
  const parts = [
    { key: "input", label: t("account.usage.input"), value: totals.inputTokens, className: "bg-[var(--accent)]" },
    { key: "output", label: t("account.usage.output"), value: totals.outputTokens, className: "bg-[#4D7CFF]" },
    { key: "cache", label: t("account.usage.cache"), value: cache, className: "bg-slate-400" },
  ].filter((part) => part.value > 0);
  const sum = parts.reduce((acc, part) => acc + part.value, 0);
  if (sum <= 0) return null;

  return (
    <div className="grid gap-2">
      <div className="flex h-2.5 overflow-hidden rounded-full bg-muted">
        {parts.map((part) => (
          <div
            key={part.key}
            className={cn("h-full min-w-[2px]", part.className)}
            style={{ width: `${(part.value / sum) * 100}%` }}
            title={`${part.label}: ${compactTokens(part.value)}`}
          />
        ))}
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-muted-foreground">
        {parts.map((part) => (
          <span key={part.key} className="inline-flex items-center gap-1.5">
            <span className={cn("h-2 w-2 rounded-full", part.className)} />
            <span>{part.label}</span>
            <span className="font-mono tabular-nums text-foreground/80">{compactTokens(part.value)}</span>
            <span className="tabular-nums">({Math.round((part.value / sum) * 100)}%)</span>
          </span>
        ))}
      </div>
    </div>
  );
}

export function UsageModelTable({
  models,
  t,
  emptyMessage,
}: {
  models: UsageModelRow[];
  t: Translate;
  emptyMessage?: string;
}) {
  if (!models.length) {
    return (
      <p className="rounded-lg border border-dashed border-border px-3 py-6 text-center text-sm text-muted-foreground">
        {emptyMessage || t("account.usage.empty")}
      </p>
    );
  }
  const grand = Math.max(1, models.reduce((acc, row) => acc + row.totalTokens, 0));
  return (
    <div className="overflow-hidden rounded-md border">
      <table className="w-full table-fixed border-collapse text-[11px]">
        <colgroup>
          <col className="w-[34%]" />
          <col className="w-[12%]" />
          <col className="w-[12%]" />
          <col className="w-[12%]" />
          <col className="w-[12%]" />
          <col className="w-[10%]" />
          <col className="w-[8%]" />
        </colgroup>
        <thead className="bg-muted/50 text-left font-mono uppercase tracking-[0.08em] text-muted-foreground">
          <tr>
            <th className="px-1.5 py-2">{t("account.usage.model")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.input")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.output")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.cacheRead")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.cacheWrite")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.total")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.share")}</th>
          </tr>
        </thead>
        <tbody>
          {models.map((row) => (
            <tr key={row.model} className="border-t">
              <td className="whitespace-normal break-all px-1.5 py-2 font-medium leading-4">{row.model || "-"}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono tabular-nums">{compactTokens(row.inputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono tabular-nums">{compactTokens(row.outputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono tabular-nums">{compactTokens(row.cacheReadTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono tabular-nums">{compactTokens(row.cacheCreationTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono font-semibold tabular-nums">{compactTokens(row.totalTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono tabular-nums text-muted-foreground">
                {Math.round((row.totalTokens / grand) * 100)}%
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

const SERIES = [
  { key: "inputTokens" as const, color: "#0052FF", labelKey: "account.usage.input" as const },
  { key: "outputTokens" as const, color: "#4D7CFF", labelKey: "account.usage.output" as const },
  { key: "cache" as const, color: "#94a3b8", labelKey: "account.usage.cache" as const },
];

function bucketCache(bucket: UsageDailyBucket) {
  return bucket.cacheReadTokens + bucket.cacheCreationTokens;
}

export function UsageTrendChart({
  daily,
  label,
  t,
}: {
  daily: UsageDailyBucket[];
  label: string;
  t: Translate;
}) {
  const [hoverIdx, setHoverIdx] = React.useState<number | null>(null);
  const width = 640;
  const height = 180;
  const padding = { left: 40, right: 12, top: 16, bottom: 30 };
  const chartWidth = width - padding.left - padding.right;
  const chartHeight = height - padding.top - padding.bottom;

  if (!daily.length) return null;

  const maxY = Math.max(
    1,
    ...daily.map((bucket) => bucket.inputTokens + bucket.outputTokens + bucketCache(bucket)),
  );
  const barGap = daily.length > 20 ? 1 : 2;
  const barWidth = Math.max(4, chartWidth / daily.length - barGap);

  const shouldShowLabel = (idx: number) => {
    if (daily.length <= 10) return true;
    if (idx === 0 || idx === daily.length - 1) return true;
    return idx % Math.ceil(daily.length / 8) === 0;
  };

  const hover = hoverIdx != null ? daily[hoverIdx] : null;

  return (
    <div className="grid gap-2">
      <div className="overflow-x-auto rounded-md border bg-muted/10 p-2">
        <svg
          viewBox={`0 0 ${width} ${height}`}
          className="h-[180px] min-w-[620px] w-full"
          role="img"
          aria-label={label}
          onMouseLeave={() => setHoverIdx(null)}
        >
          <line
            x1={padding.left}
            y1={padding.top}
            x2={padding.left}
            y2={padding.top + chartHeight}
            stroke="currentColor"
            className="text-border"
          />
          <line
            x1={padding.left}
            y1={padding.top + chartHeight}
            x2={padding.left + chartWidth}
            y2={padding.top + chartHeight}
            stroke="currentColor"
            className="text-border"
          />
          <text
            x={padding.left - 6}
            y={padding.top + 8}
            textAnchor="end"
            className="fill-muted-foreground text-[10px]"
          >
            {compactTokens(maxY)}
          </text>
          <text
            x={padding.left - 6}
            y={padding.top + chartHeight}
            textAnchor="end"
            className="fill-muted-foreground text-[10px]"
          >
            0
          </text>
          {daily.map((bucket, idx) => {
            const x =
              padding.left +
              (daily.length <= 1 ? chartWidth / 2 - barWidth / 2 : (idx / daily.length) * chartWidth + barGap / 2);
            const values = [
              bucket.inputTokens,
              bucket.outputTokens,
              bucketCache(bucket),
            ];
            let yCursor = padding.top + chartHeight;
            return (
              <g
                key={bucket.date}
                onMouseEnter={() => setHoverIdx(idx)}
                style={{ cursor: "pointer" }}
              >
                <rect
                  x={x}
                  y={padding.top}
                  width={barWidth}
                  height={chartHeight}
                  fill="transparent"
                />
                {values.map((value, seriesIdx) => {
                  const h = (value / maxY) * chartHeight;
                  yCursor -= h;
                  if (h <= 0) return null;
                  return (
                    <rect
                      key={SERIES[seriesIdx].key}
                      x={x}
                      y={yCursor}
                      width={barWidth}
                      height={Math.max(h, 0.5)}
                      fill={SERIES[seriesIdx].color}
                      opacity={hoverIdx == null || hoverIdx === idx ? 1 : 0.35}
                      rx={1}
                    />
                  );
                })}
              </g>
            );
          })}
          {daily.map((bucket, idx) => {
            if (!shouldShowLabel(idx)) return null;
            const x =
              padding.left +
              (daily.length <= 1
                ? chartWidth / 2
                : (idx / daily.length) * chartWidth + barWidth / 2);
            return (
              <text
                key={`label-${bucket.date}`}
                x={x}
                y={height - 8}
                textAnchor="middle"
                className="fill-muted-foreground text-[10px]"
              >
                {bucket.date.length >= 10 ? bucket.date.slice(5, 10) : bucket.date}
              </text>
            );
          })}
        </svg>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <div className="flex flex-wrap gap-3">
          {SERIES.map((series) => (
            <span key={series.key} className="inline-flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-sm" style={{ background: series.color }} />
              {t(series.labelKey)}
            </span>
          ))}
        </div>
        {hover ? (
          <div className="font-mono tabular-nums text-foreground">
            {hover.date}: {compactTokens(hover.totalTokens)} · in {compactTokens(hover.inputTokens)} · out{" "}
            {compactTokens(hover.outputTokens)} · cache {compactTokens(bucketCache(hover))}
          </div>
        ) : (
          <div>{t("account.usage.trendHint")}</div>
        )}
      </div>
    </div>
  );
}
