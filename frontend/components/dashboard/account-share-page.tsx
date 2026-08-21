"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Button, Chip } from "@heroui/react";
import {
  ArrowUpRight,
  Check,
  ExternalLink,
  Loader2,
  RotateCcw,
  X,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { subdomainTunnelUrl } from "@/components/dashboard/share-dashboard-utils";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  acceptShareMarketPriceChange,
  getShareMarketOwnedListings,
  getShareMarketSubscriptions,
  rejectShareMarketPriceChange,
  releaseShareMarketSubscription,
} from "@/lib/api";
import {
  DASHBOARD_ACCOUNT_BILLING_PATH,
  DASHBOARD_ACCOUNT_SHARE_PATH,
  shareMarketHref,
} from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import type { ShareMarketListing, ShareMarketSubscription } from "@/lib/types";
import {
  isCoreShareApp,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";

type ShareMonitorTab = "user" | "provider";
type PendingAction = {
  title: string;
  description: string;
  label: string;
  run: () => Promise<unknown>;
};

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

function anomalyRank(status: string) {
  return [
    "billing_control_failed",
    "revoke_failed",
    "grant_failed",
    "billing_suspended",
    "billing_suspend_pending",
    "billing_resume_pending",
    "revoke_pending",
    "grant_pending",
  ].indexOf(status);
}

function sortSubscriptions(left: ShareMarketSubscription, right: ShareMarketSubscription) {
  const leftRank = anomalyRank(left.status);
  const rightRank = anomalyRank(right.status);
  return (
    (leftRank < 0 ? 99 : leftRank) - (rightRank < 0 ? 99 : rightRank)
    || Date.parse(right.updatedAt) - Date.parse(left.updatedAt)
    || left.shareName.localeCompare(right.shareName)
  );
}

function offerLabel(
  subscription: ShareMarketSubscription,
  locale: string,
  t: ReturnType<typeof useLocaleText>["t"],
) {
  const serviceTerm = subscription.serviceDurationDays == null
    ? t("shareMarket.serviceDuration.permanent")
    : t("shareMarket.serviceDuration.daysValue", { count: subscription.serviceDurationDays });
  const price = subscription.dailyRateMinor == null
    ? t("shareMarket.free")
    : `${formatUsdMoney(subscription.dailyRateMinor, locale)} / ${t("marketBilling.day")}`;
  return `${price} · ${serviceTerm}`;
}

function formatDate(value: string | undefined, locale: string) {
  if (!value) return "-";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(timestamp))
    : value;
}

