"use client";

import * as React from "react";
import { Button, Drawer, Modal } from "@heroui/react";
import {
  Check,
  ChevronDown,
  Clock3,
  Info,
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
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { ProviderContactsList } from "@/components/common/provider-contacts";
import {
  MarketAccessDialog,
  marketEligibilityFromError,
} from "@/components/common/seller-approval-dialog";
import { ShareProviderStatusPanel } from "@/components/dashboard/share-provider-status-panel";
import {
  MarketFundingDecisionCard,
  MarketRecurringFundingSummaryCard,
  MarketFundingSummaryCard,
  MarketFundingTopupDialog,
} from "@/components/dashboard/market-funding-topup-dialog";
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
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError, quoteShareMarketSeat, rentShareMarketSeat } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import {
  marketFundingConflictFromError,
  recurringFundingForRenewal,
  type MarketTopupFunding,
} from "@/lib/market-funding";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type {
  MarketEligibility,
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketProviderFamily,
  ShareMarketRentAppService,
  ShareMarketRentQuote,
  ShareMarketSeat,
  ShareMarketSubscription,
} from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";
import { cn } from "@/lib/utils";
import {
  PROVIDER_FAMILY_KEYS,
  activeSubscriptionForShare,
  formatSeatPrice,
  formatTokenLimit,
  isCoreShareApp,
  isSeatIdle,
  listingIdleCount,
  marketProviderStatusView,
  shareMarketMutationError,
} from "@/components/dashboard/share-market/market-utils";
import {
  canOptionallyTopupRent,
  catalogOwnerOptions,
  filterMergedCatalogListings,
  initialCatalogSeat,
  MARKET_CATALOG_PAGE_SIZE,
  mergeCatalogWithRentedListings,
  pageForShareId,
  paginateListings,
  preserveCatalogSeat,
  rentConfirmPrimaryAction,
  rentedShareIdsFromSubscriptions,
  sortCatalogListings,
  type MarketCatalogSort,
} from "@/components/dashboard/share-market/buyer-catalog-utils";
import { MarketListingFilters } from "@/components/dashboard/share-market/market-listing-filters";
import { MarketPagination } from "@/components/dashboard/share-market/market-pagination";
import {
  RentalActions,
  RentalTermLine,
  ShareMarketRentalHistory,
  useShareMarketRentalActions,
} from "@/components/dashboard/share-market/rental-controls";
import { needsRentalAttention, partitionShareMarketSubscriptions } from "@/components/dashboard/share-market/subscription-utils";

type SeatCard = { listing: ShareMarketListing; seat: ShareMarketSeat };
type SelectedListing = { listing: ShareMarketListing; seat?: ShareMarketSeat };
type SeatAction = "rent" | "approval" | "login" | "rented" | "granting" | "selling" | "unavailable";
type RentTarget = SeatCard & { quote: ShareMarketRentQuote; idempotencyKey: string };

function rentAppLabel(app: string) {
  return isCoreShareApp(app) ? SHARE_APP_LABELS[app] : app;
}

function rentAppModel(service: ShareMarketRentAppService, t: ReturnType<typeof useLocaleText>["t"]) {
  if (service.modelMode === "passthrough") return t("shareMarket.modelPassthrough");
  return service.upstreamModel || service.models?.join(" / ") || t("shareMarket.catalog.modelUnknown");
}

const RENT_BLOCK_REASON_KEYS: Record<string, MessageKey> = {
  owner: "shareMarket.blockReason.owner",
  already_renting: "shareMarket.blockReason.already_renting",
  direct_access: "shareMarket.blockReason.direct_access",
  login_required: "shareMarket.blockReason.login_required",
  share_unavailable: "shareMarket.blockReason.share_unavailable",
  share_offline: "shareMarket.blockReason.share_offline",
  seat_unavailable: "shareMarket.blockReason.seat_unavailable",
  approval_required: "shareMarket.blockReason.approval_required",
  access_required: "shareMarket.blockReason.access_required",
  credit_required: "shareMarket.blockReason.credit_required",
  buyer_restricted: "shareMarket.blockReason.buyer_restricted",
  settlement_required: "shareMarket.blockReason.settlement_required",
  credit_limit_reached: "shareMarket.blockReason.credit_limit_reached",
  relationship_closed: "shareMarket.blockReason.relationship_closed",
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
  onRelease?: () => void;
  onAcceptPrice?: () => void;
  onRejectPrice?: () => void;
};

