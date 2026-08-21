"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import {
  Ban,
  Copy,
  Loader2,
  MessageCircle,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  UserRoundX,
  X,
} from "lucide-react";
import { useClientChat } from "@/components/chat/client-chat";
import { CompactSelect } from "@/components/common/compact-select";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { SegmentedControl } from "@/components/common/segmented-control";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  addShareMarketSeat,
  cancelShareMarketPriceChange,
  closeShareMarketListing,
  createShareMarketListing,
  deleteShareMarketListing,
  deleteShareMarketSeat,
  forceRevokeShareMarketSubscription,
  getShareMarketOwnedShares,
  proposeShareMarketPriceChange,
  updateShareMarketSeat,
} from "@/lib/api";
import { MARKET_CURRENCY } from "@/lib/market-money";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type {
  ShareMarketListing,
  ShareMarketOwnedShare,
  ShareMarketSeat,
  ShareMarketSeatInput,
  ShareMarketSubscription,
  ShareTokenPeriod,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  capabilityModelLabel,
  formatSeatPrice,
  formatTokenLimit,
  isCoreShareApp,
  subscriptionStatusKey,
} from "@/components/dashboard/share-market/market-utils";

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
type ConfirmAction = {
  title: string;
  description: string;
  label: string;
  tone: "warning" | "danger";
  run: () => Promise<unknown>;
};

function grantFailureMessage(t: TFn, code?: string) {
  switch (code) {
    case "cc_switch_share_revision_conflict":
      return t("shareMarket.authorizationFailure.revisionConflict");
    case "cc_switch_share_policy_divergent":
      return t("shareMarket.authorizationFailure.policyDivergent");
    case "cc_switch_share_binding_immutable":
      return t("shareMarket.authorizationFailure.bindingImmutable");
    case "control_ack_timeout":
      return t("shareMarket.authorizationFailure.controlTimeout");
    default:
      return t("shareMarket.authorizationFailure.generic");
  }
}

const TOKEN_PERIODS: ShareTokenPeriod[] = [
  "lifetime",
  "day",
  "week",
  "sevenDays",
  "calendarMonth",
  "thirtyDays",
];

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
    tokenLimit: seat.tokenLimit == null ? "" : String(seat.tokenLimit),
    tokenPeriod: seat.tokenPeriod,
    paid: !seat.isFree,
    price: seat.dailyRateMinor == null ? "" : (seat.dailyRateMinor / 100).toFixed(2),
    serviceDurationMode: seat.serviceDurationDays == null ? "permanent" : "fixed",
    serviceDurationDays: String(seat.serviceDurationDays ?? 1),
    serviceDurationTouched: true,
  };
}

function positiveOptional(value: string, label: string, t: TFn) {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1) {
    throw new Error(t("shareMarket.error.positiveInteger", { field: label }));
  }
  return parsed;
}

function normalizedSeat(draft: SeatDraft, t: TFn): ShareMarketSeatInput {
  const parallelLimit = positiveOptional(draft.parallelLimit, t("shareMarket.parallel"), t);
  const tokenLimit = positiveOptional(draft.tokenLimit, t("shareMarket.tokens"), t);
  const serviceDurationDays = draft.serviceDurationMode === "permanent"
    ? undefined
    : positiveOptional(draft.serviceDurationDays, t("shareMarket.serviceDuration.days"), t);
  if (serviceDurationDays != null && serviceDurationDays > 365) {
    throw new Error(t("shareMarket.error.serviceDuration"));
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
    throw new Error(t("shareMarket.error.price"));
  }
  return { ...base, dailyRateMinor, currency: MARKET_CURRENCY };
}

function fieldClass() {
  return "h-10 min-w-0 rounded-md border border-slate-200 bg-white px-3 text-sm text-slate-900 outline-none focus:border-slate-400";
}

