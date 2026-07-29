"use client";

import * as React from "react";
import { useSearchParams } from "next/navigation";
import { Button, Checkbox, Dropdown, Modal, toast } from "@heroui/react";
import {
  Ban,
  ChevronDown,
  CircleDollarSign,
  Copy,
  ExternalLink,
  Loader2,
  MoreHorizontal,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Settings,
  Trash2,
  UserRoundX,
  X,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { CompactSelect } from "@/components/common/compact-select";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import {
  ProviderContactButton,
  ProviderContactsList,
  ProviderPaymentMethodsList,
} from "@/components/common/provider-contacts";
import { UserBlacklistPanel } from "@/components/common/user-blacklist-panel";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { SeatSortHeader } from "@/components/dashboard/share-market/seat-sort-header";
import {
  CLEARED_SEAT_SORT,
  sortSeatsByLifecycle,
  sortSeatRows,
  toggleSeatSort,
  type SeatSortKey,
  type SeatSortPrefs,
} from "@/components/dashboard/share-market/seat-table-utils";
import { subdomainTunnelUrl, providerQuotaStatusLine, providerStatusIdentity, providerActualModelNames } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  addShareMarketSeat,
  closeShareMarketListing,
  createShareMarketListing,
  declareShareMarketPaid,
  deleteShareMarketListing,
  deleteShareMarketSeat,
  forceRevokeShareMarketSubscription,
  getShareMarketCatalog,
  getShareMarketOwnedShares,
  createShareMarketBlock,
  liftShareMarketBlock,
  releaseShareMarketSubscription,
  rentShareMarketSeat,
  updateShareMarketSeat,
} from "@/lib/api";
import type { ShareMarketTabParam } from "@/lib/dashboard-nav";
import { formatBillingCountdown } from "@/lib/billing-urgency";
import { usePersistentState } from "@/lib/use-persistent-state";
import type {
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketOwnedShare,
  ShareMarketSeat,
  ShareMarketSeatInput,
  ShareMarketSubscription,
  ShareTokenPeriod,
  ShareUpstreamProvider,
} from "@/lib/types";
import type { AppLocale } from "@/lib/i18n";
import { cn, compactTokens } from "@/lib/utils";

type MarketTab = ShareMarketTabParam;
type TFn = ReturnType<typeof useLocaleText>["t"];
type ConfirmAction = {
  id: string;
  title: string;
  description: string;
  confirmLabel: string;
  tone: "danger" | "warning";
  run: () => Promise<unknown>;
};
type SeatDraft = {
  parallelLimit: string;
  tokenLimit: string;
  tokenPeriod: ShareTokenPeriod;
  paid: boolean;
  price: string;
  currency: string;
  periodUnit: "day" | "week" | "month";
  periodCount: string;
};

const TOKEN_PERIODS: ShareTokenPeriod[] = [
  "lifetime",
  "day",
  "week",
  "sevenDays",
  "calendarMonth",
  "thirtyDays",
];

function marketTabFromParam(value: string | null): MarketTab {
  return value === "mine" || value === "rentals" || value === "all" ? value : "all";
}

function replaceMarketQuery(tab: MarketTab, focusShareId: string) {
  const url = new URL(window.location.href);
  url.searchParams.set("tab", tab);
  if (focusShareId) url.searchParams.set("focus", focusShareId);
  else url.searchParams.delete("focus");
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

function canOpenSharePayment(subscription: ShareMarketSubscription) {
  return subscription.canDeclarePaid
    || (
      subscription.canRelease
      && subscription.status === "grant_pending"
      && subscription.priceMinor != null
    );
}

function sharePaymentDeadline(subscription: ShareMarketSubscription, trialHours: number) {
  const authoritative = subscription.paymentDeadline || subscription.openInvoice?.deadlineAt;
  if (authoritative || subscription.status !== "grant_pending" || subscription.priceMinor == null) {
    return authoritative;
  }
  if (!Number.isFinite(trialHours) || trialHours <= 0) return undefined;
  const createdAt = Date.parse(subscription.createdAt);
  if (!Number.isFinite(createdAt)) return undefined;
  return new Date(createdAt + trialHours * 60 * 60 * 1_000).toISOString();
}

function emptySeat(supportedPeriods: ShareTokenPeriod[] = TOKEN_PERIODS): SeatDraft {
  return {
    parallelLimit: "",
    tokenLimit: "",
    tokenPeriod: supportedPeriods.includes("lifetime") ? "lifetime" : supportedPeriods[0] || "lifetime",
    paid: false,
    price: "",
    currency: "CNY",
    periodUnit: "month",
    periodCount: "1",
  };
}

function draftFromSeat(seat: ShareMarketSeat): SeatDraft {
  return {
    parallelLimit: seat.parallelLimit == null ? "" : String(seat.parallelLimit),
    tokenLimit: seat.tokenLimit == null ? "" : String(seat.tokenLimit),
    tokenPeriod: seat.tokenPeriod,
    paid: !seat.isFree,
    price: seat.priceMinor == null ? "" : (seat.priceMinor / 100).toFixed(2),
    currency: seat.currency || "CNY",
    periodUnit: (seat.periodUnit as SeatDraft["periodUnit"]) || "month",
    periodCount: String(seat.periodCount || 1),
  };
}

function parsePositiveOptional(value: string, label: string, t: TFn) {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1) {
    throw new Error(t("shareMarket.error.positiveInteger", { field: label }));
  }
  return parsed;
}

function seatInput(draft: SeatDraft, t: TFn): ShareMarketSeatInput {
  const parallelLimit = parsePositiveOptional(draft.parallelLimit, t("shareMarket.parallel"), t);
  const tokenLimit = parsePositiveOptional(draft.tokenLimit, t("shareMarket.tokens"), t);
  if (!draft.paid) return { parallelLimit, tokenLimit, tokenPeriod: draft.tokenPeriod };
  const price = draft.price.trim();
  const amount = Number(price);
  const periodCount = Number(draft.periodCount);
  const priceMinor = Math.round(amount * 100);
  if (!/^\d+(?:\.\d{1,2})?$/.test(price) || amount <= 0 || !Number.isSafeInteger(priceMinor)) {
    throw new Error(t("shareMarket.error.price"));
  }
  if (!Number.isSafeInteger(periodCount) || periodCount < 1 || periodCount > 365) {
    throw new Error(t("shareMarket.error.billingPeriod"));
  }
  const currency = draft.currency.trim().toUpperCase();
  if (currency !== "CNY" && currency !== "USD") {
    throw new Error(t("shareMarket.error.currency"));
  }
  return {
    parallelLimit,
    tokenLimit,
    tokenPeriod: draft.tokenPeriod,
    priceMinor,
    currency,
    periodUnit: draft.periodUnit,
    periodCount,
  };
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
  const patch = (value: Partial<SeatDraft>) => onChange({ ...draft, ...value });
  const periods = supportedPeriods?.length ? supportedPeriods : TOKEN_PERIODS;
  const selectTrigger = "min-h-10 w-full text-sm";
  return (
    <div className="grid min-w-0 gap-3">
      <div className="grid min-w-0 gap-3 sm:grid-cols-3">
        <label className="grid gap-1 text-xs text-slate-500">
          {t("shareMarket.parallel")}
          <input
            className={fieldClass()}
            inputMode="numeric"
            value={draft.parallelLimit}
            placeholder={t("shareMarket.dialog.unlimited")}
            onChange={(event) => patch({ parallelLimit: event.target.value })}
          />
        </label>
        <label className="grid gap-1 text-xs text-slate-500">
          {t("shareMarket.tokens")}
          <input
            className={fieldClass()}
            inputMode="numeric"
            value={draft.tokenLimit}
            placeholder={t("shareMarket.dialog.unlimited")}
            onChange={(event) => patch({ tokenLimit: event.target.value })}
          />
        </label>
        <label className="grid gap-1 text-xs text-slate-500">
          {t("shareMarket.period")}
          <CompactSelect
            value={draft.tokenPeriod}
            options={periods.map((period) => ({ value: period, label: t(`shareMarket.period.${period}`) }))}
            onChange={(value) => patch({ tokenPeriod: value as ShareTokenPeriod })}
            ariaLabel={t("shareMarket.period")}
            className="w-full"
            triggerClassName={selectTrigger}
          />
        </label>
      </div>
      <div className="grid grid-cols-2 gap-1 rounded-md bg-slate-100 p-1">
        <button
          type="button"
          className={`h-9 rounded-md text-sm font-medium ${!draft.paid ? "bg-white text-slate-900 shadow-sm" : "text-slate-500"}`}
          onClick={() => patch({ paid: false })}
        >
          {t("shareMarket.dialog.freeMode")}
        </button>
        <button
          type="button"
          className={`h-9 rounded-md text-sm font-medium ${draft.paid ? "bg-white text-slate-900 shadow-sm" : "text-slate-500"}`}
          onClick={() => patch({ paid: true })}
        >
          {t("shareMarket.dialog.paidMode")}
        </button>
      </div>
      {draft.paid ? (
        <div className="grid min-w-0 gap-3 sm:grid-cols-2">
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.dialog.amount")}
            <input
              className={fieldClass()}
              inputMode="decimal"
              value={draft.price}
              onChange={(event) => patch({ price: event.target.value })}
            />
          </label>
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.dialog.currency")}
            <CompactSelect
              value={draft.currency === "USD" ? "USD" : "CNY"}
              options={[
                { value: "CNY", label: "CNY" },
                { value: "USD", label: "USD" },
              ]}
              onChange={(value) => patch({ currency: value })}
              ariaLabel={t("shareMarket.dialog.currency")}
              className="w-full"
              triggerClassName={selectTrigger}
            />
          </label>
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.dialog.billingCount")}
            <input
              className={fieldClass()}
              inputMode="numeric"
              value={draft.periodCount}
              onChange={(event) => patch({ periodCount: event.target.value })}
            />
          </label>
          <label className="grid gap-1 text-xs text-slate-500">
            {t("shareMarket.dialog.billingUnit")}
            <CompactSelect
              value={draft.periodUnit}
              options={[
                { value: "day", label: t("shareMarket.dialog.day") },
                { value: "week", label: t("shareMarket.dialog.week") },
                { value: "month", label: t("shareMarket.dialog.month") },
              ]}
              onChange={(value) => patch({ periodUnit: value as SeatDraft["periodUnit"] })}
              ariaLabel={t("shareMarket.dialog.billingUnit")}
              className="w-full"
              triggerClassName={selectTrigger}
            />
          </label>
        </div>
      ) : null}
    </div>
  );
}

