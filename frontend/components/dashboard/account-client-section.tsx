"use client";

import * as React from "react";
import Link from "next/link";
import { ArrowUpRight } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CountryFlag } from "@/components/common/country-flag";
import { ExpandableRecordTable, RecordDetailList, type RecordTableColumn } from "@/components/common/expandable-record-table";
import { formatShortDate } from "@/components/dashboard/account-share-rentals";
import { AccountInlineLoading, AccountSectionError } from "@/components/dashboard/account-section";
import type { AccountMarketRole } from "@/components/dashboard/account-share-section";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import { MarketServiceDisputeControl } from "@/components/dashboard/market-service-dispute-control";
import { MarketWalletRenewalCard } from "@/components/dashboard/market-wallet-renewal-card";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientTerminationRecord } from "@/components/market/client-terminations-panel";
import { ClientTerminationDialog } from "@/components/market/client-termination-dialog";
import { useAccountClientTerminations } from "./use-account-client-terminations";
import { Button } from "@heroui/react";
import type { ClientTermination, ClientTerminationAction } from "@/lib/client-termination";
import { trialHours, walletLifecycleKey } from "@/lib/client-checkout";
import { getClientMarketHosts, getMyClientMarketRentals } from "@/lib/api";
import { rememberView, useRememberedView } from "@/lib/cached-resource";
import { mergeHosts, mergeRentalMap } from "@/lib/client-market-refresh";
import { clientMarketMineHref, DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import type { MessageKey } from "@/lib/i18n";
import { formatUsdMoney } from "@/lib/market-money";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { compareAccountRecords } from "@/lib/account-market-records";
import { terminationCanRespond } from "@/lib/client-termination";
import type { ClientMarketHost, ClientMarketRental } from "@/lib/types";
import { cn } from "@/lib/utils";

type ClientMonitorTab = AccountMarketRole;

type AccountClientView = {
  hosts: ClientMarketHost[];
  rentals: Map<string, ClientMarketRental>;
};

type MonitorEntry = {
  key: string;
  rental?: ClientMarketRental;
  host?: ClientMarketHost;
  termination?: ClientTermination;
};

function isFreeOffer(
  pricingModel: string | undefined,
  dailyRateMinor?: number | null,
  cyclePriceMinor?: number | null,
) {
  return pricingModel === "free"
    || (dailyRateMinor == null && cyclePriceMinor == null);
}

function offerLabel(
  dailyRateMinor: number | undefined,
  cyclePriceMinor: number | undefined,
  pricingModel: string | undefined,
  freeDurationDays: number | undefined,
  locale: string,
  _currency = "USD",
) {
  if (pricingModel === "prepaid_calendar_month" && cyclePriceMinor != null) {
    const amount = formatUsdMoney(cyclePriceMinor, locale);
    return locale.startsWith("zh") ? `${amount} / 月` : `${amount} / month`;
  }
  if (isFreeOffer(pricingModel, dailyRateMinor, cyclePriceMinor)) {
    if (freeDurationDays != null) {
      return locale.startsWith("zh")
        ? `免费 / ${freeDurationDays} 天`
        : `Free / ${freeDurationDays} ${freeDurationDays === 1 ? "day" : "days"}`;
    }
    return locale.startsWith("zh") ? "免费 / 永久" : "Free / permanent";
  }
  const amount = formatUsdMoney(dailyRateMinor as number, locale);
  return locale.startsWith("zh") ? `${amount} / 天` : `${amount} / day`;
}

function freePeriodTiming(
  rental: ClientMarketRental | undefined,
  locale: string,
  t: ReturnType<typeof useLocaleText>["t"],
) {
  if (!rental || !isFreeOffer(
    rental.pricingModel,
    rental.dailyRateMinor,
    rental.cyclePriceMinor,
  )) return null;
  if (!rental.activatedAt) {
    return rental.status === "idle" ? null : t("clientMarket.freeDuration.pendingActivation");
  }
  const activated = formatDate(rental.activatedAt, locale);
  if (!rental.expiresAt) {
    return `${t("clientMarket.freeDuration.activated")}: ${activated} · ${t("clientMarket.freeDuration.permanent")}`;
  }
  const expires = formatDate(rental.expiresAt, locale);
  return `${t("clientMarket.freeDuration.activated")}: ${activated} · ${t("clientMarket.freeDuration.expires")}: ${expires}`;
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
    pricingModel: host.pricingModel,
    cyclePriceMinor: host.cyclePriceMinor,
    billingInterval: host.billingInterval,
    currency: host.currency,
    freeDurationDays: host.freeDurationDays,
    offerRevision: host.offerRevision ?? 0,

    isClientOwner,
    canRelease: !!host.installationId,
    canFinalizeRelease: false,
    hostCleanupPending: false,
    providerTerminalAccessActive: false,
    canManageProviderTerminal: false,
    updatedAt: host.updatedAt || host.createdAt || new Date(0).toISOString(),
  };
}