function SeatFields({
  draft,
  supportedPeriods,
  onChange,
}: {
  draft: SeatDraft;
  supportedPeriods?: ShareTokenPeriod[];
  onChange: (draft: SeatDraft) => void;
}) {
  const { t } = useLocaleText();
  const periods = supportedPeriods?.length ? supportedPeriods : TOKEN_PERIODS;
  const patch = (value: Partial<SeatDraft>) => onChange({ ...draft, ...value });
  return (
    <div className="grid min-w-0 gap-3">
      <div className={cn("grid min-w-0 gap-3", draft.tokenLimit.trim() ? "sm:grid-cols-3" : "sm:grid-cols-2")}>
        <label className="grid gap-1 text-xs text-slate-500">
          {t("shareMarket.parallel")}
          <input className={fieldClass()} inputMode="numeric" value={draft.parallelLimit} placeholder={t("shareMarket.dialog.unlimited")} onChange={(event) => patch({ parallelLimit: event.target.value })} />
        </label>
        <label className="grid gap-1 text-xs text-slate-500">
          {t("shareMarket.tokens")}
          <input
            className={fieldClass()}
            inputMode="numeric"
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
        items={[
          { id: "free", label: t("shareMarket.dialog.freeMode") },
          { id: "paid", label: t("shareMarket.dialog.paidMode") },
        ]}
      />
      {draft.paid ? (
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.dialog.amount")}
            <input className={fieldClass()} inputMode="decimal" value={draft.price} onChange={(event) => patch({ price: event.target.value })} />
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
          items={[
            { id: "fixed", label: t("shareMarket.serviceDuration.fixed") },
            { id: "permanent", label: t("shareMarket.serviceDuration.permanent") },
          ]}
        />
        {draft.serviceDurationMode === "fixed" ? (
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.serviceDuration.days")}
            <input type="number" min={1} max={365} className={fieldClass()} value={draft.serviceDurationDays} onChange={(event) => patch({ serviceDurationDays: event.target.value, serviceDurationTouched: true })} />
          </label>
        ) : null}
      </div>
    </div>
  );
}

export function ShareMarketAddListingDialog({ open, onOpenChange, onSaved }: { open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => void }) {
  const { t } = useLocaleText();
  const [shares, setShares] = React.useState<ShareMarketOwnedShare[]>([]);
  const [shareId, setShareId] = React.useState("");
  const [seats, setSeats] = React.useState<SeatDraft[]>([emptySeat()]);
  const [loading, setLoading] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (!open) return;
    setLoading(true);
    setError("");
    getShareMarketOwnedShares()
      .then((items) => {
        const eligible = items.filter(
          (item) => !item.alreadyListed && !item.freeAccess && item.shareStatus === "active",
        );
        setShares(eligible);
        setShareId(eligible[0]?.shareId || "");
        setSeats([emptySeat(eligible[0]?.supportedUserTokenPeriods)]);
      })
      .catch((reason) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setLoading(false));
  }, [open]);

  const selected = shares.find((share) => share.shareId === shareId);
  const save = async () => {
    if (!shareId || busy) return;
    setBusy(true);
    setError("");
    try {
      await createShareMarketListing(shareId, seats.map((seat) => normalizedSeat(seat, t)));
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
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
            {!loading && shares.length === 0 ? <p className="text-sm text-slate-500">{t("shareMarket.dialog.noShares")}</p> : null}
            {!loading && shares.length ? (
              <>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t("shareMarket.dialog.selectShare")}
                  <CompactSelect
                    value={shareId}
                    options={shares.map((share) => ({
                      value: share.shareId,
                      label: share.subdomain ? `${share.shareName} · ${share.subdomain}` : share.shareName,
                      description: `${share.ownerEmail} · ${share.supportedApps.map((app) => app in SHARE_APP_LABELS ? SHARE_APP_LABELS[app as keyof typeof SHARE_APP_LABELS] : app).join(" / ")}`,
                    }))}
                    onChange={(value) => {
                      const next = shares.find((share) => share.shareId === value);
                      setShareId(value);
                      setSeats([emptySeat(next?.supportedUserTokenPeriods)]);
                    }}
                    ariaLabel={t("shareMarket.dialog.selectShare")}
                    className="w-full"
                    triggerClassName="min-h-12 w-full text-sm"
                  />
                </label>
                <div className="grid gap-4">
                  {seats.map((seat, index) => (
                    <section key={index} className="grid gap-3 border-t border-slate-200 pt-4 first:border-0 first:pt-0">
                      <div className="flex items-center justify-between gap-2">
                        <strong className="text-sm">{t("shareMarket.seat", { position: index + 1 })}</strong>
                        <div className="flex gap-1">
                          <Button isIconOnly size="sm" variant="ghost" aria-label={t("shareMarket.copySeat")} isDisabled={seats.length >= 20} onClick={() => setSeats((items) => [...items.slice(0, index + 1), { ...items[index] }, ...items.slice(index + 1)])}><Copy className="h-4 w-4" /></Button>
                          {seats.length > 1 ? <Button isIconOnly size="sm" variant="ghost" aria-label={t("common.delete")} onClick={() => setSeats((items) => items.filter((_, itemIndex) => itemIndex !== index))}><X className="h-4 w-4" /></Button> : null}
                        </div>
                      </div>
                      <SeatFields draft={seat} supportedPeriods={selected?.supportedUserTokenPeriods} onChange={(next) => setSeats((items) => items.map((item, itemIndex) => itemIndex === index ? next : item))} />
                    </section>
                  ))}
                  {seats.length < 20 ? <Button variant="outline" onClick={() => setSeats((items) => [...items, emptySeat(selected?.supportedUserTokenPeriods)])}><Plus className="h-4 w-4" />{t("shareMarket.dialog.addSeat")}</Button> : null}
                </div>
              </>
            ) : null}
            {error ? <p className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || loading || !shareId} onClick={() => void save()}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}{t("shareMarket.dialog.create")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function SeatDialog({
  listing,
  seat,
  onOpenChange,
  onSaved,
}: {
  listing: ShareMarketListing | null;
  seat?: ShareMarketSeat;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { t } = useLocaleText();
  const [draft, setDraft] = React.useState<SeatDraft>(emptySeat());
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    if (!listing) return;
    setDraft(seat ? seatDraft(seat) : emptySeat(listing.supportedUserTokenPeriods));
    setError("");
  }, [listing, seat]);
  const save = async () => {
    if (!listing || busy) return;
    setBusy(true);
    setError("");
    try {
      const input = normalizedSeat(draft, t);
      if (seat) await updateShareMarketSeat(seat.id, input, seat.offerRevision);
      else await addShareMarketSeat(listing.id, input);
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal.Backdrop isOpen={!!listing} onOpenChange={(open) => !busy && onOpenChange(open)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{seat ? t("shareMarket.manage") : t("shareMarket.addSeat")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid gap-4">
            <SeatFields draft={draft} supportedPeriods={listing?.supportedUserTokenPeriods} onChange={setDraft} />
            {error ? <p className="text-sm text-rose-700">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy} onClick={() => void save()}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("common.save")}</Button>
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
    setBusy(true);
    try {
      await proposeShareMarketPriceChange(subscription.id, dailyRateMinor, subscription.offerRevision);
      onOpenChange(false);
      onSaved();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
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

function AppSummary({ listing }: { listing: ShareMarketListing }) {
  const { t } = useLocaleText();
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-500">
      {listing.appCapabilities.map((capability) => (
        <span key={capability.app} className="inline-flex min-w-0 items-center gap-1.5">
          {isCoreShareApp(capability.app) ? <ShareAppLogo app={capability.app} size={14} /> : null}
          <span className="truncate">{capabilityModelLabel(capability, t("shareMarket.modelPassthrough"), t("shareMarket.catalog.modelUnknown"))}</span>
        </span>
      ))}
    </div>
  );
}

export function ShareMarketOwnerWorkspace({
  listings,
  loading,
  focusedShareId,
  onChanged,
  onInteractionChange,
}: {
  listings: ShareMarketListing[];
  loading: boolean;
  focusedShareId?: string;
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
}) {
  const { locale, t } = useLocaleText();
  const chat = useClientChat();
  const [addOpen, setAddOpen] = React.useState(false);
  const [seatDialog, setSeatDialog] = React.useState<{ listing: ShareMarketListing; seat?: ShareMarketSeat } | null>(null);
  const [priceDialog, setPriceDialog] = React.useState<ShareMarketSubscription | null>(null);
  const [confirm, setConfirm] = React.useState<ConfirmAction | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const focusedRef = React.useRef("");
  const interactionActive = addOpen || !!seatDialog || !!priceDialog || !!confirm || busy;

  React.useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);

  React.useEffect(() => {
    if (!focusedShareId || loading || focusedRef.current === focusedShareId) return;
    const target = document.getElementById(`share-market-listing-${focusedShareId}`);
    if (!target) return;
    focusedRef.current = focusedShareId;
    target.scrollIntoView({ block: "start" });
    target.focus({ preventScroll: true });
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
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid min-w-0 gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-slate-200 pb-3">
        <div>
          <h2 className="text-sm font-semibold text-slate-900">{t("shareMarket.workspace.selling")}</h2>
          <p className="mt-0.5 text-xs text-slate-500">{t("shareMarket.workspace.sellingHint")}</p>
        </div>
        <div className="flex gap-2">
          <Button isIconOnly variant="ghost" aria-label={t("common.reload")} isDisabled={loading} onClick={() => void onChanged()}><RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} /></Button>
          <Button size="sm" variant="primary" onClick={() => setAddOpen(true)}><Plus className="h-4 w-4" />{t("shareMarket.addShare")}</Button>
        </div>
      </div>
      {error ? <p className="border-l-2 border-rose-400 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}
      {listings.map((listing) => (
        <section
          key={listing.id}
          id={`share-market-listing-${listing.shareId}`}
          tabIndex={-1}
          className={cn(
            "grid min-w-0 scroll-mt-20 gap-3 border-b border-slate-200 pb-5 outline-none last:border-0",
            focusedShareId === listing.shareId && "border-l-2 border-l-accent pl-3",
          )}
        >
          <header className="flex min-w-0 flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <strong className="truncate text-sm text-slate-900">{listing.shareName}</strong>
                <span className={listing.shareOnline ? "text-xs font-medium text-emerald-700" : "text-xs font-medium text-rose-700"}>{listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}</span>
                <span className="text-xs text-slate-500">{listing.status === "closed" ? t("shareMarket.closed") : t("account.share.listingActive")}</span>
              </div>
              <p className="mt-1 break-all font-mono text-[11px] text-slate-400">{listing.subdomain}</p>
              <div className="mt-2"><AppSummary listing={listing} /></div>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="outline" onClick={() => void chat.openClientChat(listing.installationId)}><MessageCircle className="h-4 w-4" />{t("shareMarket.groupChat")}</Button>
              <Button size="sm" variant="outline" onClick={() => setSeatDialog({ listing })}><Plus className="h-4 w-4" />{t("shareMarket.addSeat")}</Button>
              {listing.status === "active" ? (
                <Button size="sm" variant="outline" onClick={() => setConfirm({ title: t("shareMarket.confirm.closeTitle"), description: t("shareMarket.confirm.closeDescription", { share: listing.shareName }), label: t("shareMarket.closeListing"), tone: "warning", run: () => closeShareMarketListing(listing.id) })}><Ban className="h-4 w-4" />{t("shareMarket.closeListing")}</Button>
              ) : listing.canDelete ? (
                <Button size="sm" variant="outline" onClick={() => setConfirm({ title: t("shareMarket.confirm.deleteListingTitle"), description: t("shareMarket.confirm.deleteListingDescription", { share: listing.shareName }), label: t("shareMarket.deleteListing"), tone: "danger", run: () => deleteShareMarketListing(listing.id) })}><Trash2 className="h-4 w-4" />{t("shareMarket.deleteListing")}</Button>
              ) : null}
            </div>
          </header>

          <div className="overflow-x-auto rounded-md border border-slate-200">
            <table className="w-full min-w-[760px] border-collapse text-left text-sm">
              <thead className="bg-slate-50 text-xs text-slate-500">
                <tr>
                  <th className="px-3 py-2 font-medium">{t("shareMarket.col.seat")}</th>
                  <th className="px-3 py-2 font-medium">{t("shareMarket.col.amount")}</th>
                  <th className="px-3 py-2 font-medium">{t("shareMarket.col.parallel")}</th>
                  <th className="px-3 py-2 font-medium">{t("shareMarket.col.tokens")}</th>
                  <th className="px-3 py-2 font-medium">{t("shareMarket.renter")}</th>
                  <th className="px-3 py-2 font-medium">{t("shareMarket.col.status")}</th>
                  <th className="px-3 py-2 text-right font-medium">{t("common.actions")}</th>
                </tr>
              </thead>
              <tbody>
                {listing.seats.map((seat) => {
                  const subscription = seat.subscription;
                  const statusKey = subscriptionStatusKey(subscription?.status || "");
                  const grantFailed = subscription?.status === "grant_failed";
                  return (
                    <tr key={seat.id} className="border-t border-slate-100 align-top">
                      <td className="px-3 py-3 font-medium">#{seat.position}</td>
                      <td className="px-3 py-3">{formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</td>
                      <td className="px-3 py-3">{seat.parallelLimit ?? t("common.unlimited")}</td>
                      <td className="max-w-[15rem] px-3 py-3">{formatTokenLimit(seat, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`))}</td>
                      <td className="max-w-[14rem] px-3 py-3"><span className="block truncate" title={subscription?.renterEmail}>{subscription?.renterEmail || "-"}</span></td>
                      <td className="max-w-[20rem] px-3 py-3">
                        <span>{statusKey ? t(statusKey) : seat.status === "available" ? t("shareMarket.available") : seat.status}</span>
                        {grantFailed ? (
                          <div className="mt-1 grid gap-0.5 text-xs text-slate-600">
                            <p>{grantFailureMessage(t, subscription.failureCode)}</p>
                            {subscription.failureCode ? <p className="break-all font-mono text-[10px] text-slate-500">{t("shareMarket.authorizationFailure.code", { code: subscription.failureCode })}</p> : null}
                            {subscription.grantAttempts != null ? <p>{t("shareMarket.authorizationFailure.attempts", { count: subscription.grantAttempts })}</p> : null}
                            {subscription.releaseReason ? <p className="break-words text-slate-500">{t("shareMarket.authorizationFailure.reason", { reason: subscription.releaseReason })}</p> : null}
                          </div>
                        ) : null}
                      </td>
                      <td className="px-3 py-2">
                        <div className="flex justify-end gap-1">
                          {!subscription && seat.status === "available" && !seat.readOnly ? (
                            <>
                              <Button isIconOnly size="sm" variant="ghost" aria-label={t("shareMarket.manage")} onClick={() => setSeatDialog({ listing, seat })}><Pencil className="h-4 w-4" /></Button>
                              <Button isIconOnly size="sm" variant="ghost" aria-label={t("shareMarket.copySeat")} onClick={() => void run(() => addShareMarketSeat(listing.id, normalizedSeat(seatDraft(seat), t)))}><Copy className="h-4 w-4" /></Button>
                            </>
                          ) : null}
                          {seat.canDelete ? (
                            <Button
                              isIconOnly
                              size="sm"
                              variant="ghost"
                              aria-label={t("shareMarket.deleteSeat")}
                              onClick={() => setConfirm({
                                title: t(grantFailed ? "shareMarket.confirm.deleteFailedTitle" : "shareMarket.confirm.deleteTitle"),
                                description: grantFailed
                                  ? t("shareMarket.confirm.deleteFailedDescription")
                                  : t("shareMarket.confirm.deleteDescription", { position: seat.position }),
                                label: t("shareMarket.deleteSeat"),
                                tone: "danger",
                                run: () => deleteShareMarketSeat(seat.id),
                              })}
                            >
                              <Trash2 className="h-4 w-4" />
                            </Button>
                          ) : null}
                          {subscription?.canProposePriceChange ? <Button size="sm" variant="ghost" onClick={() => setPriceDialog(subscription)}>{t("shareMarket.priceChange.action")}</Button> : null}
                          {subscription?.priceChange?.canCancel ? <Button size="sm" variant="ghost" onClick={() => void run(() => cancelShareMarketPriceChange(subscription.priceChange!.id))}>{t("shareMarket.priceChange.cancel")}</Button> : null}
                          {subscription?.canForceRevoke ? (
                            <>
                              <Button isIconOnly size="sm" variant="ghost" aria-label={t("shareMarket.forceRevoke")} onClick={() => setConfirm({ title: t("shareMarket.confirm.revokeTitle"), description: t("shareMarket.confirm.revokeDescription", { email: subscription.renterEmail || "-" }), label: t("shareMarket.forceRevoke"), tone: "warning", run: () => forceRevokeShareMarketSubscription(subscription.id, { denyFutureAccess: false }) })}><RefreshCw className="h-4 w-4" /></Button>
                              <Button isIconOnly size="sm" variant="ghost" aria-label={t("shareMarket.denyAndRevoke")} onClick={() => setConfirm({ title: t("shareMarket.confirm.denyTitle"), description: t("shareMarket.confirm.denyDescription", { email: subscription.renterEmail || "-" }), label: t("shareMarket.denyAndRevoke"), tone: "danger", run: () => forceRevokeShareMarketSubscription(subscription.id, { denyFutureAccess: true }) })}><UserRoundX className="h-4 w-4" /></Button>
                            </>
                          ) : null}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>
      ))}
      {!listings.length && !loading ? (
        <div className="grid min-h-48 place-items-center border-y border-dashed border-slate-200 text-center text-sm text-slate-500">
          <div className="grid gap-3">
            <span>{t("shareMarket.workspace.noListings")}</span>
            <Button size="sm" variant="primary" onClick={() => setAddOpen(true)}><Plus className="h-4 w-4" />{t("shareMarket.addShare")}</Button>
          </div>
        </div>
      ) : null}

      <ShareMarketAddListingDialog open={addOpen} onOpenChange={setAddOpen} onSaved={() => void onChanged()} />
      <SeatDialog listing={seatDialog?.listing || null} seat={seatDialog?.seat} onOpenChange={(open) => !open && setSeatDialog(null)} onSaved={() => void onChanged()} />
      <PriceDialog subscription={priceDialog} onOpenChange={(open) => !open && setPriceDialog(null)} onSaved={() => void onChanged()} />
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
