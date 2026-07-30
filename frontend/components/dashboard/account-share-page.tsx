"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Chip, Tabs } from "@heroui/react";
import { ArrowUpRight, ExternalLink, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { subdomainTunnelUrl } from "@/components/dashboard/share-dashboard-utils";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getShareMarketCatalog } from "@/lib/api";
import {
  DASHBOARD_ACCOUNT_BILLING_PATH,
  DASHBOARD_ACCOUNT_SHARE_PATH,
  shareMarketHref,
} from "@/lib/dashboard-nav";
import type { MessageKey } from "@/lib/i18n";
import type {
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketSubscription,
} from "@/lib/types";

type ShareMonitorTab = "user" | "provider";

function subscriptionStatusKey(status: string): MessageKey {
  switch (status) {
    case "grant_pending":
      return "shareMarket.subscription.grantPending";
    case "active_free":
      return "shareMarket.subscription.activeFree";
    case "active_postpaid":
      return "shareMarket.subscription.activePostpaid";
    case "billing_suspend_pending":
      return "shareMarket.subscription.billingSuspendPending";
    case "billing_suspended":
      return "shareMarket.subscription.billingSuspended";
    case "billing_resume_pending":
      return "shareMarket.subscription.billingResumePending";
    case "billing_control_failed":
      return "shareMarket.subscription.billingControlRetry";
    case "revoke_pending":
      return "shareMarket.subscription.revokePending";
    case "revoke_failed":
      return "shareMarket.subscription.revokeFailed";
    case "grant_failed":
      return "shareMarket.subscription.grantFailed";
    case "released":
      return "shareMarket.subscription.released";
    default:
      return "account.share.status.unknown";
  }
}

function isAnomalous(status: string) {
  return (
    status === "grant_pending" ||
    status === "billing_suspend_pending" ||
    status === "billing_suspended" ||
    status === "billing_resume_pending" ||
    status === "billing_control_failed" ||
    status === "revoke_pending" ||
    status === "revoke_failed" ||
    status === "grant_failed"
  );
}

function anomalyRank(status: string) {
  switch (status) {
    case "billing_control_failed":
      return 0;
    case "revoke_failed":
      return 1;
    case "grant_failed":
      return 2;
    case "billing_suspended":
      return 3;
    case "billing_suspend_pending":
      return 4;
    case "billing_resume_pending":
      return 5;
    case "revoke_pending":
      return 6;
    case "grant_pending":
      return 7;
    default:
      return 10;
  }
}

function sortSubscriptions(left: ShareMarketSubscription, right: ShareMarketSubscription) {
  return (
    anomalyRank(left.status) - anomalyRank(right.status) ||
    Date.parse(right.updatedAt) - Date.parse(left.updatedAt) ||
    left.shareName.localeCompare(right.shareName)
  );
}

function offerLabel(subscription: ShareMarketSubscription, locale: string, freeLabel: string) {
  if (subscription.dailyRateMinor == null) return freeLabel;
  const amount = (subscription.dailyRateMinor / 100).toFixed(2);
  const currency = subscription.currency || "CNY";
  return locale.startsWith("zh") ? `${currency} ${amount} / 天` : `${currency} ${amount} / day`;
}

function formatDate(value: string | undefined, locale: string) {
  if (!value) return "-";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(timestamp))
    : value;
}

function shareOpenUrl(subdomain?: string | null) {
  return subdomainTunnelUrl(subdomain);
}

