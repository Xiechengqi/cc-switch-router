"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Chip, Tabs } from "@heroui/react";
import { ArrowUpRight, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CountryFlag } from "@/components/common/country-flag";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientMarketHosts, getMyClientMarketRentals } from "@/lib/api";
import { mergeHosts, mergeRentalMap } from "@/lib/client-market-refresh";
import { clientMarketMineHref, DASHBOARD_ACCOUNT_BILLING_PATH, DASHBOARD_ACCOUNT_CLIENT_PATH } from "@/lib/dashboard-nav";
import type { MessageKey } from "@/lib/i18n";
import type { ClientMarketHost, ClientMarketRental } from "@/lib/types";

type ClientMonitorTab = "user" | "provider";

type MonitorEntry = {
  key: string;
  rental?: ClientMarketRental;
  host?: ClientMarketHost;
};

const RECENT_RELEASED_MS = 30 * 24 * 60 * 60 * 1000;

function isFreeForeverOffer(dailyRateMinor?: number | null) {
  return !dailyRateMinor;
}

function offerLabel(
  dailyRateMinor: number | undefined,
  locale: string,
  currency = "USD",
) {
  if (isFreeForeverOffer(dailyRateMinor)) {
    return locale.startsWith("zh") ? "免费 / 永久" : "Free / forever";
  }
  const amount = new Intl.NumberFormat(locale, { style: "currency", currency: currency === "CNY" ? "CNY" : "USD" }).format(
    (dailyRateMinor as number) / 100,
  );
  return locale.startsWith("zh") ? `${amount} / 天` : `${amount} / day`;
}

function formatDate(value: string | undefined, locale: string) {
  if (!value) return "—";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(timestamp))
    : value;
}

