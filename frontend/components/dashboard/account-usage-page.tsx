"use client";

import * as React from "react";
import { useSearchParams } from "next/navigation";
import { BarChart3, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { AccountInlineLoading, AccountSectionError, AccountSectionHeader } from "@/components/dashboard/account-section";
import {
  UsageModelTable,
  UsagePeriodChips,
  UsageTokenMixBar,
  UsageTotalsStrip,
  UsageTrendChart,
} from "@/components/dashboard/account-usage-shared";
import { ProviderUsageInstallations } from "@/components/dashboard/provider-usage-installations";
import { UsageCardSettings } from "@/components/dashboard/usage-card-settings";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMyUsageConsumer, getMyUsageProvider } from "@/lib/api";
import { useCachedResource } from "@/lib/cached-resource";
import { ACCOUNT_USAGE_VIEWS, accountUsageView, type AccountUsageView } from "@/lib/dashboard-nav";
import type { AccountUsagePeriod, AccountUsageResponse, ProviderUsageResponse } from "@/lib/types";
import { replaceCurrentSearch } from "@/lib/url-search";
import { cn } from "@/lib/utils";

/**
 * Tokens the account used and tokens others used through it, one period for
 * both views. Each view and period is cached, so switching back is instant;
 * a period still loading keeps the last numbers on screen, dimmed.
 */
export function AccountUsagePage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const searchParams = useSearchParams();
  const view = accountUsageView(searchParams.get("view"));
  const [period, setPeriod] = React.useState<AccountUsagePeriod>("7d");
  const actor = session?.user?.id || session?.user?.email || "";
  const key = actor ? `usage:${view}:${actor}:${period}` : null;
  const fetcher = React.useCallback(
    (): Promise<AccountUsageResponse | ProviderUsageResponse> =>
      view === "provider" ? getMyUsageProvider(period) : getMyUsageConsumer(period),
    [period, view],
  );
  const usage = useCachedResource(key, fetcher);
  const lastShown = React.useRef<{ view: AccountUsageView; data: AccountUsageResponse | ProviderUsageResponse } | null>(null);
  if (usage.data) lastShown.current = { view, data: usage.data };
  // Another period's numbers stand in only while this one loads, never after it failed.
  const shown = usage.data ?? (!usage.error && lastShown.current?.view === view ? lastShown.current.data : undefined);
  const stale = !usage.data && !!shown;

  const setView = (next: AccountUsageView) => replaceCurrentSearch((params) => params.set("view", next));

  return (
    <div className="grid min-w-0 gap-6">
      <AccountSectionHeader
        icon={BarChart3}
        title={t("account.usage.title")}
        hint={t("account.usage.hint")}
        actions={(
          <>
            {usage.refreshing && shown ? (
              <span role="status" className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                <Loader2 className="h-3.5 w-3.5 animate-spin motion-reduce:animate-none" aria-hidden />
                {t("account.usage.refreshing")}
              </span>
            ) : null}
            <UsagePeriodChips period={period} onChange={setPeriod} t={t} />
          </>
        )}
      />
      <SegmentedControl
        value={view}
        onChange={setView}
        ariaLabel={t("account.usage.view")}
        size="md"
        className="w-full max-w-sm"
        fullWidth
        items={ACCOUNT_USAGE_VIEWS.map((id) => ({ id, label: t(`account.usage.view.${id}`) }))}
      />

      {usage.error && !usage.refreshing ? (
        <AccountSectionError
          message={usage.error instanceof Error ? usage.error.message : String(usage.error)}
          onRetry={() => void usage.refresh()}
        />
      ) : null}

      {shown ? (
        <div className={cn("grid min-w-0 gap-6 transition-opacity", stale && "opacity-60")} aria-busy={stale}>
          <UsageTotalsStrip totals={shown} t={t} />
          <UsageTokenMixBar totals={shown} t={t} />
          {"installations" in shown ? <ProviderUsageView usage={shown} /> : <ConsumerUsageView usage={shown} />}
        </div>
      ) : !usage.error || usage.refreshing ? <AccountInlineLoading label={t("account.usage.loading")} /> : null}

      {view === "consumer" ? <UsageCardSettings /> : null}
    </div>
  );
}

function ConsumerUsageView({ usage }: { usage: AccountUsageResponse }) {
  const { t } = useLocaleText();
  const models = React.useMemo(
    () => [...(usage.models || [])].sort((a, b) => b.totalTokens - a.totalTokens),
    [usage.models],
  );
  return (
    <>
      <section className="grid gap-2">
        <h3 className="text-sm font-semibold text-foreground">{t("account.consumerUsage.trend")}</h3>
        <UsageTrendChart daily={usage.daily || []} label={t("account.consumerUsage.trend")} t={t} />
      </section>
      <section className="grid gap-2">
        <h3 className="text-sm font-semibold text-foreground">{t("account.usage.models")}</h3>
        <UsageModelTable models={models} t={t} />
      </section>
    </>
  );
}

function ProviderUsageView({ usage }: { usage: ProviderUsageResponse }) {
  const { t } = useLocaleText();
  return (
    <>
      {usage.daily?.length ? (
        <section className="grid gap-2">
          <h3 className="text-sm font-semibold text-foreground">{t("account.consumerUsage.trend")}</h3>
          <UsageTrendChart daily={usage.daily} label={t("account.consumerUsage.trend")} t={t} />
        </section>
      ) : null}
      <ProviderUsageInstallations installations={usage.installations} />
    </>
  );
}