function SubscriptionCard({
  subscription,
  perspective,
  busy,
  onRelease,
  onAcceptPrice,
  onRejectPrice,
}: {
  subscription: ShareMarketSubscription;
  perspective: ShareMonitorTab;
  busy: boolean;
  onRelease: () => void;
  onAcceptPrice: () => void;
  onRejectPrice: () => void;
}) {
  const { locale, t } = useLocaleText();
  const anomalous = isAnomalous(subscription.status);
  const openUrl = subdomainTunnelUrl(subscription.subdomain);
  const statusKey = subscriptionStatusKey(subscription.status);
  const manageHref = perspective === "provider"
    ? shareMarketHref({ workspace: "selling", shareId: subscription.shareId })
    : undefined;
  const serviceTiming = subscription.expiresAt
    ? `${t("shareMarket.serviceDuration.started")}: ${formatDate(subscription.createdAt, locale)} · ${t("shareMarket.serviceDuration.expires")}: ${formatDate(subscription.expiresAt, locale)}`
    : `${t("shareMarket.serviceDuration.started")}: ${formatDate(subscription.createdAt, locale)} · ${t("shareMarket.serviceDuration.permanent")}`;
  const app = isCoreShareApp(subscription.appType) ? subscription.appType : null;
  const priceChange = subscription.priceChange;
  return (
    <section className={`grid gap-3 rounded-md border bg-card p-4 shadow-sm sm:p-5 ${anomalous ? "border-rose-200 ring-1 ring-rose-100" : "border-border"}`}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {app ? <ShareAppLogo app={app} size={18} /> : null}
        <strong className="truncate text-sm">{subscription.shareName}</strong>
        <Chip size="sm" variant={anomalous ? "primary" : "tertiary"}>{statusKey ? t(statusKey) : subscription.status}</Chip>
        <Chip size="sm" variant="tertiary">{subscription.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}</Chip>
        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {perspective === "user" ? t("account.share.provider", { owner: subscription.ownerEmail }) : t("account.share.renter", { email: subscription.renterEmail || "-" })}
        </span>
      </div>
      <dl className="grid gap-3 text-sm sm:grid-cols-2">
        <div>
          <dt className="text-xs text-muted-foreground">{t("account.share.offer")}</dt>
          <dd className="mt-0.5 font-medium">{offerLabel(subscription, locale, t)}</dd>
          <dd className="mt-0.5 text-xs text-muted-foreground">{serviceTiming}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">{t("account.share.updated")}</dt>
          <dd className="mt-0.5 text-muted-foreground">{formatDate(subscription.updatedAt, locale)}</dd>
        </div>
      </dl>
      {priceChange ? (
        <div className="grid gap-2 border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-sm text-amber-950">
          <strong>{t(`shareMarket.priceChange.status.${priceChange.status}`)}</strong>
          <span>{t("shareMarket.priceChange.summary", {
            previous: formatUsdMoney(priceChange.previousDailyRateMinor, locale),
            proposed: formatUsdMoney(priceChange.proposedDailyRateMinor, locale),
          })}</span>
          {perspective === "user" && priceChange.status === "pending" ? (
            <div className="flex gap-2">
              <Button size="sm" variant="primary" isDisabled={busy} onClick={onAcceptPrice}><Check className="h-4 w-4" />{t("shareMarket.priceChange.accept")}</Button>
              <Button size="sm" variant="outline" isDisabled={busy} onClick={onRejectPrice}><X className="h-4 w-4" />{t("shareMarket.priceChange.reject")}</Button>
            </div>
          ) : null}
        </div>
      ) : null}
      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <p className="text-xs text-muted-foreground">
          {perspective === "user" ? t("account.share.manageHint") : t("account.share.providerManageHint")}
        </p>
        <div className="flex flex-wrap gap-2">
          {subscription.dailyRateMinor != null ? (
            <Link href={DASHBOARD_ACCOUNT_BILLING_PATH} className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted">
              {t("marketBilling.open")}<ArrowUpRight className="h-3.5 w-3.5" />
            </Link>
          ) : null}
          {openUrl ? (
            <a href={openUrl} target="_blank" rel="noopener noreferrer" className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted">
              {t("account.share.openShare")}<ExternalLink className="h-3.5 w-3.5" />
            </a>
          ) : null}
          {perspective === "user" && subscription.canRelease ? (
            <Button size="sm" variant="outline" isDisabled={busy} onClick={onRelease}><RotateCcw className="h-4 w-4" />{t("shareMarket.release")}</Button>
          ) : null}
          {manageHref ? (
            <Link href={manageHref} className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-muted">
              {t("account.share.manageInMarket")}<ArrowUpRight className="h-3.5 w-3.5" />
            </Link>
          ) : null}
        </div>
      </div>
    </section>
  );
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
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const router = useRouter();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const tab: ShareMonitorTab = searchParams.get("tab") === "provider" ? "provider" : "user";
  const [subscriptions, setSubscriptions] = React.useState<ShareMarketSubscription[]>([]);
  const [ownedListings, setOwnedListings] = React.useState<ShareMarketListing[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [busyId, setBusyId] = React.useState("");
  const [action, setAction] = React.useState<PendingAction | null>(null);
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
    if (!authed) return;
    const timer = window.setInterval(
      () => void load({ silent: true, skipIfBusy: true }),
      CLIENT_MARKET_POLL_MS,
    );
    return () => window.clearInterval(timer);
  }, [authed, load]);

  const run = async (id: string, operation: () => Promise<unknown>) => {
    setBusyId(id);
    setError("");
    try { await operation(); setAction(null); await load({ silent: true }); }
    catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setBusyId(""); }
  };
  const userSubscriptions = React.useMemo(() => [...subscriptions].sort(sortSubscriptions), [subscriptions]);
  const providerSubscriptions = React.useMemo(() => ownedListings.flatMap((listing) => listing.seats.flatMap((seat) => seat.subscription ? [seat.subscription] : [])).sort(sortSubscriptions), [ownedListings]);

  if (!authed) return <p className="py-6 text-sm text-muted-foreground">{t("shareMarket.loginRequired")}</p>;
  if (loading) return <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  return (
    <div className="grid min-w-0 gap-4">
      <div><h2 className="text-base font-semibold text-foreground">{t("account.nav.share")}</h2><p className="mt-0.5 text-sm text-muted-foreground">{t("account.shareHint")}</p></div>
      <SegmentedControl value={tab} onChange={setTab} ariaLabel={t("account.nav.share")} size="md" className="w-full max-w-sm" fullWidth items={[{ id: "user", label: t("account.share.tab.user") }, { id: "provider", label: t("account.share.tab.provider") }]} />
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}
      {tab === "user" ? (
        userSubscriptions.length ? <div className="grid gap-3">{userSubscriptions.map((subscription) => (
          <SubscriptionCard
            key={subscription.id}
            subscription={subscription}
            perspective="user"
            busy={busyId === subscription.id}
            onRelease={() => setAction({ title: t("shareMarket.confirm.releaseTitle"), description: t("shareMarket.confirm.releaseDescription", { share: subscription.shareName }), label: t("shareMarket.release"), run: () => releaseShareMarketSubscription(subscription.id) })}
            onAcceptPrice={() => subscription.priceChange && setAction({ title: t("shareMarket.priceChange.acceptTitle"), description: t("shareMarket.priceChange.acceptDescription", { previous: formatUsdMoney(subscription.priceChange.previousDailyRateMinor, locale), proposed: formatUsdMoney(subscription.priceChange.proposedDailyRateMinor, locale) }), label: t("shareMarket.priceChange.accept"), run: () => acceptShareMarketPriceChange(subscription.priceChange!.id) })}
            onRejectPrice={() => subscription.priceChange && void run(subscription.id, () => rejectShareMarketPriceChange(subscription.priceChange!.id))}
          />
        ))}</div> : <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-12 text-center text-sm text-muted-foreground"><span>{t("account.share.userEmpty")}</span><Link href={shareMarketHref()} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link></div>
      ) : ownedListings.length || providerSubscriptions.length ? (
        <div className="grid gap-3">{ownedListings.map((listing) => <ListingSummaryCard key={listing.id} listing={listing} />)}{providerSubscriptions.map((subscription) => <SubscriptionCard key={subscription.id} subscription={subscription} perspective="provider" busy={false} onRelease={() => {}} onAcceptPrice={() => {}} onRejectPrice={() => {}} />)}</div>
      ) : <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-border px-4 py-12 text-center text-sm text-muted-foreground"><span>{t("account.share.providerEmpty")}</span><Link href={shareMarketHref()} className="inline-flex items-center gap-1 font-medium text-accent hover:underline">{t("account.share.openMarket")}<ArrowUpRight className="h-3.5 w-3.5" /></Link></div>}
      <ConfirmAlertDialog open={!!action} title={action?.title || ""} description={action?.description || ""} confirmLabel={action?.label || ""} cancelLabel={t("common.cancel")} tone="warning" busy={!!busyId} onConfirm={() => action && void run("confirm", action.run)} onOpenChange={(open) => !open && !busyId && setAction(null)} />
    </div>
  );
}
