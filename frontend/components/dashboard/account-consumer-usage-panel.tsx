"use client";

import * as React from "react";
import { Checkbox } from "@heroui/react";
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
import { UsageEmbedBuilder } from "@/components/dashboard/usage-embed-builder";
import {
  getMyUsageCardSettings,
  getMyUsageConsumer,
  updateMyUsageCardSettings,
} from "@/lib/api";
import type {
  AccountUsagePeriod,
  AccountUsageResponse,
  UsageCardSettingsResponse,
} from "@/lib/types";
import {
  DEFAULT_EMBED_OPTIONS,
  loadUsageCardPreferences,
  saveUsageCardPreferences,
  type UsageEmbedOptions,
} from "@/lib/usage-card-preferences";

export function AccountConsumerUsagePanel() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;

  const [period, setPeriod] = React.useState<AccountUsagePeriod>("7d");
  const [usage, setUsage] = React.useState<AccountUsageResponse | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");

  const [cardSettings, setCardSettings] = React.useState<UsageCardSettingsResponse | null>(null);
  const [cardSettingsLoading, setCardSettingsLoading] = React.useState(false);
  const [cardSettingsBusy, setCardSettingsBusy] = React.useState(false);
  const [cardSettingsError, setCardSettingsError] = React.useState("");
  const [host, setHost] = React.useState("router.example.com");
  const [embedOptions, setEmbedOptions] = React.useState<UsageEmbedOptions>({
    ...DEFAULT_EMBED_OPTIONS,
  });
  const cardSettingsMutationRef = React.useRef(0);

  React.useEffect(() => {
    if (typeof window !== "undefined" && window.location.host) {
      setHost(window.location.host);
    }
  }, []);

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
    cardSettingsMutationRef.current += 1;
    setCardSettings(null);
    setCardSettingsBusy(false);
    setEmbedOptions({ ...DEFAULT_EMBED_OPTIONS });
    setCardSettingsLoading(true);
    setCardSettingsError("");
    getMyUsageCardSettings()
      .then((data) => {
        if (cancelled) return;
        setCardSettings(data);
        setEmbedOptions(loadUsageCardPreferences(window.localStorage, data.userId));
      })
      .catch((err) => {
        if (!cancelled) {
          setCardSettings(null);
          setCardSettingsError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setCardSettingsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [authed, session?.user?.id]);

  const sortedModels = React.useMemo(() => {
    const rows = [...(usage?.models || [])];
    rows.sort((a, b) => b.totalTokens - a.totalTokens);
    return rows;
  }, [usage?.models]);

  const updatePublicStats = async (enabled: boolean) => {
    if (!cardSettings || cardSettingsBusy) return;
    const previous = cardSettings;
    const mutationId = cardSettingsMutationRef.current + 1;
    cardSettingsMutationRef.current = mutationId;
    setCardSettings({ ...cardSettings, publicStatsEnabled: enabled });
    setCardSettingsBusy(true);
    setCardSettingsError("");
    try {
      const next = await updateMyUsageCardSettings({
        publicStatsEnabled: enabled,
      });
      if (cardSettingsMutationRef.current === mutationId) {
        setCardSettings(next);
      }
    } catch (err) {
      if (cardSettingsMutationRef.current === mutationId) {
        setCardSettings(previous);
        setCardSettingsError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (cardSettingsMutationRef.current === mutationId) {
        setCardSettingsBusy(false);
      }
    }
  };

  const updateEmbedOptions = (next: UsageEmbedOptions) => {
    setEmbedOptions(next);
    if (cardSettings?.userId) {
      saveUsageCardPreferences(window.localStorage, cardSettings.userId, next);
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
          <h3 className="text-sm font-semibold text-foreground">{t("account.consumerUsage.card")}</h3>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("account.consumerUsage.cardHint")}</p>
        </div>

        {cardSettingsError ? (
          <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{cardSettingsError}</div>
        ) : null}

        {cardSettingsLoading && !cardSettings ? (
          <div className="flex items-center gap-2 py-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("account.loading")}
          </div>
        ) : null}

        {cardSettings ? (
          <>
            <div className="grid gap-1 text-sm">
              <span className="text-muted-foreground">{t("account.consumerUsage.cardIdentity")}</span>
              <span className="break-all font-mono text-foreground">{cardSettings.email}</span>
            </div>

            <Checkbox
              isSelected={cardSettings.publicStatsEnabled}
              isDisabled={cardSettingsBusy}
              onChange={(value: boolean) => void updatePublicStats(value)}
            >
              <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                <Checkbox.Indicator />
              </Checkbox.Control>
              <Checkbox.Content>
                <span className="inline-flex items-center gap-2 text-sm text-foreground">
                  {t("account.consumerUsage.publicToggle")}
                  {cardSettingsBusy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                </span>
              </Checkbox.Content>
            </Checkbox>
          </>
        ) : null}

        <div className="grid gap-2">
          <div className="text-sm font-semibold text-foreground">{t("account.consumerUsage.embed")}</div>
          <p className="text-sm text-muted-foreground">{t("account.consumerUsage.embedHint")}</p>
          <UsageEmbedBuilder
            host={host}
            userId={cardSettings?.userId || ""}
            enabled={!!cardSettings?.userId && !!cardSettings.publicStatsEnabled}
            options={embedOptions}
            onChange={updateEmbedOptions}
            t={t}
          />
        </div>
      </section>
    </div>
  );
}