const LINK_CLASS = "inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

/** Everything both tables and the details derive from one monitor entry. */
function entryFacts(entry: MonitorEntry) {
  const rental = entry.rental;
  const host = entry.host;
  const termination = entry.termination;
  const status = rental?.status || (termination ? "released" : host?.status) || "unknown";
  const dailyRateMinor = rental?.dailyRateMinor ?? host?.dailyRateMinor;
  const cyclePriceMinor = rental?.cyclePriceMinor ?? host?.cyclePriceMinor;
  const pricingModel = rental?.pricingModel ?? host?.pricingModel;
  const title =
    termination?.clientSubdomain ||
    host?.clientSubdomain ||
    host?.hostname ||
    host?.ip ||
    (rental?.installationId.startsWith("host:")
      ? rental.installationId.slice(5, 17)
      : rental?.installationId.slice(0, 12)) ||
    host?.id.slice(0, 12) ||
    termination?.installationId.slice(0, 12) ||
    "—";
  const installationId = rental?.installationId || termination?.installationId || host?.installationId;
  const marketHref = installationId && !installationId.startsWith("host:")
      ? clientMarketMineHref(installationId)
      : clientMarketMineHref();
  return {
    rental,
    host,
    status,
    title,
    dailyRateMinor,
    cyclePriceMinor,
    pricingModel,
    free: termination && !rental && !host ? BigInt(termination.periodAmountUnits) === 0n : isFreeOffer(pricingModel, dailyRateMinor, cyclePriceMinor),
    freeDurationDays: rental?.freeDurationDays ?? host?.freeDurationDays,
    currency: rental?.currency || host?.currency || "USD",
    anomalous: isAnomalous(status) || rental?.hostCleanupPending === true,
    providerEmail: rental?.hostOwnerEmail || termination?.providerEmail || host?.hostOwnerEmail || "",
    renterEmail: rental?.clientOwnerEmail || termination?.buyerEmail || host?.clientOwnerEmail || "",
    marketHref,
    updatedAt: rental?.updatedAt || termination?.updatedAt || host?.updatedAt || host?.createdAt,
  };
}

function entryPrice(entry: MonitorEntry, locale: string) {
  if (entry.termination && !entry.rental && !entry.host) return formatWalletUnits(entry.termination.periodAmountUnits, "$", locale);
  const facts = entryFacts(entry);
  return offerLabel(facts.dailyRateMinor, facts.cyclePriceMinor, facts.pricingModel, facts.freeDurationDays, locale);
}

function clientTermText(entry: MonitorEntry, locale: string, t: ReturnType<typeof useLocaleText>["t"]) {
  const facts = entryFacts(entry);
  const rental = facts.rental;
  if (!rental || facts.status === "released") return "—";
  if (rental.walletContract) {
    return t(walletLifecycleKey(rental.walletContract), {
      date: formatShortDate(rental.walletContract.periodEnd, locale),
      hours: trialHours(rental.walletContract.trialSecondsRemaining, locale),
    });
  }
  if (!facts.free) return "—";
  if (!rental.activatedAt) {
    return rental.status === "idle" ? "—" : t("clientMarket.freeDuration.pendingActivation");
  }
  return rental.expiresAt
    ? t("clientCheckout.lifecycle.expires", { date: formatShortDate(rental.expiresAt, locale) })
    : t("clientMarket.freeDuration.permanent");
}

