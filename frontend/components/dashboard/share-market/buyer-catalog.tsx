"use client";

/* Hallmark · component: Share rental confirmation · genre: modern-minimal · theme: existing router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: pass (40–41) · pre-emit critique: P5 H5 E5 S5 R5 V4
 */

import * as React from "react";
import { Button, Drawer } from "@heroui/react";
import {
  Check,
  Loader2,
  LogIn,
  MessageCircle,
  RefreshCw,
  Send,
  ShoppingCart,
  UserRound,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientChat } from "@/components/chat/client-chat";
import { ProviderContactsList } from "@/components/common/provider-contacts";
import {
  MarketAccessDialog,
} from "@/components/common/seller-approval-dialog";
import { ShareProviderStatusPanel } from "@/components/dashboard/share-provider-status-panel";
import {
  CatalogSeatPreviewList,
  MARKET_SHARE_CARD_GRID_CLASS,
  MarketShareCard,
  MarketShareCardMetric,
  listingCardId,
  listingUptimeValue,
} from "@/components/dashboard/share-market/market-share-card";
import { ShareModelHealthHeatmap } from "@/components/dashboard/share-model-health-heatmap";
import { ListingPricingCard } from "@/components/dashboard/share-market/listing-pricing-card";
import { CatalogSeatDialog } from "@/components/dashboard/share-market/catalog-seat-dialog";
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";
import type {
  MarketEligibility,
  ShareMarketCatalog,
  ShareMarketListing,
  ShareProviderCategory,
  ShareMarketSeat,
  ShareMarketSubscription,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  activeSubscriptionForShare,
  formatSeatPrice,
  formatSeatQuota,
  isSeatIdle,
  listingIdleCount,
  marketProviderStatusView,
} from "@/components/dashboard/share-market/market-utils";
import {
  catalogOwnerOptions,
  catalogSeatSubscription,
  catalogSeatTarget,
  filterMergedCatalogListings,
  initialCatalogSeat,
  MARKET_CATALOG_PAGE_SIZE,
  mergeCatalogWithRentedListings,
  pageForShareId,
  paginateListings,
  preserveCatalogSeat,
  rentedShareIdsFromSubscriptions,
  sortCatalogListings,
  type CatalogSeatSelection,
  type MarketCatalogSort,
} from "@/components/dashboard/share-market/buyer-catalog-utils";
import { SeatDiscountNote } from "@/components/dashboard/share-market/seat-value-quota";
import { MarketListingFilters } from "@/components/dashboard/share-market/market-listing-filters";
import { MarketPagination } from "@/components/dashboard/share-market/market-pagination";
import {
  RentalActions,
  RentalTermLine,
  useShareMarketRentalActions,
} from "@/components/dashboard/share-market/rental-controls";
import { useShareSeatCheckout } from "./share-seat-checkout";
import { AccountShareRentals } from "@/components/dashboard/account-share-rentals";
import { needsRentalAttention, partitionShareMarketSubscriptions } from "@/components/dashboard/share-market/subscription-utils";

type SeatCard = { listing: ShareMarketListing; seat: ShareMarketSeat };
type SelectedListing = { listing: ShareMarketListing; seat?: ShareMarketSeat };
type SeatAction = "rent" | "approval" | "login" | "rented" | "granting" | "selling" | "unavailable";

const RENT_BLOCK_REASON_KEYS: Record<string, MessageKey> = {
  owner: "shareMarket.blockReason.owner",
  already_renting: "shareMarket.blockReason.already_renting",
  direct_access: "shareMarket.blockReason.direct_access",
  login_required: "shareMarket.blockReason.login_required",
  share_unavailable: "shareMarket.blockReason.share_unavailable",
  share_offline: "shareMarket.blockReason.share_offline",
  runtime_syncing: "shareMarket.blockReason.runtime_syncing",
  share_expired: "shareMarket.blockReason.share_expired",
  share_upgrade_required: "shareMarket.blockReason.share_upgrade_required",
  share_tokens_exhausted: "shareMarket.blockReason.share_tokens_exhausted",
  model_unavailable: "shareMarket.blockReason.model_unavailable",
  app_unavailable: "shareMarket.blockReason.app_unavailable",
  provider_unavailable: "shareMarket.blockReason.provider_unavailable",
  service_unavailable: "shareMarket.blockReason.service_unavailable",
  seat_unavailable: "shareMarket.blockReason.seat_unavailable",
  approval_required: "shareMarket.blockReason.approval_required",
  access_required: "shareMarket.blockReason.access_required",
  unavailable: "shareMarket.blockReason.unavailable",
};

