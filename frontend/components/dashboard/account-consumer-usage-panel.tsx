"use client";

import * as React from "react";
import { Button, Checkbox, toast } from "@heroui/react";
import { Activity, Copy, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMyProfile, getMyUsageConsumer, updateMyProfile } from "@/lib/api";
import type {
  AccountUsagePeriod,
  AccountUsageResponse,
  UsageDailyBucket,
  UsageModelRow,
  UsageTokenTotals,
  UserProfileResponse,
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
  if (!models.length) {
    return (
      <p className="rounded-lg border border-dashed border-border px-3 py-6 text-center text-sm text-muted-foreground">
        {t("account.usage.empty")}
      </p>
    );
  }
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

function DailyTrendChart({ daily, label }: { daily: UsageDailyBucket[]; label: string }) {
  const width = 620;
  const height = 160;
  const padding = { left: 34, right: 12, top: 12, bottom: 28 };
  const chartWidth = width - padding.left - padding.right;
  const chartHeight = height - padding.top - padding.bottom;
  const maxY = Math.max(1, ...daily.map((bucket) => bucket.totalTokens));
  const points = daily.map((bucket, idx) => {
    const x = padding.left + (daily.length <= 1 ? 0 : (idx / (daily.length - 1)) * chartWidth);
    const y = padding.top + chartHeight - (bucket.totalTokens / maxY) * chartHeight;
    return { x, y, bucket };
  });
  const polyline = points.map((point) => `${point.x.toFixed(1)},${point.y.toFixed(1)}`).join(" ");
  const shouldShowLabel = (idx: number) => {
    if (daily.length <= 10) return true;
    if (idx === 0 || idx === daily.length - 1) return true;
    return idx % Math.ceil(daily.length / 8) === 0;
  };

  if (!daily.length) return null;

  return (
    <div className="overflow-x-auto rounded-md border bg-muted/10 p-2">
      <svg viewBox={`0 0 ${width} ${height}`} className="h-[160px] min-w-[620px] w-full" role="img" aria-label={label}>
        <line x1={padding.left} y1={padding.top} x2={padding.left} y2={padding.top + chartHeight} stroke="currentColor" className="text-border" />
        <line x1={padding.left} y1={padding.top + chartHeight} x2={padding.left + chartWidth} y2={padding.top + chartHeight} stroke="currentColor" className="text-border" />
        <text x={padding.left - 6} y={padding.top + 8} textAnchor="end" className="fill-muted-foreground text-[10px]">
          {compactTokens(maxY)}
        </text>
        <text x={padding.left - 6} y={padding.top + chartHeight} textAnchor="end" className="fill-muted-foreground text-[10px]">
          0
        </text>
        {polyline ? <polyline fill="none" stroke="#2563eb" strokeWidth="2" points={polyline} /> : null}
        {points.map((point, idx) => (
          <circle key={point.bucket.date} cx={point.x} cy={point.y} r={2.5} fill="#2563eb" />
        ))}
        {daily.map((bucket, idx) => {
          if (!shouldShowLabel(idx)) return null;
          const x = points[idx]?.x ?? padding.left;
          return (
            <text key={`label-${bucket.date}`} x={x} y={height - 8} textAnchor="middle" className="fill-muted-foreground text-[10px]">
              {bucket.date.length >= 10 ? bucket.date.slice(5, 10) : bucket.date}
            </text>
          );
        })}
      </svg>
    </div>
  );
}

function buildEmbedMarkdown(host: string, username: string, period: AccountUsagePeriod) {
  return `![TokenSwitch Usage](https://${host}/v1/public/embed/usage/${encodeURIComponent(username)}.svg?period=${period})`;
}

function buildEmbedSrc(host: string, username: string, period: AccountUsagePeriod, stamp: number) {
  return `https://${host}/v1/public/embed/usage/${encodeURIComponent(username)}.svg?period=${period}&t=${stamp}`;
}

export function AccountConsumerUsagePanel() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;

  const [period, setPeriod] = React.useState<AccountUsagePeriod>("7d");
  const [usage, setUsage] = React.useState<AccountUsageResponse | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");

  const [username, setUsername] = React.useState("");
  const [publicStatsEnabled, setPublicStatsEnabled] = React.useState(false);
  const [profileBusy, setProfileBusy] = React.useState(false);
  const [profileError, setProfileError] = React.useState("");
  const [host, setHost] = React.useState("router.example.com");
  const [copied, setCopied] = React.useState(false);
  const [previewStamp, setPreviewStamp] = React.useState(() => Date.now());

  React.useEffect(() => {
    if (typeof window !== "undefined" && window.location.host) {
      setHost(window.location.host);
    }
  }, []);

  React.useEffect(() => {
    setPreviewStamp(Date.now());
  }, [period, publicStatsEnabled, username]);

  React.useEffect(() => {
    if (!authed) return;
    let cancelled = false;
    setLoading(true);
    setError("");
    getMyUsageConsumer(period)
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

  React.useEffect(() => {
    if (!authed) return;
    let cancelled = false;
    getMyProfile()
      .then((data: UserProfileResponse) => {
        if (cancelled) return;
        setUsername(data.username || "");
        setPublicStatsEnabled(!!data.publicStatsEnabled);
      })
      .catch((err) => {
        if (!cancelled) setProfileError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [authed]);

  const sortedModels = React.useMemo(() => {
    const rows = [...(usage?.models || [])];
    rows.sort((a, b) => b.totalTokens - a.totalTokens);
    return rows;
  }, [usage?.models]);

  const canEmbed = publicStatsEnabled && !!username.trim();
  const embedMarkdown = canEmbed ? buildEmbedMarkdown(host, username.trim(), period) : "";
  const embedPreviewSrc = canEmbed ? buildEmbedSrc(host, username.trim(), period, previewStamp) : "";

  const saveProfile = async () => {
    setProfileBusy(true);
    setProfileError("");
    try {
      const next = await updateMyProfile({
        username: username.trim() || null,
        publicStatsEnabled,
      });
      setUsername(next.username || "");
      setPublicStatsEnabled(!!next.publicStatsEnabled);
      toast.success(t("account.consumerUsage.profileSaved"));
    } catch (err) {
      setProfileError(err instanceof Error ? err.message : String(err));
    } finally {
      setProfileBusy(false);
    }
  };

  const copyEmbed = async () => {
    if (!embedMarkdown) return;
    await navigator.clipboard.writeText(embedMarkdown);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

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
            <Activity className="h-4 w-4 text-muted-foreground" aria-hidden />
            {t("account.consumerUsage.title")}
          </h2>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("account.consumerUsage.hint")}</p>
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

          <section className="grid gap-2">
            <h3 className="text-sm font-semibold text-foreground">{t("account.consumerUsage.trend")}</h3>
            <DailyTrendChart daily={usage.daily || []} label={t("account.consumerUsage.trend")} />
          </section>

          <section className="grid gap-2">
            <h3 className="text-sm font-semibold text-foreground">{t("account.usage.models")}</h3>
            <ModelTable models={sortedModels} t={t} />
          </section>
        </>
      ) : null}

      <section className="grid gap-3 rounded-xl border border-border bg-card p-4 sm:p-5">
        <div>
          <h3 className="text-sm font-semibold text-foreground">{t("account.consumerUsage.profile")}</h3>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("account.consumerUsage.profileHint")}</p>
        </div>

        {profileError ? (
          <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{profileError}</div>
        ) : null}

        <label className="grid gap-1 text-sm">
          <span className="text-muted-foreground">{t("account.consumerUsage.username")}</span>
          <input
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            placeholder={t("account.consumerUsage.usernamePlaceholder")}
            className="h-9 rounded-md border border-border bg-background px-3 font-mono text-sm outline-none focus:border-primary/50"
            autoComplete="off"
            spellCheck={false}
          />
        </label>

        <Checkbox
          isSelected={publicStatsEnabled}
          onChange={(value: boolean) => setPublicStatsEnabled(value)}
        >
          <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
            <Checkbox.Indicator />
          </Checkbox.Control>
          <Checkbox.Content>
            <span className="text-sm text-foreground">{t("account.consumerUsage.publicToggle")}</span>
          </Checkbox.Content>
        </Checkbox>

        <div>
          <Button variant="primary" size="sm" onClick={() => void saveProfile()} isDisabled={profileBusy}>
            {profileBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            {t("account.consumerUsage.saveProfile")}
          </Button>
        </div>

        <div className="grid gap-2">
          <div className="text-sm font-semibold text-foreground">{t("account.consumerUsage.embed")}</div>
          {canEmbed ? (
            <>
              <p className="text-sm text-muted-foreground">{t("account.consumerUsage.embedHint")}</p>
              <div className="relative rounded-lg border border-border bg-muted/30 p-3">
                <Button
                  variant="ghost"
                  size="sm"
                  className="absolute right-2 top-2 h-8"
                  onClick={() => void copyEmbed()}
                >
                  <Copy className="h-3.5 w-3.5" />
                  {copied ? t("account.consumerUsage.copied") : t("account.consumerUsage.copyEmbed")}
                </Button>
                <pre className="overflow-x-auto whitespace-pre-wrap break-all pr-24 font-mono text-[11px] leading-relaxed text-foreground sm:text-xs">
                  {embedMarkdown}
                </pre>
              </div>
              <div className="grid gap-2">
                <div className="text-sm font-semibold text-foreground">{t("account.consumerUsage.preview")}</div>
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img
                  key={embedPreviewSrc}
                  src={embedPreviewSrc}
                  alt={t("account.consumerUsage.preview")}
                  className="max-w-full rounded-lg border border-border bg-muted/20"
                  width={680}
                />
              </div>
            </>
          ) : (
            <p className="text-sm text-muted-foreground">{t("account.consumerUsage.embedPrivate")}</p>
          )}
        </div>

      </section>
    </div>
  );
}
