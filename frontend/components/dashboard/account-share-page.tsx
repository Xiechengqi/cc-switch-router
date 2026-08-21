"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Chip } from "@heroui/react";
import {
  ArrowUpRight,
  ExternalLink,
  Loader2,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { subdomainTunnelUrl } from "@/components/dashboard/share-dashboard-utils";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import {
  ShareMarketBuyerRentals,
  ShareMarketSubscriptionCard,
  sortShareMarketSubscriptions,
} from "@/components/dashboard/share-market/buyer-rentals";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getShareMarketOwnedListings,
  getShareMarketSubscriptions,
} from "@/lib/api";
import {
  DASHBOARD_ACCOUNT_SHARE_PATH,
  shareMarketHref,
} from "@/lib/dashboard-nav";
import type { ShareMarketListing, ShareMarketSubscription } from "@/lib/types";
import { isCoreShareApp } from "@/components/dashboard/share-market/market-utils";

type ShareMonitorTab = "user" | "provider";
function isAnomalous(status: string) {
  return [
    "grant_pending",
    "billing_suspend_pending",
    "billing_suspended",
    "billing_resume_pending",
    "billing_control_failed",
    "revoke_pending",
    "revoke_failed",
    "grant_failed",
  ].includes(status);
}

function ListingSummaryCard({ listing }: { listing: ShareMarketListing }) {
  const { t } = useLocaleText();
  const available = listing.seats.filter((seat) => seat.status === "available").length;
  const occupied = listing.seats.filter((seat) => ["occupied", "reserved", "revoking"].includes(seat.status)).length;
  const attention = listing.seats.filter((seat) => seat.subscription && isAnomalous(seat.subscription.status)).length;
  const openUrl = subdomainTunnelUrl(listing.subdomain);
  return (
    <section className="grid gap-3 rounded-md border border-border bg-card p-4 shadow-sm sm:p-5">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {listing.supportedApps.filter(isCoreShareApp).map((app) => <ShareAppLogo key={app} app={app} size={18} />)}
        <strong className="truncate text-sm">{listing.shareName}</strong>
        <Chip size="sm" variant="tertiary">{listing.status === "closed" ? t("shareMarket.closed") : t("account.share.listingActive")}</Chip>
        <Chip size="sm" variant="tertiary">{listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}</Chip>
      </div>
      <dl className="grid grid-cols-3 gap-3 text-sm">
        <div><dt className="text-xs text-muted-foreground">{t("account.share.seatsAvailable")}</dt><dd className="font-medium">{available}</dd></div>
        <div><dt className="text-xs text-muted-foreground">{t("account.share.seatsOccupied")}</dt><dd className="font-medium">{occupied}</dd></div>
        <div><dt className="text-xs text-muted-foreground">{t("account.share.seatsAttention")}</dt><dd className={attention ? "font-medium text-rose-600" : "font-medium"}>{attention}</dd></div>
      </dl>
      <div className="flex flex-wrap justify-end gap-2 border-t border-border/70 pt-3">
        {openUrl ? <a href={openUrl} target="_blank" rel="noopener noreferrer" className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted">{t("account.share.openShare")}<ExternalLink className="h-3.5 w-3.5" /></a> : null}
        <Link href={shareMarketHref({ workspace: "selling", shareId: listing.shareId })} className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted">{t("account.share.manageInMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link>
      </div>
    </section>
  );
}

export function AccountSharePage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const router = useRouter();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const tab: ShareMonitorTab = searchParams.get("tab") === "provider" ? "provider" : "user";
  const [subscriptions, setSubscriptions] = React.useState<ShareMarketSubscription[]>([]);
  const [ownedListings, setOwnedListings] = React.useState<ShareMarketListing[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [pausePolling, setPausePolling] = React.useState(false);
  const abortRef = React.useRef<AbortController | null>(null);

  const setTab = (next: ShareMonitorTab) => {
    const params = new URLSearchParams(searchParams.toString());
    params.set("tab", next);
    router.replace(`${DASHBOARD_ACCOUNT_SHARE_PATH}?${params.toString()}`, { scroll: false });
  };
  const load = React.useCallback(async ({
    silent = false,
    skipIfBusy = false,
  }: { silent?: boolean; skipIfBusy?: boolean } = {}) => {
    if (!authed) return;
    if (skipIfBusy && abortRef.current) return;
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    if (!silent) setLoading(true);
    if (!skipIfBusy) setError("");
    try {
      const [nextSubscriptions, nextListings] = await Promise.all([
        getShareMarketSubscriptions(controller.signal),
        getShareMarketOwnedListings(controller.signal),
      ]);
      if (controller.signal.aborted) return;
      setSubscriptions(nextSubscriptions.subscriptions);
      setOwnedListings(nextListings.listings);
    } catch (reason) {
      if (!controller.signal.aborted && !skipIfBusy) setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null;
        setLoading(false);
      }
    }
  }, [authed]);
  React.useEffect(() => { if (authed) void load(); else setLoading(false); return () => abortRef.current?.abort(); }, [authed, load]);
  React.useEffect(() => {
    if (!authed || pausePolling) return;
    const timer = window.setInterval(
      () => void load({ silent: true, skipIfBusy: true }),
      CLIENT_MARKET_POLL_MS,
    );
    return () => window.clearInterval(timer);
  }, [authed, load, pausePolling]);

  const providerSubscriptions = React.useMemo(
    () => ownedListings
      .flatMap((listing) => listing.seats.flatMap((seat) => seat.subscription ? [seat.subscription] : []))
      .sort(sortShareMarketSubscriptions),
    [ownedListings],
  );

  if (!authed) return <p className="py-6 text-sm text-muted-foreground">{t("shareMarket.loginRequired")}</p>;
  if (loading) return <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  return (
    <div className="grid min-w-0 gap-4">
      <div><h2 className="text-base font-semibold text-foreground">{t("account.nav.share")}</h2><p className="mt-0.5 text-sm text-muted-foreground">{t("account.shareHint")}</p></div>
      <SegmentedControl value={tab} onChange={setTab} ariaLabel={t("account.nav.share")} size="md" className="w-full max-w-sm" fullWidth items={[{ id: "user", label: t("account.share.tab.user") }, { id: "provider", label: t("account.share.tab.provider") }]} />
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}
      {tab === "user" ? (
        <ShareMarketBuyerRentals
          subscriptions={subscriptions}
          loading={false}
          onChanged={() => load({ silent: true })}
          onInteractionChange={setPausePolling}
        />
      ) : ownedListings.length || providerSubscriptions.length ? (
        <div className="grid gap-3">
          {ownedListings.map((listing) => <ListingSummaryCard key={listing.id} listing={listing} />)}
          {providerSubscriptions.map((subscription) => (
            <ShareMarketSubscriptionCard key={subscription.id} subscription={subscription} perspective="provider" busy={false} />
          ))}
        </div>
      ) : <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-12 text-center text-sm text-muted-foreground"><span>{t("account.share.providerEmpty")}</span><Link href={shareMarketHref()} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link></div>}
    </div>
  );
}
