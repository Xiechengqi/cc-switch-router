"use client";

import * as React from "react";
import { Button, Checkbox, toast } from "@heroui/react";
import { Activity, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  UsageModelTable,
  UsagePeriodChips,
  UsageTokenMixBar,
  UsageTotalsStrip,
  UsageTrendChart,
} from "@/components/dashboard/account-usage-shared";
import {
  DEFAULT_EMBED_OPTIONS,
  UsageEmbedBuilder,
  type UsageEmbedOptions,
} from "@/components/dashboard/usage-embed-builder";
import { getMyProfile, getMyUsageConsumer, updateMyProfile } from "@/lib/api";
import type { AccountUsagePeriod, AccountUsageResponse, UserProfileResponse } from "@/lib/types";

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
  const [embedOptions, setEmbedOptions] = React.useState<UsageEmbedOptions>({
    ...DEFAULT_EMBED_OPTIONS,
    period: "7d",
  });

  React.useEffect(() => {
    if (typeof window !== "undefined" && window.location.host) {
      setHost(window.location.host);
    }
  }, []);

  React.useEffect(() => {
    setEmbedOptions((prev) => ({ ...prev, period }));
  }, [period]);

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
        <UsagePeriodChips
          period={period}
          onChange={setPeriod}
          t={t}
        />
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
          <UsageTotalsStrip totals={usage} t={t} />
          <UsageTokenMixBar totals={usage} t={t} />

          <section className="grid gap-2">
            <h3 className="text-sm font-semibold text-foreground">{t("account.consumerUsage.trend")}</h3>
            <UsageTrendChart daily={usage.daily || []} label={t("account.consumerUsage.trend")} t={t} />
          </section>

          <section className="grid gap-2">
            <h3 className="text-sm font-semibold text-foreground">{t("account.usage.models")}</h3>
            <UsageModelTable models={sortedModels} t={t} />
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
          <p className="text-sm text-muted-foreground">{t("account.consumerUsage.embedHint")}</p>
          <UsageEmbedBuilder
            host={host}
            username={username.trim()}
            enabled={canEmbed}
            options={embedOptions}
            onChange={(next) => {
              setEmbedOptions(next);
              if (next.period !== period) setPeriod(next.period);
            }}
            t={t}
          />
        </div>
      </section>
    </div>
  );
}