function EntryDetail({ entry, perspective, onChanged, onTerminationOpen }: {
  entry: MonitorEntry;
  perspective: ClientMonitorTab;
  onChanged: () => Promise<void> | void;
  onTerminationOpen: (item: ClientTermination, action?: ClientTerminationAction) => void;
}) {
  const { locale, t } = useLocaleText();
  const facts = entryFacts(entry);
  const rental = facts.rental;
  const freeTiming = freePeriodTiming(rental, locale, t);
  const contractId = rental?.walletContract?.id || rental?.walletContractId;
  return (
    <div className="grid min-w-0 gap-3">
      <RecordDetailList items={[
        { label: t("account.client.offer"), value: entryPrice(entry, locale) },
        { label: t("account.market.col.status"), value: t(statusLabelKey(facts.status)) },
        ...(freeTiming ? [{ label: t("account.market.detail.service"), value: freeTiming }] : []),
        ...(facts.host?.status && facts.host.status !== facts.status
          ? [{ label: t("account.market.detail.hostStatus"), value: facts.host.status }] : []),
        perspective === "user"
          ? { label: t("account.market.col.provider"), value: <span className="break-all">{facts.providerEmail || "—"}</span> }
          : { label: t("account.market.col.renter"), value: <span className="break-all">{facts.renterEmail || t("account.client.noRenter")}</span> },
        { label: t("account.client.updated"), value: formatDate(facts.updatedAt, locale) },
        { label: t("account.market.col.term"), value: clientTermText(entry, locale, t) },
      ]} />
      {rental?.hostCleanupPending ? <p className="rounded-md bg-amber-50 px-3 py-2 text-xs text-amber-900">{t("clientMarket.release.hostPending")}</p> : null}
      {perspective === "user" && facts.status !== "released" && contractId && rental?.cyclePriceMinor != null ? (
        <MarketWalletRenewalCard
          contractId={contractId}
          refreshKey={JSON.stringify([rental.walletContract?.status, rental.walletContract?.renewalPolicy, rental.walletContract?.periodEnd, rental.walletContract?.trialSecondsRemaining])}
          compact
          onChanged={onChanged}
        />
      ) : null}
      {perspective === "user" && rental?.walletContractId && (facts.status !== "released" || rental.openWalletDispute) ? (
        <MarketServiceDisputeControl contractId={rental.walletContractId} initiallyOpen={rental.openWalletDispute} onChanged={onChanged} />
      ) : null}
      {entry.termination ? <section className="grid min-w-0 gap-3 border-t border-border pt-3" aria-label={t("clientTermination.title")}>
        <h4 className="text-sm font-semibold">{t("clientTermination.title")}</h4>
        <ClientTerminationRecord item={entry.termination} role={perspective === "user" ? "buyer" : "provider"} onOpen={onTerminationOpen} />
      </section> : null}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {perspective === "user" && !facts.free ? <Link href={DASHBOARD_ACCOUNT_WALLET_PATH} className={LINK_CLASS}>{t("account.nav.wallet")}<ArrowUpRight className="h-3.5 w-3.5" aria-hidden /></Link> : null}
        <Link href={facts.marketHref} className={LINK_CLASS}>{t("account.client.manageInMarket")}<ArrowUpRight className="h-3.5 w-3.5" aria-hidden /></Link>
      </div>
    </div>
  );
}