function statusLabelKey(status: string): MessageKey {
  switch (status) {
    case "active":
      return "account.client.status.active";
    case "billing_suspended":
      return "account.client.status.billingSuspended";
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
  return status === "billing_suspended" || status === "release_failed" || status === "releasing";
}

function isRecentReleased(rental: ClientMarketRental, now = Date.now()) {
  if (rental.status !== "released") return false;
  const updated = Date.parse(rental.updatedAt);
  if (!Number.isFinite(updated)) return true;
  return now - updated <= RECENT_RELEASED_MS;
}

/** Host-only / rental-missing rows during provisioning reconciliation. */
function synthesizeRentalFromHost(
  host: ClientMarketHost,
  isClientOwner: boolean,
): ClientMarketRental {
  const installationId = host.installationId || `host:${host.id}`;
  return {
    installationId,
    hostId: host.id,
    providerId: host.providerId || "",
    hostOwnerEmail: host.hostOwnerEmail,
    clientOwnerEmail: host.clientOwnerEmail || "",
    status: host.installationId ? "active" : "idle",
    dailyRateMinor: host.dailyRateMinor,
    currency: host.currency,
    offerRevision: host.offerRevision ?? 0,
    paymentMethodKinds: host.paymentMethodKinds || [],
    isClientOwner,
    canRelease: !!host.installationId,
    updatedAt: host.updatedAt || host.createdAt || new Date(0).toISOString(),
  };
}

function entryRank(status: string) {
  if (status === "release_failed") return 0;
  if (status === "billing_suspended") return 1;
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
  const rental = entry.rental;
  const host = entry.host;
  const status = rental?.status || host?.status || "unknown";
  const dailyRateMinor = rental?.dailyRateMinor ?? host?.dailyRateMinor;
  const currency = rental?.currency || host?.currency || "USD";
  const subdomain = host?.clientSubdomain;
  const title =
    subdomain ||
    host?.hostname ||
    host?.ip ||
    (rental?.installationId.startsWith("host:")
      ? rental.installationId.slice(5, 17)
      : rental?.installationId.slice(0, 12)) ||
    host?.id.slice(0, 12) ||
    "—";
  const anomalous = isAnomalous(status);
  const providerEmail = rental?.hostOwnerEmail || host?.hostOwnerEmail || "—";
  const renterEmail = rental?.clientOwnerEmail || host?.clientOwnerEmail || "";
  const marketHref = host?.installationId
    ? clientMarketMineHref(host.installationId)
    : rental && !rental.installationId.startsWith("host:")
      ? clientMarketMineHref(rental.installationId)
      : clientMarketMineHref();
  const updatedAt = rental?.updatedAt || host?.updatedAt || host?.createdAt;

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
          <dd className="font-medium">{offerLabel(dailyRateMinor, locale, currency)}</dd>
        </div>
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.nav.billing")}</dt>
          <dd className="font-medium">{dailyRateMinor ? t("marketBilling.status.active") : t("shareMarket.free")}</dd>
        </div>
        <div className="grid gap-0.5 sm:col-span-2">
          <dt className="text-xs text-muted-foreground">{t("account.client.updated")}</dt>
          <dd className="text-muted-foreground">
            {formatDate(updatedAt, locale)}
          </dd>
        </div>
      </dl>

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <p className="text-xs text-muted-foreground">{t("account.client.readOnlyHint")}</p>
        <div className="flex flex-wrap gap-2">
          {dailyRateMinor ? <Link href={DASHBOARD_ACCOUNT_BILLING_PATH} className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted">{t("marketBilling.open")}<ArrowUpRight className="h-3.5 w-3.5" aria-hidden /></Link> : null}
          <Link href={marketHref} className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted">{t("account.client.manageInMarket")}<ArrowUpRight className="h-3.5 w-3.5" aria-hidden /></Link>
        </div>
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
 * Includes free-forever Hosts (rental + host fallback). Actions stay on Client Market.
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
  const [rentalByInstallation, setRentalByInstallation] = React.useState<
    Map<string, ClientMarketRental>
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
      const [nextHosts, rentals] = await Promise.all([
        getClientMarketHosts(undefined, controller.signal),
        getMyClientMarketRentals(controller.signal),
      ]);
      if (controller.signal.aborted) return;
      setHosts((prev) => mergeHosts(prev, nextHosts));
      setRentalByInstallation((prev) => mergeRentalMap(prev, rentals));
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

  const allRentals = React.useMemo(
    () => Array.from(rentalByInstallation.values()),
    [rentalByInstallation],
  );

  const userEntries = React.useMemo(() => {
    const entries: MonitorEntry[] = [];
    const seen = new Set<string>();

    for (const rental of allRentals) {
      if (!rental.isClientOwner) continue;
      if (rental.status === "released" && !isRecentReleased(rental)) continue;
      seen.add(rental.installationId);
      entries.push({
        key: rental.installationId,
        rental,
        host: hostsByInstallation.get(rental.installationId),
      });
    }

    // Rentals may appear on Hosts before the next rental poll catches up.
    for (const host of hosts) {
      if (host.isClientOwner !== true || !host.installationId) continue;
      if (seen.has(host.installationId)) continue;
      seen.add(host.installationId);
      entries.push({
        key: host.installationId,
        rental: synthesizeRentalFromHost(host, true),
        host,
      });
    }

    return entries.sort((left, right) => {
      const leftStatus = left.rental?.status || left.host?.status || "";
      const rightStatus = right.rental?.status || right.host?.status || "";
      return (
        entryRank(leftStatus) - entryRank(rightStatus) || left.key.localeCompare(right.key)
      );
    });
  }, [allRentals, hosts, hostsByInstallation]);

  const providerEntries = React.useMemo(() => {
    const entries: MonitorEntry[] = [];
    const seenInstallations = new Set<string>();
    const seenHosts = new Set<string>();

    for (const rental of allRentals) {
      const host =
        hostsByInstallation.get(rental.installationId) || hostsById.get(rental.hostId);
      const isProvider =
        (!!viewerUserId && rental.providerId === viewerUserId) || host?.isHostOwner === true;
      if (!isProvider) continue;
      if (rental.status === "released" && !isRecentReleased(rental)) continue;
      seenInstallations.add(rental.installationId);
      if (host) seenHosts.add(host.id);
      entries.push({ key: rental.installationId, rental, host });
    }

    for (const host of hosts) {
      if (host.isHostOwner !== true) continue;

      // Allocated Client on this Host missing from the latest rental response.
      if (host.installationId && !seenInstallations.has(host.installationId)) {
        seenInstallations.add(host.installationId);
        seenHosts.add(host.id);
        entries.push({
          key: host.installationId,
          rental: synthesizeRentalFromHost(host, host.isClientOwner === true),
          host,
        });
        continue;
      }

      // Idle free-forever Hosts: still show under Provider monitor.
      if (
        !host.installationId &&
        isFreeForeverOffer(host.dailyRateMinor) &&
        !seenHosts.has(host.id)
      ) {
        seenHosts.add(host.id);
        entries.push({
          key: `host:${host.id}`,
          rental: synthesizeRentalFromHost(host, false),
          host,
        });
      }
    }

    return entries.sort((left, right) => {
      const leftStatus = left.rental?.status || left.host?.status || "";
      const rightStatus = right.rental?.status || right.host?.status || "";
      const leftRenter = left.rental?.clientOwnerEmail || left.host?.clientOwnerEmail || "";
      const rightRenter = right.rental?.clientOwnerEmail || right.host?.clientOwnerEmail || "";
      return (
        entryRank(leftStatus) - entryRank(rightStatus) ||
        leftRenter.localeCompare(rightRenter) ||
        left.key.localeCompare(right.key)
      );
    });
  }, [allRentals, hosts, hostsById, hostsByInstallation, viewerUserId]);

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
