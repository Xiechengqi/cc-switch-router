"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import {
  Clock3,
  Copy,
  Loader2,
  MessageCircle,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Trash2,
  X,
} from "lucide-react";
import { useClientChat } from "@/components/chat/client-chat";
import { CompactSelect } from "@/components/common/compact-select";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { SegmentedControl } from "@/components/common/segmented-control";
import {
  PaidOfferReadinessNotice,
  usePaidOfferReadiness,
} from "@/components/dashboard/share-market/paid-offer-readiness";
import { expiryTitle } from "@/components/dashboard/share-dashboard-utils";
import { filterMarketListings } from "@/components/dashboard/share-market/buyer-catalog-utils";
import { MarketListingFilters } from "@/components/dashboard/share-market/market-listing-filters";
import { MarketShareIdentity } from "@/components/dashboard/share-market/market-share-identity";
import {
  CatalogSeatPreviewList,
  MARKET_SHARE_CARD_GRID_CLASS,
  MarketShareCard,
  listingCardId,
} from "@/components/dashboard/share-market/market-share-card";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  addShareMarketSeat,
  cancelShareMarketPriceChange,
  closeShareMarketListing,
  getShareUserLimitStatus,
  createShareMarketListing,
  deleteShareMarketListing,
  deleteShareMarketSeat,
  forceRevokeShareMarketSubscription,
  getShareMarketOwnedShares,
  proposeShareMarketPriceChange,
  quoteShareMarketSubscriptionTermination,
  reopenShareMarketListing,
  retryShareMarketSubscriptionGrant,
  terminateShareMarketSubscription,
  updateShareMarketSeat,
  ApiError,
} from "@/lib/api";
import { formatUsdMoney, MARKET_CURRENCY } from "@/lib/market-money";

import {
  formatTokenMillions,
  millionsInputToTokens,
  tokensToMillionsInput,
} from "@/lib/token-units";
import type {
  ShareMarketListing,
  ShareMarketOwnedShare,
  ShareMarketProviderFamily,
  ShareMarketSeat,
  ShareMarketSeatInput,
  ShareMarketSubscription,
  ShareMarketTerminationQuote,
  ShareTokenPeriod,
  ShareUserLimitStatusRow,
} from "@/lib/types";
import { cn, compactTokens } from "@/lib/utils";
import {
  formatSeatPrice,
  formatTokenLimit,
  grantFailureMessageKey,
  integrityReasonText,
  integrityStatusKey,
  isSeatIdle,
  refundStatusKey,
  shareMarketMutationError,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";
import {
  activeListingSeatCount,
  canCreateOwnedShareListing,
  isPriceOnlySeatAttention,
  listingClosedRentalSeats,
  listingLiveSeats,
  listingOccupancyCounts,
  needsOwnedSeatAttention,
  ownedShareBlockedReasonKey,
  partitionOwnedListings,
  reopenableListingSeats,
  reopenBlockedReasonKey,
} from "@/components/dashboard/share-market/owner-workspace-utils";

type TFn = ReturnType<typeof useLocaleText>["t"];
type SeatDraft = {
  parallelLimit: string;
  tokenLimit: string;
  tokenPeriod: ShareTokenPeriod;
  paid: boolean;
  price: string;
  serviceDurationMode: "fixed" | "permanent";
  serviceDurationDays: string;
  serviceDurationTouched: boolean;
};
type SeatDraftField = "parallelLimit" | "tokenLimit" | "price" | "serviceDurationDays";
type SeatDraftValidation = {
  message: string;
  field?: SeatDraftField;
};
type ConfirmAction = {
  title: string;
  description: string;
  label: string;
  tone: "warning" | "danger";
  run: () => Promise<unknown>;
};

const TOKEN_PERIODS: ShareTokenPeriod[] = [
  "lifetime",
  "day",
  "week",
  "sevenDays",
  "calendarMonth",
  "thirtyDays",
];
const MAX_DAILY_RATE_MINOR = 100_000_000;

class SeatDraftError extends Error {
  readonly field: SeatDraftField;

  constructor(field: SeatDraftField, message: string) {
    super(message);
    this.name = "SeatDraftError";
    this.field = field;
  }
}

function emptySeat(periods: ShareTokenPeriod[] = TOKEN_PERIODS): SeatDraft {
  return {
    parallelLimit: "",
    tokenLimit: "",
    tokenPeriod: periods.includes("lifetime") ? "lifetime" : periods[0] || "lifetime",
    paid: false,
    price: "",
    serviceDurationMode: "fixed",
    serviceDurationDays: "1",
    serviceDurationTouched: false,
  };
}

function seatDraft(seat: ShareMarketSeat): SeatDraft {
  return {
    parallelLimit: seat.parallelLimit == null ? "" : String(seat.parallelLimit),
    tokenLimit: seat.tokenLimit == null ? "" : tokensToMillionsInput(seat.tokenLimit),
    tokenPeriod: seat.tokenPeriod,
    paid: !seat.isFree,
    price: seat.dailyRateMinor == null ? "" : (seat.dailyRateMinor / 100).toFixed(2),
    serviceDurationMode: seat.serviceDurationDays == null ? "permanent" : "fixed",
    serviceDurationDays: String(seat.serviceDurationDays ?? 1),
    serviceDurationTouched: true,
  };
}

function positiveOptional(
  value: string,
  label: string,
  field: "parallelLimit" | "serviceDurationDays",
  t: TFn,
) {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1) {
    throw new SeatDraftError(
      field,
      t("shareMarket.error.positiveInteger", { field: label }),
    );
  }
  return parsed;
}

function positiveOptionalTokenMillions(value: string, label: string, t: TFn) {
  if (!value.trim()) return undefined;
  const parsed = millionsInputToTokens(value);
  if (parsed == null || parsed < 1) {
    throw new SeatDraftError(
      "tokenLimit",
      t("shareMarket.error.positiveMillions", { field: label }),
    );
  }
  return parsed;
}

function normalizedSeat(draft: SeatDraft, t: TFn): ShareMarketSeatInput {
  const parallelLimit = positiveOptional(
    draft.parallelLimit,
    t("shareMarket.parallel"),
    "parallelLimit",
    t,
  );
  const tokenLimit = positiveOptionalTokenMillions(draft.tokenLimit, t("shareMarket.tokens"), t);
  const serviceDurationDays = draft.serviceDurationMode === "permanent"
    ? undefined
    : positiveOptional(
        draft.serviceDurationDays,
        t("shareMarket.serviceDuration.days"),
        "serviceDurationDays",
        t,
      );
  if (serviceDurationDays != null && serviceDurationDays > 365) {
    throw new SeatDraftError(
      "serviceDurationDays",
      t("shareMarket.error.serviceDuration"),
    );
  }
  const base: ShareMarketSeatInput = {
    parallelLimit,
    tokenLimit,
    tokenPeriod: tokenLimit == null ? "lifetime" : draft.tokenPeriod,
    serviceDurationDays,
  };
  if (!draft.paid) return base;
  const amount = Number(draft.price);
  const dailyRateMinor = Math.round(amount * 100);
  if (!/^\d+(?:\.\d{1,2})?$/.test(draft.price.trim()) || amount <= 0 || !Number.isSafeInteger(dailyRateMinor)) {
    throw new SeatDraftError("price", t("shareMarket.error.price"));
  }
  if (dailyRateMinor > MAX_DAILY_RATE_MINOR) {
    throw new SeatDraftError("price", t("shareMarket.error.priceRange"));
  }
  return { ...base, dailyRateMinor, currency: MARKET_CURRENCY };
}

function seatDraftValidation(
  draft: SeatDraft,
  t: TFn,
  shareParallelLimit?: number,
): SeatDraftValidation {
  try {
    const seat = normalizedSeat(draft, t);
    if (
      shareParallelLimit != null &&
      shareParallelLimit >= 0 &&
      seat.parallelLimit != null &&
      seat.parallelLimit > shareParallelLimit
    ) {
      return {
        field: "parallelLimit",
        message: t("shareMarket.error.parallelExceedsShareValue", {
          limit: shareParallelLimit,
        }),
      };
    }
    return { message: "" };
  } catch (reason) {
    return {
      field: reason instanceof SeatDraftError ? reason.field : undefined,
      message: reason instanceof Error ? reason.message : String(reason),
    };
  }
}

