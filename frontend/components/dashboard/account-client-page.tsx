"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Chip, Tabs } from "@heroui/react";
import { ArrowUpRight, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CountryFlag } from "@/components/common/country-flag";
import { BillingUrgencyChip } from "@/components/dashboard/billing-urgency-chip";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientMarketHosts, getMyClientMarketBilling } from "@/lib/api";
import {
  billingDeadlineTarget,
  formatBillingAbsoluteDate,
  formatBillingCountdown,
} from "@/lib/billing-urgency";
import { mergeBillingMap, mergeHosts } from "@/lib/client-market-refresh";
import { clientMarketMineHref, DASHBOARD_ACCOUNT_CLIENT_PATH } from "@/lib/dashboard-nav";
import type { MessageKey } from "@/lib/i18n";
import type { ClientMarketBilling, ClientMarketHost } from "@/lib/types";

type ClientMonitorTab = "user" | "provider";

const RECENT_RELEASED_MS = 30 * 24 * 60 * 60 * 1000;

function offerLabel(billing: ClientMarketBilling, locale: string) {
  if (!billing.priceCents || !billing.rentalPeriodDays) {
    return locale.startsWith("zh") ? "免费 / 永久" : "Free / forever";
  }
  const amount = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" }).format(
    billing.priceCents / 100,
  );
  return locale.startsWith("zh")
    ? `${amount} / ${billing.rentalPeriodDays} 天`
    : `${amount} / ${billing.rentalPeriodDays} days`;
}

function statusLabelKey(status: string): MessageKey {
  switch (status) {
    case "active":
      return "account.client.status.active";
    case "payment_due":
      return "account.client.status.paymentDue";
    case "releasing":
      return "account.client.status.releasing";
    case "release_failed":
      return "account.client.status.releaseFailed";
    case "released":
      return "account.client.status.released";
    default:
      return "account.client.status.unknown";
  }
}

function isAnomalous(status: string) {
  return status === "payment_due" || status === "release_failed" || status === "releasing";
}

function isRecentReleased(billing: ClientMarketBilling, now = Date.now()) {
  if (billing.status !== "released") return false;
  const updated = Date.parse(billing.updatedAt);
  if (!Number.isFinite(updated)) return true;
  return now - updated <= RECENT_RELEASED_MS;
}

function MonitorCard({
  billing,
  host,
  perspective,
}: {
  billing: ClientMarketBilling;
  host?: ClientMarketHost;
  perspective: ClientMonitorTab;
}) {
  const { locale, t } = useLocaleText();
  const subdomain = host?.clientSubdomain;
  const title =
    subdomain || host?.hostname || host?.ip || billing.installationId.slice(0, 12);
  const deadline = billingDeadlineTarget(billing);
  const countdown = formatBillingCountdown(deadline, locale);
  const absolute = formatBillingAbsoluteDate(deadline, locale);
  const anomalous = isAnomalous(billing.status);

  return (
    <section
      className={`grid gap-3 rounded-xl border bg-card p-4 shadow-sm sm:p-5 ${
        anomalous ? "border-rose-200/80 ring-1 ring-rose-100" : "border-border"
      }`}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {host?.countryCode ? (
          <CountryFlag code={host.countryCode} className="h-3.5 w-5 rounded-sm object-cover" />
        ) : null}
        <strong className="truncate text-sm">{title}</strong>
        <Chip size="sm" variant={anomalous ? "primary" : "tertiary"}>
          {t(statusLabelKey(billing.status))}
        </Chip>
        {host?.status ? (
          <Chip size="sm" variant="tertiary">
            {host.status}
          </Chip>
        ) : null}
        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {perspective === "user"
            ? t("account.client.provider", { owner: billing.hostOwnerEmail })
            : t("account.client.renter", { email: billing.clientOwnerEmail })}
        </span>
      </div>

      <dl className="grid gap-2 text-sm sm:grid-cols-2">
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.client.offer")}</dt>
          <dd className="font-medium">{offerLabel(billing, locale)}</dd>
        </div>
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.client.deadline")}</dt>
          <dd className="font-medium">
            {absolute || t("account.client.noDeadline")}
            {countdown ? (
              <span className="ml-2 text-xs font-normal text-muted-foreground">({countdown})</span>
            ) : null}
          </dd>
        </div>
        <div className="grid gap-0.5 sm:col-span-2">
          <dt className="text-xs text-muted-foreground">{t("account.client.updated")}</dt>
          <dd className="text-muted-foreground">
            {formatBillingAbsoluteDate(billing.updatedAt, locale) || billing.updatedAt}
          </dd>
        </div>
      </dl>

      {billing.isClientOwner && billing.status !== "released" ? (
        <div className="flex flex-wrap items-center gap-2">
          <BillingUrgencyChip billing={billing} showPayButton={false} />
        </div>
      ) : null}

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <p className="text-xs text-muted-foreground">{t("account.client.readOnlyHint")}</p>
        <Link
          href={clientMarketMineHref(billing.installationId)}
          className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted"
        >
          {t("account.client.manageInMarket")}
          <ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
        </Link>
      </div>
    </section>
  );
}