function ListingCard({
  listing,
  focused,
  onOpen,
  onRent,
  subscription,
  busy,
  actions,
}: {
  listing: ShareMarketListing;
  focused: boolean;
  onOpen: (seat?: ShareMarketSeat) => void;
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
            onOpen={onOpen}
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
                onAcceptPrice={actions?.onAcceptPrice}
                onRejectPrice={actions?.onRejectPrice}
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
          {formatTokenLimit(seat, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`))}
        </span>
        <span className="mt-0.5 block text-slate-500">
          {seat.pricingModel === "prepaid_calendar_month"
            ? t("shareMarket.serviceDuration.monthlyManaged")
            : seat.serviceDurationDays == null
              ? t("shareMarket.serviceDuration.permanent")
              : t("shareMarket.serviceDuration.daysValue", { count: seat.serviceDurationDays })}
          {!seat.isFree && seat.trialHours != null ? ` · ${t("shareMarket.dialog.trialHours")} ${seat.trialHours}` : ""}
        </span>
      </span>
      <strong className="shrink-0 text-sm tabular-nums">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</strong>
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
  const [family, setFamily] = React.useState<ShareMarketProviderFamily | "all">("all");
  const [mine, setMine] = React.useState(initialMine);
  const [idleOnly, setIdleOnly] = React.useState(false);
  const [sort, setSort] = React.useState<MarketCatalogSort>("recommended");
  const [page, setPage] = React.useState(1);
  const [selected, setSelected] = React.useState<SelectedListing | null>(null);
  const [rentTarget, setRentTarget] = React.useState<RentTarget | null>(null);
  const [rentAutoRenew, setRentAutoRenew] = React.useState(false);
  const [topupFunding, setTopupFunding] = React.useState<MarketTopupFunding>();
  const [accessTarget, setAccessTarget] = React.useState<(SeatCard & { eligibility: MarketEligibility }) | null>(null);
  const [busySeatId, setBusySeatId] = React.useState("");
  const [error, setError] = React.useState("");
  const [rentNotice, setRentNotice] = React.useState("");
  const [rentQuoteInvalidated, setRentQuoteInvalidated] = React.useState(false);
  const [quoteNowMs, setQuoteNowMs] = React.useState(() => Date.now());
  const focusedRef = React.useRef("");
  const skipPageResetRef = React.useRef(false);
  const rentals = useShareMarketRentalActions(onChanged);
  const pausePolling = !!rentTarget || !!topupFunding || !!accessTarget || !!busySeatId || rentals.interactionActive;

  React.useEffect(() => {
    onInteractionChange?.(pausePolling);
    return () => onInteractionChange?.(false);
  }, [onInteractionChange, pausePolling]);

  React.useEffect(() => {
    if (!rentTarget) return;
    setQuoteNowMs(Date.now());
    const timer = window.setInterval(() => setQuoteNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [rentTarget]);

  const rentedShareIds = React.useMemo(
    () => rentedShareIdsFromSubscriptions(subscriptions),
    [subscriptions],
  );
  const mergedListings = React.useMemo(
    () => mergeCatalogWithRentedListings(catalog.listings, authed ? rentedListings : []),
    [authed, catalog.listings, rentedListings],
  );
  const filteredListings = React.useMemo(
    () => sortCatalogListings(
      filterMergedCatalogListings(mergedListings, {
        mine: authed && mine,
        idleOnly,
        family,
        query,
        owner: owners,
        rentedShareIds,
      }),
      subscriptions,
      sort,
    ),
    [authed, family, idleOnly, mergedListings, mine, owners, query, rentedShareIds, sort, subscriptions],
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
  }, [family, idleOnly, mine, owners, query, sort]);

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
    setSelected({ listing });
    window.requestAnimationFrame(() => document.getElementById(`share-market-catalog-${focusedShareId}`)?.scrollIntoView({ block: "start" }));
  }, [filteredListings, focusedShareId, mergedListings]);

  const openListing = (listing: ShareMarketListing, seat?: ShareMarketSeat) => {
    setError("");
    setRentNotice("");
    setRentQuoteInvalidated(false);
    setSelected({
      listing,
      seat: initialCatalogSeat(listing.seats, seat),
    });
  };

  const triggerSeat = async (item: SeatCard) => {
    const action = seatAction(item.listing, item.seat, subscriptions, authed);
    if (action === "login") return window.dispatchEvent(new Event("router-open-login"));
    if (action === "selling") return onSwitchSelling?.();
    if (action === "rented" || action === "granting") return;
    if (action === "approval") return setAccessTarget({ ...item, eligibility: item.seat.eligibility });
    if (action !== "rent" || busySeatId) return;
    setBusySeatId(item.seat.id);
    setError("");
    setRentNotice("");
    try {
      const quote = await quoteShareMarketSeat(item.seat.id);
      setRentQuoteInvalidated(false);
      setRentAutoRenew(false);
      setRentTarget({ ...item, quote, idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}` });
    } catch (reason) {
      const eligibility = marketEligibilityFromError(reason);
      if (eligibility) setAccessTarget({ ...item, eligibility });
      else setError(shareMarketMutationError(reason, t));
    } finally {
      setBusySeatId("");
    }
  };

  const refreshQuote = async () => {
    if (!rentTarget || busySeatId) return;
    setBusySeatId(rentTarget.seat.id);
    setError("");
    setRentNotice("");
    setRentQuoteInvalidated(true);
    try {
      const quote = await quoteShareMarketSeat(rentTarget.seat.id);
      setRentQuoteInvalidated(false);
      setRentTarget({
        listing: rentTarget.listing,
        seat: rentTarget.seat,
        quote,
        idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}`,
      });
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusySeatId("");
    }
  };

  const confirmRent = async () => {
    if (!rentTarget || busySeatId) return;
    if (rentQuoteInvalidated || Date.parse(rentTarget.quote.expiresAt) <= Date.now()) {
      await refreshQuote();
      return;
    }
    setBusySeatId(rentTarget.seat.id);
    setError("");
    setRentNotice("");
    try {
      await rentShareMarketSeat(
        rentTarget.seat.id,
        rentTarget.quote.id,
        rentTarget.idempotencyKey,
        rentAutoRenew,
      );
      setRentTarget(null);
      setRentNotice("");
      setRentQuoteInvalidated(false);
      setPage(1);
      await onChanged();
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 410) {
        setRentQuoteInvalidated(true);
        setError(t("shareMarket.rentConfirm.expired"));
      } else {
        const fundingConflict = marketFundingConflictFromError(reason);
        if (fundingConflict) {
          setRentQuoteInvalidated(true);
          try {
            const quote = await quoteShareMarketSeat(rentTarget.seat.id);
            setRentQuoteInvalidated(false);
            setRentTarget({
              listing: rentTarget.listing,
              seat: rentTarget.seat,
              quote,
              idempotencyKey: `share-rent:${quote.id}:${crypto.randomUUID()}`,
            });
            setRentNotice(t("shareMarket.rentConfirm.fundingChanged"));
            setError("");
          } catch (refreshReason) {
            setError(shareMarketMutationError(refreshReason, t));
          }
        } else {
          setError(shareMarketMutationError(reason, t));
        }
      }
    } finally {
      setBusySeatId("");
    }
  };

  const selectedAction = selected?.seat
    ? seatAction(selected.listing, selected.seat, subscriptions, authed)
    : "unavailable";
  const quoteRemainingSeconds = rentTarget
    ? Math.max(0, Math.ceil((Date.parse(rentTarget.quote.expiresAt) - quoteNowMs) / 1_000))
    : 0;
  const quoteExpired = !!rentTarget && quoteRemainingSeconds <= 0;
  const quoteRequiresRefresh = quoteExpired || rentQuoteInvalidated;
  const rentFunding = rentTarget?.quote.funding;
  const rentRecurringFunding = rentTarget?.quote.recurringFunding
    ? recurringFundingForRenewal(rentTarget.quote.recurringFunding, rentAutoRenew)
    : undefined;
  const effectiveRentFunding = rentRecurringFunding ?? rentFunding;
  const rentPrimaryAction = rentConfirmPrimaryAction(quoteRequiresRefresh, effectiveRentFunding);
  const optionalTopup = canOptionallyTopupRent(quoteRequiresRefresh, effectiveRentFunding);
  const quoteExpiringSoon = !quoteRequiresRefresh && quoteRemainingSeconds <= 30;
  const rentQuoteStatusDetails = !rentTarget
    ? ""
    : quoteExpired
      ? t("shareMarket.rentConfirm.expired")
      : rentQuoteInvalidated
        ? t("shareMarket.rentConfirm.refreshRequired")
        : t("shareMarket.rentConfirm.quoteExpiry", {
          time: new Intl.DateTimeFormat(locale, { timeStyle: "medium" }).format(new Date(rentTarget.quote.expiresAt)),
        });
  const rentOffer = rentTarget?.quote.offer;
  const rentIsFree = rentOffer?.pricingModel === "free"
    || (rentOffer?.dailyRateMinor == null && rentOffer?.cyclePriceMinor == null);
  const rentPrice = rentOffer
    ? formatSeatPrice(
      {
        isFree: rentIsFree,
        dailyRateMinor: rentOffer.dailyRateMinor,
        cyclePriceMinor: rentOffer.cyclePriceMinor,
      },
      locale,
      t("shareMarket.free"),
      t("marketBilling.day"),
    )
    : "";
  const rentTrialTokens = rentOffer?.trialTokenLimit != null && rentOffer.trialTokenLimit > 0
    ? t("shareMarket.rentConfirm.trialTokenCompact", {
      tokens: formatTokenMillions(rentOffer.trialTokenLimit, locale),
    })
    : "";
  const rentTrialSummary = !rentOffer
    ? ""
    : rentIsFree
      ? t("shareMarket.rentConfirm.freeBilling")
      : rentTarget.quote.trialSecondsRemaining > 0
        ? t("shareMarket.rentConfirm.remainingTrialCompact", {
          hours: (rentTarget.quote.trialSecondsRemaining / 3_600).toFixed(1),
          tokens: rentTrialTokens,
        })
        : t("shareMarket.rentConfirm.noTrialCompact");
  const rentQuotaSummary = rentOffer
    ? t("shareMarket.rentConfirm.quotaSummary", {
      parallel: rentTarget.quote.offer.parallelLimit == null
        ? t("common.unlimited")
        : rentTarget.quote.offer.parallelLimit,
      tokens: formatTokenLimit(
        rentOffer,
        locale,
        t("common.unlimited"),
        (period) => t(`shareMarket.period.${period}`),
      ),
    })
    : "";
  const rentTermSummary = !rentOffer
    ? ""
    : rentOffer.pricingModel === "prepaid_calendar_month"
      ? t("shareMarket.rentConfirm.termMonthlyCompact")
    : rentOffer.serviceDurationDays == null
      ? t("shareMarket.rentConfirm.termPermanentCompact")
      : t("shareMarket.rentConfirm.termFixedCompact", {
        days: rentOffer.serviceDurationDays,
      });
  const rentTrialHours = rentOffer?.trialHours ?? catalog.trialHours;
  const rentTrialTokenLimit = rentOffer?.trialTokenLimit ?? 0;
  const rentBillingDetails = !rentOffer
    ? ""
    : rentIsFree
      ? t("shareMarket.rentConfirm.freeBilling")
      : rentOffer.pricingModel === "prepaid_calendar_month"
        ? t("shareMarket.rentConfirm.prepaidMonthly", {
          hours: rentTrialHours,
          tokens: rentTrialTokenLimit > 0
            ? t("shareMarket.rentConfirm.trialTokens", {
              tokens: formatTokenMillions(rentTrialTokenLimit, locale),
            })
            : "",
        })
      : rentTrialHours > 0 || rentTrialTokenLimit > 0
        ? t("shareMarket.rentConfirm.postpaid", {
          hours: rentTrialHours,
          tokens: rentTrialTokenLimit > 0
            ? t("shareMarket.rentConfirm.trialTokens", {
              tokens: formatTokenMillions(rentTrialTokenLimit, locale),
            })
            : "",
        })
        : t("shareMarket.rentConfirm.noTrial");
  const closeRentDialog = () => {
    if (busySeatId) return;
    setRentTarget(null);
    setRentAutoRenew(false);
    setRentNotice("");
    setRentQuoteInvalidated(false);
    setError("");
  };

  const displayError = error || rentals.error;

  return (
    <div className="grid min-w-0 gap-4">
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
      {/*
        The two rules that apply to every seat in the market, stated once before anyone
        picks one. They used to appear only in the confirm dialog, which is exactly the
        moment a rider is deciding whether to trust the whole thing — the free window and
        the "you are not charged now" rule belong before that, not as a surprise inside it.
      */}
      <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-[11px] leading-5 text-slate-600">
        <Clock3 className="h-3.5 w-3.5 shrink-0 text-slate-400" aria-hidden />
        <strong className="font-semibold text-slate-800">{t("shareMarket.catalog.trial")}</strong>
        <span className="text-slate-300" aria-hidden>·</span>
        <span className="min-w-0">{t("shareMarket.catalog.postpaidHint")}</span>
        <span className="ml-auto shrink-0 tabular-nums text-slate-400">{t("shareMarket.catalog.listingCount", { count: filteredListings.length })}</span>
      </p>
      <div className={MARKET_SHARE_CARD_GRID_CLASS}>
        {paged.items.map((listing) => {
          const subscription = activeSubscriptionForShare(subscriptions, listing.shareId);
          return (
            <ListingCard
              key={listing.id}
              listing={listing}
              focused={focusedShareId === listing.shareId}
              onOpen={(seat) => openListing(listing, seat)}
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
        <ShareMarketRentalHistory
          subscriptions={history}
          listings={mergedListings}
          nextCursor={nextCursor}
          historyTotal={historyTotal}
          loadingMore={loadingMore}
          onLoadMore={onLoadMore}
        />
      ) : null}
      {rentals.dialog}

      <Drawer.Backdrop isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Drawer.Content placement="right">
          <Drawer.Dialog className={drawerDialogClassName}>
            <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" />
            <Drawer.Header>
              <div className="min-w-0 pr-10">
                <Drawer.Heading className="truncate text-base">{selected?.listing.shareName}</Drawer.Heading>
              </div>
            </Drawer.Header>
            <Drawer.Body className="overflow-y-auto pb-28">
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
                    <div className="flex min-w-0 items-center gap-2 text-sm"><UserRound className="h-4 w-4 text-slate-400" /><span className="min-w-0 break-all font-medium">{selected.listing.ownerEmail}</span><PaymentMethodIcons kinds={selected.listing.paymentMethodKinds} /></div>
                    <ProviderContactsList contacts={selected.listing.contacts} />
                    <Button variant="outline" onClick={() => void chat.openClientChat(selected.listing.installationId)}><MessageCircle className="h-4 w-4" />{t("marketApproval.openChat")}</Button>
                  </section>
                  {error ? <p className="text-sm text-rose-700">{error}</p> : null}
                </div>
              ) : null}
            </Drawer.Body>
            {selected ? (
              <div className="absolute inset-x-0 bottom-0 flex items-center justify-between gap-3 border-t border-slate-200 bg-white px-5 py-4">
                <div className="min-w-0">
                  <strong className="block truncate text-sm">{selected.seat ? `${t("shareMarket.catalog.seatPosition", { position: selected.seat.position })} · ${formatSeatPrice(selected.seat, locale, t("shareMarket.free"), t("marketBilling.day"))}` : t("shareMarket.catalog.selectSeatFirst")}</strong>
                  <span className="text-xs text-slate-500">{selected.seat?.rentBlockReason ? t(rentBlockReasonKey(selected.seat.rentBlockReason)) : t("shareMarket.catalog.frozenTermsHint")}</span>
                </div>
                <Button
                  variant="primary"
                  isDisabled={!selected.seat || selectedAction === "unavailable" || selectedAction === "rented" || selectedAction === "granting" || !!busySeatId}
                  onClick={() => selected.seat && void triggerSeat({ listing: selected.listing, seat: selected.seat })}
                >
                  {busySeatId || selectedAction === "granting" ? <Loader2 className="h-4 w-4 animate-spin" /> : selectedAction === "approval" ? <Send className="h-4 w-4" /> : selectedAction === "login" ? <LogIn className="h-4 w-4" /> : selectedAction === "rented" ? <Check className="h-4 w-4" /> : <ShoppingCart className="h-4 w-4" />}
                  {selected.seat ? actionLabel(selectedAction, t) : t("shareMarket.catalog.chooseSeat")}
                </Button>
              </div>
            ) : null}
          </Drawer.Dialog>
        </Drawer.Content>
      </Drawer.Backdrop>

      <Modal.Backdrop isOpen={!!rentTarget} onOpenChange={(open) => !open && closeRentDialog()}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light max-h-[calc(100dvh-1rem)] w-[min(560px,calc(100vw-1rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
            <Modal.Header>
              {rentTarget ? (
                <div className="flex min-w-0 flex-1 flex-wrap items-start justify-between gap-3 pr-8">
                  <div className="min-w-0 flex-1">
                    <Modal.Heading className="break-words">
                      {t("shareMarket.rentConfirm.titleSeat", {
                        position: rentTarget.quote.offer.seatPosition,
                      })}
                    </Modal.Heading>
                    <p className="mt-1 truncate text-xs text-slate-500" title={`${rentTarget.quote.offer.shareName} · ${rentTarget.quote.offer.ownerEmail}`}>
                      {rentTarget.quote.offer.shareName}
                      {rentTarget.quote.offer.ownerEmail.toLocaleLowerCase() !== rentTarget.quote.offer.shareName.toLocaleLowerCase()
                        ? ` · ${t("shareMarket.catalog.seller")} ${rentTarget.quote.offer.ownerEmail}`
                        : ""}
                    </p>
                  </div>
                  <span
                    className={cn(
                      "shrink-0 whitespace-nowrap rounded-full px-2 py-1 text-xs font-medium tabular-nums",
                      quoteRequiresRefresh
                        ? "bg-rose-100 text-rose-700"
                        : quoteExpiringSoon
                          ? "bg-amber-100 text-amber-800"
                          : "bg-slate-100 text-slate-600",
                    )}
                    role={quoteRequiresRefresh ? "alert" : undefined}
                    title={rentQuoteStatusDetails}
                  >
                    {quoteExpired
                      ? t("shareMarket.rentConfirm.expiredShort")
                      : rentQuoteInvalidated
                        ? t("shareMarket.rentConfirm.refreshRequiredShort")
                        : t("shareMarket.rentConfirm.expiresInShort", { seconds: quoteRemainingSeconds })}
                  </span>
                </div>
              ) : <Modal.Heading>{t("shareMarket.rentConfirm.title")}</Modal.Heading>}
            </Modal.Header>
            <Modal.Body className="grid max-h-[min(72dvh,680px)] gap-4 overflow-y-auto">
              {rentTarget ? (
                <>
                  <section className="grid gap-4 border-y border-slate-200 py-4">
                    <div className="grid min-w-0 gap-2 sm:grid-cols-[minmax(0,auto)_minmax(0,1fr)] sm:items-end sm:gap-6">
                      <div className="min-w-0">
                        <span className="block text-xs text-slate-500">{t("shareMarket.rentConfirm.priceLabel")}</span>
                        <strong className="mt-1 block break-words text-2xl font-semibold tracking-tight text-slate-950 tabular-nums">{rentPrice}</strong>
                      </div>
                      <p className="min-w-0 text-sm leading-6 text-slate-600 sm:text-right">{rentTrialSummary}</p>
                    </div>
                    <dl className="grid min-w-0 gap-4 text-sm sm:grid-cols-2">
                      <div className="min-w-0 sm:col-span-2">
                        <dt className="text-xs text-slate-500">{t("shareMarket.catalog.enabledApps")}</dt>
                        <dd className="mt-1 grid gap-1">
                          {rentTarget.quote.offer.service.apps.map((service) => (
                            <div key={service.app} className="min-w-0 break-words">
                              <span className="font-medium text-slate-900">{rentAppLabel(service.app)}</span>
                              <span className="text-slate-500"> · {rentAppModel(service, t)}</span>
                            </div>
                          ))}
                        </dd>
                      </div>
                      <div className="min-w-0">
                        <dt className="text-xs text-slate-500">{t("shareMarket.rentConfirm.usageQuota")}</dt>
                        <dd className="mt-1 break-words font-medium text-slate-900">{rentQuotaSummary}</dd>
                      </div>
                      <div className="min-w-0">
                        <dt className="text-xs text-slate-500">{t("shareMarket.catalog.serviceTerm")}</dt>
                        <dd className="mt-1 break-words font-medium text-slate-900">{rentTermSummary}</dd>
                      </div>
                    </dl>
                  </section>

                  {rentRecurringFunding ? (
                    <section className="grid gap-3">
                      <label className="flex cursor-pointer items-start gap-3 rounded-md border border-slate-200 bg-white p-3 text-sm">
                        <input
                          type="checkbox"
                          className="mt-1 h-4 w-4 accent-emerald-600"
                          checked={rentAutoRenew}
                          disabled={!!busySeatId || quoteRequiresRefresh}
                          onChange={(event) => setRentAutoRenew(event.target.checked)}
                        />
                        <span className="min-w-0">
                          <strong className="block text-slate-900">{t("marketRecurring.autoRenew")}</strong>
                          <span className="mt-0.5 block text-xs leading-5 text-slate-600">
                            {t("marketRecurring.autoRenewAtCheckout", {
                              amount: formatSeatPrice(
                                {
                                  isFree: false,
                                  dailyRateMinor: rentOffer?.dailyRateMinor,
                                  cyclePriceMinor: rentOffer?.cyclePriceMinor,
                                },
                                locale,
                                t("shareMarket.free"),
                                t("marketBilling.day"),
                              ),
                            })}
                          </span>
                        </span>
                      </label>
                      <MarketRecurringFundingSummaryCard funding={rentRecurringFunding} />
                      {optionalTopup ? (
                        <Button
                          size="sm"
                          variant="outline"
                          className="justify-self-start"
                          isDisabled={!!busySeatId}
                          onClick={() => setTopupFunding(rentRecurringFunding)}
                        >
                          {t("marketFunding.topup.optionalAction")}
                        </Button>
                      ) : null}
                    </section>
                  ) : rentFunding ? (
                    <MarketFundingDecisionCard
                      funding={rentFunding}
                      topupDisabled={!!busySeatId}
                      onTopup={optionalTopup ? () => setTopupFunding(rentFunding) : undefined}
                    />
                  ) : null}

                  {rentNotice ? <p role="status" className="text-sm leading-6 text-amber-800">{rentNotice}</p> : null}
                  {error ? <p role="alert" className="text-sm leading-6 text-rose-700">{error}</p> : null}

                  <details className="group border-t border-slate-200">
                    <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-3 rounded-sm py-2 text-sm font-medium text-slate-700 hover:text-slate-950 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary active:text-slate-950 [&::-webkit-details-marker]:hidden">
                      <span className="flex min-w-0 items-center gap-2">
                        <Info className="h-4 w-4 shrink-0 text-slate-400" aria-hidden="true" />
                        <span className="whitespace-nowrap">{t("shareMarket.rentConfirm.details")}</span>
                      </span>
                      <ChevronDown className="h-4 w-4 shrink-0 text-slate-400 group-open:rotate-180" aria-hidden="true" />
                    </summary>
                    <div className="grid gap-5 pb-1 pt-3">
                      <section className="grid gap-2">
                        <h3 className="text-sm font-semibold text-slate-900">{t("shareMarket.rentConfirm.billingDetails")}</h3>
                        <div className="grid gap-2 text-xs leading-5 text-slate-600">
                          <p>{rentBillingDetails}</p>
                          <p>
                            {rentTarget.quote.offer.pricingModel === "prepaid_calendar_month"
                              ? t("shareMarket.rentConfirm.serviceMonthly")
                              : rentTarget.quote.offer.serviceDurationDays == null
                              ? t("shareMarket.rentConfirm.servicePermanent")
                              : t("shareMarket.rentConfirm.serviceFixed", {
                                days: rentTarget.quote.offer.serviceDurationDays,
                              })}
                          </p>
                        </div>
                      </section>

                      {rentRecurringFunding
                        ? <MarketRecurringFundingSummaryCard funding={rentRecurringFunding} />
                        : rentFunding
                          ? <MarketFundingSummaryCard funding={rentFunding} />
                          : null}

                      <section className="grid gap-2">
                        <h3 className="text-sm font-semibold text-slate-900">{t("shareMarket.rentConfirm.technicalDetails")}</h3>
                        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-3 text-xs leading-5">
                          <dt className="text-slate-500">{t("shareMarket.catalog.enabledApps")}</dt>
                          <dd className="grid min-w-0 gap-2">
                            {rentTarget.quote.offer.service.apps.map((service) => (
                              <div key={service.app} className="grid min-w-0 gap-0.5">
                                <strong className="font-medium text-slate-800">{rentAppLabel(service.app)}</strong>
                                <span className="break-words text-slate-600">{t(PROVIDER_FAMILY_KEYS[service.providerFamily])}{service.providerType ? ` · ${service.providerType}` : ""}</span>
                                <span className="break-words text-slate-500">{rentAppModel(service, t)}</span>
                              </div>
                            ))}
                          </dd>
                          <dt className="text-slate-500">{t("shareMarket.catalog.shareCapacity")}</dt>
                          <dd>{rentTarget.quote.offer.service.shareParallelLimit == null ? t("common.unlimited") : t("shareMarket.parallelShort", { value: rentTarget.quote.offer.service.shareParallelLimit })}</dd>
                          <dt className="text-slate-500">{t("shareMarket.catalog.shareTokens")}</dt>
                          <dd>{rentTarget.quote.offer.service.shareTokenLimit == null ? t("common.unlimited") : `${formatTokenMillions(rentTarget.quote.offer.service.shareTokensUsed, locale)} / ${formatTokenMillions(rentTarget.quote.offer.service.shareTokenLimit, locale)}`}</dd>
                        </dl>
                      </section>

                      <p className="text-xs leading-5 text-slate-500">
                        {rentQuoteStatusDetails}
                      </p>
                    </div>
                  </details>
                </>
              ) : null}
            </Modal.Body>
            <Modal.Footer className="flex-wrap">
              <Button className="min-h-11 whitespace-nowrap" variant="ghost" isDisabled={!!busySeatId} onClick={closeRentDialog}>{t("common.cancel")}</Button>
              {rentPrimaryAction === "refresh" ? (
                <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={!!busySeatId} onClick={() => void refreshQuote()}>
                  {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
                  {t("shareMarket.rentConfirm.requote")}
                </Button>
              ) : null}
              {rentPrimaryAction === "topup" && effectiveRentFunding ? (
                <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={!!busySeatId} onClick={() => setTopupFunding(effectiveRentFunding)}>
                  {t("marketFunding.topup.requiredAction")}
                </Button>
              ) : null}
              {rentPrimaryAction === "confirm" ? (
                <Button className="min-h-11 whitespace-nowrap" variant="primary" isDisabled={!!busySeatId} onClick={() => void confirmRent()}>
                  {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShoppingCart className="h-4 w-4" />}
                  {rentIsFree
                    ? t("shareMarket.rentConfirm.confirmFree")
                    : t("shareMarket.rentConfirm.confirm")}
                </Button>
              ) : null}
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <MarketFundingTopupDialog
        open={!!topupFunding}
        funding={topupFunding}
        onClose={() => setTopupFunding(undefined)}
        onCredited={async () => {
          setTopupFunding(undefined);
          await refreshQuote();
        }}
      />

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