function rentBlockReasonKey(reason: string): MessageKey {
  return RENT_BLOCK_REASON_KEYS[reason] || "shareMarket.blockReason.unavailable";
}

function seatAction(
  listing: ShareMarketListing,
  seat: ShareMarketSeat,
  subscriptions: ShareMarketSubscription[],
  authed: boolean,
): SeatAction {
  if (listing.isOwner) return "selling";
  const mine = activeSubscriptionForShare(subscriptions, listing.shareId);
  if (mine) return mine.status === "grant_pending" ? "granting" : "rented";
  if (!authed) return "login";
  if (seat.canRent) return "rent";
  if (seat.rentPrerequisitesMet && !seat.eligibility.allowed) return "approval";
  return "unavailable";
}

type RentalCardActions = {
  onTerminate?: () => void;
  onViewTermination?: () => void;
  onRelease?: () => void;
  onAcceptPrice?: () => void;
  onRejectPrice?: () => void;
};

function ListingCard({
  listing,
  focused,
  onOpen,
  onOpenSeat,
  onRent,
  subscription,
  busy,
  actions,
}: {
  listing: ShareMarketListing;
  focused: boolean;
  onOpen: () => void;
  onOpenSeat: (seat: ShareMarketSeat) => void;
  onRent?: (seat: ShareMarketSeat) => void;
  subscription?: ShareMarketSubscription;
  busy?: boolean;
  actions?: RentalCardActions;
}) {
  const { t } = useLocaleText();
  const mineSeatIds = subscription ? [subscription.seatId] : [];
  const attention = subscription ? needsRentalAttention(subscription) : false;
  return (
    <MarketShareCard
      listing={listing}
      focused={focused}
      attention={attention}
      rented={!!subscription}
      cardId={listingCardId("catalog", listing.shareId)}
      onOpen={() => onOpen()}
      footer={(
        <div className="grid content-start">
          <CatalogSeatPreviewList
            listing={listing}
            seats={listing.seats}
            onOpen={(seat) => seat ? onOpenSeat(seat) : onOpen()}
            onRent={onRent}
            preferredSeatIds={mineSeatIds}
            mineSeatIds={mineSeatIds}
            attentionSeatIds={attention ? mineSeatIds : []}
          />
          {subscription ? (
            <RentalTermLine subscription={subscription} className="px-1.5 pt-1" />
          ) : null}
          {subscription ? (
            <div data-no-card-open className="px-1.5 pt-1">
              <RentalActions
                subscription={subscription}
                t={t}
                busy={busy}
                onRelease={actions?.onRelease}
                onTerminate={actions?.onTerminate}
                onViewTermination={actions?.onViewTermination}

              />
            </div>
          ) : null}
        </div>
      )}
    />
  );
}

