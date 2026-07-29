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

type MonitorEntry = {
  key: string;
  billing?: ClientMarketBilling;
  host?: ClientMarketHost;
};

const RECENT_RELEASED_MS = 30 * 24 * 60 * 60 * 1000;

function isFreeForeverOffer(priceCents?: number | null, rentalPeriodDays?: number | null) {
  return !priceCents || !rentalPeriodDays;
}

function offerLabel(
  priceCents: number | undefined,
  rentalPeriodDays: number | undefined,
  locale: string,
) {
  if (isFreeForeverOffer(priceCents, rentalPeriodDays)) {
    return locale.startsWith("zh") ? "免费 / 永久" : "Free / forever";
  }
  const amount = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" }).format(
    (priceCents as number) / 100,
  );
  return locale.startsWith("zh")
    ? `${amount} / ${rentalPeriodDays} 天`
    : `${amount} / ${rentalPeriodDays} days`;
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
    case "idle":
      return "account.client.status.idle";
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

/** Host-only / billing-missing rows (common for free-forever allocations). */
function synthesizeBillingFromHost(
  host: ClientMarketHost,
  isClientOwner: boolean,
): ClientMarketBilling {
  const installationId = host.installationId || `host:${host.id}`;
  return {
    installationId,
    hostId: host.id,
    providerId: host.providerId || "",
    hostOwnerEmail: host.hostOwnerEmail,
    clientOwnerEmail: host.clientOwnerEmail || "",
    status: host.installationId ? "active" : "idle",
    priceCents: host.priceCents,
    rentalPeriodDays: host.rentalPeriodDays,
    offerRevision: host.offerRevision ?? 0,
    paymentMethodKinds: host.paymentMethodKinds || [],
    isClientOwner,
    canDeclarePaid: false,
    canRelease: !!host.installationId,
    updatedAt: host.updatedAt || host.createdAt || new Date(0).toISOString(),
  };
}

function entryRank(status: string) {
  if (status === "release_failed") return 0;
  if (status === "payment_due") return 1;
  if (status === "releasing") return 2;
  if (status === "active") return 3;
  if (status === "idle") return 4;
  return 5;
}

function MonitorCard({
  entry,
  perspective,
}: {
  entry: MonitorEntry;
  perspective: ClientMonitorTab;
}) {
  const { locale, t } = useLocaleText();
  const billing = entry.billing;
  const host = entry.host;
  const status = billing?.status || host?.status || "unknown";
  const priceCents = billing?.priceCents ?? host?.priceCents;
  const rentalPeriodDays = billing?.rentalPeriodDays ?? host?.rentalPeriodDays;
  const subdomain = host?.clientSubdomain;
  const title =
    subdomain ||
    host?.hostname ||
    host?.ip ||
    (billing?.installationId.startsWith("host:")
      ? billing.installationId.slice(5, 17)
      : billing?.installationId.slice(0, 12)) ||
    host?.id.slice(0, 12) ||
    "—";
  const deadline = billing ? billingDeadlineTarget(billing) : undefined;
  const countdown = formatBillingCountdown(deadline, locale);
  const absolute = formatBillingAbsoluteDate(deadline, locale);
  const anomalous = isAnomalous(status);
  const providerEmail = billing?.hostOwnerEmail || host?.hostOwnerEmail || "—";
  const renterEmail = billing?.clientOwnerEmail || host?.clientOwnerEmail || "";
  const marketHref = host?.installationId
    ? clientMarketMineHref(host.installationId)
    : billing && !billing.installationId.startsWith("host:")
      ? clientMarketMineHref(billing.installationId)
      : clientMarketMineHref();
  const updatedAt = billing?.updatedAt || host?.updatedAt || host?.createdAt;

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
          {t(statusLabelKey(status))}
        </Chip>
        {host?.status && host.status !== status ? (
          <Chip size="sm" variant="tertiary">
            {host.status}
          </Chip>
        ) : null}
        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {perspective === "user"
            ? t("account.client.provider", { owner: providerEmail })
            : renterEmail
              ? t("account.client.renter", { email: renterEmail })
              : t("account.client.noRenter")}
        </span>
      </div>

      <dl className="grid gap-2 text-sm sm:grid-cols-2">
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.client.offer")}</dt>
          <dd className="font-medium">{offerLabel(priceCents, rentalPeriodDays, locale)}</dd>
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
            {formatBillingAbsoluteDate(updatedAt, locale) || updatedAt || "—"}
          </dd>
        </div>
      </dl>

      {billing?.isClientOwner && billing.status !== "released" && billing.status !== "idle" ? (
        <div className="flex flex-wrap items-center gap-2">
          <BillingUrgencyChip billing={billing} showPayButton={false} />
        </div>
      ) : null}

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <p className="text-xs text-muted-foreground">{t("account.client.readOnlyHint")}</p>
        <Link
          href={marketHref}
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
  entries,
  perspective,
  emptyKey,
}: {
  entries: MonitorEntry[];
  perspective: ClientMonitorTab;
  emptyKey: "account.client.userEmpty" | "account.client.providerEmpty";
}) {
  const { t } = useLocaleText();
  if (!entries.length) {
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
      {entries.map((entry) => (
        <MonitorCard key={`${perspective}:${entry.key}`} entry={entry} perspective={perspective} />
      ))}
    </div>
  );
}