function SubscriptionMonitorCard({
  subscription,
  perspective,
}: {
  subscription: ShareMarketSubscription;
  perspective: ShareMonitorTab;
}) {
  const { locale, t } = useLocaleText();
  const anomalous = isAnomalous(subscription.status);
  const openUrl = shareOpenUrl(subscription.subdomain);
  const manageHref =
    perspective === "provider"
      ? shareMarketHref({ tab: "mine", shareId: subscription.shareId })
      : shareMarketHref({ tab: "rentals", shareId: subscription.shareId });

  return (
    <section
      className={`grid gap-3 rounded-xl border bg-card p-4 shadow-sm sm:p-5 ${
        anomalous ? "border-rose-200/80 ring-1 ring-rose-100" : "border-border"
      }`}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <ShareAppLogo
          app={
            subscription.appType === "claude" || subscription.appType === "gemini"
              ? subscription.appType
              : "codex"
          }
          size={18}
        />
        <strong className="truncate text-sm">{subscription.shareName}</strong>
        <Chip size="sm" variant={anomalous ? "primary" : "tertiary"}>
          {t(subscriptionStatusKey(subscription.status))}
        </Chip>
        {subscription.shareOnline != null ? (
          <Chip size="sm" variant="tertiary">
            {subscription.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
          </Chip>
        ) : null}
        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {perspective === "user"
            ? t("account.share.provider", { owner: subscription.ownerEmail })
            : t("account.share.renter", { email: subscription.renterEmail })}
        </span>
      </div>

      <dl className="grid gap-2 text-sm sm:grid-cols-2">
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.share.offer")}</dt>
          <dd className="font-medium">{offerLabel(subscription, locale, t("shareMarket.free"))}</dd>
        </div>
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.share.updated")}</dt>
          <dd className="text-muted-foreground">
            {formatDate(subscription.updatedAt, locale)}
          </dd>
        </div>
      </dl>

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <p className="text-xs text-muted-foreground">{t("account.share.readOnlyHint")}</p>
        <div className="flex flex-wrap gap-2">
          {subscription.dailyRateMinor != null ? (
            <Link
              href={DASHBOARD_ACCOUNT_BILLING_PATH}
              className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted"
            >
              {t("marketBilling.open")}
              <ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
            </Link>
          ) : null}
          {openUrl ? (
            <a
              href={openUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted"
            >
              {t("account.share.openShare")}
              <ExternalLink className="h-3.5 w-3.5" aria-hidden />
            </a>
          ) : null}
          <Link
            href={manageHref}
            className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted"
          >
            {t("account.share.manageInMarket")}
            <ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
          </Link>
        </div>
      </div>
    </section>
  );
}

function ListingSummaryCard({ listing }: { listing: ShareMarketListing }) {
  const { t } = useLocaleText();
  const seats = listing.seats;
  const available = seats.filter((seat) => seat.status === "available").length;
  const occupied = seats.filter(
    (seat) => seat.status === "occupied" || seat.status === "reserved" || seat.status === "revoking",
  ).length;
  const anomalousSeats = seats.filter((seat) => {
    const status = seat.subscription?.status;
    return status ? isAnomalous(status) : false;
  }).length;
  const openUrl = shareOpenUrl(listing.subdomain);

  return (
    <section className="grid gap-3 rounded-xl border border-border bg-card p-4 shadow-sm sm:p-5">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <ShareAppLogo
          app={listing.appType === "claude" || listing.appType === "gemini" ? listing.appType : "codex"}
          size={18}
        />
        <strong className="truncate text-sm">{listing.shareName}</strong>
        <Chip size="sm" variant="tertiary">
          {listing.status === "closed" ? t("shareMarket.closed") : t("account.share.listingActive")}
        </Chip>
        <Chip size="sm" variant="tertiary">
          {listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
        </Chip>
      </div>
      <dl className="grid gap-2 text-sm sm:grid-cols-3">
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.share.seatsAvailable")}</dt>
          <dd className="font-medium">{available}</dd>
        </div>
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.share.seatsOccupied")}</dt>
          <dd className="font-medium">{occupied}</dd>
        </div>
        <div className="grid gap-0.5">
          <dt className="text-xs text-muted-foreground">{t("account.share.seatsAttention")}</dt>
          <dd className={`font-medium ${anomalousSeats ? "text-rose-600" : ""}`}>{anomalousSeats}</dd>
        </div>
      </dl>
      {listing.status === "closed" ? (
        <p className="text-xs text-muted-foreground">{t("account.share.closedListingHint")}</p>
      ) : null}
      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <p className="text-xs text-muted-foreground">{t("account.share.readOnlyHint")}</p>
        <div className="flex flex-wrap gap-2">
          {openUrl ? (
            <a
              href={openUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted"
            >
              {t("account.share.openShare")}
              <ExternalLink className="h-3.5 w-3.5" aria-hidden />
            </a>
          ) : null}
          <Link
            href={shareMarketHref({ tab: "mine", shareId: listing.shareId })}
            className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium text-foreground transition-colors hover:border-accent/30 hover:bg-muted"
          >
            {t("account.share.manageInMarket")}
            <ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
          </Link>
        </div>
      </div>
    </section>
  );
}

/**
 * Account → Share: read-only Provider / User monitor for Share Market rentals.
 * Actions live exclusively on Share Market.
 */
export function AccountSharePage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const router = useRouter();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;

  const tabParam = searchParams.get("tab");
  const tab: ShareMonitorTab = tabParam === "provider" ? "provider" : "user";

  const [catalog, setCatalog] = React.useState<ShareMarketCatalog | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const refreshAbortRef = React.useRef<AbortController | null>(null);

  const setTab = React.useCallback(
    (next: ShareMonitorTab) => {
      const params = new URLSearchParams(searchParams.toString());
      params.set("tab", next);
      router.replace(`${DASHBOARD_ACCOUNT_SHARE_PATH}?${params.toString()}`, { scroll: false });
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
      const next = await getShareMarketCatalog(controller.signal);
      if (controller.signal.aborted) return;
      setCatalog(next);
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

  const userSubscriptions = React.useMemo(
    () => [...(catalog?.mySubscriptions || [])].sort(sortSubscriptions),
    [catalog],
  );

  const ownedListings = React.useMemo(
    () => (catalog?.listings || []).filter((listing) => listing.isOwner),
    [catalog],
  );

  const providerSubscriptions = React.useMemo(() => {
    const rows: ShareMarketSubscription[] = [];
    for (const listing of ownedListings) {
      for (const seat of listing.seats) {
        if (seat.subscription) rows.push(seat.subscription);
      }
    }
    return rows.sort(sortSubscriptions);
  }, [ownedListings]);

  if (!authed) {
    return <p className="py-6 text-sm text-muted-foreground">{t("shareMarket.loginRequired")}</p>;
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
        <h2 className="text-base font-semibold text-foreground">{t("account.nav.share")}</h2>
        <p className="mt-0.5 text-sm text-muted-foreground">{t("account.shareHint")}</p>
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
            {t("account.share.tab.user")}
          </Tabs.Tab>
          <Tabs.Tab id="provider" className="px-3 py-2 text-sm font-medium !text-slate-900 data-[selected=true]:!text-slate-900">
            {t("account.share.tab.provider")}
          </Tabs.Tab>
        </Tabs.List>
      </Tabs>

      {tab === "user" ? (
        userSubscriptions.length ? (
          <div className="grid gap-3">
            {userSubscriptions.map((subscription) => (
              <SubscriptionMonitorCard
                key={`user:${subscription.id}`}
                subscription={subscription}
                perspective="user"
              />
            ))}
          </div>
        ) : (
          <div className="grid justify-items-center gap-2 rounded-xl border border-dashed border-border bg-card/40 px-4 py-12 text-center text-sm text-muted-foreground">
            <span>{t("account.share.userEmpty")}</span>
            <Link
              href={shareMarketHref({ tab: "all" })}
              className="inline-flex items-center gap-1 text-sm font-medium text-accent hover:underline"
            >
              {t("account.share.openMarket")}
              <ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
            </Link>
          </div>
        )
      ) : ownedListings.length || providerSubscriptions.length ? (
        <div className="grid gap-3">
          {ownedListings.map((listing) => (
            <ListingSummaryCard key={`listing:${listing.id}`} listing={listing} />
          ))}
          {providerSubscriptions.map((subscription) => (
            <SubscriptionMonitorCard
              key={`provider:${subscription.id}`}
              subscription={subscription}
              perspective="provider"
            />
          ))}
        </div>
      ) : (
        <div className="grid justify-items-center gap-2 rounded-xl border border-dashed border-border bg-card/40 px-4 py-12 text-center text-sm text-muted-foreground">
          <span>{t("account.share.providerEmpty")}</span>
          <Link
            href={shareMarketHref({ tab: "mine" })}
            className="inline-flex items-center gap-1 text-sm font-medium text-accent hover:underline"
          >
            {t("account.share.openMarket")}
            <ArrowUpRight className="h-3.5 w-3.5" aria-hidden />
          </Link>
        </div>
      )}
    </div>
  );
}