function SeatChoice({ seat, selected, onSelect }: { seat: ShareMarketSeat; selected: boolean; onSelect: () => void }) {
  const { locale, t } = useLocaleText();
  const idle = isSeatIdle(seat);
  return (
    <button
      type="button"
      disabled={!idle}
      className={cn(
        "grid w-full grid-cols-[auto_minmax(0,1fr)_auto] gap-3 rounded-lg border p-3 text-left transition-colors active:bg-primary/10 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
        selected ? "border-primary bg-primary/5 ring-1 ring-primary/20" : "border-slate-200",
        idle ? "hover:border-primary/40" : "cursor-not-allowed bg-slate-50 opacity-60",
      )}
      onClick={onSelect}
    >
      <span className="grid h-7 w-7 place-items-center rounded-full bg-white text-xs font-bold text-slate-700 ring-1 ring-slate-200">{seat.position}</span>
      <span className="min-w-0 text-xs">
        <strong className={idle ? "text-emerald-700" : "text-slate-500"}>{idle ? t("shareMarket.available") : t("shareMarket.occupied")}</strong>
        {!idle && seat.subscription?.renterEmail ? (
          <span className="mt-0.5 block break-all text-slate-500" title={seat.subscription.renterEmail}>
            {seat.subscription.renterEmail}
          </span>
        ) : null}
        <span className="mt-1 block text-slate-600">
          {t("shareMarket.parallelShort", { value: seat.parallelLimit == null ? "∞" : seat.parallelLimit })}
          {" · "}
          {formatSeatQuota(seat, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`), (amount) => t("shareMarket.quota.usdAmount", { amount }))}
        </span>
        <SeatDiscountNote seat={seat} />
        <span className="mt-0.5 block text-slate-500">
          {seat.pricingModel === "prepaid_calendar_month"
            ? t("shareMarket.serviceDuration.monthlyManaged")
            : seat.serviceDurationDays == null
              ? t("shareMarket.serviceDuration.permanent")
              : t("shareMarket.serviceDuration.daysValue", { count: seat.serviceDurationDays })}
          {!seat.isFree && seat.trialHours != null ? ` · ${t("shareMarket.dialog.trialHours")} ${seat.trialHours}` : ""}
        </span>
      </span>
      <strong className="shrink-0 text-sm tabular-nums">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("market.day"))}</strong>
    </button>
  );
}

function actionLabel(action: SeatAction, t: ReturnType<typeof useLocaleText>["t"]) {
  if (action === "rent") return t("shareMarket.rentSelected");
  if (action === "approval") return t("marketApproval.apply");
  if (action === "login") return t("nav.login");
  if (action === "rented") return t("shareMarket.catalog.rented");
  if (action === "granting") return t("shareMarket.catalog.granting");
  if (action === "selling") return t("shareMarket.workspace.selling");
  return t("shareMarket.unavailable");
}

export function ShareMarketBuyerCatalog({
  catalog,
  subscriptions,
  rentedListings = [],
  authed,
  focusedShareId,
  marketProviderId,
  onClearMarketProvider,
  initialMine = false,
  onChanged,
  onInteractionChange,
  onSwitchSelling,
  nextCursor,
  historyTotal,
  loadingMore = false,
  onLoadMore,
}: {
  catalog: ShareMarketCatalog;
  subscriptions: ShareMarketSubscription[];
  rentedListings?: ShareMarketListing[];
  authed: boolean;
  focusedShareId?: string;
  marketProviderId?: string;
  onClearMarketProvider?: () => void;
  initialMine?: boolean;
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
  onSwitchSelling?: () => void;
  nextCursor?: string | null;
  historyTotal?: number;
  loadingMore?: boolean;
  onLoadMore?: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const chat = useClientChat();
  const [query, setQuery] = React.useState("");
  const [owners, setOwners] = React.useState<string[]>([]);
  const [family, setFamily] = React.useState<ShareProviderCategory | "all">("all");
  const [mine, setMine] = React.useState(initialMine);
  const [idleOnly, setIdleOnly] = React.useState(false);
  const [sort, setSort] = React.useState<MarketCatalogSort>("recommended");
  const [page, setPage] = React.useState(1);
  const [selected, setSelected] = React.useState<SelectedListing | null>(null);
  const [seatSelection, setSeatSelection] = React.useState<CatalogSeatSelection | null>(null);
  const [accessTarget, setAccessTarget] = React.useState<(SeatCard & { eligibility: MarketEligibility }) | null>(null);
  const focusedRef = React.useRef("");
  const skipPageResetRef = React.useRef(false);
  const rentals = useShareMarketRentalActions(onChanged);
  const checkout = useShareSeatCheckout({
    onChanged: async () => { setPage(1); await onChanged(); },
    onPresented: () => { setSelected(null); setSeatSelection(null); },
    onAccessRequired: (item, eligibility) => setAccessTarget({ ...item, eligibility }),
    restoreTarget: draft => {
      const listing = catalog.listings.find(l => l.shareId === draft.shareId);
      const seat = listing?.seats.find(s => s.id === draft.seatId);
      return listing && seat ? { listing, seat } : undefined;
    },
  });
  const { busySeatId, error, setError } = checkout;
  const pausePolling = !!seatSelection || checkout.interactionActive || !!accessTarget || !!busySeatId || rentals.interactionActive;

  React.useEffect(() => {
    onInteractionChange?.(pausePolling);
    return () => onInteractionChange?.(false);
  }, [onInteractionChange, pausePolling]);

  const rentedShareIds = React.useMemo(
    () => rentedShareIdsFromSubscriptions(subscriptions),
    [subscriptions],
  );
  const mergedListings = React.useMemo(
    () => mergeCatalogWithRentedListings(catalog.listings, authed ? rentedListings : []),
    [authed, catalog.listings, rentedListings],
  );
  const seatTarget = React.useMemo(
    () => catalogSeatTarget(mergedListings, seatSelection),
    [mergedListings, seatSelection],
  );
  React.useEffect(() => {
    if (seatSelection && !seatTarget) setSeatSelection(null);
  }, [seatSelection, seatTarget]);
  const filteredListings = React.useMemo(
    () => sortCatalogListings(
      filterMergedCatalogListings(mergedListings, {
        mine: authed && mine,
        idleOnly,
        family,
        query,
        owner: owners,
        marketProviderId,
        rentedShareIds,
      }),
      subscriptions,
      sort,
      catalog.recommendationMode,
    ),
    [authed, catalog.recommendationMode, family, idleOnly, marketProviderId, mergedListings, mine, owners, query, rentedShareIds, sort, subscriptions],
  );
  const paged = React.useMemo(
    () => paginateListings(filteredListings, page, MARKET_CATALOG_PAGE_SIZE),
    [filteredListings, page],
  );
  const { history } = React.useMemo(
    () => partitionShareMarketSubscriptions(subscriptions),
    [subscriptions],
  );

  React.useEffect(() => {
    setPage((current) => Math.min(current, paged.pageCount));
  }, [paged.pageCount]);

  React.useEffect(() => {
    if (skipPageResetRef.current) {
      skipPageResetRef.current = false;
      return;
    }
    setPage(1);
  }, [family, idleOnly, marketProviderId, mine, owners, query, sort]);

  React.useEffect(() => {
    if (!selected) return;
    const listing = mergedListings.find((item) => item.id === selected.listing.id)
      || mergedListings.find((item) => item.shareId === selected.listing.shareId);
    if (!listing) return setSelected(null);
    const seat = selected.seat ? preserveCatalogSeat(listing.seats, selected.seat.id) : undefined;
    if (listing !== selected.listing || seat !== selected.seat) {
      setSelected({ listing, seat });
    }
  }, [mergedListings, selected]);

  React.useEffect(() => {
    if (!focusedShareId || focusedRef.current === focusedShareId) return;
    const listing = mergedListings.find((item) => item.shareId === focusedShareId);
    if (!listing) return;
    focusedRef.current = focusedShareId;
    const nextPage = pageForShareId(filteredListings, focusedShareId, MARKET_CATALOG_PAGE_SIZE);
    skipPageResetRef.current = true;
    setPage(nextPage);
    setSeatSelection(null);
    setSelected({ listing });
    window.requestAnimationFrame(() => document.getElementById(`share-market-catalog-${focusedShareId}`)?.scrollIntoView({ block: "start" }));
  }, [filteredListings, focusedShareId, mergedListings]);

  const closeDetails = () => {
    setSelected(null);
    setSeatSelection(null);
  };

  const openListing = (listing: ShareMarketListing) => {
    setError("");
    setSeatSelection(null);
    setSelected({
      listing,
      seat: initialCatalogSeat(listing.seats),
    });
    void onChanged();
  };

  const openSeat = (listing: ShareMarketListing, seat: ShareMarketSeat) => {
    setError("");
    setSelected(null);
    setSeatSelection({ shareId: listing.shareId, seatId: seat.id });
    void onChanged();
  };

  const triggerSeat = async (item: SeatCard) => {
    const action = seatAction(item.listing, item.seat, subscriptions, authed);
    if (action === "login") {
      closeDetails();
      return window.dispatchEvent(new Event("router-open-login"));
    }
    if (action === "selling") {
      closeDetails();
      return onSwitchSelling?.();
    }
    if (action === "rented" || action === "granting") return;
    if (action === "approval") {
      closeDetails();
      return setAccessTarget({ ...item, eligibility: item.seat.eligibility });
    }
    if (action !== "rent" || busySeatId) return;
    await checkout.open(item);
  };

  const selectedSubscription = selected
    ? activeSubscriptionForShare(subscriptions, selected.listing.shareId)
    : undefined;
  const selectedRentalActions = selectedSubscription ? rentals.rowActions(selectedSubscription) : undefined;
  const runRentalDetailAction = (action?: () => void) => {
    if (!action) return;
    closeDetails();
    action();
  };
  const seatSubscription = catalogSeatSubscription(subscriptions, seatSelection);
  const seatRentalActions = seatSubscription ? rentals.rowActions(seatSubscription) : undefined;
  const detailSeatAction = seatTarget
    ? !seatTarget.listing.isOwner && !isSeatIdle(seatTarget.seat)
      ? "unavailable"
      : seatAction(seatTarget.listing, seatTarget.seat, subscriptions, authed)
    : "unavailable";
  const selectedAction = selected?.seat
    ? seatAction(selected.listing, selected.seat, subscriptions, authed)
    : "unavailable";
  const displayError = error || rentals.error;

  return (
    <div className="grid min-w-0 gap-4">
      {marketProviderId ? (
        <div className="flex min-h-11 min-w-0 flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-100 px-3 py-2 text-sm">
          <span className="min-w-0 text-slate-700">
            {t("providers.marketFilter.share")}
          </span>
          <Button
            size="sm"
            variant="ghost"
            className="min-h-11 shrink-0 whitespace-nowrap"
            onClick={onClearMarketProvider}
          >
            {t("providers.marketFilter.clear")}
          </Button>
        </div>
      ) : null}
      <MarketListingFilters
        listings={mergedListings}
        family={family}
        query={query}
        owners={owners}
        ownerOptions={catalogOwnerOptions(mergedListings, rentedShareIds)}
        onFamilyChange={setFamily}
        onQueryChange={setQuery}
        onOwnersChange={setOwners}
        mine={mine}
        mineCount={rentedShareIds.size}
        mineEnabled={authed && rentedShareIds.size > 0}
        onMineChange={authed ? setMine : undefined}
        idleOnly={idleOnly}
        onIdleOnlyChange={setIdleOnly}
        sort={sort}
        onSortChange={setSort}
      />

      {displayError ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{displayError}</p> : null}
      <p className="text-right text-[11px] tabular-nums text-slate-400">
        {t("shareMarket.catalog.listingCount", { count: filteredListings.length })}
      </p>
      <div className={MARKET_SHARE_CARD_GRID_CLASS}>
        {paged.items.map((listing) => {
          const subscription = activeSubscriptionForShare(subscriptions, listing.shareId);
          return (
            <ListingCard
              key={listing.id}
              listing={listing}
              focused={focusedShareId === listing.shareId}
              onOpen={() => openListing(listing)}
              onOpenSeat={(seat) => openSeat(listing, seat)}
              onRent={(seat) => void triggerSeat({ listing, seat })}
              subscription={subscription}
              busy={subscription ? rentals.busyId === subscription.id : false}
              actions={subscription ? rentals.rowActions(subscription) : undefined}
            />
          );
        })}
      </div>
      {!paged.items.length ? (
        <div className="grid min-h-48 place-items-center border-y border-dashed border-slate-200 text-sm text-slate-500">
          {mine ? t("shareMarket.catalog.mineEmpty") : t("shareMarket.catalog.empty")}
        </div>
      ) : null}
      <MarketPagination page={paged.page} pageCount={paged.pageCount} onPageChange={setPage} />
      {authed ? (
        <AccountShareRentals
          historyOnly
          historyPageSize={5}
          subscriptions={history}
          onChanged={onChanged}
          nextCursor={nextCursor}
          historyTotal={historyTotal}
          loadingMore={loadingMore}
          onLoadMore={onLoadMore}
        />
      ) : null}
      {rentals.dialog}

      <Drawer.Backdrop isOpen={!!selected && !checkout.interactionActive && !accessTarget} onOpenChange={(open) => !open && setSelected(null)}>
        <Drawer.Content placement="right">
          <Drawer.Dialog className={drawerDialogClassName}>
            <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" />
            <Drawer.Header>
              <div className="min-w-0 pr-10">
                <Drawer.Heading className="truncate text-base">{selected?.listing.shareName}</Drawer.Heading>
              </div>
            </Drawer.Header>
            <Drawer.Body className="min-h-0 overflow-y-auto">
              {selected ? (
                <div className="grid gap-5">
                  <section className="grid gap-3">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("dashboard.providers")}</h3>
                    <ShareProviderStatusPanel
                      view={marketProviderStatusView(selected.listing, locale, {
                        unknown: t("shareMarket.catalog.providerUnknown"),
                        passthrough: t("shareMarket.modelPassthrough"),
                      })}
                    />
                  </section>
                  <section className="grid gap-3">
                    <dl className="grid grid-cols-3 gap-3">
                      <MarketShareCardMetric label="TTFT" value={selected.listing.performance.averageTtftMs == null ? "-" : `${(selected.listing.performance.averageTtftMs / 1_000).toFixed(2)}s`} />
                      <MarketShareCardMetric label="TPS" value={selected.listing.performance.averageTps == null ? "-" : selected.listing.performance.averageTps.toFixed(1)} />
                      <MarketShareCardMetric label={t("shareMarket.catalog.uptime24h")} value={listingUptimeValue(selected.listing)} title={t("shareMarket.catalog.coverage24hValue", { value: selected.listing.reliability.observationCoverage24h.toFixed(1) })} />
                    </dl>
                    <p className={cn("text-xs", selected.listing.reliability.sufficientCoverage ? "text-slate-500" : "text-amber-700")}>
                      {selected.listing.reliability.sufficientCoverage
                        ? t("shareMarket.catalog.observedMinutesValue", { count: selected.listing.reliability.observedMinutes24h })
                        : t("shareMarket.catalog.coverageInsufficient", { count: selected.listing.reliability.observedMinutes24h })}
                    </p>
                  </section>
                  <ListingPricingCard listingId={selected.listing.id} />
                  <ShareModelHealthHeatmap shareId={selected.listing.shareId} />
                  <section className="grid gap-3">
                    <div className="flex items-center justify-between">
                      <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.chooseSeat")}</h3>
                      <span className="text-xs text-slate-400">{t("shareMarket.catalog.idleSeats", { count: listingIdleCount(selected.listing) })}</span>
                    </div>
                    {selected.listing.seats.map((seat) => <SeatChoice key={seat.id} seat={seat} selected={selected.seat?.id === seat.id} onSelect={() => setSelected({ ...selected, seat })} />)}
                  </section>
                  <section className="grid gap-3">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.seller")}</h3>
                    <div className="flex min-w-0 items-center gap-2 text-sm"><UserRound className="h-4 w-4 text-slate-400" /><span className="min-w-0 break-all font-medium">{selected.listing.ownerEmail}</span></div>
                    <ProviderContactsList contacts={selected.listing.contacts} />
                    <Button variant="outline" onClick={() => void chat.openClientChat(selected.listing.installationId)}><MessageCircle className="h-4 w-4" />{t("marketApproval.openChat")}</Button>
                  </section>
                  {error ? <p className="text-sm text-rose-700">{error}</p> : null}
                </div>
              ) : null}
            </Drawer.Body>
            {selected ? (
              <div className="grid shrink-0 gap-3 border-t border-slate-200 bg-white pt-4">
                <div className="min-w-0">
                  <strong className="block truncate text-sm">{selectedSubscription
                    ? `${t("shareMarket.catalog.seatPosition", { position: selectedSubscription.seatPosition })} · ${actionLabel("rented", t)}`
                    : selected.seat ? `${t("shareMarket.catalog.seatPosition", { position: selected.seat.position })} · ${formatSeatPrice(selected.seat, locale, t("shareMarket.free"), t("market.day"))}` : t("shareMarket.catalog.selectSeatFirst")}</strong>
                  {selectedSubscription ? <RentalTermLine subscription={selectedSubscription} />
                    : <span className="text-xs text-slate-500">{selected.seat?.rentBlockReason ? t(rentBlockReasonKey(selected.seat.rentBlockReason)) : t("shareMarket.catalog.frozenTermsHint")}</span>}
                </div>
                {selectedSubscription ? <RentalActions
                  subscription={selectedSubscription}
                  t={t}
                  busy={rentals.busyId === selectedSubscription.id}
                  onRelease={selectedRentalActions?.onRelease ? () => runRentalDetailAction(selectedRentalActions.onRelease) : undefined}
                  onTerminate={selectedRentalActions?.onTerminate ? () => runRentalDetailAction(selectedRentalActions.onTerminate) : undefined}
                  onViewTermination={selectedRentalActions?.onViewTermination ? () => runRentalDetailAction(selectedRentalActions.onViewTermination) : undefined}
                /> : <Button
                  variant="primary"
                  isDisabled={!selected.seat || selectedAction === "unavailable" || selectedAction === "rented" || selectedAction === "granting" || !!busySeatId}
                  onClick={() => selected.seat && void triggerSeat({ listing: selected.listing, seat: selected.seat })}
                >
                  {busySeatId || selectedAction === "granting" ? <Loader2 className="h-4 w-4 animate-spin" /> : selectedAction === "approval" ? <Send className="h-4 w-4" /> : selectedAction === "login" ? <LogIn className="h-4 w-4" /> : selectedAction === "rented" ? <Check className="h-4 w-4" /> : <ShoppingCart className="h-4 w-4" />}
                  {selected.seat ? actionLabel(selectedAction, t) : t("shareMarket.catalog.chooseSeat")}
                </Button>}
              </div>
            ) : null}
          </Drawer.Dialog>
        </Drawer.Content>
      </Drawer.Backdrop>

      <CatalogSeatDialog
        target={checkout.interactionActive || accessTarget ? null : seatTarget}
        subscription={seatSubscription}
        busy={!!busySeatId || !!seatSubscription && rentals.busyId === seatSubscription.id}
        error={error}
        blockedReason={!seatSubscription && seatTarget?.seat.rentBlockReason
          ? t(rentBlockReasonKey(seatTarget.seat.rentBlockReason)) : undefined}
        onClose={() => !busySeatId && setSeatSelection(null)}
        action={seatSubscription ? <RentalActions
          subscription={seatSubscription}
          t={t}
          busy={rentals.busyId === seatSubscription.id}
          onRelease={seatRentalActions?.onRelease ? () => runRentalDetailAction(seatRentalActions.onRelease) : undefined}
          onTerminate={seatRentalActions?.onTerminate ? () => runRentalDetailAction(seatRentalActions.onTerminate) : undefined}
          onViewTermination={seatRentalActions?.onViewTermination ? () => runRentalDetailAction(seatRentalActions.onViewTermination) : undefined}
        /> : <Button
          variant="primary"
          isDisabled={!seatTarget || detailSeatAction === "unavailable" || detailSeatAction === "rented" || detailSeatAction === "granting" || !!busySeatId}
          onClick={() => seatTarget && void triggerSeat(seatTarget)}
        >
          {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : detailSeatAction === "approval" ? <Send className="h-4 w-4" /> : detailSeatAction === "login" ? <LogIn className="h-4 w-4" /> : <ShoppingCart className="h-4 w-4" />}
          {actionLabel(detailSeatAction, t)}
        </Button>}
      />

      {checkout.dialog}

      <MarketAccessDialog
        open={!!accessTarget}
        product="share"
        ownerEmail={accessTarget?.listing.ownerEmail || ""}
        buyerEmail={session?.user?.email || ""}
        contacts={accessTarget?.listing.contacts}
        targetKind="share_seat"
        targetId={accessTarget?.seat.id || ""}
        currency={accessTarget?.seat.currency}
        eligibility={accessTarget?.eligibility || { allowed: false, status: "access_required" }}
        onOpenChange={(open) => !open && setAccessTarget(null)}
        onOpenChat={() => accessTarget ? chat.openClientChat(accessTarget.listing.installationId) : undefined}
        onRequestChanged={onChanged}
      />
    </div>
  );
}