function MonitorTable({
  entries,
  perspective,
  emptyKey,
  focusKey,
  onChanged,
  onTerminationOpen,
}: {
  entries: MonitorEntry[];
  perspective: ClientMonitorTab;
  emptyKey: "account.client.userEmpty" | "account.client.providerEmpty";
  focusKey?: string | null;
  onChanged: () => Promise<void> | void;
  onTerminationOpen: (item: ClientTermination, action?: ClientTerminationAction) => void;
}) {
  const { locale, t } = useLocaleText();
  const counterparty = (entry: MonitorEntry) => {
    const facts = entryFacts(entry);
    return perspective === "user" ? facts.providerEmail || "—" : facts.renterEmail || t("account.market.notRented");
  };
  const columns: RecordTableColumn<MonitorEntry>[] = [
    {
      id: "client",
      header: t(perspective === "user" ? "account.market.col.client" : "account.market.col.host"),
      cell: (entry) => {
        const facts = entryFacts(entry);
        return (
          <div className="grid min-w-0 gap-0.5">
            <span className="flex min-w-0 items-center gap-2">
              {facts.host?.countryCode ? <CountryFlag code={facts.host.countryCode} /> : null}
              <span className="min-w-0 truncate font-medium">{facts.title}</span>
            </span>
            <span className="truncate text-xs text-muted-foreground md:hidden">{counterparty(entry)}</span>
          </div>
        );
      },
    },
    {
      id: "counterparty",
      header: t(perspective === "user" ? "account.market.col.provider" : "account.market.col.renter"),
      minWidth: "md",
      cell: (entry) => <span className="block truncate text-muted-foreground" title={counterparty(entry)}>{counterparty(entry)}</span>,
    },
    {
      id: "status",
      header: t("account.market.col.status"),
      className: "w-16 sm:w-32",
      cell: (entry) => {
        const facts = entryFacts(entry);
        return (
          <span className={cn("block truncate", facts.anomalous ? "font-medium text-amber-700" : null)}>
            {t(facts.status === "released" ? "account.client.status.ended" : statusLabelKey(facts.status))}
          </span>
        );
      },
    },
    {
      id: "price",
      header: t("account.market.col.price"),
      className: "w-20 sm:w-32",
      cell: (entry) => {
        return <span className="block truncate tabular-nums">{entryPrice(entry, locale)}</span>;
      },
    },
    perspective === "user"
      ? {
        id: "term",
        header: t("account.market.col.term"),
        minWidth: "lg",
        className: "w-48",
        cell: (entry) => <span className="block truncate text-muted-foreground">{clientTermText(entry, locale, t)}</span>,
      }
      : {
        id: "updated",
        header: t("account.market.col.updated"),
        minWidth: "lg",
        className: "w-44",
        cell: (entry) => <span className="block truncate text-muted-foreground">{formatDate(entryFacts(entry).updatedAt, locale)}</span>,
      },
    { id: "manage", header: <span className="sr-only">{t("account.market.col.actions")}</span>, minWidth: "sm", className: "w-12",
      cell: (entry) => <Link href={entryFacts(entry).marketHref} aria-label={`${t("account.client.manageInMarket")} · ${entryFacts(entry).title}`} className="inline-flex size-11 items-center justify-center rounded-md hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring"><ArrowUpRight className="size-4" aria-hidden /></Link> },
  ];
  return (
    <ExpandableRecordTable
      rows={entries}
      rowKey={(entry) => entry.key}
      rowLabel={(entry) => entryFacts(entry).title}
      ariaLabel={t(perspective === "user" ? "account.rentals.clientSection" : "account.listings.clientSection")}
      columns={columns}
      rowTone={(entry) => {
        const status = entryFacts(entry).status;
        return status === "release_failed" ? "danger" : entryFacts(entry).anomalous ? "attention" : undefined;
      }}
      focusKey={focusKey}
      renderDetail={(entry) => <EntryDetail entry={entry} perspective={perspective} onChanged={onChanged} onTerminationOpen={onTerminationOpen} />}
      empty={(
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
      )}
    />
  );
}

/**
 * Client Market leases of this account: what it rents (`user`) or the Hosts it
 * rents out (`provider`), including idle free Hosts. Renewal, disputes and
 * terminations are handled in place; the rest stays on Client Market.
 */
export function AccountClientSection({ role, focusKey }: { role: ClientMonitorTab; focusKey?: string | null }) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id || session.user?.email : undefined;
  return <AccountClientWorkspace key={actor || "anonymous"} tab={role} focusKey={focusKey ?? null} />;
}