function AddListingDialog({
  open,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
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
    setSeats([emptySeat()]);
    getShareMarketOwnedShares()
      .then((items) => {
        const eligible = items.filter((item) => !item.alreadyListed && item.shareStatus === "active");
        setShares(eligible);
        setShareId(eligible[0]?.shareId || "");
        setSeats([emptySeat(eligible[0]?.supportedUserTokenPeriods)]);
      })
      .catch((reason) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setLoading(false));
  }, [open]);

  const selectedShare = shares.find((share) => share.shareId === shareId);
  const supportedPeriods = selectedShare?.supportedUserTokenPeriods;

  const submit = async () => {
    if (!shareId || busy) return;
    setBusy(true);
    setError("");
    try {
      await createShareMarketListing(shareId, seats.map((seat) => seatInput(seat, t)));
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
            {loading ? <div className="flex items-center gap-2 py-8 text-sm text-slate-500"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div> : null}
            {!loading ? (
              <>
                {shares.length === 0 ? <p className="text-sm text-slate-500">{t("shareMarket.dialog.noShares")}</p> : (
                  <>
                <label className="grid gap-1 text-xs text-slate-500">
                  {t("shareMarket.dialog.selectShare")}
                  <CompactSelect
                    value={shareId}
                    options={shares.map((share) => ({
                      value: share.shareId,
                      label: `${share.shareName} · ${share.appType}`,
                    }))}
                    onChange={(nextId) => {
                      const nextShare = shares.find((share) => share.shareId === nextId);
                      setShareId(nextId);
                      setSeats([emptySeat(nextShare?.supportedUserTokenPeriods)]);
                    }}
                    ariaLabel={t("shareMarket.dialog.selectShare")}
                    className="w-full"
                    triggerClassName="min-h-10 w-full text-sm"
                  />
                </label>
                  <div className="grid gap-3">
                    <div className="text-sm font-semibold text-slate-900">{t("shareMarket.dialog.seats")}</div>
                    {seats.map((seat, index) => (
                      <section key={index} className="grid gap-3 border-t border-slate-200 pt-4 first:border-0 first:pt-0">
                        <div className="flex items-center justify-between gap-3">
                          <div className="flex min-w-0 items-center gap-0.5">
                            <span className="text-sm font-medium">{t("shareMarket.seat", { position: index + 1 })}</span>
                            <Button
                              isIconOnly
                              size="sm"
                              variant="ghost"
                              className="h-7 w-7 min-w-7"
                              aria-label={t("shareMarket.copySeat")}
                              isDisabled={seats.length >= 20}
                              onClick={() =>
                                setSeats((items) => {
                                  if (items.length >= 20) return items;
                                  const clone: SeatDraft = { ...items[index] };
                                  return [...items.slice(0, index + 1), clone, ...items.slice(index + 1)];
                                })
                              }
                            >
                              <Copy className="h-3.5 w-3.5" />
                            </Button>
                          </div>
                          {seats.length > 1 ? (
                            <Button isIconOnly size="sm" variant="ghost" aria-label={t("common.delete")} onClick={() => setSeats((items) => items.filter((_, itemIndex) => itemIndex !== index))}>
                              <X className="h-4 w-4" />
                            </Button>
                          ) : null}
                        </div>
                        <SeatFields draft={seat} supportedPeriods={supportedPeriods} onChange={(next) => setSeats((items) => items.map((item, itemIndex) => itemIndex === index ? next : item))} />
                      </section>
                    ))}
                    {seats.length < 20 ? (
                      <Button variant="outline" onClick={() => setSeats((items) => [...items, emptySeat(supportedPeriods)])}>
                        <Plus className="h-4 w-4" />{t("shareMarket.dialog.addSeat")}
                      </Button>
                    ) : null}
                  </div>
                  </>
                )}
              </>
            ) : null}
            {error ? <p className="text-sm text-red-600">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="outline" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={!shareId || busy || loading} onClick={() => void submit()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
              {t("shareMarket.dialog.create")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function SeatDialog({
  open,
  listingId,
  seat,
  supportedPeriods,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  listingId: string;
  seat?: ShareMarketSeat;
  supportedPeriods?: ShareTokenPeriod[];
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { t } = useLocaleText();
  const [draft, setDraft] = React.useState<SeatDraft>(emptySeat());
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    if (!open) return;
    setDraft(seat ? draftFromSeat(seat) : emptySeat(supportedPeriods));
    setError("");
  }, [open, seat, supportedPeriods]);
  const save = async () => {
    setBusy(true);
    setError("");
    try {
      const input = seatInput(draft, t);
      if (seat) await updateShareMarketSeat(seat.id, input, seat.offerRevision);
      else await addShareMarketSeat(listingId, input);
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
        <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{seat ? t("common.edit") : t("shareMarket.addSeat")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid gap-4"><SeatFields draft={draft} supportedPeriods={supportedPeriods} onChange={setDraft} />{error ? <p className="text-sm text-red-600">{error}</p> : null}</Modal.Body>
          <Modal.Footer>
            <Button variant="outline" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy} onClick={() => void save()}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("common.save")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function shareOpenUrl(subdomain?: string | null) {
  return subdomainTunnelUrl(subdomain);
}

type MarketLayout = "seats" | "shares";
const LAYOUT_STORAGE_KEY = "cc-switch.shareMarket.layout";
const ONLINE_FILTER_KEY = "cc-switch.shareMarket.onlineFilter";
const SHARE_FILTER_KEY = "cc-switch.shareMarket.shareFilter";
const STATUS_FILTER_KEY = "cc-switch.shareMarket.statusFilter";
const OWNER_FILTER_KEY = "cc-switch.shareMarket.ownerFilter";
const SORT_PREFS_KEY = "cc-switch.shareMarket.seatSort";

function listingProviderLines(provider: ShareUpstreamProvider | undefined, locale: AppLocale, unavailable: string) {
  if (!provider) {
    return { identity: unavailable, quota: "-", models: "-" };
  }
  return {
    identity: providerStatusIdentity(provider),
    quota: providerQuotaStatusLine(provider, locale),
    models: providerActualModelNames(provider),
  };
}

function listingProviderHeader(provider: ShareUpstreamProvider | undefined, unavailable: string) {
  const name = provider?.providerName?.trim()
    || provider?.providerType?.trim()
    || provider?.kind?.trim()
    || unavailable;
  const tierCandidate = provider?.subscriptionLevel?.trim() || provider?.quota?.plan?.trim();
  const tier = tierCandidate && tierCandidate.toLocaleLowerCase() !== name.toLocaleLowerCase()
    ? tierCandidate
    : undefined;
  const upstreamModels = Array.from(new Set(
    (provider?.models || [])
      .filter((item) => String(item.slot || "").trim().toLocaleLowerCase() !== "available")
      .map((item) => String(item.actualModel || "").trim())
      .filter(Boolean),
  ));
  return { name, tier, upstreamModels, hasProvider: !!provider };
}

function listingProviderTitle(provider: ShareUpstreamProvider | undefined, unavailable: string, t: TFn) {
  const header = listingProviderHeader(provider, unavailable);
  const identity = header.tier ? `${header.name} [${header.tier}]` : header.name;
  const strategy = !header.hasProvider
    ? t("shareMarket.modelUnknown")
    : header.upstreamModels.length > 0
    ? t("shareMarket.upstreamModel", { model: header.upstreamModels.join(" / ") })
    : t("shareMarket.modelPassthrough");
  return `${identity} · ${strategy}`;
}

function shareMarketTabTone(active: boolean) {
  return active ? "bg-white font-medium text-foreground shadow-sm" : "text-slate-700";
}

function LayoutModeToggle({
  layout,
  onChange,
  seatsLabel,
  sharesLabel,
  ariaLabel,
}: {
  layout: MarketLayout;
  onChange: (next: MarketLayout) => void;
  seatsLabel: string;
  sharesLabel: string;
  ariaLabel: string;
}) {
  return (
    <div
      role="group"
      aria-label={ariaLabel}
      className="inline-flex shrink-0 overflow-hidden rounded-lg border border-slate-200 bg-slate-100 p-0.5 text-[11px]"
    >
      <button
        type="button"
        onClick={() => onChange("seats")}
        className={cn(
          "rounded-md px-2.5 py-1.5 transition-colors",
          layout === "seats" ? "bg-white font-semibold text-foreground shadow-sm" : "font-medium text-slate-500 hover:text-slate-700",
        )}
        aria-pressed={layout === "seats"}
      >
        {seatsLabel}
      </button>
      <button
        type="button"
        onClick={() => onChange("shares")}
        className={cn(
          "rounded-md px-2.5 py-1.5 transition-colors",
          layout === "shares" ? "bg-white font-semibold text-foreground shadow-sm" : "font-medium text-slate-500 hover:text-slate-700",
        )}
        aria-pressed={layout === "shares"}
      >
        {sharesLabel}
      </button>
    </div>
  );
}

function formatShareLimit(value?: number | null) {
  if (value == null) return "—";
  if (Number(value) < 0) return "∞";
  return compactTokens(value);
}

function formatPrice(seat: Pick<ShareMarketSeat, "isFree" | "priceMinor" | "currency" | "periodUnit" | "periodCount">, free: string) {
  if (seat.isFree || seat.priceMinor == null) return free;
  const amount = new Intl.NumberFormat(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(seat.priceMinor / 100);
  return `${amount} ${seat.currency || ""} / ${seat.periodCount || 1} ${seat.periodUnit || ""}`;
}

function statusLabel(status: string, t: ReturnType<typeof useLocaleText>["t"]) {
  if (status === "available") return t("shareMarket.available");
  if (status === "occupied") return t("shareMarket.occupied");
  if (status === "revoking") return t("shareMarket.revoking");
  if (status === "retired") return t("shareMarket.retired");
  if (status === "disabled") return t("shareMarket.disabled");
  return t("shareMarket.pending");
}

function seatStatusLabel(seat: ShareMarketSeat, subscription: ShareMarketSubscription | undefined, t: TFn) {
  if (seat.readOnly || seat.status === "retired") {
    return subscription?.status === "grant_failed"
      ? subscriptionStatusLabel(subscription.status, t)
      : t("shareMarket.retired");
  }
  return subscription ? subscriptionStatusLabel(subscription.status, t) : statusLabel(seat.status, t);
}

function subscriptionStatusLabel(status: string, t: TFn) {
  const keys = {
    grant_pending: "shareMarket.subscription.grantPending",
    active_free: "shareMarket.subscription.activeFree",
    trial_payment_due: "shareMarket.subscription.trialPaymentDue",
    active_paid: "shareMarket.subscription.activePaid",
    renewal_due: "shareMarket.subscription.renewalDue",
    revoke_pending: "shareMarket.subscription.revokePending",
    revoke_failed: "shareMarket.subscription.revokeFailed",
    grant_failed: "shareMarket.subscription.grantFailed",
    released: "shareMarket.subscription.released",
  } as const;
  const key = keys[status as keyof typeof keys];
  return key ? t(key) : status.replaceAll("_", " ");
}

function readStoredLayout(): MarketLayout {
  if (typeof window === "undefined") return "seats";
  try {
    const value = window.localStorage.getItem(LAYOUT_STORAGE_KEY);
    return value === "shares" ? "shares" : "seats";
  } catch {
    return "seats";
  }
}

function writeStoredLayout(layout: MarketLayout) {
  try {
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, layout);
  } catch {
    // ignore quota / private mode
  }
}

type SeatTableRow = {
  key: string;
  listing: ShareMarketListing;
  seat: ShareMarketSeat;
  subscription?: ShareMarketSubscription;
};

function isTerminalSubscription(status: string) {
  return ["released", "grant_failed"].includes(status);
}

function guestCanSeeAvailable(listing: ShareMarketListing, seat: ShareMarketSeat) {
  return listing.status === "active" && listing.shareStatus === "active" && seat.status === "available";
}

/** Rent/login CTA for non-owners on available seats (same rules in seats + Share layouts). */
function seatRentAction(
  listing: ShareMarketListing,
  seat: ShareMarketSeat,
  authed: boolean,
): "rent" | "login" | null {
  if (listing.isOwner) return null;
  if (listing.status !== "active" || seat.status !== "available") return null;
  if (seat.canRent) return "rent";
  if (!authed && guestCanSeeAvailable(listing, seat)) return "login";
  // canRent can lag false while the seat is clearly open; still offer rent/login
  // and let the API enforce block / already-renting / direct-grant rules.
  if (listing.shareStatus === "active") return authed ? "rent" : "login";
  return null;
}

function buildSeatRows(
  catalog: ShareMarketCatalog | null,
  tab: MarketTab,
  authed: boolean,
): SeatTableRow[] {
  if (!catalog) return [];
  if (tab === "rentals") {
    const bySeat = new Map<string, { listing?: ShareMarketListing; seat?: ShareMarketSeat }>();
    for (const listing of catalog.listings) {
      for (const seat of listing.seats) {
        bySeat.set(seat.id, { listing, seat });
      }
    }
    return catalog.mySubscriptions.map((subscription) => {
      const matched = bySeat.get(subscription.seatId);
      const listing =
        matched?.listing ||
        ({
          id: subscription.listingId,
          shareId: subscription.shareId,
          shareName: subscription.shareName,
          appType: subscription.appType,
          ownerEmail: subscription.ownerEmail,
          status: "active",
          shareStatus: "active",
          subdomain: subscription.subdomain,
          shareOnline: !!subscription.shareOnline,
          isOwner: false,
          contacts: subscription.contacts,
          paymentMethods: subscription.paymentMethods,
          supportedUserTokenPeriods: [],
          seats: [],
          createdAt: subscription.createdAt,
          updatedAt: subscription.updatedAt,
        } satisfies ShareMarketListing);
      const seat =
        matched?.seat ||
        ({
          id: subscription.seatId,
          position: 0,
          status: isTerminalSubscription(subscription.status) ? "retired" : "occupied",
          offerRevision: subscription.offerRevision,
          isFree: subscription.priceMinor == null,
          canRent: false,
          readOnly: isTerminalSubscription(subscription.status),
          retiredAt: subscription.releasedAt,
          parallelLimit: undefined,
          tokenLimit: undefined,
          tokenPeriod: "lifetime" as const,
          priceMinor: subscription.priceMinor,
          currency: subscription.currency,
          periodUnit:
            subscription.periodUnit === "day" ||
            subscription.periodUnit === "week" ||
            subscription.periodUnit === "month"
              ? subscription.periodUnit
              : undefined,
          periodCount: subscription.periodCount,
          subscription,
        } satisfies ShareMarketSeat);
      return {
        key: `rental-${subscription.id}`,
        listing,
        seat,
        subscription,
      };
    });
  }

  const listings = tab === "mine" ? catalog.listings.filter((listing) => listing.isOwner) : catalog.listings;
  const rows: SeatTableRow[] = [];
  for (const listing of listings) {
    for (const seat of listing.seats) {
      // Seats and Share layouts share the same catalog content; only the presentation differs.
      rows.push({ key: seat.id, listing, seat, subscription: seat.subscription });
    }
  }

  if (tab === "all" && authed) {
    const seen = new Set(rows.map((row) => row.seat.id));
    for (const subscription of catalog.mySubscriptions) {
      if (seen.has(subscription.seatId)) continue;
      const listing = catalog.listings.find((item) => item.id === subscription.listingId);
      const seat = listing?.seats.find((item) => item.id === subscription.seatId);
      if (!listing || !seat) continue;
      rows.push({ key: seat.id, listing, seat, subscription });
      seen.add(seat.id);
    }
  }

  return rows;
}

function ProviderExpandPanel({
  listing,
  locale,
  t,
}: {
  listing: ShareMarketListing;
  locale: AppLocale;
  t: TFn;
}) {
  const shareUrl = shareOpenUrl(listing.subdomain);
  const provider = listingProviderLines(listing.upstreamProvider, locale, t("dashboard.providerUnavailable"));
  const tokensUsed = listing.tokensUsed || 0;
  const tokenLimit = listing.tokenLimit;
  const usagePercent =
    tokenLimit != null && Number(tokenLimit) > 0
      ? Math.min(100, Math.max(0, (tokensUsed / Number(tokenLimit)) * 100))
      : null;
  return (
    <div className="grid gap-3 border-t border-slate-100 bg-slate-50/80 px-4 py-3 text-[11px] text-slate-700 sm:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)]">
      <div className="grid min-w-0 gap-1 rounded-md border border-slate-200 bg-white px-2.5 py-2">
        <div className="min-w-0 truncate font-semibold" title={provider.quota}>
          {provider.quota && provider.quota !== "-" ? provider.quota : provider.identity}
        </div>
        <div className="min-w-0 truncate text-slate-500" title={provider.identity}>{provider.identity}</div>
        {provider.models && provider.models !== "-" ? (
          <div className="min-w-0 truncate text-slate-500" title={provider.models}>{provider.models}</div>
        ) : null}
        {shareUrl ? (
          <a
            href={shareUrl}
            target="_blank"
            rel="noreferrer"
            className="mt-1 inline-flex min-w-0 items-center gap-1 truncate font-medium text-slate-900 underline-offset-2 hover:underline"
            title={shareUrl}
          >
            <span className="truncate">{shareUrl}</span>
            <ExternalLink className="h-3 w-3 shrink-0 text-slate-400" aria-hidden />
          </a>
        ) : (
          <span className="mt-1 text-slate-500">{t("shareMarket.shareUrl")}: —</span>
        )}
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div className="min-w-0">
          <span className="block text-slate-500">{t("dashboard.usage")}</span>
          <strong className="tabular-nums text-slate-900">
            {compactTokens(tokensUsed)} / {formatShareLimit(tokenLimit)}
          </strong>
          {usagePercent != null ? (
            <div className="mt-1 h-1 overflow-hidden rounded-full bg-slate-100">
              <div className={`h-full rounded-full ${usagePercent >= 90 ? "bg-rose-500" : "bg-primary/70"}`} style={{ width: `${usagePercent}%` }} />
            </div>
          ) : null}
        </div>
        <div className="min-w-0">
          <span className="block text-slate-500">{t("dashboard.parallel")}</span>
          <strong className="tabular-nums text-slate-900">{formatShareLimit(listing.parallelLimit)}</strong>
        </div>
        <div className="min-w-0 col-span-2">
          <div className="flex items-center gap-0.5 text-slate-500">
            <span>{t("shareMarket.owner")}</span>
            <ProviderContactButton contacts={listing.contacts} paymentMethods={listing.paymentMethods} />
          </div>
          <span className="block truncate text-slate-600" title={listing.ownerEmail}>{listing.ownerEmail}</span>
        </div>
      </div>
    </div>
  );
}

function PaymentDialog({
  subscription,
  onClose,
  onPaid,
}: {
  subscription: ShareMarketSubscription | null;
  onClose: () => void;
  onPaid: () => void;
}) {
  const { t } = useLocaleText();
  const [confirmed, setConfirmed] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const invoice = subscription?.openInvoice;
  const paymentMethods = subscription?.paymentMethods || [];
  const hasPaymentMethods = paymentMethods.length > 0;
  const confirmationRevision = [
    subscription?.id,
    invoice?.id,
    invoice?.amountMinor,
    subscription?.offerRevision,
    subscription?.paymentProfileUpdatedAt,
  ].join(":");
  React.useEffect(() => {
    setConfirmed(false);
    setError("");
  }, [confirmationRevision]);
  const submit = async () => {
    if (!subscription || !invoice || !subscription.paymentProfileUpdatedAt) return;
    setBusy(true);
    setError("");
    try {
      await declareShareMarketPaid(subscription.id, {
        invoiceId: invoice.id,
        offerRevision: subscription.offerRevision,
        amountMinorConfirmed: invoice.amountMinor,
        paymentProfileUpdatedAt: subscription.paymentProfileUpdatedAt,
      });
      onClose();
      onPaid();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal.Backdrop isOpen={!!subscription} onOpenChange={(next) => !next && !busy && onClose()}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("shareMarket.paymentDetails")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[70vh] gap-4 overflow-y-auto">
            {invoice ? (
              <div className="flex items-baseline justify-between gap-3 border-b border-slate-200 pb-3">
                <span className="text-sm text-slate-500">{subscription?.shareName}</span>
                <strong className="text-lg">{(invoice.amountMinor / 100).toFixed(2)} {invoice.currency}</strong>
              </div>
            ) : null}
            {subscription && !invoice ? (
              <div className="flex items-center gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">
                <Loader2 className="h-4 w-4 shrink-0 animate-spin" />
                <span>{t("shareMarket.paymentPreparing")}</span>
              </div>
            ) : null}
            <div className="grid gap-3">
              <ProviderContactsList contacts={subscription?.contacts} />
              <ProviderPaymentMethodsList paymentMethods={paymentMethods} />
              {!hasPaymentMethods ? <p className="text-sm text-slate-500">{t("shareMarket.noPaymentMethods")}</p> : null}
            </div>
            <Checkbox isDisabled={!invoice} isSelected={confirmed} onChange={setConfirmed}>
              <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
              <Checkbox.Content><span className="text-sm text-slate-700">{t("shareMarket.confirmPaid")}</span></Checkbox.Content>
            </Checkbox>
            {error ? <p className="text-sm text-red-600">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="outline" isDisabled={busy} onClick={onClose}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={!confirmed || !invoice || !hasPaymentMethods || busy || !subscription?.paymentProfileUpdatedAt} onClick={() => void submit()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <CircleDollarSign className="h-4 w-4" />}{t("shareMarket.declarePaid")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function SharePaymentAction({
  subscription,
  trialHours,
  onPay,
}: {
  subscription: ShareMarketSubscription;
  trialHours: number;
  onPay: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [, refreshCountdown] = React.useState(0);
  const countdownId = React.useId();
  const deadline = sharePaymentDeadline(subscription, trialHours);

  React.useEffect(() => {
    if (!deadline) return;
    const timer = window.setInterval(() => refreshCountdown((value) => value + 1), 30_000);
    return () => window.clearInterval(timer);
  }, [deadline]);

  const countdown = formatBillingCountdown(deadline, locale);
  return (
    <div className="grid justify-items-center gap-0.5">
      <Button
        size="sm"
        variant="primary"
        aria-describedby={countdown ? countdownId : undefined}
        onClick={onPay}
      >
        <CircleDollarSign className="h-4 w-4" />
        {t("shareMarket.goPay")}
      </Button>
      {countdown ? (
        <span id={countdownId} className="max-w-28 text-center text-[10px] leading-4 text-amber-700">
          {t("shareMarket.paymentCountdown", { countdown })}
        </span>
      ) : null}
    </div>
  );
}

export function ShareMarketPage() {
  const { t, locale } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const [catalog, setCatalog] = React.useState<ShareMarketCatalog | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [tab, setTabState] = React.useState<MarketTab>(() => marketTabFromParam(searchParams.get("tab")));
  const [focusShareId, setFocusShareId] = React.useState(() => searchParams.get("focus") || "");
  const [layout, setLayoutState] = React.useState<MarketLayout>("seats");
  const [expandedSeatIds, setExpandedSeatIds] = React.useState<Set<string>>(() => new Set());
  const [query, setQuery] = React.useState("");
  const [onlineFilters, setOnlineFilters] = usePersistentState<string[]>(ONLINE_FILTER_KEY, []);
  const [shareFilters, setShareFilters] = usePersistentState<string[]>(SHARE_FILTER_KEY, []);
  const [statusFilters, setStatusFilters] = usePersistentState<string[]>(STATUS_FILTER_KEY, []);
  const [ownerFilters, setOwnerFilters] = usePersistentState<string[]>(OWNER_FILTER_KEY, []);
  const [sortPrefs, setSortPrefs] = usePersistentState<SeatSortPrefs>(SORT_PREFS_KEY, CLEARED_SEAT_SORT);
  const [addOpen, setAddOpen] = React.useState(false);
  const [seatDialog, setSeatDialog] = React.useState<{ listingId: string; seat?: ShareMarketSeat; supportedPeriods?: ShareTokenPeriod[] } | null>(null);
  const [paymentSubscriptionId, setPaymentSubscriptionId] = React.useState("");
  const [confirmAction, setConfirmAction] = React.useState<ConfirmAction | null>(null);
  const [busyId, setBusyId] = React.useState("");
  const focusedRef = React.useRef<string>("");

  React.useEffect(() => {
    setLayoutState(readStoredLayout());
  }, []);

  React.useEffect(() => {
    const syncFromHistory = () => {
      const params = new URLSearchParams(window.location.search);
      setTabState(marketTabFromParam(params.get("tab")));
      setFocusShareId(params.get("focus") || "");
    };
    window.addEventListener("popstate", syncFromHistory);
    return () => window.removeEventListener("popstate", syncFromHistory);
  }, []);

  const setLayout = React.useCallback((next: MarketLayout) => {
    setLayoutState(next);
    writeStoredLayout(next);
  }, []);

  const setTab = React.useCallback(
    (next: MarketTab) => {
      setTabState(next);
      replaceMarketQuery(next, focusShareId);
    },
    [focusShareId],
  );

  const openShareManage = React.useCallback(
    (shareId: string) => {
      setLayout("shares");
      focusedRef.current = "";
      setTabState("mine");
      setFocusShareId(shareId);
      replaceMarketQuery("mine", shareId);
    },
    [setLayout],
  );

  const load = React.useCallback(async (silent = false) => {
    if (!silent) setLoading(true);
    try {
      setCatalog(await getShareMarketCatalog());
      setError("");
    } catch (reason) {
      if (!silent) setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (!silent) setLoading(false);
    }
  }, []);

  React.useEffect(() => { void load(); }, [load, session?.user?.id]);
  const paymentSubscription = React.useMemo(
    () => catalog?.mySubscriptions.find((subscription) => subscription.id === paymentSubscriptionId) || null,
    [catalog, paymentSubscriptionId],
  );
  React.useEffect(() => {
    if (!paymentSubscriptionId || !catalog) return;
    if (!paymentSubscription || !canOpenSharePayment(paymentSubscription)) {
      setPaymentSubscriptionId("");
    }
  }, [catalog, paymentSubscription, paymentSubscriptionId]);
  React.useEffect(() => {
    const waitingForPayment = !!paymentSubscription
      && paymentSubscription.status === "grant_pending"
      && !paymentSubscription.openInvoice;
    const timer = window.setInterval(() => {
      if (waitingForPayment || (!addOpen && !seatDialog && !paymentSubscriptionId && !confirmAction && !busyId)) {
        void load(true);
      }
    }, waitingForPayment ? 1_000 : 5_000);
    return () => window.clearInterval(timer);
  }, [addOpen, busyId, confirmAction, load, paymentSubscription, paymentSubscriptionId, seatDialog]);

  const act = async (id: string, action: () => Promise<unknown>) => {
    if (busyId) return;
    setBusyId(id);
    try {
      await action();
      await load(true);
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusyId("");
    }
  };

  const runConfirmedAction = async () => {
    if (!confirmAction) return;
    await act(confirmAction.id, confirmAction.run);
    setConfirmAction(null);
  };

  const listings = React.useMemo(() => {
    if (!catalog) return [];
    const base = tab === "mine" ? catalog.listings.filter((listing) => listing.isOwner) : catalog.listings;
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return base;
    return base.filter((listing) => {
      const providerTitle = listingProviderTitle(
        listing.upstreamProvider,
        t("dashboard.providerUnavailable"),
        t,
      ).toLocaleLowerCase();
      const haystack = [
        listing.shareName,
        listing.ownerEmail,
        listing.subdomain,
        providerTitle,
        ...listing.seats.map((seat) => seat.subscription?.renterEmail),
      ]
        .filter(Boolean)
        .join(" ")
        .toLocaleLowerCase();
      return haystack.includes(normalized);
    });
  }, [catalog, query, t, tab]);

  const seatRows = React.useMemo(() => {
    const rows = buildSeatRows(catalog, tab, authed).map((row) => {
      const statusKey = row.subscription?.status || row.seat.status;
      const providerTitle = listingProviderTitle(
        row.listing.upstreamProvider,
        t("dashboard.providerUnavailable"),
        t,
      );
      const searchText = [
        row.listing.shareName,
        row.listing.ownerEmail,
        row.listing.subdomain,
        providerTitle,
        row.subscription?.renterEmail,
        statusKey,
        String(row.seat.position),
      ]
        .filter(Boolean)
        .join(" ")
        .toLocaleLowerCase();
      return { ...row, statusKey, searchText };
    });
    const normalized = query.trim().toLocaleLowerCase();
    const filtered = rows.filter((row) => {
      if (normalized && !row.searchText.includes(normalized)) return false;
      if (onlineFilters.length) {
        const onlineKey = row.listing.shareOnline ? "online" : "offline";
        if (!onlineFilters.includes(onlineKey)) return false;
      }
      if (shareFilters.length && !shareFilters.includes(row.listing.shareName || row.listing.shareId)) {
        return false;
      }
      if (statusFilters.length && !statusFilters.includes(row.statusKey)) return false;
      if (ownerFilters.length && !ownerFilters.includes(row.listing.ownerEmail)) return false;
      return true;
    });
    return sortSeatRows(filtered, sortPrefs);
  }, [
    authed,
    catalog,
    onlineFilters,
    ownerFilters,
    query,
    shareFilters,
    sortPrefs,
    statusFilters,
    t,
    tab,
  ]);

  const onlineOptions = React.useMemo(
    () => [
      { value: "online", label: t("shareMarket.online") },
      { value: "offline", label: t("shareMarket.offline") },
    ],
    [t],
  );
  const shareOptions = React.useMemo(() => {
    const names = new Set<string>();
    for (const row of buildSeatRows(catalog, tab, authed)) {
      names.add(row.listing.shareName || row.listing.shareId);
    }
    return Array.from(names)
      .sort((a, b) => a.localeCompare(b))
      .map((value) => ({ value, label: value }));
  }, [authed, catalog, tab]);
  const statusOptions = React.useMemo(() => {
    const labels = new Map<string, string>();
    for (const row of buildSeatRows(catalog, tab, authed)) {
      const key = row.subscription?.status || row.seat.status;
      labels.set(key, seatStatusLabel(row.seat, row.subscription, t));
    }
    return Array.from(labels)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([value, label]) => ({
        value,
        label,
      }));
  }, [authed, catalog, t, tab]);
  const ownerOptions = React.useMemo(() => {
    const emails = new Set<string>();
    for (const row of buildSeatRows(catalog, tab, authed)) {
      if (row.listing.ownerEmail) emails.add(row.listing.ownerEmail);
    }
    return Array.from(emails)
      .sort((a, b) => a.localeCompare(b))
      .map((value) => ({ value, label: value }));
  }, [authed, catalog, tab]);
  const hasSeatFilters =
    onlineFilters.length > 0 || shareFilters.length > 0 || statusFilters.length > 0 || ownerFilters.length > 0;

  const onSeatSort = React.useCallback((key: SeatSortKey) => {
    setSortPrefs((current) => toggleSeatSort(current, key));
  }, [setSortPrefs]);

  const clearSeatFilters = React.useCallback(() => {
    setOnlineFilters([]);
    setShareFilters([]);
    setStatusFilters([]);
    setOwnerFilters([]);
  }, [setOnlineFilters, setOwnerFilters, setShareFilters, setStatusFilters]);

  const date = (value?: string) => value ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value)) : "";
  const tokenPeriod = (period: ShareTokenPeriod) => t(`shareMarket.period.${period}`);

  React.useEffect(() => {
    if (!focusShareId || !catalog || layout !== "shares" || focusedRef.current === focusShareId) return;
    const target =
      document.querySelector(`[data-share-id="${CSS.escape(focusShareId)}"]`) ||
      document.querySelector(`[data-subscription-share-id="${CSS.escape(focusShareId)}"]`);
    if (target instanceof HTMLElement) {
      target.scrollIntoView({ behavior: "smooth", block: "center" });
      focusedRef.current = focusShareId;
    }
  }, [catalog, focusShareId, layout, tab]);

  const toggleExpanded = (seatId: string) => {
    setExpandedSeatIds((current) => {
      const next = new Set(current);
      if (next.has(seatId)) next.delete(seatId);
      else next.add(seatId);
      return next;
    });
  };

  const renderSubscription = (subscription: ShareMarketSubscription, ownerView = false) => {
    const openUrl = shareOpenUrl(subscription.subdomain);
    return (
    <div
      key={subscription.id}
      data-subscription-share-id={subscription.shareId}
      className="grid gap-3 border-b border-slate-200 py-4 last:border-0 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center"
    >
      <div className="min-w-0">
        <div className="flex min-w-0 items-center gap-2">
          <ShareAppLogo app={(subscription.appType === "claude" || subscription.appType === "gemini" ? subscription.appType : "codex")} size={18} />
          <strong className="truncate text-sm">{subscription.shareName}</strong>
          <span className="rounded-sm bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600">{subscriptionStatusLabel(subscription.status, t)}</span>
          {subscription.shareOnline != null ? (
            <span className={`rounded-sm px-1.5 py-0.5 text-xs ${subscription.shareOnline ? "bg-emerald-50 text-emerald-700" : "bg-slate-100 text-slate-500"}`}>
              {subscription.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
            </span>
          ) : null}
        </div>
        <div className="mt-1 flex min-w-0 items-center gap-0.5">
          <p className="min-w-0 truncate text-xs text-slate-500">{ownerView ? subscription.renterEmail : subscription.ownerEmail}</p>
          {!ownerView ? <ProviderContactButton contacts={subscription.contacts} paymentMethods={subscription.paymentMethods} /> : null}
        </div>
        <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-slate-600">
          {subscription.trialEndsAt && subscription.status === "trial_payment_due" ? <span>{t("shareMarket.trialEnds", { time: date(subscription.trialEndsAt) })}</span> : null}
          {subscription.paymentDeadline ? <span>{t("shareMarket.paymentDeadline", { time: date(subscription.paymentDeadline) })}</span> : null}
          {subscription.currentPeriodEnd ? <span>{t("shareMarket.periodEnds", { time: date(subscription.currentPeriodEnd) })}</span> : null}
        </div>
      </div>
      <div className="flex flex-wrap items-start justify-start gap-2 sm:justify-end">
        {openUrl ? (
          <Button size="sm" variant="outline" onClick={() => window.open(openUrl, "_blank", "noopener,noreferrer")}>
            <ExternalLink className="h-4 w-4" />{t("shareMarket.openShare")}
          </Button>
        ) : null}
        {!ownerView && canOpenSharePayment(subscription) ? (
          <SharePaymentAction
            subscription={subscription}
            trialHours={catalog?.trialHours || 0}
            onPay={() => setPaymentSubscriptionId(subscription.id)}
          />
        ) : null}
        {!ownerView && subscription.canRelease ? <Button size="sm" variant="outline" isDisabled={!!busyId} onClick={() => setConfirmAction({ id: subscription.id, title: t("shareMarket.confirm.releaseTitle"), description: t("shareMarket.confirm.releaseDescription", { share: subscription.shareName }), confirmLabel: t("shareMarket.release"), tone: "warning", run: () => releaseShareMarketSubscription(subscription.id) })}><RotateCcw className="h-4 w-4" />{t("shareMarket.release")}</Button> : null}
        {ownerView && subscription.canForceRevoke ? (
          <>
            <Button size="sm" variant="outline" isDisabled={!!busyId} onClick={() => setConfirmAction({ id: subscription.id, title: t("shareMarket.confirm.revokeTitle"), description: t("shareMarket.confirm.revokeDescription", { email: subscription.renterEmail }), confirmLabel: t("shareMarket.forceRevoke"), tone: "warning", run: () => forceRevokeShareMarketSubscription(subscription.id, { blockUser: false }) })}>{t("shareMarket.forceRevoke")}</Button>
            <Button size="sm" variant="danger" isDisabled={!!busyId} onClick={() => setConfirmAction({ id: subscription.id, title: t("shareMarket.confirm.blockTitle"), description: t("shareMarket.confirm.blockDescription", { email: subscription.renterEmail }), confirmLabel: t("shareMarket.blockAndRevoke"), tone: "danger", run: () => forceRevokeShareMarketSubscription(subscription.id, { blockUser: true }) })}><UserRoundX className="h-4 w-4" />{t("shareMarket.blockAndRevoke")}</Button>
          </>
        ) : null}
      </div>
    </div>
    );
  };

  const renderSeatActions = (
    listing: ShareMarketListing,
    seat: ShareMarketSeat,
    subscription?: ShareMarketSubscription,
  ) => {
    const rentAction = seatRentAction(listing, seat, authed);
    const canEdit = listing.isOwner && !seat.readOnly && seat.status === "available";
    const canDelete = listing.isOwner
      && !seat.readOnly
      && (seat.status === "available" || seat.status === "disabled");
    return (
      <div className="flex items-start justify-end gap-1">
        {rentAction === "rent" ? (
          <Button
            size="sm"
            variant="primary"
            isDisabled={!!busyId}
            onClick={() => {
              if (!listing.shareOnline) toast.info(t("shareMarket.rentOfflineHint"));
              void act(seat.id, () => rentShareMarketSeat(seat.id, seat.offerRevision));
            }}
          >
            {busyId === seat.id ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            {t("shareMarket.rent")}
          </Button>
        ) : null}
        {rentAction === "login" ? (
          <Button size="sm" variant="outline" onClick={() => window.dispatchEvent(new Event("router-open-login"))}>
            {t("nav.login")}
          </Button>
        ) : null}
        {subscription && canOpenSharePayment(subscription) ? (
          <SharePaymentAction
            subscription={subscription}
            trialHours={catalog?.trialHours || 0}
            onPay={() => setPaymentSubscriptionId(subscription.id)}
          />
        ) : null}
        {subscription?.canRelease ? (
          <Button
            size="sm"
            variant="outline"
            isDisabled={!!busyId}
            onClick={() => setConfirmAction({
              id: subscription.id,
              title: t("shareMarket.confirm.releaseTitle"),
              description: t("shareMarket.confirm.releaseDescription", { share: subscription.shareName }),
              confirmLabel: t("shareMarket.release"),
              tone: "warning",
              run: () => releaseShareMarketSubscription(subscription.id),
            })}
          >
            <RotateCcw className="h-4 w-4" />
            {t("shareMarket.release")}
          </Button>
        ) : null}
        {listing.isOwner ? (
          <Dropdown>
            <Dropdown.Trigger className="shrink-0 outline-none">
              <Button
                isIconOnly
                size="sm"
                variant="ghost"
                className="h-8 w-8 min-w-8"
                isDisabled={!!busyId}
                aria-label={t("shareMarket.seatActions")}
              >
                {busyId === `copy-${seat.id}` ? <Loader2 className="h-4 w-4 animate-spin" /> : <MoreHorizontal className="h-4 w-4" />}
              </Button>
            </Dropdown.Trigger>
            <Dropdown.Popover placement="bottom right">
              <Dropdown.Menu aria-label={t("shareMarket.seatActions")}>
                {canEdit ? (
                  <Dropdown.Item
                    id={`edit-${seat.id}`}
                    onAction={() => setSeatDialog({
                      listingId: listing.id,
                      seat,
                      supportedPeriods: listing.supportedUserTokenPeriods,
                    })}
                  >
                    <Pencil className="h-4 w-4" />
                    {t("common.edit")}
                  </Dropdown.Item>
                ) : null}
                <Dropdown.Item
                  id={`copy-${seat.id}`}
                  onAction={() => void act(`copy-${seat.id}`, () =>
                    addShareMarketSeat(listing.id, seatInput(draftFromSeat(seat), t)))}
                >
                  <Copy className="h-4 w-4" />
                  {t("shareMarket.createFromSeat")}
                </Dropdown.Item>
                {subscription?.canForceRevoke ? (
                  <Dropdown.Item
                    id={`revoke-${subscription.id}`}
                    onAction={() => setConfirmAction({
                      id: subscription.id,
                      title: t("shareMarket.confirm.revokeTitle"),
                      description: t("shareMarket.confirm.revokeDescription", { email: subscription.renterEmail }),
                      confirmLabel: t("shareMarket.forceRevoke"),
                      tone: "warning",
                      run: () => forceRevokeShareMarketSubscription(subscription.id, { blockUser: false }),
                    })}
                  >
                    <RotateCcw className="h-4 w-4" />
                    {t("shareMarket.forceRevoke")}
                  </Dropdown.Item>
                ) : null}
                {subscription?.canForceRevoke ? (
                  <Dropdown.Item
                    id={`block-${subscription.id}`}
                    className="text-destructive"
                    onAction={() => setConfirmAction({
                      id: subscription.id,
                      title: t("shareMarket.confirm.blockTitle"),
                      description: t("shareMarket.confirm.blockDescription", { email: subscription.renterEmail }),
                      confirmLabel: t("shareMarket.blockAndRevoke"),
                      tone: "danger",
                      run: () => forceRevokeShareMarketSubscription(subscription.id, { blockUser: true }),
                    })}
                  >
                    <UserRoundX className="h-4 w-4" />
                    {t("shareMarket.blockAndRevoke")}
                  </Dropdown.Item>
                ) : null}
                <Dropdown.Item id={`manage-${seat.id}`} onAction={() => openShareManage(listing.shareId)}>
                  <Settings className="h-4 w-4" />
                  {t("shareMarket.manage")}
                </Dropdown.Item>
                {canDelete ? (
                  <Dropdown.Item
                    id={`delete-${seat.id}`}
                    className="text-destructive"
                    onAction={() => setConfirmAction({
                      id: seat.id,
                      title: t("shareMarket.confirm.deleteTitle"),
                      description: t("shareMarket.confirm.deleteDescription", { position: seat.position }),
                      confirmLabel: t("shareMarket.deleteSeat"),
                      tone: "danger",
                      run: () => deleteShareMarketSeat(seat.id),
                    })}
                  >
                    <Trash2 className="h-4 w-4" />
                    {t("shareMarket.deleteSeat")}
                  </Dropdown.Item>
                ) : null}
              </Dropdown.Menu>
            </Dropdown.Popover>
          </Dropdown>
        ) : null}
      </div>
    );
  };

  const renderListing = (listing: ShareMarketListing) => {
    const shareUrl = shareOpenUrl(listing.subdomain);
    const providerTitle = listingProviderTitle(listing.upstreamProvider, t("dashboard.providerUnavailable"), t);
    const providerHeader = listingProviderHeader(listing.upstreamProvider, t("dashboard.providerUnavailable"));
    const providerIdentity = providerHeader.tier
      ? `${providerHeader.name} [${providerHeader.tier}]`
      : providerHeader.name;
    const modelStrategy = !providerHeader.hasProvider
      ? t("shareMarket.modelUnknown")
      : providerHeader.upstreamModels.length > 0
      ? t("shareMarket.upstreamModel", { model: providerHeader.upstreamModels.join(" / ") })
      : t("shareMarket.modelPassthrough");
    const listingStatus = listing.status === "closed"
      ? t("shareMarket.closed")
      : listing.shareStatus !== "active"
        ? t("shareMarket.unavailable")
        : listing.shareOnline
          ? t("shareMarket.online")
          : t("shareMarket.offline");
    const statusTone = listing.status === "closed" || listing.shareStatus !== "active"
      ? "bg-amber-50 text-amber-700"
      : listing.shareOnline
        ? "bg-emerald-50 text-emerald-700"
        : "bg-slate-100 text-slate-500";
    const hasRentalHistory = listing.seats.some((seat) => seat.readOnly || !!seat.subscription);
    const focused = focusShareId === listing.shareId;
    return (
    <article
      key={listing.id}
      data-share-id={listing.shareId}
      className={`overflow-hidden rounded-md border bg-white ${focused ? "border-primary ring-2 ring-primary/20" : "border-slate-200"}`}
    >
      <header className="flex flex-wrap items-start justify-between gap-3 border-b border-slate-200 px-4 py-3">
        <div className="grid min-w-0 flex-1 gap-2.5">
          <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2">
            <ShareAppLogo app={(listing.appType === "claude" || listing.appType === "gemini" ? listing.appType : "codex")} size={18} />
            {shareUrl ? (
              <a
                href={shareUrl}
                target="_blank"
                rel="noreferrer"
                className="inline-flex min-w-0 max-w-full items-center gap-1 truncate text-sm font-semibold text-slate-900 underline-offset-2 hover:underline"
                title={providerTitle}
              >
                <span className="truncate">{providerIdentity}</span>
                <ExternalLink className="h-3 w-3 shrink-0 text-slate-400" aria-hidden />
              </a>
            ) : (
              <h2 className="min-w-0 truncate text-sm font-semibold text-slate-900" title={providerTitle}>{providerIdentity}</h2>
            )}
            <span className="max-w-full truncate text-xs text-slate-600" title={modelStrategy}>
              {modelStrategy}
            </span>
            <div className="flex min-w-0 items-center gap-1 text-xs text-slate-500">
              <span className="shrink-0">{t("shareMarket.owner")}:</span>
              <span className="max-w-[16rem] truncate text-slate-700" title={listing.ownerEmail}>{listing.ownerEmail}</span>
              <ProviderContactButton contacts={listing.contacts} paymentMethods={listing.paymentMethods} />
            </div>
            <span className={`rounded-sm px-1.5 py-0.5 text-[10px] font-medium ${statusTone}`}>{listingStatus}</span>
          </div>

          {listing.isOwner && listing.status === "closed" ? (
            <p className="max-w-2xl text-xs leading-relaxed text-slate-500">{t("shareMarket.closedHint")}</p>
          ) : null}
        </div>
        {listing.isOwner ? (
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" onClick={() => setSeatDialog({ listingId: listing.id, supportedPeriods: listing.supportedUserTokenPeriods })}><Plus className="h-4 w-4" />{t("shareMarket.addSeat")}</Button>
            {listing.status === "active" ? <Button size="sm" variant="outline" isDisabled={!!busyId} onClick={() => setConfirmAction({ id: listing.id, title: t("shareMarket.confirm.closeTitle"), description: t("shareMarket.confirm.closeDescription", { share: listing.shareName }), confirmLabel: t("shareMarket.closeListing"), tone: "warning", run: () => closeShareMarketListing(listing.id) })}><Ban className="h-4 w-4" />{t("shareMarket.closeListing")}</Button> : null}
            {listing.status === "closed" && !hasRentalHistory ? (
              <Button
                size="sm"
                variant="danger"
                isDisabled={!!busyId}
                onClick={() =>
                  setConfirmAction({
                    id: listing.id,
                    title: t("shareMarket.confirm.deleteListingTitle"),
                    description: t("shareMarket.confirm.deleteListingDescription", { share: listing.shareName }),
                    confirmLabel: t("shareMarket.deleteListing"),
                    tone: "danger",
                    run: () => deleteShareMarketListing(listing.id),
                  })
                }
              >
                <Trash2 className="h-4 w-4" />
                {t("shareMarket.deleteListing")}
              </Button>
            ) : null}
          </div>
        ) : null}
      </header>
      <div className="overflow-x-auto">
        <table className="w-full min-w-[860px] text-left text-sm">
          <thead className="bg-slate-50 text-xs font-medium text-slate-500"><tr><th className="px-4 py-2.5">{t("shareMarket.col.seat")}</th><th className="px-3 py-2.5">{t("shareMarket.parallel")}</th><th className="px-3 py-2.5">{t("shareMarket.tokens")}</th><th className="px-3 py-2.5">{t("shareMarket.dialog.amount")}</th><th className="px-3 py-2.5">{t("shareMarket.renter")}</th><th className="px-3 py-2.5">{t("shareMarket.status")}</th><th className="px-4 py-2.5 text-right">{t("common.actions")}</th></tr></thead>
          <tbody>
            {sortSeatsByLifecycle(listing.seats).map((seat) => (
                <tr key={seat.id} className={`border-t border-slate-100 first:border-0 ${seat.readOnly ? "bg-slate-50/60 text-slate-500" : ""}`}>
                  <td className="px-4 py-3 font-medium tabular-nums">{seat.position}</td>
                  <td className="px-3 py-3 text-slate-600">{seat.parallelLimit ?? t("common.unlimited")}</td>
                  <td className="px-3 py-3 text-slate-600">{seat.tokenLimit?.toLocaleString() ?? t("common.unlimited")} · {tokenPeriod(seat.tokenPeriod)}</td>
                  <td className="px-3 py-3 font-medium">{formatPrice(seat, t("shareMarket.free"))}</td>
                  <td className="max-w-[14rem] px-3 py-3"><span className="block truncate text-slate-600" title={seat.subscription?.renterEmail}>{seat.subscription?.renterEmail || "—"}</span></td>
                  <td className="px-3 py-3"><span className="rounded-sm bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600">{seatStatusLabel(seat, seat.subscription, t)}</span></td>
                  <td className="px-4 py-3">{renderSeatActions(listing, seat, seat.subscription)}</td>
                </tr>
            ))}
          </tbody>
        </table>
      </div>
    </article>
    );
  };

  const renderSeatFirstTable = () => (
    <section className="rounded-md border border-slate-200 bg-white">
      <div className="overflow-x-auto">
        <table className="w-full min-w-[1120px] text-left text-sm">
          <thead className="bg-slate-50 text-xs font-medium text-slate-500">
            <tr>
              <th className="sticky top-0 z-10 w-10 border-b border-slate-200 bg-slate-50 px-2 py-2" />
              <SeatSortHeader
                columnKey="online"
                sortPrefs={sortPrefs}
                onSort={onSeatSort}
                filter={
                  <CompactRegionMultiSelect
                    variant="header"
                    columnLabel={t("shareMarket.col.online")}
                    values={onlineFilters}
                    onChange={setOnlineFilters}
                    options={onlineOptions}
                    allLabel={t("shareMarket.allOnline")}
                    moreLabel={(count) => `+${count}`}
                    clearLabel={t("shareMarket.filterClear")}
                    ariaLabel={t("shareMarket.filterOnline")}
                    className="w-full max-w-[8.5rem]"
                  />
                }
              />
              <SeatSortHeader
                columnKey="share"
                sortPrefs={sortPrefs}
                onSort={onSeatSort}
                filter={
                  <CompactRegionMultiSelect
                    variant="header"
                    columnLabel={t("shareMarket.col.share")}
                    values={shareFilters}
                    onChange={setShareFilters}
                    options={shareOptions}
                    allLabel={t("shareMarket.allShares")}
                    moreLabel={(count) => `+${count}`}
                    clearLabel={t("shareMarket.filterClear")}
                    ariaLabel={t("shareMarket.filterShare")}
                    className="w-full max-w-[10rem]"
                  />
                }
              />
              <SeatSortHeader columnKey="seat" sortPrefs={sortPrefs} onSort={onSeatSort} />
              <SeatSortHeader columnKey="parallel" sortPrefs={sortPrefs} onSort={onSeatSort} />
              <SeatSortHeader columnKey="tokens" sortPrefs={sortPrefs} onSort={onSeatSort} />
              <SeatSortHeader columnKey="amount" sortPrefs={sortPrefs} onSort={onSeatSort} />
              <th className="sticky top-0 z-10 border-b border-slate-200 bg-slate-50 px-3 py-2 text-xs font-medium text-slate-500">
                {t("shareMarket.renter")}
              </th>
              <SeatSortHeader
                columnKey="status"
                sortPrefs={sortPrefs}
                onSort={onSeatSort}
                filter={
                  <CompactRegionMultiSelect
                    variant="header"
                    columnLabel={t("shareMarket.col.status")}
                    values={statusFilters}
                    onChange={setStatusFilters}
                    options={statusOptions}
                    allLabel={t("shareMarket.allStatuses")}
                    moreLabel={(count) => `+${count}`}
                    clearLabel={t("shareMarket.filterClear")}
                    ariaLabel={t("shareMarket.filterStatus")}
                    className="w-full max-w-[9rem]"
                  />
                }
              />
              <SeatSortHeader
                columnKey="owner"
                sortPrefs={sortPrefs}
                onSort={onSeatSort}
                filter={
                  <CompactRegionMultiSelect
                    variant="header"
                    columnLabel={t("shareMarket.owner")}
                    values={ownerFilters}
                    onChange={setOwnerFilters}
                    options={ownerOptions}
                    allLabel={t("shareMarket.allOwners")}
                    moreLabel={(count) => `+${count}`}
                    clearLabel={t("shareMarket.filterClear")}
                    ariaLabel={t("shareMarket.filterOwner")}
                    className="w-full max-w-[11rem]"
                  />
                }
              />
              <th
                scope="col"
                className="sticky top-0 z-10 border-b border-slate-200 bg-slate-50 px-4 py-2 text-right text-xs font-medium text-slate-500"
              >
                <div className="flex items-center justify-end gap-2">
                  {hasSeatFilters ? (
                    <button
                      type="button"
                      className="text-[11px] font-medium text-accent hover:underline"
                      onClick={clearSeatFilters}
                    >
                      {t("shareMarket.filterClear")}
                    </button>
                  ) : null}
                  <span>{t("common.actions")}</span>
                </div>
              </th>
            </tr>
          </thead>
          <tbody>
            {seatRows.map((row) => {
              const { listing, seat, subscription } = row;
              const expanded = expandedSeatIds.has(seat.id);
              const statusText = seatStatusLabel(seat, subscription, t);
              const providerTitle = listingProviderTitle(
                listing.upstreamProvider,
                t("dashboard.providerUnavailable"),
                t,
              );
              return (
                <React.Fragment key={row.key}>
                  <tr className={`border-t border-slate-100 ${seat.readOnly ? "bg-slate-50/60 text-slate-500" : ""}`}>
                    <td className="px-3 py-3">
                      <button
                        type="button"
                        className="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 hover:text-slate-800"
                        aria-expanded={expanded}
                        aria-label={expanded ? t("shareMarket.collapseDetails") : t("shareMarket.expandDetails")}
                        onClick={() => toggleExpanded(seat.id)}
                      >
                        <ChevronDown className={`h-4 w-4 transition-transform ${expanded ? "rotate-180" : ""}`} />
                      </button>
                    </td>
                    <td className="px-3 py-3">
                      <span className={`inline-flex items-center gap-1.5 text-xs ${listing.shareOnline ? "text-emerald-700" : "text-slate-500"}`}>
                        <span className={`h-2 w-2 rounded-full ${listing.shareOnline ? "bg-emerald-500" : "bg-slate-400"}`} />
                        {listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
                      </span>
                    </td>
                    <td className="px-3 py-3">
                      <div className="flex min-w-0 items-center gap-2">
                        <ShareAppLogo app={(listing.appType === "claude" || listing.appType === "gemini" ? listing.appType : "codex")} size={16} />
                        <span className="truncate font-medium text-slate-900" title={providerTitle}>
                          {providerTitle}
                        </span>
                      </div>
                    </td>
                    <td className="px-3 py-3 font-medium tabular-nums">
                      {seat.position > 0 ? seat.position : "—"}
                    </td>
                    <td className="px-3 py-3 text-slate-600">{seat.parallelLimit ?? t("common.unlimited")}</td>
                    <td className="px-3 py-3 text-slate-600">
                      {seat.tokenLimit?.toLocaleString() ?? t("common.unlimited")}
                      {seat.tokenPeriod ? ` · ${tokenPeriod(seat.tokenPeriod)}` : ""}
                    </td>
                    <td className="px-3 py-3 font-medium">{formatPrice(seat, t("shareMarket.free"))}</td>
                    <td className="max-w-[14rem] px-3 py-3">
                      <span className="block truncate text-xs text-slate-600" title={subscription?.renterEmail}>
                        {subscription?.renterEmail || "—"}
                      </span>
                    </td>
                    <td className="px-3 py-3">
                      <span className="rounded-sm bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600">{statusText}</span>
                    </td>
                    <td className="px-3 py-3">
                      <div className="flex min-w-0 max-w-[12rem] items-center gap-0.5">
                        <span className="min-w-0 truncate text-xs text-slate-600" title={listing.ownerEmail}>
                          {listing.ownerEmail}
                        </span>
                        <ProviderContactButton contacts={listing.contacts} paymentMethods={listing.paymentMethods} />
                      </div>
                    </td>
                    <td className="px-4 py-3">{renderSeatActions(listing, seat, subscription)}</td>
                  </tr>
                  {expanded ? (
                    <tr className="border-t border-slate-100 bg-slate-50/70">
                      <td colSpan={11} className="px-4 py-3">
                        <ProviderExpandPanel listing={listing} locale={locale} t={t} />
                      </td>
                    </tr>
                  ) : null}
                </React.Fragment>
              );
            })}
          </tbody>
        </table>
      </div>
      {catalog && seatRows.length === 0 ? (
        <p className="border-t border-slate-100 py-10 text-center text-sm text-slate-500">{t("shareMarket.empty")}</p>
      ) : null}
    </section>
  );

  const marketTabs: { id: MarketTab; label: string }[] = [
    { id: "all", label: t("shareMarket.tab.all") },
    { id: "mine", label: t("shareMarket.tab.mine") },
    { id: "rentals", label: t("shareMarket.tab.rentals") },
  ];

  return (
    <div className="mx-auto grid w-full max-w-7xl gap-5 px-1 pb-10">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <LayoutModeToggle
            layout={layout}
            onChange={setLayout}
            seatsLabel={t("shareMarket.layout.seats")}
            sharesLabel={t("shareMarket.layout.shares")}
            ariaLabel={t("shareMarket.layoutToggle")}
          />
          <div className="inline-flex max-w-full overflow-x-auto rounded-lg bg-slate-100 p-1 text-[11px]">
            {marketTabs.map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setTab(item.id)}
                className={`rounded-md px-2.5 py-1.5 transition-colors ${shareMarketTabTone(tab === item.id)}`}
              >
                {item.label}
              </button>
            ))}
          </div>
          <label className="flex min-w-[12rem] max-w-sm flex-1 items-center gap-2 rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 text-sm shadow-sm">
            <Search className="h-4 w-4 shrink-0 text-slate-400" aria-hidden />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-slate-400"
              placeholder={t("shareMarket.search")}
              aria-label={t("shareMarket.searchAria")}
            />
            {query ? (
              <button type="button" className="rounded p-0.5 text-slate-400 hover:bg-slate-100 hover:text-slate-700" aria-label={t("common.close")} onClick={() => setQuery("")}>
                <X className="h-3.5 w-3.5" />
              </button>
            ) : null}
          </label>
        </div>
        <div className="flex gap-2">
          <Button isIconOnly variant="ghost" aria-label={t("common.reload")} isDisabled={loading} onClick={() => void load()}><RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} /></Button>
          <Button variant="primary" size="sm" className="h-8" isDisabled={authLoading} onClick={() => authed ? setAddOpen(true) : window.dispatchEvent(new Event("router-open-login"))}>
            <Plus className="h-4 w-4" />
            {t("shareMarket.addShare")}
          </Button>
        </div>
      </div>
      {loading && !catalog ? <div className="flex items-center gap-2 py-12 text-sm text-slate-500"><Loader2 className="h-4 w-4 animate-spin" />{t("shareMarket.loading")}</div> : null}
      {error ? <div className="rounded-md border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">{error}</div> : null}

      {layout === "seats" ? renderSeatFirstTable() : null}

      {layout === "shares" && tab !== "rentals" ? (
        <div className="grid gap-4">
          {listings.map(renderListing)}
          {catalog && listings.length === 0 ? <p className="py-10 text-center text-sm text-slate-500">{t("shareMarket.empty")}</p> : null}
        </div>
      ) : null}

      {layout === "shares" && tab === "rentals" ? (
        <section className="rounded-md border border-slate-200 bg-white px-4">
          {(catalog?.mySubscriptions || []).map((subscription) => renderSubscription(subscription))}
          {catalog && catalog.mySubscriptions.length === 0 ? <p className="py-10 text-center text-sm text-slate-500">{authed ? t("shareMarket.empty") : t("shareMarket.loginRequired")}</p> : null}
        </section>
      ) : null}

      <UserBlacklistPanel
        enabled={authed}
        hosting={(catalog?.listings || []).some((listing) => listing.isOwner)}
        entries={(catalog?.ownerBlocks || []).map((block) => ({
          id: block.blockedUserId,
          email: block.blockedEmail,
          reason: block.reason,
          createdAt: block.createdAt,
        }))}
        hint={t("shareMarket.blocksHint")}
        empty={t("shareMarket.noBlocks")}
        reasonLabel={(reason) =>
          reason === "owner_force_revoke" || reason === "manual"
            ? reason === "manual"
              ? t("clientMarket.blockReason.manual")
              : t("shareMarket.ownerRevokeReason")
            : reason.replaceAll("_", " ")
        }
        onAdd={async (emails) => {
          for (const email of emails) {
            await createShareMarketBlock({ email, reason: "manual" });
          }
          if (emails.length === 1) {
            toast.success(t("shareMarket.blockedAddedToast", { email: emails[0] }));
          } else {
            toast.success(t("shareMarket.blockedAddedCountToast", { count: emails.length }));
          }
          await load(true);
        }}
        onLift={async (id) => {
          const target = (catalog?.ownerBlocks || []).find((block) => block.blockedUserId === id);
          await liftShareMarketBlock(id);
          if (target) toast.success(t("shareMarket.unblockedToast", { email: target.blockedEmail }));
          await load(true);
        }}
      />
      <AddListingDialog open={addOpen} onOpenChange={setAddOpen} onSaved={() => void load(true)} />
      <SeatDialog open={!!seatDialog} listingId={seatDialog?.listingId || ""} seat={seatDialog?.seat} supportedPeriods={seatDialog?.supportedPeriods} onOpenChange={(next) => !next && setSeatDialog(null)} onSaved={() => void load(true)} />
      <PaymentDialog
        subscription={paymentSubscription}
        onClose={() => setPaymentSubscriptionId("")}
        onPaid={() => void load(true)}
      />
      <ConfirmAlertDialog
        open={!!confirmAction}
        title={confirmAction?.title || ""}
        description={confirmAction?.description || ""}
        confirmLabel={confirmAction?.confirmLabel || ""}
        cancelLabel={t("common.cancel")}
        tone={confirmAction?.tone || "warning"}
        busy={!!busyId}
        onConfirm={() => void runConfirmedAction()}
        onOpenChange={(next) => !next && !busyId && setConfirmAction(null)}
      />
    </div>
  );
}
