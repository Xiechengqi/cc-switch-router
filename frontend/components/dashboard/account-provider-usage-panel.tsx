"use client";

import * as React from "react";
import { BarChart3, ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMyUsageProvider } from "@/lib/api";
import type {
  AccountUsagePeriod,
  ProviderInstallationUsage,
  ProviderShareUsage,
  ProviderUsageResponse,
  UsageModelRow,
  UsageTokenTotals,
} from "@/lib/types";
import { compactTokens, cn } from "@/lib/utils";

const PERIODS: readonly AccountUsagePeriod[] = ["24h", "7d", "30d"];

function TotalsStrip({ totals, t }: { totals: UsageTokenTotals; t: (key: Parameters<ReturnType<typeof useLocaleText>["t"]>[0]) => string }) {
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

function ModelTable({ models, t }: { models: UsageModelRow[]; t: (key: Parameters<ReturnType<typeof useLocaleText>["t"]>[0]) => string }) {
  if (!models.length) return null;
  return (
    <div className="overflow-hidden rounded-md border">
      <table className="w-full table-fixed border-collapse text-[11px]">
        <colgroup>
          <col className="w-[36%]" />
          <col className="w-[13%]" />
          <col className="w-[13%]" />
          <col className="w-[13%]" />
          <col className="w-[13%]" />
          <col className="w-[12%]" />
        </colgroup>
        <thead className="bg-muted/50 text-left font-mono uppercase tracking-[0.08em] text-muted-foreground">
          <tr>
            <th className="px-1.5 py-2">{t("account.usage.model")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.input")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.output")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.cacheRead")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.cacheWrite")}</th>
            <th className="px-1.5 py-2 text-right">{t("account.usage.total")}</th>
          </tr>
        </thead>
        <tbody>
          {models.map((row) => (
            <tr key={row.model} className="border-t">
              <td className="whitespace-normal break-all px-1.5 py-2 font-medium leading-4">{row.model || "-"}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.inputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.outputTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.cacheReadTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono">{compactTokens(row.cacheCreationTokens)}</td>
              <td className="overflow-hidden px-1.5 py-2 text-right font-mono font-semibold">{compactTokens(row.totalTokens)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ShareBlock({ share }: { share: ProviderShareUsage }) {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  return (
    <div className="rounded-lg border border-border bg-background">
      <button
        type="button"
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm"
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" /> : <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />}
        <span className="min-w-0 flex-1 truncate font-medium">{share.shareName || share.shareId}</span>
        <span className="font-mono text-xs text-muted-foreground">{compactTokens(share.totalTokens)}</span>
      </button>
      {open ? (
        <div className="grid gap-3 border-t px-3 py-2">
          <ModelTable models={share.models || []} t={t} />
          {(share.callers || []).length ? (
            <div className="overflow-hidden rounded-md border">
              <table className="w-full table-fixed border-collapse text-[11px]">
                <colgroup>
                  <col className="w-[55%]" />
                  <col className="w-[45%]" />
                </colgroup>
                <thead className="bg-muted/50 text-left font-mono uppercase tracking-[0.08em] text-muted-foreground">
                  <tr>
                    <th className="px-1.5 py-2">{t("account.providerUsage.caller")}</th>
                    <th className="px-1.5 py-2 text-right">{t("account.usage.total")}</th>
                  </tr>
                </thead>
                <tbody>
                  {(share.callers || []).map((row) => (
                    <tr key={row.email} className="border-t">
                      <td className="overflow-hidden px-1.5 py-2">{row.email || "-"}</td>
                      <td className="overflow-hidden px-1.5 py-2 text-right font-mono font-semibold">
                        {compactTokens(row.totalTokens)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function InstallationBlock({ installation }: { installation: ProviderInstallationUsage }) {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(true);
  const label = installation.label || installation.installationId;
  return (
    <section className="grid gap-2 rounded-xl border border-border bg-card p-3 sm:p-4">
      <button
        type="button"
        className="flex w-full items-center gap-2 text-left"
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground" /> : <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />}
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">{label}</div>
          <div className="truncate font-mono text-[11px] text-muted-foreground">{installation.installationId}</div>
        </div>
        <div className="text-right">
          <div className="font-mono text-sm font-semibold tabular-nums">{compactTokens(installation.totalTokens)}</div>
          <div className="text-[11px] text-muted-foreground">
            {t("account.providerUsage.shares")}: {installation.shares.length}
          </div>
        </div>
      </button>
      {open ? (
        <div className="grid gap-2 pl-1 sm:pl-2">
          {installation.shares.map((share) => (
            <ShareBlock key={share.shareId} share={share} />
          ))}
        </div>
      ) : null}
    </section>
  );
}

export function AccountProviderUsagePanel() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;

  const [period, setPeriod] = React.useState<AccountUsagePeriod>("7d");
  const [usage, setUsage] = React.useState<ProviderUsageResponse | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (!authed) return;
    let cancelled = false;
    setLoading(true);
    setError("");
    getMyUsageProvider(period)
      .then((data) => {
        if (!cancelled) setUsage(data);
      })
      .catch((err) => {
        if (!cancelled) {
          setUsage(null);
          setError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [authed, period]);

  if (authLoading) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("account.loading")}
      </div>
    );
  }

  if (!authed) {
    return <p className="py-6 text-sm text-muted-foreground">{t("account.usage.signInRequired")}</p>;
  }

  return (
    <div className="grid min-w-0 gap-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="flex items-center gap-2 text-base font-semibold text-foreground">
            <BarChart3 className="h-4 w-4 text-muted-foreground" aria-hidden />
            {t("account.providerUsage.title")}
          </h2>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("account.providerUsage.hint")}</p>
        </div>
        <div className="flex flex-wrap items-center gap-1">
          {PERIODS.map((item) => (
            <button
              key={item}
              type="button"
              className={cn(
                "rounded-md border px-2 py-1 text-xs transition-colors",
                period === item
                  ? "border-primary/40 bg-primary/10 text-primary"
                  : "border-border bg-muted/20 text-muted-foreground hover:bg-muted/40",
              )}
              onClick={() => setPeriod(item)}
            >
              {t(`account.usage.period.${item}`)}
            </button>
          ))}
        </div>
      </div>

      {error ? (
        <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div>
      ) : null}

      {loading && !usage ? (
        <div className="flex items-center gap-2 py-4 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("account.usage.loading")}
        </div>
      ) : null}

      {usage ? (
        <>
          <TotalsStrip totals={usage} t={t} />
          <div>
            <h3 className="mb-2 text-sm font-semibold text-foreground">{t("account.providerUsage.installations")}</h3>
            {usage.installations.length ? (
              <div className="grid gap-3">
                {usage.installations.map((installation) => (
                  <InstallationBlock key={installation.installationId} installation={installation} />
                ))}
              </div>
            ) : (
              <p className="rounded-lg border border-dashed border-border px-3 py-6 text-center text-sm text-muted-foreground">
                {t("account.providerUsage.noInstallations")}
              </p>
            )}
          </div>
        </>
      ) : null}
    </div>
  );
}