/**
 * Account → Client Market: read-only Provider / User monitor.
 * Includes free-forever Hosts (billing + host fallback). Actions stay on Client Market.
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

  const hostsById = React.useMemo(() => {
    const map = new Map<string, ClientMarketHost>();
    for (const host of hosts) map.set(host.id, host);
    return map;
  }, [hosts]);

  const allBillings = React.useMemo(
    () => Array.from(billingByInstallation.values()),
    [billingByInstallation],
  );

  const userEntries = React.useMemo(() => {
    const entries: MonitorEntry[] = [];
    const seen = new Set<string>();

    for (const billing of allBillings) {
      if (!billing.isClientOwner) continue;
      if (billing.status === "released" && !isRecentReleased(billing)) continue;
      seen.add(billing.installationId);
      entries.push({
        key: billing.installationId,
        billing,
        host: hostsByInstallation.get(billing.installationId),
      });
    }

    // Free-forever (and any) rentals that appear on hosts but lack / missed billing rows.
    for (const host of hosts) {
      if (host.isClientOwner !== true || !host.installationId) continue;
      if (seen.has(host.installationId)) continue;
      seen.add(host.installationId);
      entries.push({
        key: host.installationId,
        billing: synthesizeBillingFromHost(host, true),
        host,
      });
    }

    return entries.sort((left, right) => {
      const leftStatus = left.billing?.status || left.host?.status || "";
      const rightStatus = right.billing?.status || right.host?.status || "";
      return (
        entryRank(leftStatus) - entryRank(rightStatus) || left.key.localeCompare(right.key)
      );
    });
  }, [allBillings, hosts, hostsByInstallation]);

  const providerEntries = React.useMemo(() => {
    const entries: MonitorEntry[] = [];
    const seenInstallations = new Set<string>();
    const seenHosts = new Set<string>();

    for (const billing of allBillings) {
      const host =
        hostsByInstallation.get(billing.installationId) || hostsById.get(billing.hostId);
      const isProvider =
        (!!viewerUserId && billing.providerId === viewerUserId) || host?.isHostOwner === true;
      if (!isProvider) continue;
      if (billing.status === "released" && !isRecentReleased(billing)) continue;
      seenInstallations.add(billing.installationId);
      if (host) seenHosts.add(host.id);
      entries.push({ key: billing.installationId, billing, host });
    }

    for (const host of hosts) {
      if (host.isHostOwner !== true) continue;

      // Allocated Client on this Host (incl. free forever) missing from billing.
      if (host.installationId && !seenInstallations.has(host.installationId)) {
        seenInstallations.add(host.installationId);
        seenHosts.add(host.id);
        entries.push({
          key: host.installationId,
          billing: synthesizeBillingFromHost(host, host.isClientOwner === true),
          host,
        });
        continue;
      }

      // Idle free-forever Hosts: still show under Provider monitor.
      if (
        !host.installationId &&
        isFreeForeverOffer(host.priceCents, host.rentalPeriodDays) &&
        !seenHosts.has(host.id)
      ) {
        seenHosts.add(host.id);
        entries.push({
          key: `host:${host.id}`,
          billing: synthesizeBillingFromHost(host, false),
          host,
        });
      }
    }

    return entries.sort((left, right) => {
      const leftStatus = left.billing?.status || left.host?.status || "";
      const rightStatus = right.billing?.status || right.host?.status || "";
      const leftRenter = left.billing?.clientOwnerEmail || left.host?.clientOwnerEmail || "";
      const rightRenter = right.billing?.clientOwnerEmail || right.host?.clientOwnerEmail || "";
      return (
        entryRank(leftStatus) - entryRank(rightStatus) ||
        leftRenter.localeCompare(rightRenter) ||
        left.key.localeCompare(right.key)
      );
    });
  }, [allBillings, hosts, hostsById, hostsByInstallation, viewerUserId]);

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
          <Tabs.Tab id="user" className="px-3 py-2 text-sm font-medium !text-slate-900 data-[selected=true]:!text-slate-900">
            {t("account.client.tab.user")}
          </Tabs.Tab>
          <Tabs.Tab id="provider" className="px-3 py-2 text-sm font-medium !text-slate-900 data-[selected=true]:!text-slate-900">
            {t("account.client.tab.provider")}
          </Tabs.Tab>
        </Tabs.List>
      </Tabs>

      {tab === "user" ? (
        <MonitorList
          entries={userEntries}
          perspective="user"
          emptyKey="account.client.userEmpty"
        />
      ) : (
        <MonitorList
          entries={providerEntries}
          perspective="provider"
          emptyKey="account.client.providerEmpty"
        />
      )}
    </div>
  );
}