function fieldClass(invalid = false) {
  return cn(
    "h-10 min-w-0 rounded-md border bg-white px-3 text-sm text-slate-900 outline-none focus:border-slate-400 disabled:cursor-not-allowed disabled:bg-slate-50 disabled:text-slate-500",
    invalid ? "border-rose-400" : "border-slate-200",
  );
}

function SeatFields({
  draft,
  supportedPeriods,
  disabled = false,
  validation = { message: "" },
  onChange,
}: {
  draft: SeatDraft;
  supportedPeriods?: ShareTokenPeriod[];
  disabled?: boolean;
  validation?: SeatDraftValidation;
  onChange: (draft: SeatDraft) => void;
}) {
  const { t } = useLocaleText();
  const errorId = React.useId();
  const periods = supportedPeriods?.length ? supportedPeriods : TOKEN_PERIODS;
  const patch = (value: Partial<SeatDraft>) => onChange({ ...draft, ...value });
  const invalid = (field: SeatDraftField) => validation.field === field;
  const describedBy = (field: SeatDraftField) => invalid(field) ? errorId : undefined;
  return (
    <div className="grid min-w-0 gap-3">
      <div className={cn("grid min-w-0 gap-3", draft.tokenLimit.trim() ? "sm:grid-cols-3" : "sm:grid-cols-2")}>
        <label className="grid gap-1 text-xs text-slate-500">
          {t("shareMarket.parallel")}
          <input className={fieldClass(invalid("parallelLimit"))} inputMode="numeric" disabled={disabled} aria-invalid={invalid("parallelLimit")} aria-describedby={describedBy("parallelLimit")} value={draft.parallelLimit} placeholder={t("shareMarket.dialog.unlimited")} onChange={(event) => patch({ parallelLimit: event.target.value })} />
        </label>
        <label className="grid gap-1 text-xs text-slate-500">
          {t("shareMarket.tokensMillions")}
          <input
            className={fieldClass(invalid("tokenLimit"))}
            inputMode="decimal"
            disabled={disabled}
            aria-invalid={invalid("tokenLimit")}
            aria-describedby={describedBy("tokenLimit")}
            value={draft.tokenLimit}
            placeholder={t("shareMarket.dialog.unlimited")}
            onChange={(event) => {
              const tokenLimit = event.target.value;
              patch({
                tokenLimit,
                tokenPeriod: tokenLimit.trim() && !periods.includes(draft.tokenPeriod)
                  ? periods[0] || "lifetime"
                  : draft.tokenPeriod,
              });
            }}
          />
        </label>
        {draft.tokenLimit.trim() ? (
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.tokenPeriod")}
            <CompactSelect
              value={draft.tokenPeriod}
              options={periods.map((period) => ({ value: period, label: t(`shareMarket.period.${period}`) }))}
              onChange={(value) => patch({ tokenPeriod: value as ShareTokenPeriod })}
              ariaLabel={t("shareMarket.tokenPeriod")}
              disabled={disabled}
              className="w-full"
              triggerClassName="h-10 w-full text-sm"
            />
          </label>
        ) : null}
      </div>
      <SegmentedControl
        value={draft.paid ? "paid" : "free"}
        onChange={(value) => {
          const paid = value === "paid";
          patch({
            paid,
            ...(!draft.serviceDurationTouched
              ? {
                  serviceDurationMode: paid ? "permanent" as const : "fixed" as const,
                  serviceDurationDays: "1",
                }
              : {}),
          });
        }}
        ariaLabel={t("shareMarket.dialog.amount")}
        size="md"
        fullWidth
        disabled={disabled}
        items={[
          { id: "free", label: t("shareMarket.dialog.freeMode") },
          { id: "paid", label: t("shareMarket.dialog.paidMode") },
        ]}
      />
      {draft.paid ? (
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.dialog.amount")}
            <input className={fieldClass(invalid("price"))} inputMode="decimal" disabled={disabled} aria-invalid={invalid("price")} aria-describedby={describedBy("price")} value={draft.price} onChange={(event) => patch({ price: event.target.value })} />
          </label>
          <div className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.dialog.currency")}
            <div className="flex h-10 items-center rounded-md border border-slate-200 bg-slate-50 px-3 text-sm font-medium">{MARKET_CURRENCY}</div>
          </div>
        </div>
      ) : null}
      <div className="grid gap-2">
        <span className="text-xs text-slate-500">{t("shareMarket.serviceDuration.label")}</span>
        <SegmentedControl
          value={draft.serviceDurationMode}
          onChange={(value) => patch({ serviceDurationMode: value, serviceDurationTouched: true })}
          ariaLabel={t("shareMarket.serviceDuration.label")}
          size="md"
          fullWidth
          disabled={disabled}
          items={[
            { id: "fixed", label: t("shareMarket.serviceDuration.fixed") },
            { id: "permanent", label: t("shareMarket.serviceDuration.permanent") },
          ]}
        />
        {draft.serviceDurationMode === "fixed" ? (
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.serviceDuration.days")}
            <input type="number" min={1} max={365} className={fieldClass(invalid("serviceDurationDays"))} disabled={disabled} aria-invalid={invalid("serviceDurationDays")} aria-describedby={describedBy("serviceDurationDays")} value={draft.serviceDurationDays} onChange={(event) => patch({ serviceDurationDays: event.target.value, serviceDurationTouched: true })} />
          </label>
        ) : null}
      </div>
      {validation.message ? <p id={errorId} role="alert" className="text-xs leading-5 text-rose-700">{validation.message}</p> : null}
    </div>
  );
}

function ShareCapacitySummary({
  parallelLimit,
  tokenLimit,
  expiresAt,
}: {
  parallelLimit?: number;
  tokenLimit?: number;
  expiresAt?: string;
}) {
  const { locale, t } = useLocaleText();
  return (
    <div className="flex min-w-0 flex-wrap gap-x-4 gap-y-1 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600">
      <strong className="text-slate-700">{t("shareMarket.dialog.shareCapacity")}</strong>
      <span>{t("shareMarket.dialog.capacityParallel", {
        value: parallelLimit == null ? t("common.unlimited") : parallelLimit,
      })}</span>
      <span>{t("shareMarket.dialog.capacityTokens", {
        value: tokenLimit == null ? t("common.unlimited") : formatTokenMillions(tokenLimit, locale),
      })}</span>
      {expiresAt ? (
        <span>{t("shareMarket.dialog.capacityExpires", {
          value: expiryTitle(expiresAt) === "∞" ? t("common.unlimited") : expiryTitle(expiresAt),
        })}</span>
      ) : null}
    </div>
  );
}