function MonitorList({
  billings,
  hostsByInstallation,
  perspective,
  emptyKey,
}: {
  billings: ClientMarketBilling[];
  hostsByInstallation: Map<string, ClientMarketHost>;
  perspective: ClientMonitorTab;
  emptyKey: "account.client.userEmpty" | "account.client.providerEmpty";
}) {
  const { t } = useLocaleText();
  if (!billings.length) {
    return (
      <div className="grid justify-items-center gap-2 rounded-xl border border-dashed border-border bg-card/40 px-4 py-12 text-center text-sm text-muted-foreground">
        <span>{t(emptyKey)}</span>
        <Link
          href={clientMarketMineHref()}
          className="inline-flex items-center gap-1 text-sm font-medium text-accent hover:underline"
        >
          {t("account.client.openMarket")}
          <ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
        </Link>
      </div>
    );
  }

  return (
    <div className="grid gap-3">
      {billings.map((billing) => (
        <MonitorCard
          key={`${perspective}:${billing.installationId}`}
          billing={billing}
          host={hostsByInstallation.get(billing.installationId)}
          perspective={perspective}
        />
      ))}
    </div>
  );
}

/**
 * Account → Client: read-only Provider / User monitor for Market rentals.
 * Actions live exclusively on Client Market.
 */
export function AccountClientPage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const router = useRouter();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const viewerUserId = session?.user?.id;

  const tabParam = searchParams.get("tab");
  const tab: ClientMonitorTab = tabParam === "provider" ? "provider" : "user";

  const [hosts, setHosts] = React.useState<ClientMarketHost[]>([]);
  const [billingByInstallation, setBillingByInstallation] = React.useState<
    Map<string, ClientMarketBilling>
  >(new Map());
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const refreshAbortRef = React.useRef<AbortController | null>(null);

  const setTab = React.useCallback(
    (next: ClientMonitorTab) => {
      const params = new URLSearchParams(searchParams.toString());
      params.set("tab", next);
      router.replace(`${DASHBOARD_ACCOUNT_CLIENT_PATH}?${params.toString()}`, { scroll: false });
    },
    [router, searchParams],
  );

  const load = React.useCallback(async (options?: { silent?: boolean }) => {
    const silent = options?.silent === true;
    if (silent && refreshAbortRef.current) return;
    if (!silent) refreshAbortRef.current?.abort();
    const controller = new AbortController();
    refreshAbortRef.current = controller;
    if (!silent) {
      setLoading(true);
      setError("");
    }
    try {
      const [nextHosts, billing] = await Promise.all([
        getClientMarketHosts(undefined, controller.signal),
        getMyClientMarketBilling(controller.signal),
      ]);
      if (controller.signal.aborted) return;
      setHosts((prev) => mergeHosts(prev, nextHosts));
      setBillingByInstallation((prev) => mergeBillingMap(prev, billing));
    } catch (err) {
      if (controller.signal.aborted) return;
      if (!silent) setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (refreshAbortRef.current === controller) refreshAbortRef.current = null;
      if (!silent) setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    if (!authed) {
      setLoading(false);
      return;
    }
    void load();
  }, [authed, load]);

  React.useEffect(() => {
    if (!authed) return;
    const timer = window.setInterval(() => void load({ silent: true }), CLIENT_MARKET_POLL_MS);
    return () => window.clearInterval(timer);
  }, [authed, load]);

  const hostsByInstallation = React.useMemo(() => {
    const map = new Map<string, ClientMarketHost>();
    for (const host of hosts) {
      if (host.installationId) map.set(host.installationId, host);
    }
    return map;
  }, [hosts]);

  const allBillings = React.useMemo(
    () => Array.from(billingByInstallation.values()),
    [billingByInstallation],
  );

  const userBillings = React.useMemo(
    () =>
      allBillings
        .filter(
          (billing) =>
            billing.isClientOwner &&
            (billing.status !== "released" || isRecentReleased(billing)),
        )
        .sort((left, right) => {
          const rank = (status: string) => {
            if (status === "release_failed") return 0;
            if (status === "payment_due") return 1;
            if (status === "releasing") return 2;
            if (status === "active") return 3;
            return 4;
          };
          return (
            rank(left.status) - rank(right.status) ||
            left.installationId.localeCompare(right.installationId)
          );
        }),
    [allBillings],
  );

  const providerBillings = React.useMemo(
    () =>
      allBillings
        .filter(
          (billing) =>
            !!viewerUserId &&
            billing.providerId === viewerUserId &&
            (billing.status !== "released" || isRecentReleased(billing)),
        )
        .sort((left, right) => {
          const rank = (status: string) => {
            if (status === "payment_due") return 0;
            if (status === "release_failed") return 1;
            if (status === "releasing") return 2;
            if (status === "active") return 3;
            return 4;
          };
          return (
            rank(left.status) - rank(right.status) ||
            left.clientOwnerEmail.localeCompare(right.clientOwnerEmail) ||
            left.installationId.localeCompare(right.installationId)
          );
        }),
    [allBillings, viewerUserId],
  );

  if (!authed) {
    return <p className="py-6 text-sm text-muted-foreground">{t("clientMarket.rentals.loginRequired")}</p>;
  }

  if (loading) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />…
      </div>
    );
  }

  if (error) {
    return <p className="py-6 text-sm text-rose-600">{error}</p>;
  }

  return (
    <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4">
      <div>
        <h2 className="text-base font-semibold text-foreground">{t("account.nav.client")}</h2>
        <p className="mt-0.5 text-sm text-muted-foreground">{t("account.clientHint")}</p>
      </div>

      <Tabs
        selectedKey={tab}
        onSelectionChange={(key) => {
          if (key === "user" || key === "provider") setTab(key);
        }}
        variant="secondary"
        className="min-w-0 text-foreground"
      >
        <Tabs.List className="grid w-full max-w-sm grid-cols-2 text-foreground">
          <Tabs.Tab id="user" className="px-3 py-2 text-sm">
            {t("account.client.tab.user")}
          </Tabs.Tab>
          <Tabs.Tab id="provider" className="px-3 py-2 text-sm">
            {t("account.client.tab.provider")}
          </Tabs.Tab>
        </Tabs.List>
      </Tabs>

      {tab === "user" ? (
        <MonitorList
          billings={userBillings}
          hostsByInstallation={hostsByInstallation}
          perspective="user"
          emptyKey="account.client.userEmpty"
        />
      ) : (
        <MonitorList
          billings={providerBillings}
          hostsByInstallation={hostsByInstallation}
          perspective="provider"
          emptyKey="account.client.providerEmpty"
        />
      )}
    </div>
  );
}
