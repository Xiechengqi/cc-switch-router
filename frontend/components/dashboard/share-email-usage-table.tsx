"use client";

import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import type { AppLocale } from "@/lib/i18n";
import type { ShareUsageByEmailResponse } from "@/lib/types";
import { compactTokens } from "@/lib/utils";
import { formatUsdMicros } from "@/lib/usd-micros";

export function visibleShareEmailUsage(
  usage: ShareUsageByEmailResponse | null,
  shareId: string,
  period: string,
  loading: boolean,
  error: string,
): ShareUsageByEmailResponse | null {
  return !loading && !error && usage?.shareId === shareId && usage.period === period
    ? usage
    : null;
}

export function ShareUsageEquivalent({
  amount,
  coverage,
  t,
  summary = false,
}: {
  amount: string | null;
  coverage: number;
  t: TFn;
  summary?: boolean;
}) {
  const partial = amount !== null && coverage < 100;
  const coverageLabel = Math.floor(Math.max(0, Math.min(100, coverage)) * 10) / 10;
  const hint = amount === null
    ? t("dashboard.usageEmail.unpriced")
    : partial
      ? `${t("dashboard.usageEmail.equivalentHint")} ${t("dashboard.usageEmail.pricedCoverage", { percent: coverageLabel })}`
      : t("dashboard.usageEmail.equivalentHint");
  return (
    <span className={summary ? "shrink-0 text-right font-medium text-foreground" : "block text-[10px] font-normal text-muted-foreground"} title={hint}>
      {summary ? `${t("dashboard.usageEmail.equivalentUsd")} ` : null}
      {amount === null ? "—" : `≈ ${amount}`}
      {partial ? <span className="text-amber-700"> · {coverageLabel}%</span> : null}
    </span>
  );
}

export function ShareUsageTable({
  usage,
  t,
  locale,
}: {
  usage: ShareUsageByEmailResponse;
  t: TFn;
  locale: AppLocale;
}) {
  const roleLabel = (role: string) => {
    if (role === "owner") return t("dashboard.usageEmail.role.owner");
    if (role === "shareto") return t("dashboard.usageEmail.role.shareto");
    if (role === "gateway") return t("dashboard.usageEmail.role.gateway");
    if (role === "deprecated") return t("dashboard.usageEmail.role.deprecated");
    return role || "-";
  };
  return (
    <div className="overflow-hidden rounded-md border">
      <table className="w-full table-fixed border-collapse text-[11px]">
        <colgroup>
          <col className="w-[28%]" />
          <col className="w-[12%]" />
          <col className="w-[8%]" />
          <col className="w-[8%]" />
          <col className="w-[9%]" />
          <col className="w-[9%]" />
          <col className="w-[17%]" />
          <col className="w-[9%]" />
        </colgroup>
        <thead className="bg-muted/50 text-left font-mono uppercase tracking-[0.08em] text-muted-foreground">
          <tr>
            <th className="px-1.5 py-2">{t("dashboard.usageEmail.email")}</th>
            <th className="px-1.5 py-2">{t("dashboard.usageEmail.role")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.input")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.output")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.cacheRead")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.cacheWrite")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.total")}</th>
            <th className="px-1.5 py-2 text-right">{t("dashboard.usageEmail.percent")}</th>
          </tr>
        </thead>
        <tbody>
          {usage.rows.map((row) => (
            <tr key={row.email} className="border-t">
              <td className="whitespace-normal break-all px-1.5 py-2 font-medium leading-4">{row.email}</td>
              <td className="break-words px-1.5 py-2 text-muted-foreground">{roleLabel(row.role)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.inputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.outputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.cacheReadTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.cacheCreationTokens)}</td>
              <td className="px-1.5 py-2 text-right font-mono font-semibold">
                <div>{compactTokens(row.totalTokens)}</div>
                <ShareUsageEquivalent
                  amount={formatUsdMicros(row.equivalentUsdMicros, locale)}
                  coverage={row.pricedCoveragePercent}
                  t={t}
                />
              </td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{Math.round(row.percent)}%</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