export function ShareMarketAddListingDialog({
  open,
  onOpenChange,
  onSaved,
  onReopenListing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
  onReopenListing?: (listingId: string) => void;
}) {
  const { t } = useLocaleText();
  const [shares, setShares] = React.useState<ShareMarketOwnedShare[]>([]);
  const [shareId, setShareId] = React.useState("");
  const [seats, setSeats] = React.useState<SeatDraft[]>([emptySeat()]);
  const [loading, setLoading] = React.useState(false);
  const [loadRevision, setLoadRevision] = React.useState(0);
  const [loadError, setLoadError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    let active = true;
    setShares([]);
    setShareId("");
    setSeats([emptySeat()]);
    setLoading(true);
    setLoadError("");
    setError("");
    getShareMarketOwnedShares(controller.signal)
      .then((items) => {
        if (!active) return;
        const eligible = items.filter(canCreateOwnedShareListing);
        setShares(items);
        setShareId(eligible[0]?.shareId || "");
        setSeats([emptySeat(eligible[0]?.supportedUserTokenPeriods)]);
      })
      .catch((reason) => {
        if (active && !controller.signal.aborted) {
          setLoadError(shareMarketMutationError(reason, t));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [loadRevision, open, t]);

  const eligibleShares = shares.filter(canCreateOwnedShareListing);
  const blockedShares = shares.filter((item) => !eligibleShares.includes(item));
  const selected = eligibleShares.find((share) => share.shareId === shareId);
  const seatErrors = seats.map((seat) => seatDraftValidation(seat, t, selected?.parallelLimit));
  const hasPaidSeat = seats.some((seat) => seat.paid);
  const paidReadiness = usePaidOfferReadiness(open && hasPaidSeat);
  const formInvalid = seatErrors.some((validation) => !!validation.message) ||
    (hasPaidSeat && paidReadiness.blocked);
  const save = async () => {
    if (!shareId || busy || formInvalid) return;
    setBusy(true);
    setError("");
    try {
      await createShareMarketListing(shareId, seats.map((seat) => normalizedSeat(seat, t)));
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(720px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("shareMarket.dialog.title")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[75vh] gap-4 overflow-y-auto">
            {loading ? <div className="flex items-center gap-2 py-6 text-sm text-slate-500"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div> : null}
            {!loading && !loadError && eligibleShares.length === 0 ? <p className="text-sm text-slate-500">{t("shareMarket.dialog.noShares")}</p> : null}
            {!loading && loadError ? (
              <div role="alert" className="flex flex-wrap items-center gap-3 rounded-md border border-rose-200 bg-rose-50 px-3 py-2.5 text-sm text-rose-800">
                <span className="min-w-[12rem] flex-1">{loadError}</span>
                <Button className="whitespace-nowrap" size="sm" variant="outline" onClick={() => setLoadRevision((current) => current + 1)}>
                  <RefreshCw className="h-4 w-4" />
                  {t("common.retry")}
                </Button>
              </div>
            ) : null}
            {!loading && eligibleShares.length ? (
              <>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t("shareMarket.dialog.selectShare")}
                  <CompactSelect
                    value={shareId}
                    options={eligibleShares.map((share) => ({
                      value: share.shareId,
                      label: share.subdomain || share.shareName,
                      content: <MarketShareIdentity source={share} />,
                    }))}
                    onChange={(value) => {
                      const next = eligibleShares.find((share) => share.shareId === value);
                      setShareId(value);
                      setSeats([emptySeat(next?.supportedUserTokenPeriods)]);
                    }}
                    ariaLabel={t("shareMarket.dialog.selectShare")}
                    className="w-full"
                    triggerClassName="min-h-9 w-full text-sm"
                  />
                </label>
                {selected ? <ShareCapacitySummary parallelLimit={selected.parallelLimit} tokenLimit={selected.tokenLimit} expiresAt={selected.expiresAt} /> : null}
                <div className="grid gap-4">
                  {seats.map((seat, index) => (
                    <section key={index} className="grid gap-3 border-t border-slate-200 pt-4 first:border-0 first:pt-0">
                      <div className="flex items-center justify-between gap-2">
                        <strong className="text-sm">{t("shareMarket.seat", { position: index + 1 })}</strong>
                        <div className="flex gap-1">
                          <Button isIconOnly size="sm" variant="ghost" aria-label={t("shareMarket.copySeat")} isDisabled={busy || seats.length >= 20} onClick={() => setSeats((items) => [...items.slice(0, index + 1), { ...items[index] }, ...items.slice(index + 1)])}><Copy className="h-4 w-4" /></Button>
                          {seats.length > 1 ? <Button isIconOnly size="sm" variant="ghost" aria-label={t("common.delete")} isDisabled={busy} onClick={() => setSeats((items) => items.filter((_, itemIndex) => itemIndex !== index))}><X className="h-4 w-4" /></Button> : null}
                        </div>
                      </div>
                      <SeatFields draft={seat} supportedPeriods={selected?.supportedUserTokenPeriods} disabled={busy} validation={seatErrors[index]} onChange={(next) => setSeats((items) => items.map((item, itemIndex) => itemIndex === index ? next : item))} />
                    </section>
                  ))}
                  {seats.length < 20 ? <Button variant="outline" isDisabled={busy} onClick={() => setSeats((items) => [...items, emptySeat(selected?.supportedUserTokenPeriods)])}><Plus className="h-4 w-4" />{t("shareMarket.dialog.addSeat")}</Button> : null}
                </div>
                {hasPaidSeat ? <PaidOfferReadinessNotice readiness={paidReadiness} /> : null}
              </>
            ) : null}
            {!loading && blockedShares.length ? (
              <section className="grid gap-2 border-t border-slate-200 pt-4">
                <strong className="text-xs font-semibold text-slate-700">{t("shareMarket.dialog.blockedTitle")}</strong>
                <div className="grid gap-2">
                  {blockedShares.map((share) => (
                    <div key={share.shareId} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-500">
                      <span className="min-w-0 flex-1">
                        <MarketShareIdentity source={share} />
                      </span>
                      <span>{t(ownedShareBlockedReasonKey(share.createBlockedReason))}</span>
                      {share.reopenListingId && onReopenListing ? (
                        <Button
                          size="sm"
                          variant="ghost"
                          className="whitespace-nowrap"
                          onClick={() => {
                            onOpenChange(false);
                            onReopenListing(share.reopenListingId!);
                          }}
                        >
                          <RotateCcw className="h-4 w-4" />
                          {t("shareMarket.reopen.action")}
                        </Button>
                      ) : null}
                    </div>
                  ))}
                </div>
              </section>
            ) : null}
            {error ? <p className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || loading || !shareId || formInvalid} onClick={() => void save()}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}{t("shareMarket.dialog.create")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

type ReopenExistingDraft = {
  seat: ShareMarketSeat;
  selected: boolean;
  draft: SeatDraft;
};

function ReopenListingDialog({
  listing,
  onOpenChange,
  onSaved,
}: {
  listing: ShareMarketListing | null;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { t } = useLocaleText();
  const [existing, setExisting] = React.useState<ReopenExistingDraft[]>([]);
  const [newSeats, setNewSeats] = React.useState<SeatDraft[]>([]);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (!listing) return;
    const reusable = reopenableListingSeats(listing);
    setExisting(reusable.map((seat) => ({ seat, selected: true, draft: seatDraft(seat) })));
    setNewSeats(reusable.length ? [] : [emptySeat(listing.supportedUserTokenPeriods)]);
    setBusy(false);
    setError("");
  }, [listing]);

  const selectedExisting = existing.filter((item) => item.selected);
  const activeSeatCount = listing ? activeListingSeatCount(listing) : 0;
  const requestedSeatCount = selectedExisting.length + newSeats.length;
  const existingErrors = existing.map((item) =>
    item.selected && listing
      ? seatDraftValidation(item.draft, t, listing.parallelLimit)
      : { message: "" },
  );
  const newSeatErrors = newSeats.map((seat) =>
    listing ? seatDraftValidation(seat, t, listing.parallelLimit) : { message: "" },
  );
  const hasPaidSeat = selectedExisting.some((item) => item.draft.paid)
    || newSeats.some((item) => item.paid);
  const paidReadiness = usePaidOfferReadiness(!!listing && hasPaidSeat);
  const atSeatLimit = activeSeatCount + requestedSeatCount >= 20;
  const formInvalid = requestedSeatCount === 0
    || !listing?.canReopen
    || activeSeatCount + requestedSeatCount > 20
    || existingErrors.some((validation) => !!validation.message)
    || newSeatErrors.some((validation) => !!validation.message)
    || (hasPaidSeat && paidReadiness.blocked);

  const save = async () => {
    if (!listing || busy || formInvalid) return;
    setBusy(true);
    setError("");
    try {
      await reopenShareMarketListing(listing.id, {
        existingSeats: selectedExisting.map((item) => ({
          seatId: item.seat.id,
          offerRevision: item.seat.offerRevision,
          seat: normalizedSeat(item.draft, t),
        })),
        newSeats: newSeats.map((seat) => normalizedSeat(seat, t)),
      });
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal.Backdrop isOpen={!!listing} onOpenChange={(next) => !next && !busy && onOpenChange(false)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(760px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("shareMarket.reopen.title")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[75vh] gap-4 overflow-y-auto">
            {listing ? (
              <>
                <div className="grid gap-1 text-sm text-slate-600">
                  <strong className="text-slate-900">{listing.shareName}</strong>
                  <span>{t("shareMarket.reopen.hint")}</span>
                </div>
                <ShareCapacitySummary parallelLimit={listing.parallelLimit} tokenLimit={listing.tokenLimit} />
                {!listing.canReopen ? (
                  <p role="alert" className="border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-xs font-medium text-amber-800">
                    {t(reopenBlockedReasonKey(listing.reopenBlockedReason))}
                  </p>
                ) : null}
                {activeSeatCount ? (
                  <p className="border-l-2 border-emerald-400 bg-emerald-50 px-3 py-2 text-xs text-emerald-800">
                    {t("shareMarket.reopen.activeSeatsHint", { count: activeSeatCount })}
                  </p>
                ) : null}

                {existing.length ? (
                  <section className="grid gap-3">
                    <div>
                      <strong className="text-sm text-slate-900">{t("shareMarket.reopen.existingSeats")}</strong>
                      <p className="mt-0.5 text-xs text-slate-500">{t("shareMarket.reopen.existingSeatsHint")}</p>
                    </div>
                    {existing.map((item, index) => (
                      <section key={item.seat.id} className="grid gap-3 border-t border-slate-200 pt-4">
                        <div className="flex min-w-0 items-center justify-between gap-3">
                          <label className="flex min-w-0 items-center gap-2 text-sm font-medium text-slate-900">
                            <input
                              type="checkbox"
                              className="h-4 w-4 accent-emerald-600"
                              checked={item.selected}
                              disabled={busy}
                              onChange={(event) => setExisting((items) => items.map((current, itemIndex) =>
                                itemIndex === index ? { ...current, selected: event.target.checked } : current
                              ))}
                            />
                            {t("shareMarket.seat", { position: item.seat.position })}
                          </label>
                          <Button
                            isIconOnly
                            size="sm"
                            variant="ghost"
                            aria-label={t("shareMarket.copySeat")}
                            isDisabled={busy || atSeatLimit}
                            onClick={() => setNewSeats((items) => [...items, { ...item.draft }])}
                          >
                            <Copy className="h-4 w-4" />
                          </Button>
                        </div>
                        <SeatFields
                          draft={item.draft}
                          supportedPeriods={listing.supportedUserTokenPeriods}
                          disabled={busy || !item.selected}
                          validation={existingErrors[index]}
                          onChange={(next) => setExisting((items) => items.map((current, itemIndex) =>
                            itemIndex === index ? { ...current, draft: next } : current
                          ))}
                        />
                      </section>
                    ))}
                  </section>
                ) : (
                  <p className="text-sm text-slate-500">{t("shareMarket.reopen.noExistingSeats")}</p>
                )}

                {newSeats.length ? (
                  <section className="grid gap-3">
                    <strong className="text-sm text-slate-900">{t("shareMarket.reopen.newSeats")}</strong>
                    {newSeats.map((seat, index) => (
                      <section key={index} className="grid gap-3 border-t border-slate-200 pt-4">
                        <div className="flex items-center justify-between gap-2">
                          <strong className="text-sm">{t("shareMarket.reopen.newSeat", { position: index + 1 })}</strong>
                          <div className="flex gap-1">
                            <Button isIconOnly size="sm" variant="ghost" aria-label={t("shareMarket.copySeat")} isDisabled={busy || atSeatLimit} onClick={() => setNewSeats((items) => [...items.slice(0, index + 1), { ...items[index] }, ...items.slice(index + 1)])}><Copy className="h-4 w-4" /></Button>
                            <Button isIconOnly size="sm" variant="ghost" aria-label={t("common.delete")} isDisabled={busy} onClick={() => setNewSeats((items) => items.filter((_, itemIndex) => itemIndex !== index))}><X className="h-4 w-4" /></Button>
                          </div>
                        </div>
                        <SeatFields draft={seat} supportedPeriods={listing.supportedUserTokenPeriods} disabled={busy} validation={newSeatErrors[index]} onChange={(next) => setNewSeats((items) => items.map((current, itemIndex) => itemIndex === index ? next : current))} />
                      </section>
                    ))}
                  </section>
                ) : null}
                {!atSeatLimit ? (
                  <Button variant="outline" isDisabled={busy} onClick={() => setNewSeats((items) => [...items, emptySeat(listing.supportedUserTokenPeriods)])}>
                    <Plus className="h-4 w-4" />{t("shareMarket.reopen.addNewSeat")}
                  </Button>
                ) : null}
                {requestedSeatCount === 0 ? <p role="alert" className="text-xs text-rose-700">{t("shareMarket.reopen.selectSeat")}</p> : null}
                {hasPaidSeat ? <PaidOfferReadinessNotice readiness={paidReadiness} /> : null}
              </>
            ) : null}
            {error ? <p role="alert" className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || !listing || formInvalid} onClick={() => void save()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
              {t("shareMarket.reopen.confirm")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function SeatDialog({
  listing,
  seat,
  template,
  onOpenChange,
  onSaved,
}: {
  listing: ShareMarketListing | null;
  seat?: ShareMarketSeat;
  template?: ShareMarketSeat;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { t } = useLocaleText();
  const [draft, setDraft] = React.useState<SeatDraft>(() =>
    seat
      ? seatDraft(seat)
      : template
        ? seatDraft(template)
        : listing
          ? emptySeat(listing.supportedUserTokenPeriods)
          : emptySeat(),
  );
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    if (!listing) return;
    setDraft(seat ? seatDraft(seat) : template ? seatDraft(template) : emptySeat(listing.supportedUserTokenPeriods));
    setError("");
  }, [listing, seat, template]);
  const validation = listing
    ? seatDraftValidation(draft, t, listing.parallelLimit)
    : { message: "" };
  const paidReadiness = usePaidOfferReadiness(!!listing && draft.paid);
  const save = async () => {
    if (!listing || busy || validation.message || (draft.paid && paidReadiness.blocked)) return;
    setBusy(true);
    setError("");
    try {
      const input = normalizedSeat(draft, t);
      if (seat) await updateShareMarketSeat(seat.id, input, seat.offerRevision);
      else await addShareMarketSeat(listing.id, input);
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal.Backdrop isOpen={!!listing} onOpenChange={(open) => !busy && onOpenChange(open)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{seat ? t("shareMarket.manage") : template ? t("shareMarket.createFromSeat") : t("shareMarket.addSeat")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[75vh] gap-4 overflow-y-auto">
            {listing ? <ShareCapacitySummary parallelLimit={listing.parallelLimit} tokenLimit={listing.tokenLimit} /> : null}
            <SeatFields draft={draft} supportedPeriods={listing?.supportedUserTokenPeriods} disabled={busy} validation={validation} onChange={setDraft} />
            {draft.paid ? <PaidOfferReadinessNotice readiness={paidReadiness} /> : null}
            {error ? <p className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || !!validation.message || (draft.paid && paidReadiness.blocked)} onClick={() => void save()}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("common.save")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function PriceDialog({ subscription, onOpenChange, onSaved }: { subscription: ShareMarketSubscription | null; onOpenChange: (open: boolean) => void; onSaved: () => void }) {
  const { locale, t } = useLocaleText();
  const [price, setPrice] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    if (!subscription) return;
    setPrice(subscription.dailyRateMinor == null ? "" : (subscription.dailyRateMinor / 100).toFixed(2));
    setError("");
  }, [subscription]);
  const save = async () => {
    if (!subscription || busy) return;
    const amount = Number(price);
    const dailyRateMinor = Math.round(amount * 100);
    if (!/^\d+(?:\.\d{1,2})?$/.test(price.trim()) || amount <= 0 || !Number.isSafeInteger(dailyRateMinor)) {
      setError(t("shareMarket.error.price"));
      return;
    }
    if (dailyRateMinor > MAX_DAILY_RATE_MINOR) {
      setError(t("shareMarket.error.priceRange"));
      return;
    }
    setBusy(true);
    try {
      await proposeShareMarketPriceChange(subscription.id, dailyRateMinor, subscription.offerRevision);
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal.Backdrop isOpen={!!subscription} onOpenChange={(open) => !busy && onOpenChange(open)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(480px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("shareMarket.priceChange.title")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid gap-3">
            {subscription ? <p className="text-sm text-slate-500">{t("shareMarket.priceChange.current", { amount: formatSeatPrice({ isFree: false, dailyRateMinor: subscription.dailyRateMinor }, locale, t("shareMarket.free"), t("marketBilling.day")) })}</p> : null}
            <label className="grid gap-1 text-xs text-slate-500">{t("shareMarket.priceChange.newDailyPrice")}<input className={fieldClass()} inputMode="decimal" value={price} onChange={(event) => setPrice(event.target.value)} /></label>
            <p className="text-xs leading-5 text-slate-500">{t("shareMarket.priceChange.consentNotice")}</p>
            {error ? <p className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy} onClick={() => void save()}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("shareMarket.priceChange.propose")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function TerminationDialog({
  target,
  onOpenChange,
  onSaved,
}: {
  target: { subscription: ShareMarketSubscription; denyFutureAccess: boolean } | null;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [quote, setQuote] = React.useState<ShareMarketTerminationQuote | null>(null);
  const [loadingQuote, setLoadingQuote] = React.useState(false);
  const [committing, setCommitting] = React.useState(false);
  const [error, setError] = React.useState("");
  const [quoteNowMs, setQuoteNowMs] = React.useState(() => Date.now());
  const idempotencyKey = React.useRef("");
  const quoteRequestRef = React.useRef<AbortController | null>(null);

  const requestQuote = React.useCallback(async (
    currentTarget: NonNullable<typeof target>,
  ) => {
    quoteRequestRef.current?.abort();
    const controller = new AbortController();
    quoteRequestRef.current = controller;
    setQuote(null);
    setError("");
    setLoadingQuote(true);
    idempotencyKey.current = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random()}`;
    try {
      const value = await quoteShareMarketSubscriptionTermination(
        currentTarget.subscription.id,
        controller.signal,
      );
      if (controller.signal.aborted || quoteRequestRef.current !== controller) return;
      setQuote(value);
      setQuoteNowMs(Date.now());
    } catch (reason) {
      if (controller.signal.aborted || quoteRequestRef.current !== controller) return;
      setError(shareMarketMutationError(reason, t));
    } finally {
      if (quoteRequestRef.current === controller) {
        quoteRequestRef.current = null;
        setLoadingQuote(false);
      }
    }
  }, [t]);

  React.useEffect(() => {
    if (!target) {
      quoteRequestRef.current?.abort();
      quoteRequestRef.current = null;
      setQuote(null);
      setError("");
      setLoadingQuote(false);
      return;
    }
    void requestQuote(target);
    return () => quoteRequestRef.current?.abort();
  }, [requestQuote, target]);

  React.useEffect(() => {
    if (!target || !quote) return;
    const timer = window.setInterval(() => setQuoteNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [quote, target]);

  const quoteRemainingSeconds = quote
    ? Math.max(0, Math.ceil((Date.parse(quote.expiresAt) - quoteNowMs) / 1_000))
    : 0;
  const quoteExpired = !!quote && quoteRemainingSeconds <= 0;

  const confirm = async () => {
    if (!target || !quote || committing || loadingQuote) return;
    if (quoteExpired) {
      await requestQuote(target);
      return;
    }
    setCommitting(true);
    setError("");
    try {
      await terminateShareMarketSubscription(
        target.subscription.id,
        quote.id,
        idempotencyKey.current,
        target.denyFutureAccess,
      );
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 410) {
        setQuoteNowMs(Date.parse(quote.expiresAt));
        setError(t("shareMarket.termination.expired"));
      } else {
        setError(shareMarketMutationError(reason, t));
      }
    } finally {
      setCommitting(false);
    }
  };
  const calculation = quote?.calculation;
  const refreshRequired = !quote || quoteExpired;
  return (
    <Modal.Backdrop isOpen={!!target} onOpenChange={(open) => !committing && onOpenChange(open)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(520px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("shareMarket.termination.title")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid gap-4">
            {calculation ? (
              <>
                <p className="text-sm leading-6 text-slate-600">{t("shareMarket.termination.description", { email: target?.subscription.renterEmail || "-" })}</p>
                <dl className="grid grid-cols-2 gap-x-4 gap-y-3 border-y border-slate-200 py-4 text-sm">
                  <div><dt className="text-xs text-slate-500">{t("shareMarket.termination.elapsed")}</dt><dd className="mt-1 font-medium">{(calculation.elapsedBps / 100).toFixed(2)}%</dd></div>
                  <div><dt className="text-xs text-slate-500">{t("shareMarket.termination.refundRate")}</dt><dd className="mt-1 font-medium">{(calculation.refundBps / 100).toFixed(2)}%</dd></div>
                  <div><dt className="text-xs text-slate-500">{t("shareMarket.termination.netBilled")}</dt><dd className="mt-1 font-medium">{formatUsdMoney(Math.ceil(calculation.refundableBaseUnits / 86_400), locale)}</dd></div>
                  <div><dt className="text-xs text-slate-500">{t("shareMarket.termination.refundAmount")}</dt><dd className="mt-1 font-semibold text-rose-700">{formatUsdMoney(calculation.amountMinor, locale)}</dd></div>
                </dl>
                <p className="text-xs leading-5 text-slate-500">{t("shareMarket.termination.policy")}</p>
                {target?.denyFutureAccess ? <p className="text-xs font-medium text-rose-700">{t("shareMarket.termination.denyNotice")}</p> : null}
                <p className={cn("flex items-center gap-1.5 text-xs", quoteExpired ? "text-rose-700" : "text-slate-500")}>
                  <Clock3 className="h-3.5 w-3.5 shrink-0" />
                  {quoteExpired
                    ? t("shareMarket.termination.expired")
                    : t("shareMarket.termination.expiresIn", { seconds: quoteRemainingSeconds })}
                </p>
              </>
            ) : loadingQuote ? <p className="inline-flex items-center gap-2 text-sm text-slate-500"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</p> : null}
            {error ? <p className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={committing} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button
              variant="danger"
              isDisabled={committing || loadingQuote}
              onClick={() => void (refreshRequired && target ? requestQuote(target) : confirm())}
            >
              {committing || loadingQuote ? <Loader2 className="h-4 w-4 animate-spin" /> : refreshRequired ? <RefreshCw className="h-4 w-4" /> : null}
              {refreshRequired ? t("shareMarket.termination.refresh") : t("shareMarket.termination.confirm")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function seatQuotaLabel(seat: ShareMarketSeat, locale: string, t: TFn) {
  return [
    t("shareMarket.parallelShort", { value: seat.parallelLimit == null ? "∞" : seat.parallelLimit }),
    formatTokenLimit(seat, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`)),
  ].join(" · ");
}

function rentedSeatTokenUsage(
  seat: ShareMarketSeat,
  rows: ShareUserLimitStatusRow[],
  locale: string,
  t: TFn,
) {
  const email = seat.subscription?.renterEmail?.trim().toLowerCase();
  if (!email) return null;
  const row = rows.find((item) => item.email.trim().toLowerCase() === email);
  const used = row?.tokensUsed || 0;
  const limit = seat.tokenLimit ?? row?.tokenLimit;
  const period = seat.tokenPeriod || row?.tokenPeriod || "lifetime";
  const limited = limit != null && limit > 0;
  return [
    `${compactTokens(used, locale)} / ${limited ? compactTokens(limit, locale) : t("common.unlimited")}`,
    t(`shareMarket.period.${period}`),
  ].join(" · ");
}

function seatStatusLabel(seat: ShareMarketSeat, t: TFn) {
  const statusKey = subscriptionStatusKey(seat.subscription?.status || "");
  return statusKey ? t(statusKey) : isSeatIdle(seat) ? t("shareMarket.available") : seat.status;
}

export function ShareMarketOwnerWorkspace({
  listings,
  loading,
  focusedShareId,
  onChanged,
  onInteractionChange,
  showHeading = true,
}: {
  listings: ShareMarketListing[];
  loading: boolean;
  focusedShareId?: string;
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
  showHeading?: boolean;
}) {
  const { locale, t } = useLocaleText();
  const chat = useClientChat();
  const [addOpen, setAddOpen] = React.useState(false);
  const [reopenListing, setReopenListing] = React.useState<ShareMarketListing | null>(null);
  const [seatDialog, setSeatDialog] = React.useState<{ listing: ShareMarketListing; seat?: ShareMarketSeat; template?: ShareMarketSeat } | null>(null);
  const [priceDialog, setPriceDialog] = React.useState<ShareMarketSubscription | null>(null);
  const [terminationTarget, setTerminationTarget] = React.useState<{ subscription: ShareMarketSubscription; denyFutureAccess: boolean } | null>(null);
  const [confirm, setConfirm] = React.useState<ConfirmAction | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [family, setFamily] = React.useState<ShareMarketProviderFamily | "all">("all");
  const [query, setQuery] = React.useState("");
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [limitRows, setLimitRows] = React.useState<ShareUserLimitStatusRow[]>([]);
  const focusedRef = React.useRef("");
  const interactionActive = addOpen || !!reopenListing || !!seatDialog || !!priceDialog || !!terminationTarget || !!confirm || busy;
  const filteredListings = React.useMemo(
    () => filterMarketListings(listings, family, query),
    [family, listings, query],
  );
  const { attentionSeats, attentionListings, active, closed } = React.useMemo(
    () => partitionOwnedListings(filteredListings),
    [filteredListings],
  );
  const hasListings = listings.length > 0;
  const selected = selectedId ? listings.find((listing) => listing.id === selectedId) || null : null;
  const attentionShareIds = React.useMemo(() => {
    const ids = new Set(attentionListings.map((listing) => listing.id));
    for (const item of attentionSeats) ids.add(item.listing.id);
    return ids;
  }, [attentionListings, attentionSeats]);

  React.useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);

  React.useEffect(() => {
    if (!selectedId) return;
    if (!listings.some((listing) => listing.id === selectedId)) setSelectedId(null);
  }, [listings, selectedId]);

  React.useEffect(() => {
    const shareId = selected?.shareId;
    if (!shareId) {
      setLimitRows([]);
      return;
    }
    let cancelled = false;
    void getShareUserLimitStatus(shareId)
      .then((page) => {
        if (!cancelled) setLimitRows(page.rows || []);
      })
      .catch(() => {
        if (!cancelled) setLimitRows([]);
      });
    return () => {
      cancelled = true;
    };
  }, [selected?.shareId]);

  React.useEffect(() => {
    if (!focusedShareId || loading || focusedRef.current === focusedShareId) return;
    const listing = listings.find((item) => item.shareId === focusedShareId);
    const target = listing && document.getElementById(listingCardId("listing", listing.shareId));
    if (!listing || !target) return;
    focusedRef.current = focusedShareId;
    target.scrollIntoView({ block: "start" });
  }, [focusedShareId, listings, loading]);

  const run = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      setConfirm(null);
      await onChanged();
    } catch (reason) {
      setError(shareMarketMutationError(reason, t));
    } finally {
      setBusy(false);
    }
  };

  const openReopenListing = (listingId: string) => {
    const listing = listings.find((item) => item.id === listingId);
    if (listing) setReopenListing(listing);
  };

  const renderSeatStatusDetails = (seat: ShareMarketSeat, tone: "rose" | "amber" | "plain" = "rose") => {
    const subscription = seat.subscription;
    const grantFailed = subscription?.status === "grant_failed";
    const grantContractViolation = subscription?.failureCode === "share_market_grant_contract_violation";
    const hasStatusDetail = !!subscription && (
      grantFailed
      || !!subscription.releaseReason
      || !!subscription.failureCode
      || subscription.grantAttempts != null
      || subscription.integrityState !== "compatible"
      || !!subscription.terminationAdjustment
      || subscription.priceChange?.status === "pending"
    );
    if (!subscription || !hasStatusDetail) return null;
    const priceChange = subscription.priceChange;
    return (
      <div className={cn(
        "grid gap-0.5 px-3 py-2 text-xs leading-5",
        tone === "amber"
          ? "border-l-2 border-amber-400 bg-amber-50 text-amber-950"
          : tone === "rose"
            ? "border-l-2 border-rose-400 bg-rose-50 text-rose-900"
            : "text-slate-600",
      )}>
        {grantFailed || grantContractViolation ? <p className="font-medium">{t(grantFailureMessageKey(subscription.failureCode))}</p> : null}
        {subscription.failureCode ? <p className="break-all font-mono text-[10px] opacity-70">{t("shareMarket.authorizationFailure.code", { code: subscription.failureCode })}</p> : null}
        {subscription.grantAttempts != null ? <p>{t("shareMarket.authorizationFailure.attempts", { count: subscription.grantAttempts })}</p> : null}
        {subscription.releaseReason ? <p className="break-words opacity-80">{t(grantFailed ? "shareMarket.authorizationFailure.reason" : "shareMarket.subscription.statusDetail", { reason: subscription.releaseReason })}</p> : null}
        {subscription.integrityState !== "compatible" ? <p>{t(integrityStatusKey(subscription.integrityState))}{subscription.integrityReason ? ` · ${integrityReasonText(subscription.integrityReason, t)}` : ""}</p> : null}
        {subscription.terminationAdjustment ? <p>{t("shareMarket.refund.summary", { amount: formatUsdMoney(subscription.terminationAdjustment.amountMinor, locale), status: t(refundStatusKey(subscription.terminationAdjustment.status)) })}</p> : null}
        {priceChange ? (
          <p>
            {t(`shareMarket.priceChange.status.${priceChange.status}`)}
            {" · "}
            {t("shareMarket.priceChange.summary", {
              previous: formatUsdMoney(priceChange.previousDailyRateMinor, locale),
              proposed: formatUsdMoney(priceChange.proposedDailyRateMinor, locale),
            })}
          </p>
        ) : null}
      </div>
    );
  };

  const renderActionButton = (action: {
    key: string;
    label: string;
    icon?: React.ReactNode;
    iconOnly?: boolean;
    onClick: () => void;
  }) => (
    <span key={action.key} title={action.label}>
      <Button
        isIconOnly={action.iconOnly}
        size="sm"
        variant="ghost"
        className={cn("h-7 min-w-0", action.iconOnly ? "w-7 px-0" : "px-2 text-xs")}
        aria-label={action.label}
        isDisabled={busy}
        onClick={action.onClick}
      >
        {action.icon}
        {action.iconOnly ? null : action.label}
      </Button>
    </span>
  );

  const renderActionGroup = (
    compact: Array<{ key: string; label: string; icon?: React.ReactNode; iconOnly?: boolean; onClick: () => void }>,
    destructive: Array<{ key: string; label: string; icon?: React.ReactNode; iconOnly?: boolean; onClick: () => void }>,
  ) => {
    if (!compact.length && !destructive.length) return null;
    return (
      <div data-no-card-open data-no-row-toggle className="flex shrink-0 flex-wrap items-center justify-end gap-2">
        {compact.length ? (
          <div className="flex flex-wrap items-center justify-end gap-1">
            {compact.map(renderActionButton)}
          </div>
        ) : null}
        {destructive.length ? (
          <div className="flex flex-wrap items-center justify-end gap-1">
            {destructive.map(renderActionButton)}
          </div>
        ) : null}
      </div>
    );
  };

  const renderSeatActions = (listing: ShareMarketListing, seat: ShareMarketSeat) => {
    const subscription = seat.subscription;
    const grantFailed = subscription?.status === "grant_failed";
    const compact: Array<{
      key: string;
      label: string;
      icon?: React.ReactNode;
      iconOnly?: boolean;
      onClick: () => void;
    }> = [];
    const destructive: typeof compact = [];

    if (!subscription && seat.status === "available" && !seat.readOnly) {
      compact.push(
        {
          key: "edit",
          label: t("shareMarket.manage"),
          icon: <Pencil className="h-3.5 w-3.5" />,
          iconOnly: true,
          onClick: () => setSeatDialog({ listing, seat }),
        },
        {
          key: "copy",
          label: t("shareMarket.copySeat"),
          icon: <Copy className="h-3.5 w-3.5" />,
          iconOnly: true,
          onClick: () => setSeatDialog({ listing, template: seat }),
        },
      );
    }
    if (seat.canDelete) {
      destructive.push({
        key: "delete",
        label: t("shareMarket.deleteSeat"),
        onClick: () => setConfirm({
          title: t(grantFailed ? "shareMarket.confirm.deleteFailedTitle" : "shareMarket.confirm.deleteTitle"),
          description: grantFailed
            ? t("shareMarket.confirm.deleteFailedDescription")
            : t("shareMarket.confirm.deleteDescription", { position: seat.position }),
          label: t("shareMarket.deleteSeat"),
          tone: "danger",
          run: () => deleteShareMarketSeat(seat.id),
        }),
      });
    }
    if (subscription?.canRetryGrant) {
      compact.push({
        key: "retry-grant",
        label: t("shareMarket.authorizationFailure.retry"),
        icon: <RefreshCw className="h-3.5 w-3.5" />,
        onClick: () => setConfirm({
          title: t("shareMarket.authorizationFailure.retryTitle"),
          description: t("shareMarket.authorizationFailure.retryDescription", {
            email: subscription.renterEmail || "-",
          }),
          label: t("shareMarket.authorizationFailure.retry"),
          tone: "warning",
          run: () => retryShareMarketSubscriptionGrant(subscription.id),
        }),
      });
    }
    if (subscription?.canProposePriceChange) {
      destructive.push({
        key: "price",
        label: t("shareMarket.priceChange.action"),
        onClick: () => setPriceDialog(subscription),
      });
    }
    if (subscription?.priceChange?.canCancel) {
      destructive.push({
        key: "cancel-price",
        label: t("shareMarket.priceChange.cancel"),
        onClick: () => void run(() => cancelShareMarketPriceChange(subscription.priceChange!.id)),
      });
    }
    if (subscription?.canForceRevoke) {
      const requiresRefundConfirmation = subscription.dailyRateMinor != null
        && subscription.serviceDurationDays != null
        && !!subscription.serviceStartedAt;
      destructive.push(
        {
          key: "revoke",
          label: t("shareMarket.forceRevoke"),
          onClick: () => requiresRefundConfirmation
            ? setTerminationTarget({ subscription, denyFutureAccess: false })
            : setConfirm({
            title: t("shareMarket.confirm.revokeTitle"),
            description: t("shareMarket.confirm.revokeDescription", { email: subscription.renterEmail || "-" }),
            label: t("shareMarket.forceRevoke"),
            tone: "warning",
            run: () => forceRevokeShareMarketSubscription(subscription.id, { denyFutureAccess: false }),
            }),
        },
        {
          key: "deny",
          label: t("shareMarket.denyAndRevoke"),
          onClick: () => requiresRefundConfirmation
            ? setTerminationTarget({ subscription, denyFutureAccess: true })
            : setConfirm({
            title: t("shareMarket.confirm.denyTitle"),
            description: t("shareMarket.confirm.denyDescription", { email: subscription.renterEmail || "-" }),
            label: t("shareMarket.denyAndRevoke"),
            tone: "danger",
            run: () => forceRevokeShareMarketSubscription(subscription.id, { denyFutureAccess: true }),
            }),
        },
      );
    }

    return renderActionGroup(compact, destructive);
  };

  const renderListingActions = (listing: ShareMarketListing) => {
    const compact: Array<{
      key: string;
      label: string;
      icon?: React.ReactNode;
      iconOnly?: boolean;
      onClick: () => void;
    }> = [{
      key: "chat",
      label: t("shareMarket.groupChat"),
      icon: <MessageCircle className="h-3.5 w-3.5" />,
      iconOnly: true,
      onClick: () => void chat.openClientChat(listing.installationId),
    }];
    const destructive: typeof compact = [];

    if (listing.status === "active") {
      compact.push({
        key: "add",
        label: t("shareMarket.addSeat"),
        icon: <Plus className="h-3.5 w-3.5" />,
        iconOnly: true,
        onClick: () => setSeatDialog({ listing }),
      });
      destructive.push({
        key: "close",
        label: t("shareMarket.closeListing"),
        onClick: () => setConfirm({
          title: t("shareMarket.confirm.closeTitle"),
          description: t("shareMarket.confirm.closeDescription", { share: listing.shareName }),
          label: t("shareMarket.closeListing"),
          tone: "warning",
          run: () => closeShareMarketListing(listing.id),
        }),
      });
    } else {
      destructive.push({
        key: "reopen",
        label: t("shareMarket.reopen.action"),
        icon: <RotateCcw className="h-3.5 w-3.5" />,
        onClick: () => setReopenListing(listing),
      });
      if (listing.canDelete) {
        compact.push({
          key: "delete",
          label: t("shareMarket.deleteListing"),
          icon: <Trash2 className="h-3.5 w-3.5" />,
          iconOnly: true,
          onClick: () => setConfirm({
            title: t("shareMarket.confirm.deleteListingTitle"),
            description: t("shareMarket.confirm.deleteListingDescription", { share: listing.shareName }),
            label: t("shareMarket.deleteListing"),
            tone: "danger",
            run: () => deleteShareMarketListing(listing.id),
          }),
        });
      }
    }

    const reopenDisabled = listing.status !== "active" && !listing.canReopen;
    return (
      <div data-no-card-open className="flex shrink-0 flex-wrap items-center justify-end gap-2">
        {compact.length ? (
          <div className="flex flex-wrap items-center justify-end gap-1">
            {compact.map(renderActionButton)}
          </div>
        ) : null}
        {destructive.length ? (
          <div className="flex flex-wrap items-center justify-end gap-1">
            {destructive.map((action) => (
              <span key={action.key} title={action.label}>
                <Button
                  isIconOnly={action.iconOnly}
                  size="sm"
                  variant="ghost"
                  className={cn("h-7 min-w-0", action.iconOnly ? "w-7 px-0" : "px-2 text-xs")}
                  aria-label={action.label}
                  isDisabled={busy || (action.key === "reopen" && reopenDisabled)}
                  onClick={action.onClick}
                >
                  {action.icon}
                  {action.iconOnly ? null : action.label}
                </Button>
              </span>
            ))}
          </div>
        ) : null}
      </div>
    );
  };

  const listingOccupancyLabel = (listing: ShareMarketListing) => {
    if (listing.status === "closed") return t("shareMarket.closed");
    const counts = listingOccupancyCounts(listing);
    return t("shareMarket.catalog.occupancy", { idle: counts.idle, total: counts.total || listing.seats.length });
  };

  const renderListingCard = (listing: ShareMarketListing, muted = false) => {
    const counts = listingOccupancyCounts(listing);
    const reopenable = reopenableListingSeats(listing).length;
    const remaining = listingClosedRentalSeats(listing).length;
    const attention = attentionShareIds.has(listing.id);
    return (
      <MarketShareCard
        key={listing.id}
        listing={listing}
        focused={focusedShareId === listing.shareId}
        muted={muted || listing.status !== "active"}
        attention={attention}
        cardId={listingCardId("listing", listing.shareId)}
        occupancy={listingOccupancyLabel(listing)}
        onOpen={() => setSelectedId(listing.id)}
        footer={(
          <div className="grid content-start">
            <CatalogSeatPreviewList
              listing={listing}
              seats={listing.seats}
              onOpen={() => setSelectedId(listing.id)}
            />
            {listing.status !== "active" && (reopenable || remaining || counts.attention) ? (
              <p className="flex min-w-0 flex-wrap gap-x-2 gap-y-0.5 px-1.5 pt-1 text-[10px] text-slate-500">
                {reopenable ? <span>{t("shareMarket.listings.reopenableSeats", { count: reopenable })}</span> : null}
                {remaining ? <span>{t("shareMarket.listings.activeRentals", { count: remaining })}</span> : null}
                {counts.attention ? <span className="font-medium text-rose-700">{t("account.share.seatsAttention")} · {counts.attention}</span> : null}
              </p>
            ) : null}
            {renderListingActions(listing)}
          </div>
        )}
      />
    );
  };

  const addShareButton = (variant: "primary" | "outline") => (
    <Button size="sm" variant={variant} className="h-9 shrink-0" onClick={() => setAddOpen(true)}>
      <Plus className="h-4 w-4" />
      {t("shareMarket.addShare")}
    </Button>
  );

  const selectedSeats = selected
    ? [...listingLiveSeats(selected), ...selected.seats.filter((seat) => needsOwnedSeatAttention(seat))]
      .filter((seat, index, seats) => seats.findIndex((item) => item.id === seat.id) === index)
      .sort((left, right) => left.position - right.position)
    : [];

  return (
    <div className="grid min-w-0 gap-5">
      {showHeading ? (
        <div>
          <h2 className="text-sm font-semibold text-slate-900">{t("shareMarket.workspace.selling")}</h2>
          <p className="mt-0.5 text-xs text-slate-500">{t("shareMarket.workspace.sellingHint")}</p>
        </div>
      ) : null}
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}

      {loading && !listings.length ? (
        <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-slate-500">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("shareMarket.loading")}
        </div>
      ) : (
        <>
          <MarketListingFilters
            listings={listings}
            family={family}
            query={query}
            onFamilyChange={setFamily}
            onQueryChange={setQuery}
            leading={(
              <h3 id="share-listings-active" className="shrink-0 whitespace-nowrap text-xs font-semibold uppercase tracking-wide text-slate-500">
                {t("shareMarket.listings.active")}
                {active.length ? <span className="ml-1.5 tabular-nums text-slate-400">{active.length}</span> : null}
              </h3>
            )}
            trailing={addShareButton(hasListings ? "outline" : "primary")}
          />
          <section className="grid gap-2" aria-labelledby="share-listings-active">
            {active.length ? (
              <div className={MARKET_SHARE_CARD_GRID_CLASS}>
                {active.map((listing) => renderListingCard(listing))}
              </div>
            ) : (
              <div className="grid justify-items-center gap-2 rounded-md border border-dashed border-slate-200 px-4 py-10 text-center text-sm text-slate-500">
                <span>{t("shareMarket.workspace.noListings")}</span>
                {!hasListings ? (
                  <p className="text-xs text-slate-400">{t("shareMarket.workspace.sellingHint")}</p>
                ) : null}
              </div>
            )}
          </section>

          {closed.length || attentionListings.length ? (
            <section className="grid gap-2 border-t border-slate-200 pt-5" aria-labelledby="share-listings-closed">
              <h3 id="share-listings-closed" className="text-xs font-semibold uppercase tracking-wide text-slate-500">
                {t("shareMarket.listings.closed")}
                <span className="ml-1.5 tabular-nums text-slate-400">{closed.length + attentionListings.length}</span>
              </h3>
              <div className={MARKET_SHARE_CARD_GRID_CLASS}>
                {attentionListings.map((listing) => renderListingCard(listing, true))}
                {closed.map((listing) => renderListingCard(listing, true))}
              </div>
            </section>
          ) : null}
        </>
      )}

      <Modal.Backdrop isOpen={!!selected} onOpenChange={(open) => !open && setSelectedId(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(860px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("shareMarket.catalog.shareSeats")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[min(70vh,36rem)] gap-3 overflow-y-auto">
              {selected ? (
                <>
                  {selected.status === "closed" && !selected.canReopen ? (
                    <p className="border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-xs font-medium text-amber-800">
                      {t(reopenBlockedReasonKey(selected.reopenBlockedReason))}
                    </p>
                  ) : null}
                  {selectedSeats.length ? (
                    <div className="overflow-x-auto rounded-md border border-slate-200">
                      <table className="w-full min-w-[36rem] table-fixed border-collapse text-left text-xs">
                        <thead className="bg-slate-50 text-[10px] font-semibold uppercase tracking-[0.08em] text-slate-500">
                          <tr>
                            <th className="w-16 px-3 py-2">{t("shareMarket.col.seat")}</th>
                            <th className="w-28 px-2 py-2">{t("shareMarket.col.status")}</th>
                            <th className="w-28 px-2 py-2">{t("shareMarket.col.amount")}</th>
                            <th className="px-2 py-2">{t("shareMarket.col.limits")}</th>
                            <th className="w-40 px-2 py-2 text-right">{t("shareMarket.col.actions")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {selectedSeats.map((seat) => {
                            const attention = needsOwnedSeatAttention(seat);
                            const priceOnly = attention && isPriceOnlySeatAttention(seat);
                            return (
                              <tr
                                key={seat.id}
                                className={cn(
                                  "border-t border-slate-100 align-top text-slate-700",
                                  attention && (priceOnly ? "bg-amber-50/60" : "bg-rose-50/60"),
                                )}
                              >
                                <td className="px-3 py-2 font-semibold tabular-nums">#{seat.position}</td>
                                <td className="px-2 py-2 text-slate-500">
                                  <div className="grid gap-1">
                                    <span>{seatStatusLabel(seat, t)}</span>
                                    {attention ? renderSeatStatusDetails(seat, priceOnly ? "amber" : "rose") : null}
                                  </div>
                                </td>
                                <td className="px-2 py-2 tabular-nums">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</td>
                                <td className="px-2 py-2 text-slate-500">
                                  <div className="grid gap-0.5">
                                    <span>{seatQuotaLabel(seat, locale, t)}</span>
                                    {seat.subscription && !isSeatIdle(seat) ? (
                                      <span className="font-mono text-[11px] text-slate-600">
                                        {t("dashboard.userLimit.token")}: {rentedSeatTokenUsage(seat, limitRows, locale, t) || `${compactTokens(0, locale)} / ${seat.tokenLimit == null ? t("common.unlimited") : compactTokens(seat.tokenLimit, locale)}`}
                                      </span>
                                    ) : null}
                                  </div>
                                </td>
                                <td className="px-2 py-2">
                                  <div className="flex justify-end">{renderSeatActions(selected, seat)}</div>
                                </td>
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  ) : (
                    <p className="text-sm text-slate-500">{t("shareMarket.catalog.noRiders")}</p>
                  )}
                </>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              {selected ? renderListingActions(selected) : null}
              <Button variant="ghost" onClick={() => setSelectedId(null)}>{t("common.close")}</Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <ShareMarketAddListingDialog open={addOpen} onOpenChange={setAddOpen} onSaved={() => void onChanged()} onReopenListing={openReopenListing} />
      <ReopenListingDialog listing={reopenListing} onOpenChange={(open) => !open && setReopenListing(null)} onSaved={() => void onChanged()} />
      <SeatDialog key={seatDialog ? `${seatDialog.listing.id}:${seatDialog.seat?.id || `copy:${seatDialog.template?.id || "new"}`}` : "closed"} listing={seatDialog?.listing || null} seat={seatDialog?.seat} template={seatDialog?.template} onOpenChange={(open) => !open && setSeatDialog(null)} onSaved={() => void onChanged()} />
      <PriceDialog subscription={priceDialog} onOpenChange={(open) => !open && setPriceDialog(null)} onSaved={() => void onChanged()} />
      <TerminationDialog target={terminationTarget} onOpenChange={(open) => !open && setTerminationTarget(null)} onSaved={() => void onChanged()} />
      <ConfirmAlertDialog
        open={!!confirm}
        title={confirm?.title || ""}
        description={confirm?.description || ""}
        confirmLabel={confirm?.label || ""}
        cancelLabel={t("common.cancel")}
        tone={confirm?.tone || "warning"}
        busy={busy}
        onConfirm={() => confirm && void run(confirm.run)}
        onOpenChange={(open) => !open && !busy && setConfirm(null)}
      />
    </div>
  );
}