function AccountClientWorkspace({ tab, focusKey }: { tab: ClientMonitorTab; focusKey: string | null }) {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const viewerUserId = session?.user?.id;
  const terminations = useAccountClientTerminations(authed ? viewerUserId : undefined, tab === "user" ? "buyer" : "provider", focusKey);
  const [selection, setSelection] = React.useState<{ id: string; role: "buyer" | "provider"; action?: ClientTerminationAction } | null>(null);
  const openTermination = (item: ClientTermination, action?: ClientTerminationAction) => setSelection({ id: item.id, role: tab === "user" ? "buyer" : "provider", action });
  const attachTerminations = (entries: MonitorEntry[]) => {
    const byInstallation = new Map(terminations.items.map(item => [item.installationId, item]));
    const seen = new Set(entries.map(entry => entry.key));
    return [...entries.map(entry => ({ ...entry, termination: byInstallation.get(entry.key) })),
      ...terminations.items.filter(item => !seen.has(item.installationId)).map(item => ({ key: item.installationId, termination: item }))]
      .sort((a, b) => {
        const order = (entry: MonitorEntry) => {
          const facts = entryFacts(entry);
          return { id: entry.key, attention: facts.anomalous || !!entry.termination && (entry.termination.status === "cleanup_failed" || terminationCanRespond(entry.termination)),
            ended: facts.status === "released", expiresAt: entry.rental?.walletContract?.periodEnd || entry.rental?.expiresAt, updatedAt: facts.updatedAt };
        };
        return compareAccountRecords(order(a), order(b));
      });
  };

  const viewKey = authed && viewerUserId ? `account-client:${viewerUserId}` : null;
  const { remembered, takeSeed } = useRememberedView<AccountClientView>(viewKey);
  const [hosts, setHosts] = React.useState<ClientMarketHost[]>(remembered?.hosts ?? []);
  const [rentalByInstallation, setRentalByInstallation] = React.useState<
    Map<string, ClientMarketRental>
  >(() => remembered?.rentals ?? new Map());
  const [loading, setLoading] = React.useState(!remembered);
  const [loadedOk, setLoadedOk] = React.useState(remembered !== undefined);
  const [error, setError] = React.useState("");
  const refreshAbortRef = React.useRef<AbortController | null>(null);

  const load = React.useCallback(async (options?: { silent?: boolean; reportErrors?: boolean }) => {
    const silent = options?.silent === true;
    const reportErrors = !silent || options?.reportErrors === true;
    if (silent && refreshAbortRef.current) return;
    if (!silent) refreshAbortRef.current?.abort();
    const controller = new AbortController();
    refreshAbortRef.current = controller;
    if (!silent) setLoading(true);
    if (reportErrors) setError("");
    try {
      const [nextHosts, rentals] = await Promise.all([
        getClientMarketHosts(undefined, controller.signal),
        getMyClientMarketRentals(controller.signal),
      ]);
      if (controller.signal.aborted) return;
      setHosts((prev) => mergeHosts(prev, nextHosts));
      setRentalByInstallation((prev) => mergeRentalMap(prev, rentals));
      setLoadedOk(true);
    } catch (err) {
      if (controller.signal.aborted) return;
      if (reportErrors) setError(err instanceof Error ? err.message : String(err));
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
    // A remembered view is already on screen; refresh it without a spinner
    // but still say so when the refresh fails.
    void load(takeSeed() ? { silent: true, reportErrors: true } : undefined);
    return () => refreshAbortRef.current?.abort();
  }, [authed, load, takeSeed]);

  React.useEffect(() => {
    if (loadedOk) rememberView<AccountClientView>(viewKey, { hosts, rentals: rentalByInstallation });
  }, [hosts, loadedOk, rentalByInstallation, viewKey]);

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

    return entries;
  }, [allRentals, hosts, hostsByInstallation]);

  const providerEntries = React.useMemo(() => {
    const entries: MonitorEntry[] = [];
    const seenInstallations = new Set<string>();
    const seenHosts = new Set<string>();

    for (const rental of allRentals) {
      const providerHost = hostsById.get(rental.hostId);
      // A re-rented Host must not give an ended lease the new tenant's identity.
      const host = hostsByInstallation.get(rental.installationId)
        || (rental.status !== "released" && !providerHost?.installationId ? providerHost : undefined);
      const isProvider =
        (!!viewerUserId && rental.providerId === viewerUserId) || providerHost?.isHostOwner === true;
      if (!isProvider) continue;
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

      // Idle free Hosts: still show under Provider monitor.
      if (
        !host.installationId &&
        isFreeOffer(host.pricingModel, host.dailyRateMinor, host.cyclePriceMinor) &&
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

    return entries;
  }, [allRentals, hosts, hostsById, hostsByInstallation, viewerUserId]);

  if (!authed) return null;
  if (loading) return <AccountInlineLoading />;
  if (error && !loadedOk) return <AccountSectionError message={error} onRetry={() => void load()} />;

  return (
    <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4">
      {error ? <AccountSectionError message={error} onRetry={() => void load()} /> : null}
      <MonitorTable
        entries={attachTerminations(tab === "user" ? userEntries : providerEntries)}
        perspective={tab}
        emptyKey={tab === "user" ? "account.client.userEmpty" : "account.client.providerEmpty"}
        focusKey={focusKey}
        onChanged={() => load({ silent: true })}
        onTerminationOpen={openTermination}
      />
      {terminations.error ? <AccountSectionError message={t("account.market.terminationsUnavailable")} onRetry={() => void terminations.reload()} /> : null}
      {terminations.nextCursor ? <Button variant="ghost" className="min-h-11 justify-self-start" isDisabled={terminations.loading} onPress={() => void terminations.more()}>{t("account.market.earlierTerminations")}</Button> : null}
      <ClientTerminationDialog terminationId={selection?.id || null} initialRole={selection?.role} initialAction={selection?.action}
        onClose={() => setSelection(null)} onChanged={() => { void terminations.reload(); void load({ silent: true }); }} />
    </div>
  );
}
