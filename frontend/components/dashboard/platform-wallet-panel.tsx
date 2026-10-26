"use client";

/* Hallmark · component: platform wallet panel · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P4 H5 E4 S5 R5 V4
 */

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Button } from "@heroui/react";
import { ArrowUpRight, Ban, CheckCircle2, ChevronDown, Clock3, CreditCard, Loader2, RefreshCw, WalletCards } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletTransferPanel } from "@/components/dashboard/wallet-transfer-panel";
import { useAuth } from "@/components/auth/auth-provider";
import { C2CBuyPanel } from "@/components/dashboard/c2c-buy-panel";
import { C2COrdersPanel } from "@/components/dashboard/c2c-orders-panel";
import { C2CAction } from "@/components/market/c2c-primitives";
import { subscribeMarketBalanceChanged, notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { MarketBuyerDisputesList } from "@/components/dashboard/market-buyer-disputes-list";
import { WalletBadge } from "@/components/market/wallet-primitives";
import { getTransferCapacity, type TransferCapacity } from "@/lib/wallet-policy-api";
import { ApiError } from "@/lib/api";
import { messages, type MessageKey } from "@/lib/i18n";
import { paymentTokenLabel } from "@/lib/market-payment-tokens";
import { quoteWalletTopup, type TopupQuote } from "@/lib/wallet-policy-api";
import {
  createMarketWalletTopup,
  cancelMarketWalletTopup,
  getMarketWallet,
  getMarketWalletChannels,
  getMarketWalletTopup,
  getMarketWalletTopupGatewayStatus,
  getMarketWalletTopups,
  getMarketWalletTransactions,
  recheckMarketWalletTopup,
  stopMarketWalletTopup,
  type MarketWalletChannel,
  type MarketWallet,
  type MarketWalletTopup,
  type MarketTopupGatewayStatus,
  type MarketWalletTransaction,
  type MarketWalletPage,
} from "@/lib/market-wallet-api";
import { TOPUP_PRESET_MINOR, formatMinorUsd, formatTopupCountdown, formatWalletUnits, formatWalletUnitsAsCny, parseTopupMinor, parseUsdMinor, safeMarketReturnPath, shouldResetTopupIdempotency, topupAutoCheckCountdown, topupBounds, topupCancelAction, topupDisplayStatus, verifiedPayableCheckoutUrl, walletTopupCheckoutHref } from "@/lib/market-wallet-money";

const CLOSED_TOPUP_STATUS = new Set(["credited", "expired", "cancelled"]);
const HISTORY_PREVIEW_LIMIT = 5;

function moneyText(units: string, displayCurrency: string, fxRateFresh: boolean, fxRateMicros: number, locale: string) {
  return displayCurrency === "CNY" && fxRateFresh
    ? formatWalletUnitsAsCny(units, fxRateMicros, locale)
    : formatWalletUnits(units);
}

function signedWalletUnits(raw: string) {
  const text = formatWalletUnits(raw);
  return text.startsWith("−") || text.startsWith("<") ? text : `+${text}`;
}

function mergeHistoryItems<T extends { id: string }>(first: T[], rest: T[]): T[] {
  const seen = new Set(first.map((item) => item.id));
  return [...first, ...rest.filter((item) => !seen.has(item.id))];
}

async function refreshHistoryHead<T extends { id: string }>(
  fetchPage: (cursor?: string) => Promise<MarketWalletPage<T>>,
  current: T[],
): Promise<MarketWalletPage<T>> {
  const known = new Set(current.map((item) => item.id));
  let page = await fetchPage();
  let items = page.items;
  // If more than a page arrived since the last refresh, bridge the gap before
  // joining the new head to the previously loaded history.
  while (page.nextCursor && known.size && !page.items.some((item) => known.has(item.id))) {
    page = await fetchPage(page.nextCursor);
    items = mergeHistoryItems(items, page.items);
  }
  return { items, nextCursor: page.nextCursor };
}

function WalletHistorySection({ title, children, collapsed, hasMore, loading, disabled, onShowEarlier, onLoadMore }: {
  title: string;
  children: React.ReactNode;
  collapsed: boolean;
  hasMore: boolean;
  loading: boolean;
  disabled: boolean;
  onShowEarlier: () => void;
  onLoadMore: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const titleId = React.useId();
  const listRef = React.useRef<HTMLDivElement>(null);
  // Keep the preview height after reveal so older rows scroll inside the card.
  const [lockedHeight, setLockedHeight] = React.useState<number | null>(null);
  const revealEarlier = () => {
    const height = listRef.current?.getBoundingClientRect().height;
    if (height) setLockedHeight(Math.ceil(height));
    onShowEarlier();
  };
  const locked = lockedHeight != null && !collapsed;
  return (
    <section className="min-w-0 rounded-xl border border-border bg-card p-4">
      <h2 id={titleId} className="text-base font-semibold">{title}</h2>
      <div
        ref={listRef}
        role="region" aria-labelledby={titleId} tabIndex={locked ? 0 : undefined}
        className={locked
          ? "mt-2 overflow-y-auto overscroll-y-contain [scrollbar-gutter:stable] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          : "mt-2"}
        style={locked ? { height: lockedHeight } : undefined}
        onScroll={(event) => {
          if (!locked) return;
          const list = event.currentTarget;
          if (hasMore && !disabled && list.scrollHeight - list.scrollTop - list.clientHeight < 96) {
            void onLoadMore();
          }
        }}
      >
        {children}
        {collapsed ? <Button size="sm" variant="ghost" className="mt-1" isDisabled={disabled} onPress={revealEarlier}>
          {t("marketWallet.showEarlier")}
        </Button> : null}
        {!collapsed && hasMore ? <Button size="sm" variant="ghost" className="mt-1" isDisabled={disabled} onPress={() => void onLoadMore()}>
          {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
          {t(loading ? "common.loading" : "marketWallet.more")}
        </Button> : null}
      </div>
    </section>
  );
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

function walletEntryKey(entry: MarketWalletTransaction): MessageKey {
  if (entry.c2cOrderId) {
    // Hold journals use action "release" for returning a reservation. That is
    // not the C2C settlement that moves available balance to the buyer.
    if (entry.action.startsWith("reserve:")) return "marketWallet.entry.c2cHold";
    if (entry.sourceKind === "hold" || entry.action === "release_hold") return "marketWallet.entry.c2cReleaseHold";
    if (entry.action === "release") return safeBigInt(entry.deltaUnits) > 0n ? "marketWallet.entry.c2cReleaseIn" : "marketWallet.entry.c2cReleaseOut";
  }
  if (entry.sourceKind === "wallet_transfer") {
    return safeBigInt(entry.deltaUnits) > 0n ? "marketWallet.entry.transferIn" : "marketWallet.entry.transferOut";
  }
  if (entry.action.startsWith("reserve:")) return "marketWallet.entry.holdPlaced";
  const key = `marketWallet.entry.${entry.sourceKind}`;
  return (key in messages.en ? key : "marketWallet.entry.other") as MessageKey;
}

/** Localized message for a failed top-up creation. */
function topupErrorKey(cause: unknown): MessageKey | null {
  if (!(cause instanceof ApiError)) return null;
  switch (cause.code) {
    case "MARKET_TOPUP_PENDING_LIMIT": return "marketWallet.pendingLimit";
    case "MARKET_TOPUP_ABANDONED_LIMIT": return "marketWallet.abandonedLimit";
    case "MARKET_TOPUP_REVIEW_LIMIT": return "marketWallet.reviewLimit";
    case "MARKET_TOPUP_CHECKOUT_UNAVAILABLE": return "marketWallet.checkoutUnavailable";
    case "MARKET_TOPUP_PAUSED": return "marketWallet.error.topupPaused";
    case "DUJIAOPAY_RATE_LIMITED": return "marketWallet.error.gatewayRateLimited";
    case "DUJIAOPAY_NO_CAPACITY": return "marketWallet.error.gatewayNoCapacity";
    case "DUJIAOPAY_UNAVAILABLE": return "marketWallet.error.gatewayUnavailable";
    case "DUJIAOPAY_REJECTED": return "marketWallet.error.gatewayRejected";
    case "DUJIAOPAY_AUTH_FAILED": return "marketWallet.error.gatewayAuthFailed";
    default: return null;
  }
}

function closePaymentWindow(paymentWindow: Window | null) {
  try { paymentWindow?.close(); } catch { /* The payment tab may have navigated away. */ }
}

function safeBigInt(raw: string) {
  try { return BigInt(raw); } catch { return 0n; }
}

function dateText(value: string, locale: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

export function WalletActionButton({
  label,
  phase = "default",
  disabled = false,
  previewState,
}: {
  label: string;
  phase?: "default" | "loading" | "error" | "success";
  disabled?: boolean;
  previewState?: "hover" | "focus" | "active";
}) {
  return (
    <button
      type="submit"
      disabled={disabled || phase === "loading"}
      data-phase={phase}
      data-preview={previewState}
      className="inline-flex h-11 min-w-0 items-center justify-center gap-2 whitespace-nowrap rounded-md bg-primary px-4 text-sm font-semibold text-primary-foreground transition-transform duration-100 hover:bg-primary/90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:cursor-not-allowed disabled:opacity-50 data-[phase=error]:bg-destructive data-[preview=hover]:bg-primary/90 data-[preview=focus]:outline-2 data-[preview=focus]:outline-offset-2 data-[preview=focus]:outline-ring data-[preview=active]:translate-y-px motion-reduce:transition-none"
    >
      {phase === "loading" ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        : phase === "success" ? <CheckCircle2 className="size-4" aria-hidden="true" />
          : <CreditCard className="size-4" aria-hidden="true" />}
      {label}
    </button>
  );
}

export function PlatformWalletPanel({ initialWallet }: { initialWallet: MarketWallet }) {
  const { session } = useAuth();
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const searchParams = useSearchParams();
  const returnedTopupId = searchParams.get("topup");
  const [task, setTask] = React.useState<"topup" | "transfer" | "c2c">(
    searchParams.get("c2cOrder") || searchParams.get("c2cSell") === "1" ? "c2c" : "topup",
  );
  const [c2cOrderId, setC2COrderId] = React.useState<string | null>(searchParams.get("c2cOrder"));
  const linkedC2COrder = searchParams.get("c2cOrder");
  React.useEffect(() => { if (linkedC2COrder) setC2COrderId(linkedC2COrder); }, [linkedC2COrder]);
  const returnedFromCheckoutExit = searchParams.get("cancelled") === "1";
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const [wallet, setWallet] = React.useState(initialWallet);
  const [channels, setChannels] = React.useState<MarketWalletChannel[]>([]);
  const [topups, setTopups] = React.useState<MarketWalletTopup[]>([]);
  const [gatewayStatuses, setGatewayStatuses] = React.useState<Record<string, MarketTopupGatewayStatus | null>>({});
  const [statusBusyId, setStatusBusyId] = React.useState<string | null>(null);
  const [cancelConfirmId, setCancelConfirmId] = React.useState<string | null>(null);
  const [abandonConfirmId, setAbandonConfirmId] = React.useState<string | null>(null);
  const [cancelUnavailableId, setCancelUnavailableId] = React.useState<string | null>(null);
  const [reviewRetryConfirm, setReviewRetryConfirm] = React.useState(false);
  const [checkoutFallbackUrl, setCheckoutFallbackUrl] = React.useState<string | null>(null);
  const [transactions, setTransactions] = React.useState<MarketWalletTransaction[]>([]);
  const [topupCursor, setTopupCursor] = React.useState<string | null>(null);
  const [transactionCursor, setTransactionCursor] = React.useState<string | null>(null);
  const [historyExpanded, setHistoryExpanded] = React.useState({ topups: false, transactions: false });
  const [returnedTopup, setReturnedTopup] = React.useState<MarketWalletTopup | null>(null);
  const [channelId, setChannelId] = React.useState("");
  const [tokenId, setTokenId] = React.useState("");
  const [amount, setAmount] = React.useState("");
  const [amountTouched, setAmountTouched] = React.useState(false);
  const [quote, setQuote] = React.useState<{ fingerprint: string; value: TopupQuote } | null>(null);
  const [quoteError, setQuoteError] = React.useState("");
  const [quoteRefresh, setQuoteRefresh] = React.useState(0);
  const [busy, setBusy] = React.useState("");
  const [topupError, setTopupError] = React.useState("");
  const [historyError, setHistoryError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [transferCapacity, setTransferCapacity] = React.useState<TransferCapacity | null>(null);
  const [transferCapacityUnavailable, setTransferCapacityUnavailable] = React.useState(false);
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const [serverOffsetMs, setServerOffsetMs] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  const idempotencyRef = React.useRef<{ fingerprint: string; key: string; quoteId: string; orderId?: string } | null>(null);
  const busyRef = React.useRef(false);
  const statusRequestVersionRef = React.useRef<Record<string, number>>({});
  const reloadVersionRef = React.useRef(0);
  const channelReloadVersionRef = React.useRef(0);
  const historyExpandedRef = React.useRef({ topups: false, transactions: false });
  const historiesRef = React.useRef({ topups, transactions });
  const balanceFingerprint = `${wallet.version}:${wallet.postedUnits}:${wallet.heldUnits}:${wallet.availableUnits}`;
  const lastBalanceFingerprintRef = React.useRef(balanceFingerprint);
  const hasUnresolvedTopups = topups.some((order) =>
    ["created", "creating", "create_unknown", "pending", "verifying", "review_required"].includes(order.status));
  const hasQueuedRecheck = topups.some((order) => order.buyerRecheckResult === "queued");

  React.useEffect(() => {
    const changed = lastBalanceFingerprintRef.current !== balanceFingerprint;
    lastBalanceFingerprintRef.current = balanceFingerprint;
    // Payment reconciliation may credit the wallet while this page is open.
    // Publish only a change signal, never amounts or a second credit request.
    if (changed && session?.user?.id) notifyMarketBalanceChanged(session.user.id);
  }, [balanceFingerprint, session?.user?.id]);

  React.useEffect(() => {
    historiesRef.current = { topups, transactions };
  }, [topups, transactions]);

  React.useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);

  React.useEffect(() => {
    const attemptedOrderId = idempotencyRef.current?.orderId;
    if (attemptedOrderId && topups.some((order) =>
      order.id === attemptedOrderId && shouldResetTopupIdempotency(order))) {
      idempotencyRef.current = null;
    }
  }, [topups]);

  React.useEffect(() => {
    const requested = safeMarketReturnPath(searchParams.get("returnTo"));
    let saved: string | null = null;
    try {
      if (returnedTopupId) {
        const key = `marketWalletReturnTo:${returnedTopupId}`;
        saved = safeMarketReturnPath(window.sessionStorage.getItem(key));
        if (!saved) saved = safeMarketReturnPath(window.localStorage.getItem(key));
      }
    } catch {
      // Storage is optional; the wallet and checkout remain usable without it.
    }
    setReturnPath(requested ?? saved);
    const suggested = searchParams.get("amount");
    if (suggested && parseUsdMinor(suggested) != null) {
      setAmount((current) => current || suggested);
    }
  }, [returnedTopupId, searchParams]);

  React.useEffect(() => {
    const pendingIds = topups.filter((order) => order.status === "pending").map((order) => order.id);
    if (!pendingIds.length) return;
    let active = true;
    const requests = pendingIds.map((id) => {
      const version = (statusRequestVersionRef.current[id] ?? 0) + 1;
      statusRequestVersionRef.current[id] = version;
      return { id, version };
    });
    void Promise.all(requests.map(async ({ id, version }) => {
      try {
        const status = await getMarketWalletTopupGatewayStatus(id);
        return { id, version, status, offset: Date.parse(status.serverNow) - Date.now() };
      } catch {
        return { id, version, status: null, offset: null };
      }
    })).then((results) => {
      if (!active) return;
      const offset = results.find(({ offset }) => offset != null && Number.isFinite(offset))?.offset;
      if (offset != null) setServerOffsetMs(offset);
      setGatewayStatuses((current) => ({
        ...current,
        ...Object.fromEntries(results
          .filter(({ id, version }) => statusRequestVersionRef.current[id] === version)
          .map(({ id, status }) => [id, status])),
      }));
    });
    return () => { active = false; };
  }, [topups]);

  const refreshTransferCapacity = React.useCallback(async () => {
    try {
      setTransferCapacity(await getTransferCapacity());
      setTransferCapacityUnavailable(false);
    } catch {
      setTransferCapacity(null);
      setTransferCapacityUnavailable(true);
    }
  }, []);

  const refreshChannels = React.useCallback(async () => {
    const version = ++channelReloadVersionRef.current;
    const nextChannels = await getMarketWalletChannels();
    if (version !== channelReloadVersionRef.current) return;
    setChannels(nextChannels);
    setChannelId((current) => nextChannels.some((channel) => channel.id === current) ? current : nextChannels[0]?.id || "");
  }, []);

  const reload = React.useCallback(async () => {
    const version = ++reloadVersionRef.current;
    const [nextWallet, , nextTopups, nextTransactions] = await Promise.all([
      getMarketWallet(), refreshChannels(),
      refreshHistoryHead(getMarketWalletTopups, historiesRef.current.topups),
      refreshHistoryHead(getMarketWalletTransactions, historiesRef.current.transactions),
    ]);
    if (version !== reloadVersionRef.current) return;
    setWallet(nextWallet);
    setGatewayStatuses({});
    // Refresh the newest entries without discarding history already loaded by scrolling.
    setTopups((current) => mergeHistoryItems(nextTopups.items, current));
    setTopupCursor((current) => historyExpandedRef.current.topups ? current : nextTopups.nextCursor);
    setTransactions((current) => mergeHistoryItems(nextTransactions.items, current));
    setTransactionCursor((current) => historyExpandedRef.current.transactions ? current : nextTransactions.nextCursor);
    setLoaded(true);
    void refreshTransferCapacity();
  }, [refreshChannels, refreshTransferCapacity]);

  React.useEffect(() => {
    if (!loaded || !wallet.canTopup || task !== "topup") return;
    // Limits can change even with no pending order. Revalidate when returning
    // from another tab and at a bounded cadence while the form is visible.
    const refresh = () => { if (document.visibilityState === "visible") void refreshChannels().catch(() => {}); };
    const timer = window.setInterval(refresh, 30_000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [loaded, wallet.canTopup, task, refreshChannels]);

  React.useEffect(() => session?.user?.id ? subscribeMarketBalanceChanged(session.user.id, () => { void reload().catch(() => setHistoryError(t("earningConversion.refreshFailed"))); }) : undefined, [session?.user?.id, reload, t]);

  React.useEffect(() => {
    if (!hasUnresolvedTopups) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void reload().catch(() => {});
    }, hasQueuedRecheck ? 5_000 : 30_000);
    const onVisible = () => {
      if (document.visibilityState === "visible") void reload().catch(() => {});
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [hasUnresolvedTopups, hasQueuedRecheck, reload]);

  React.useEffect(() => {
    let active = true;
    const version = ++reloadVersionRef.current;
    Promise.all([
      getMarketWallet(), refreshChannels(),
      getMarketWalletTopups(), getMarketWalletTransactions(),
    ]).then(([nextWallet, , nextTopups, nextTransactions]) => {
      if (!active || version !== reloadVersionRef.current) return;
      setWallet(nextWallet);
      setTopups(nextTopups.items);
      setTopupCursor(nextTopups.nextCursor);
      setTransactions(nextTransactions.items);
      setTransactionCursor(nextTransactions.nextCursor);
      setLoaded(true);
      void refreshTransferCapacity();
    }).catch((cause) => {
      if (!active || version !== reloadVersionRef.current) return;
      setHistoryError(errorText(cause));
      setLoaded(true);
    });
    return () => { active = false; };
  }, [refreshChannels, refreshTransferCapacity]);

  React.useEffect(() => {
    if (!returnedTopupId) return;
    let active = true;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const retry = (delayMs = 5_000) => {
      if (active) timeout = setTimeout(poll, delayMs);
    };
    const poll = async () => {
      let credited = false;
      try {
        const current = await getMarketWalletTopup(returnedTopupId);
        if (!active) return;
        setReturnedTopup(current);
        credited = current.status === "credited";
        if (CLOSED_TOPUP_STATUS.has(current.status)) {
          try { window.localStorage.removeItem(`marketWalletReturnTo:${returnedTopupId}`); } catch { /* Optional storage. */ }
        }
        if (credited) {
          await reload();
        }
        setHistoryError("");
        if (current.status === "review_required") retry(60_000);
        else if (!CLOSED_TOPUP_STATUS.has(current.status)) retry();
      } catch (cause) {
        if (!active) return;
        setHistoryError(errorText(cause));
        // A temporary API failure (including one during the balance refresh)
        // must not strand a paid order on a stale balance. A rejected order ID
        // is terminal and should not spin forever.
        if (credited || !(cause instanceof ApiError) || cause.status >= 500 || cause.status === 429) retry();
      }
    };
    void poll();
    return () => { active = false; if (timeout) clearTimeout(timeout); };
  }, [returnedTopupId, reload]);

  const selectedChannel = channels.find((channel) => channel.id === channelId) ?? null;
  const directChannel = selectedChannel?.orderMode === "direct";
  const channelMethods = React.useMemo(() => directChannel ? selectedChannel?.methods ?? [] : [], [directChannel, selectedChannel]);
  const bounds = topupBounds(selectedChannel);
  const parsedAmount = bounds ? parseTopupMinor(amount, bounds) : null;
  const amountError = !!bounds && amountTouched && amount && parsedAmount == null;
  const methodReady = !directChannel || channelMethods.some((method) => method.tokenId === tokenId);
  const quoteFingerprint = `${channelId}:${parsedAmount}:${directChannel ? tokenId : ""}:${selectedChannel?.policyVersion}:${selectedChannel?.maintenanceFeeBps}:${selectedChannel?.channelRevision}:${selectedChannel?.channelSettingsVersion}:${bounds?.min}:${bounds?.max}`;
  const activeQuote = quote?.fingerprint === quoteFingerprint && Date.parse(quote.value.expiresAt) > nowMs ? quote.value : null;
  const quoteExpired = quote?.fingerprint === quoteFingerprint && Date.parse(quote.value.expiresAt) <= nowMs;

  React.useEffect(() => {
    let active = true;
    setQuoteError("");
    if (!wallet.canTopup || !channelId || parsedAmount == null || !methodReady) { setQuote(null); return; }
    const timer = window.setTimeout(() => {
      void quoteWalletTopup(channelId, parsedAmount, directChannel ? tokenId : undefined)
        .then(async (value) => {
          if (!active) return;
          if (value.policyVersion !== selectedChannel?.policyVersion
            || value.feeBps !== selectedChannel.maintenanceFeeBps
            || value.channelRevision !== selectedChannel.channelRevision
            || value.channelSettingsVersion !== selectedChannel.channelSettingsVersion) {
            setQuote(null);
            setQuoteError(t("walletPolicy.quoteExpired"));
            await refreshChannels();
            return;
          }
          setQuote({ fingerprint: quoteFingerprint, value });
        })
        .catch((cause) => { if (active) {
          setQuote(null);
          const range = cause instanceof ApiError ? topupBounds(cause.details) : null;
          setQuoteError(cause instanceof ApiError && cause.code === "MARKET_TOPUP_AMOUNT_OUT_OF_RANGE"
            ? range ? t("marketWallet.error.amountOutOfRange", { min: formatMinorUsd(range.min), max: formatMinorUsd(range.max) }) : t("marketWallet.amountRangeUnavailable")
            : errorText(cause));
          if (cause instanceof ApiError && ["MARKET_TOPUP_AMOUNT_OUT_OF_RANGE", "MARKET_TOPUP_QUOTE_CHANGED"].includes(cause.code ?? "")) void refreshChannels().catch(() => {});
        } });
    }, 450);
    return () => { active = false; window.clearTimeout(timer); };
  // The fingerprint captures the channel's monetary configuration, without
  // re-quoting just because polling returned an equivalent channel object.
  }, [channelId, parsedAmount, directChannel, tokenId, methodReady, quoteFingerprint, wallet.canTopup, quoteRefresh, quoteExpired, refreshChannels, t]);

  React.useEffect(() => {
    // A payment method belongs to one channel; a single method is preselected,
    // several require an explicit choice because they settle on different chains.
    setTokenId((current) => channelMethods.some((method) => method.tokenId === current)
      ? current
      : channelMethods.length === 1 ? channelMethods[0].tokenId : "");
  }, [channelMethods]);
  const matchingUnsettled = topups.find((order) =>
    (order.creditAmountMinor ?? order.amountMinor) === parsedAmount
      && (["verifying", "review_required"].includes(order.status)
        || (!!order.buyerAbandonedAt && !CLOSED_TOPUP_STATUS.has(order.status))));
  const startTopup = async () => {
    if (busyRef.current || !wallet.canTopup || !channelId || parsedAmount == null || !methodReady || !activeQuote) return;
    const attemptedOrderId = idempotencyRef.current?.orderId;
    if (attemptedOrderId && topups.some((order) => order.id === attemptedOrderId
      && shouldResetTopupIdempotency(order))) {
      idempotencyRef.current = null;
    }
    setReviewRetryConfirm(false);
    busyRef.current = true;
    setBusy("topup");
    setTopupError("");
    setNotice("");
    setCheckoutFallbackUrl(null);
    let paymentWindow: Window | null = null;
    try {
      // Open during the user gesture; opening after the awaited API call is
      // commonly blocked by browsers. No third-party page gets an opener.
      paymentWindow = window.open("about:blank", "_blank");
      if (paymentWindow) {
        paymentWindow.opener = null;
        paymentWindow.document.title = t("marketWallet.creating");
        paymentWindow.document.body.textContent = t("marketWallet.creating");
      }
    } catch {
      try { paymentWindow?.close(); } catch { /* The manual payment link remains available. */ }
      paymentWindow = null;
    }
    const fingerprint = `${quoteFingerprint}:${activeQuote.policyVersion}:${activeQuote.channelRevision}:${activeQuote.channelSettingsVersion}:${activeQuote.feeMinor}`;
    if (idempotencyRef.current?.fingerprint !== fingerprint) {
      idempotencyRef.current = { fingerprint, key: crypto.randomUUID(), quoteId: activeQuote.id };
    }
    const attempt = idempotencyRef.current;
    try {
      const order = await createMarketWalletTopup(channelId, parsedAmount, attempt.key, attempt.quoteId, directChannel ? tokenId : undefined);
      if (idempotencyRef.current === attempt) attempt.orderId = order.id;
      if (shouldResetTopupIdempotency(order)) idempotencyRef.current = null;
      reloadVersionRef.current += 1;
      setTopups((current) => [order, ...current.filter((item) => item.id !== order.id)]);
      if (returnPath) {
        const key = `marketWalletReturnTo:${order.id}`;
        try {
          window.sessionStorage.setItem(key, returnPath);
        } catch {
          // A blocked storage API must not block payment.
        }
        try {
          // A noopener fallback tab cannot read this tab's sessionStorage.
          window.localStorage.setItem(key, returnPath);
        } catch {
          // Return navigation is optional.
        }
        try {
          // A new tab has its own sessionStorage. Its initial copy is made
          // before the API returns, so write the per-order return path there.
          paymentWindow?.sessionStorage.setItem(key, returnPath);
        } catch {
          // A blocked storage API must not block payment.
        }
      }
      if (order.buyerAbandonedAt) {
        closePaymentWindow(paymentWindow);
        idempotencyRef.current = null;
        setNotice(t("marketWallet.abandonedHelp"));
      } else if (order.status === "credited") {
        closePaymentWindow(paymentWindow);
        void reload().catch(() => {});
      } else {
        // The same-origin checkout page owns the one fresh gateway preflight.
        // Do not close a newly opened tab merely because payment is unavailable:
        // it must show the buyer why the existing order cannot be paid.
        const localCheckout = walletTopupCheckoutHref(order.id);
        if (!paymentWindow || paymentWindow.closed) {
          setCheckoutFallbackUrl(localCheckout);
        } else {
          try {
            paymentWindow.location.replace(localCheckout);
            setNotice(t("marketWallet.openedPaymentTab"));
          } catch {
            closePaymentWindow(paymentWindow);
            setCheckoutFallbackUrl(localCheckout);
          }
        }
      }
    } catch (cause) {
      closePaymentWindow(paymentWindow);
      if (cause instanceof ApiError && cause.code === "MARKET_TOPUP_QUOTE_CHANGED") {
        idempotencyRef.current = null;
        setQuote(null);
        setQuoteRefresh((value) => value + 1);
        setTopupError(t("walletPolicy.quoteExpired"));
        void refreshChannels().catch(() => {});
        return;
      }
      const range = cause instanceof ApiError ? topupBounds(cause.details) : null;
      const key = topupErrorKey(cause);
      setTopupError(cause instanceof ApiError && cause.code === "MARKET_TOPUP_AMOUNT_OUT_OF_RANGE"
        ? range ? t("marketWallet.error.amountOutOfRange", {
          min: formatMinorUsd(range.min), max: formatMinorUsd(range.max),
        }) : t("marketWallet.amountRangeUnavailable")
        : key ? t(key) : errorText(cause));
      void reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const submitTopup = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setAmountTouched(true);
    if (busyRef.current || !loaded || !wallet.canTopup || !channelId || parsedAmount == null || !methodReady || !activeQuote) return;
    if (matchingUnsettled) setReviewRetryConfirm(true);
    else void startTopup();
  };

  const refreshGatewayStatus = async (id: string) => {
    if (statusBusyId) return;
    const version = (statusRequestVersionRef.current[id] ?? 0) + 1;
    statusRequestVersionRef.current[id] = version;
    setStatusBusyId(id);
    try {
      const status = await getMarketWalletTopupGatewayStatus(id);
      if (statusRequestVersionRef.current[id] === version) {
        setGatewayStatuses((current) => ({ ...current, [id]: status }));
      }
      const offset = Date.parse(status.serverNow) - Date.now();
      if (Number.isFinite(offset)) setServerOffsetMs(offset);
      setHistoryError("");
    } catch {
      if (statusRequestVersionRef.current[id] === version) {
        setGatewayStatuses((current) => ({ ...current, [id]: null }));
      }
      setHistoryError(t("marketWallet.gatewayStatusUnavailable"));
    } finally {
      setStatusBusyId(null);
    }
  };

  const chooseCancelAction = async (order: MarketWalletTopup) => {
    if (statusBusyId || busyRef.current) return;
    const version = (statusRequestVersionRef.current[order.id] ?? 0) + 1;
    statusRequestVersionRef.current[order.id] = version;
    setStatusBusyId(order.id);
    try {
      const status = await getMarketWalletTopupGatewayStatus(order.id);
      if (statusRequestVersionRef.current[order.id] === version) {
        setGatewayStatuses((current) => ({ ...current, [order.id]: status }));
      }
      const offset = Date.parse(status.serverNow) - Date.now();
      if (Number.isFinite(offset)) setServerOffsetMs(offset);
      const action = topupCancelAction(order.status, status, !!order.buyerAbandonedAt);
      if (action === "gateway_cancel") setCancelConfirmId(order.id);
      else if (action === "abandon") setAbandonConfirmId(order.id);
      else setCancelUnavailableId(order.id);
    } catch {
      if (statusRequestVersionRef.current[order.id] === version) {
        setGatewayStatuses((current) => ({ ...current, [order.id]: null }));
      }
      setCancelUnavailableId(order.id);
    } finally {
      setStatusBusyId(null);
    }
  };

  const requestRecheck = async (id: string) => {
    if (statusBusyId || busyRef.current) return;
    setStatusBusyId(id);
    setHistoryError("");
    setNotice("");
    try {
      const result = await recheckMarketWalletTopup(id);
      reloadVersionRef.current += 1;
      setTopups((current) => current.map((order) => order.id === id ? result.order : order));
      if (!result.queued) setNotice(t("marketWallet.recheckAlreadyQueued"));
    } catch (cause) {
      setHistoryError(errorText(cause));
    } finally {
      setStatusBusyId(null);
    }
  };

  const cancelSelectedTopup = async () => {
    const id = cancelConfirmId;
    if (!id || busyRef.current) return;
    busyRef.current = true;
    setBusy("cancelTopup");
    if (idempotencyRef.current?.orderId === id) idempotencyRef.current = null;
    setHistoryError("");
    setGatewayStatuses((current) => ({ ...current, [id]: null }));
    setCheckoutFallbackUrl(null);
    try {
      const updated = await cancelMarketWalletTopup(id);
      setTopups((current) => current.map((order) => order.id === id ? updated : order));
      setCancelConfirmId(null);
      try {
        await reload();
      } catch {
        setHistoryError(t("marketWallet.cancelRefreshFailed"));
      }
    } catch (cause) {
      setCancelConfirmId(null);
      setHistoryError(cause instanceof ApiError && cause.status === 409
        ? t("marketWallet.cancelChanged")
        : errorText(cause));
      await reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const abandonSelectedTopup = async () => {
    const id = abandonConfirmId;
    if (!id || busyRef.current) return;
    busyRef.current = true;
    setBusy("abandonTopup");
    if (idempotencyRef.current?.orderId === id) idempotencyRef.current = null;
    setHistoryError("");
    setNotice("");
    setGatewayStatuses((current) => ({ ...current, [id]: null }));
    setCheckoutFallbackUrl(null);
    try {
      const updated = await stopMarketWalletTopup(id);
      setTopups((current) => current.map((order) => order.id === id ? updated : order));
      setAbandonConfirmId(null);
      setNotice(t("marketWallet.abandonedHelp"));
      try {
        await reload();
      } catch {
        setHistoryError(t("marketWallet.abandonRefreshFailed"));
      }
    } catch (cause) {
      setAbandonConfirmId(null);
      setHistoryError(cause instanceof ApiError && cause.status === 409
        ? t("marketWallet.abandonChanged") : errorText(cause));
      await reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const loadMore = async (kind: "topups" | "transactions") => {
    if (busyRef.current) return;
    const cursor = kind === "topups" ? topupCursor : transactionCursor;
    if (!cursor) return;
    busyRef.current = true;
    setBusy(kind);
    setHistoryError("");
    const version = reloadVersionRef.current;
    try {
      if (kind === "topups") {
        const page = await getMarketWalletTopups(cursor);
        if (version !== reloadVersionRef.current) return;
        historyExpandedRef.current.topups = true;
        setTopups((current) => mergeHistoryItems(current, page.items));
        setTopupCursor(page.nextCursor);
      } else {
        const page = await getMarketWalletTransactions(cursor);
        if (version !== reloadVersionRef.current) return;
        historyExpandedRef.current.transactions = true;
        setTransactions((current) => mergeHistoryItems(current, page.items));
        setTransactionCursor(page.nextCursor);
      }
    } catch (cause) {
      setHistoryError(errorText(cause));
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const cancelUnavailableGateway = cancelUnavailableId ? gatewayStatuses[cancelUnavailableId] : null;
  const cancelUnavailableDescription = ["confirming", "paid"].includes(cancelUnavailableGateway?.gatewayStatus ?? "")
    ? t("marketWallet.cancelPaymentStarted")
    : ["expired", "canceled", "cancelled"].includes(cancelUnavailableGateway?.gatewayStatus ?? "")
      ? t("marketWallet.cancelGatewayEnding")
      : cancelUnavailableGateway?.gatewayStatus === "pending" || cancelUnavailableGateway?.gatewayStatus === "awaiting_payment"
        ? t("marketWallet.cancelSafetyHold")
        : t("marketWallet.gatewayStatusUnavailable");

  const balanceText = (units: string) => moneyText(units, displayCurrency, fxRateFresh, fxRateMicros, locale);
  const topupClosedReason = !wallet.canTopup
    ? t(wallet.mode === "shadow" ? "marketWallet.topupStatus.shadow" : wallet.mode === "enabled" ? "marketWallet.topupStatus.unavailable" : "marketWallet.topupStatus.closed")
    : null;
  const heldUnits = safeBigInt(wallet.heldUnits);
  const owedUnits = safeBigInt(wallet.committedUnits ?? "0");
  const transferableUnits = transferCapacity ? safeBigInt(transferCapacity.transferableUnits) : null;
  const balanceIsClear = heldUnits === 0n && owedUnits === 0n && transferableUnits !== null
    && transferableUnits === safeBigInt(wallet.availableUnits);
  const balanceNote = transferCapacityUnavailable
    ? t("marketWallet.balanceNoteUnavailable")
    : !transferCapacity
      ? t("common.loading")
      : balanceIsClear
        ? t("marketWallet.balanceClear")
        : t("marketWallet.balanceNote", {
            held: balanceText(wallet.heldUnits),
            owed: balanceText(wallet.committedUnits ?? "0"),
            transferable: balanceText(transferCapacity.transferableUnits),
          });

  return (
    <main className="mx-auto grid w-full max-w-5xl min-w-0 gap-6 px-4 py-6 sm:px-6" aria-label={t("account.nav.wallet")}>
      <header className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <p className="max-w-2xl text-sm leading-6 text-muted-foreground">{t("marketWallet.subtitle")}</p>
        <Button variant="outline" size="sm" isDisabled={!!busy} onPress={() => void reload().catch((cause) => setHistoryError(errorText(cause)))}>
          <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
        </Button>
      </header>

      {notice ? <p role="status" className="rounded-md bg-emerald-50 px-4 py-3 text-sm text-emerald-800">{notice}</p> : null}
      {checkoutFallbackUrl ? <p role="status" className="rounded-md bg-amber-50 px-4 py-3 text-sm text-amber-900">
        {t("marketWallet.paymentTabBlocked")} <a href={checkoutFallbackUrl} target="_blank" rel="noopener noreferrer" className="font-semibold underline underline-offset-2">{t("marketWallet.openPaymentTab")}</a>
      </p> : null}
      {returnedTopup || returnPath ? (
        <div role="status" aria-live="polite" className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-card px-4 py-3 text-sm">
          <div className="grid gap-1">
            {returnedTopup ? <span className="flex items-center gap-2">
              {returnedTopup.status === "credited" ? <CheckCircle2 className="size-4 text-emerald-700" aria-hidden="true" /> : <Clock3 className="size-4 text-muted-foreground" aria-hidden="true" />}
              {t(`marketWallet.status.${topupDisplayStatus(returnedTopup.status, undefined, !!returnedTopup.buyerAbandonedAt)}` as Parameters<typeof t>[0])}
            </span> : null}
            {returnedFromCheckoutExit && returnedTopup && !CLOSED_TOPUP_STATUS.has(returnedTopup.status) ? <span className="text-xs text-amber-700">{t("marketWallet.checkoutExitIsNotCancellation")}</span> : null}
          </div>
          {returnPath ? (
            <Link href={returnPath} className="inline-flex min-h-11 items-center whitespace-nowrap font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t(returnedTopup?.status === "credited" ? "marketWallet.returnToMarket" : "marketWallet.returnWhilePending")}<ArrowUpRight className="ml-1 size-4" aria-hidden="true" />
            </Link>
          ) : null}
        </div>
      ) : null}

      <section className="min-w-0 rounded-xl border border-border bg-card px-5 py-4" aria-label={t("marketWallet.balanceBreakdown")}>
        <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
          <p className="flex items-center gap-2 text-sm text-muted-foreground"><WalletCards className="size-4" aria-hidden="true" />{t("marketWallet.available")}</p>
          <p className="break-all text-3xl font-semibold tabular-nums tracking-tight">{balanceText(wallet.availableUnits)}</p>
        </div>
        {displayCurrency === "CNY" && fxRateFresh ? <p className="mt-1 text-right text-xs text-muted-foreground">{t("marketWallet.displayOnly", { amount: formatWalletUnits(wallet.availableUnits) })}</p> : null}
        <p className="mt-2 text-sm leading-6 text-muted-foreground">{balanceNote}</p>
      </section>

      <section className="grid min-w-0 gap-4 rounded-xl border border-border bg-card p-5 sm:p-6">
        <div role="group" aria-label={t("marketWallet.actions")} className="flex flex-wrap gap-2">
          {(["topup", "transfer", "c2c"] as const).map((item) => (
            <C2CAction key={item} type="button" secondary={task !== item} aria-pressed={task === item} disabled={!!busy} onClick={() => setTask(item)}>
              {t(item === "c2c" ? "c2c.buy" : `marketWallet.action.${item}`)}
              {item === "topup" && topupClosedReason ? <span className="ml-2 text-xs font-medium text-muted-foreground">{topupClosedReason}</span> : null}
            </C2CAction>
          ))}
        </div>
        {task === "transfer" ? <WalletTransferPanel embedded onTransferred={async () => { if (session?.user?.id) notifyMarketBalanceChanged(session.user.id); await reload(); }} walletVersion={wallet.version} /> : null}
        {task === "c2c" && session?.user?.id ? <C2CBuyPanel key={session.user.id} actor={session.user.id} onOrder={order => setC2COrderId(order.id)} /> : null}
        {task === "topup" && wallet.canTopup ? <form className="grid min-w-0 content-start gap-3" onSubmit={(event) => void submitTopup(event)}>
          {wallet.mode === "shadow" ? <p className="text-sm text-muted-foreground">{t("marketWallet.shadow")}</p> : null}
          <p className="text-sm font-semibold">{t("marketWallet.topup")}</p>
          {channels.length === 1 && selectedChannel ? <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
            {t("marketWallet.topupChannel", { channel: selectedChannel.displayName })}
            {selectedChannel.canary ? <WalletBadge tone="warning" pulse>{t("marketWallet.canaryBadge")}</WalletBadge> : null}
          </p> : <label className="grid min-w-0 gap-1 text-sm">
            <span className="flex flex-wrap items-center gap-2">{t("marketWallet.channel")}
              {selectedChannel?.canary ? <WalletBadge tone="warning" pulse>{t("marketWallet.canaryBadge")}</WalletBadge> : null}
            </span>
            <select
              className="h-11 min-w-0 rounded-md border border-input bg-background px-3 outline-2 outline-transparent focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50"
              value={channelId} onChange={(event) => { setChannelId(event.target.value); idempotencyRef.current = null; }}
              disabled={!wallet.canTopup || !channels.length || !!busy}
            >
              {!channels.length ? <option value="">{t("marketWallet.noChannel")}</option> : null}
              {channels.map((channel) => <option key={channel.id} value={channel.id}>{channel.displayName}</option>)}
            </select>
          </label>}
          {selectedChannel?.canary ? <p className="text-xs text-amber-800">{t("marketWallet.canaryHelp")}</p> : null}
          <fieldset className="grid min-w-0 gap-2 text-sm" aria-describedby="wallet-amount-help">
            <legend className="mb-1">{t("marketWallet.amount")}</legend>
            <div className="grid grid-cols-3 gap-2 sm:grid-cols-5">
              {TOPUP_PRESET_MINOR.map((minor) => {
                const allowed = !!bounds && minor >= bounds.min && minor <= bounds.max;
                const pressed = parsedAmount === minor;
                return <button
                  key={minor} type="button" aria-pressed={pressed}
                  disabled={!allowed || !wallet.canTopup || !!busy}
                  onClick={() => { setAmount((minor / 100).toFixed(2)); setAmountTouched(true); idempotencyRef.current = null; }}
                  className={`min-h-11 rounded-md border text-sm font-semibold tabular-nums focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:cursor-not-allowed disabled:opacity-40 ${pressed
                    ? "border-primary bg-primary text-primary-foreground"
                    : "border-border bg-background hover:bg-muted"}`}
                >
                  ${minor / 100}
                </button>;
              })}
            </div>
            <label className="grid min-w-0 gap-1">
              <span className="text-xs text-muted-foreground">{t("marketWallet.customAmount")}</span>
              <span className="relative grid">
                <span className="pointer-events-none absolute inset-y-0 left-3 flex items-center text-muted-foreground" aria-hidden="true">$</span>
                <input
                  id="market-wallet-topup-amount"
                  aria-label={t("marketWallet.customAmount")}
                  inputMode="decimal" autoComplete="off" value={amount}
                  onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }}
                  onBlur={() => setAmountTouched(true)}
                  aria-invalid={!!amountError} aria-describedby="wallet-amount-help"
                  className="h-11 min-w-0 rounded-md border border-input bg-background pl-7 pr-3 tabular-nums outline-2 outline-transparent focus-visible:outline-ring aria-invalid:border-destructive disabled:cursor-not-allowed disabled:opacity-50"
                  placeholder="10.00" disabled={!bounds || !wallet.canTopup || !!busy}
                />
              </span>
            </label>
            <span id="wallet-amount-help" className={`min-h-[1lh] text-xs ${amountError ? "text-destructive" : "text-muted-foreground"}`}>
              {!bounds ? t(selectedChannel ? "marketWallet.amountRangeUnavailable" : "marketWallet.noChannelHelp") : amountError
                ? t("marketWallet.amountRangeError", { min: formatMinorUsd(bounds.min), max: formatMinorUsd(bounds.max) })
                : t("marketWallet.amountRangeHelp", { min: formatMinorUsd(bounds.min), max: formatMinorUsd(bounds.max) })}
            </span>
          </fieldset>
          {directChannel ? <fieldset className="grid min-w-0 gap-2 text-sm">
            <legend className="mb-1">{t("marketWallet.paymentMethod")}</legend>
            {channelMethods.length ? <div role="radiogroup" aria-label={t("marketWallet.paymentMethod")} className="grid gap-2 sm:grid-cols-2">
              {channelMethods.map((method) => (
                <label key={method.tokenId} className="flex min-h-11 cursor-pointer items-center gap-3 rounded-md border border-border bg-background px-3 py-2 hover:bg-muted has-[:checked]:border-primary has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-ring has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-50">
                  <input
                    type="radio" name="market-wallet-topup-method" value={method.tokenId}
                    checked={tokenId === method.tokenId} disabled={!!busy}
                    onChange={() => { setTokenId(method.tokenId); idempotencyRef.current = null; }}
                    className="size-4 accent-primary"
                  />
                  <span className="grid min-w-0">
                    <span className="font-semibold">{method.symbol || paymentTokenLabel(method.tokenId).symbol}</span>
                    <span className="truncate text-xs text-muted-foreground">{method.chainName || paymentTokenLabel(method.tokenId).chainName}</span>
                  </span>
                </label>
              ))}
            </div> : <p className="text-xs text-muted-foreground">{t("marketWallet.noPaymentMethods")}</p>}
            <p className="text-xs text-muted-foreground">{t("marketWallet.directMethodHelp")}</p>
          </fieldset> : null}
          {parsedAmount != null && methodReady ? <dl aria-live="polite" className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-2 text-sm tabular-nums">
            <dt className="text-muted-foreground">{t("walletPolicy.credit")}</dt><dd>{activeQuote ? formatMinorUsd(activeQuote.creditMinor) : t("marketWallet.quotingTotal")}</dd>
            <dt className="text-muted-foreground">{t("walletPolicy.fee", { rate: (activeQuote?.feeBps ?? selectedChannel?.maintenanceFeeBps ?? 0) / 100 })}</dt><dd>{activeQuote ? formatMinorUsd(activeQuote.feeMinor) : t("marketWallet.quotingTotal")}</dd>
            <dt className="font-semibold">{t("walletPolicy.total")}</dt><dd className="font-semibold">{activeQuote ? formatMinorUsd(activeQuote.payMinor) : t("marketWallet.quotingTotal")}</dd>
          </dl> : null}
          {quoteError ? <div className="grid justify-items-start gap-2"><p role="alert" className="text-sm text-destructive">{quoteError}</p><Button variant="outline" onPress={() => setQuoteRefresh((value) => value + 1)}>{t("common.retry")}</Button></div> : null}
          <p className="text-xs leading-5 text-muted-foreground">{t("walletPolicy.disclosure")}</p>
          <WalletActionButton
            label={busy === "topup" ? t("marketWallet.creating") : activeQuote ? t("walletPolicy.pay", { amount: formatMinorUsd(activeQuote.payMinor) }) : t("marketWallet.pay")}
            phase={busy === "topup" ? "loading" : topupError ? "error" : "default"}
            disabled={!loaded || !wallet.canTopup || !channels.length || parsedAmount == null || !methodReady || !activeQuote || !!busy}
          />
          {topupError ? <p role="alert" className="text-sm text-destructive">{topupError}</p> : null}
          {matchingUnsettled ? <p className="text-xs text-amber-700">{t("marketWallet.reviewRetryNotice")}</p> : null}
          {!channels.length ? <p className="text-xs text-muted-foreground">{t("marketWallet.noChannelHelp")}</p> : null}
        </form> : task === "topup" ? <div className="grid content-start gap-3 rounded-lg bg-muted/50 p-4 text-sm text-muted-foreground">
          <p>{t(wallet.mode === "shadow" ? "marketWallet.shadow" : wallet.mode === "enabled" ? "marketWallet.topupUnavailable" : "marketWallet.notOpen")}</p>
        </div> : null}
      </section>

      {session?.user?.id ? <C2COrdersPanel key={session.user.id} actor={session.user.id} selectedId={c2cOrderId} onSelect={setC2COrderId} /> : null}

      {(wallet.canTopup || wallet.hasWalletActivity) && !loaded ? <p className="text-sm text-muted-foreground">{t("common.loading")}</p> : null}
      {historyError ? <p role="alert" className="rounded-md border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">{historyError}</p> : null}
      {wallet.canTopup || wallet.hasWalletActivity ? <div className="grid min-w-0 gap-6 lg:grid-cols-2">
        <WalletHistorySection title={t("marketWallet.topups")} collapsed={!historyExpanded.topups && topups.length > HISTORY_PREVIEW_LIMIT}
          hasMore={!!topupCursor} loading={busy === "topups"} disabled={!!busy}
          onShowEarlier={() => setHistoryExpanded((current) => ({ ...current, topups: true }))} onLoadMore={() => loadMore("topups")}>
          <ul>
            {(historyExpanded.topups ? topups : topups.slice(0, HISTORY_PREVIEW_LIMIT)).map((order) => {
              const gateway = gatewayStatuses[order.id];
              const abandoned = !!order.buyerAbandonedAt;
              const shownStatus = abandoned && order.status === "pending" && (gateway === undefined || gateway === null)
                ? "buyer_abandoned"
                : order.status === "pending" && gateway === undefined
                ? "gateway_checking"
                : order.status === "pending" && gateway === null
                  ? "gateway_unavailable"
                  : order.status === "pending" && gateway && !gateway.canPay
                    && ["awaiting_payment", "pending"].includes(gateway.gatewayStatus)
                    ? "payment_unavailable"
                  : topupDisplayStatus(order.status, gateway?.gatewayStatus, abandoned);
              const deadline = gateway?.gatewayStatus === "awaiting_payment"
                ? gateway.selectionDeadline
                : gateway?.expiresAt;
              const activePayment = !abandoned && order.status === "pending"
                && !["confirming", "paid", "expired", "canceled", "cancelled"].includes(gateway?.gatewayStatus ?? "");
              const countdown = activePayment ? topupAutoCheckCountdown(order.autoCloseAt, nowMs + serverOffsetMs) : null;
              const directOrder = order.orderMode === "direct";
              const checkoutUrl = directOrder
                ? (order.status === "pending" && !abandoned && order.hasPaymentInstructions ? walletTopupCheckoutHref(order.id) : null)
                : verifiedPayableCheckoutUrl(order.status, order.checkoutUrl, gateway, abandoned, nowMs + serverOffsetMs);
              const neverCreated = order.localCloseReason === "never_created";
              const pendingVerification = order.status === "pending" && gateway
                && !gateway.canPay && !gateway.canCancel && !gateway.canAbandon;
              const canStopPayment = !abandoned && ["created", "creating", "create_unknown", "pending", "verifying", "review_required"].includes(order.status);
              const recheckResultIsCurrent = order.buyerRecheckResult === order.status
                || (order.status === "create_unknown" && order.buyerRecheckResult === "pending");
              const lastGatewayLabel = order.lastGatewayStatus === "expired" ? t("marketWallet.status.expired")
                : ["canceled", "cancelled"].includes(order.lastGatewayStatus ?? "") ? t("marketWallet.status.cancelled")
                  : order.lastGatewayStatus === "paid" ? t("marketWallet.status.gateway_paid")
                    : order.lastGatewayStatus === "confirming" ? t("marketWallet.status.verifying")
                      : order.lastGatewayStatus === "pending" ? t("marketWallet.status.gateway_pending")
                        : order.lastGatewayStatus === "awaiting_payment" ? t("marketWallet.status.awaiting_payment")
                          : t("marketWallet.status.gateway_unknown");
              const statusTone = shownStatus === "credited" ? "text-emerald-800"
                : ["review_required", "payment_unavailable", "gateway_unavailable", "buyer_abandoned"].includes(shownStatus) || countdown?.phase === "soon" ? "text-amber-800"
                  : checkoutUrl || shownStatus === "pending" || shownStatus === "awaiting_payment" ? "text-primary"
                    : "text-muted-foreground";
              const showRecheck = order.status === "review_required" || countdown?.phase === "checking" || pendingVerification || (abandoned && order.status === "pending") || order.buyerRecheckResult === "retrying";
              const showStop = canStopPayment && !(order.status === "pending" && gateway?.canCancel);
              const needsAttention = !CLOSED_TOPUP_STATUS.has(order.status);
              return <li key={order.id} className="border-b border-border/70 text-sm last:border-b-0">
                <details className="group" open={needsAttention || undefined}>
                  <summary className="flex min-h-9 cursor-pointer list-none items-center gap-2 py-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
                    <ChevronDown className="size-3.5 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" aria-hidden="true" />
                    <span className="sr-only">{t("marketWallet.topupDetails")}</span>
                    <span className="flex min-w-0 items-center gap-1.5 font-medium tabular-nums">${((order.creditAmountMinor ?? order.amountMinor) / 100).toFixed(2)}
                      {directOrder && order.paymentTokenId ? <span className="rounded-full bg-accent/10 px-1.5 py-px text-[0.7rem] font-medium text-accent">{paymentTokenLabel(order.paymentTokenId).label}</span> : null}
                    </span>
                    <span className={`min-w-0 truncate text-xs font-medium ${statusTone}`}>{neverCreated ? t("marketWallet.status.never_created") : t(`marketWallet.status.${shownStatus}` as Parameters<typeof t>[0])}</span>
                    <span className="ml-auto shrink-0 text-xs text-muted-foreground">{dateText(order.createdAt, locale)}</span>
                  </summary>
                  <div className="grid gap-1 pb-2 pl-5 text-xs text-muted-foreground">
                    {checkoutUrl || showRecheck || showStop || order.status === "pending" ? <div className="flex flex-wrap items-center gap-1 pb-1">
                      {checkoutUrl ? <a className="inline-flex items-center gap-1 whitespace-nowrap text-xs font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" href={walletTopupCheckoutHref(order.id)} target="_blank" rel="noopener noreferrer">
                        {t("marketWallet.continuePay")}<ArrowUpRight className="size-3" aria-hidden="true" />
                      </a> : null}
                      {showRecheck ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => void requestRecheck(order.id)}>
                        <RefreshCw className={`size-4 ${statusBusyId === order.id ? "animate-spin" : ""}`} aria-hidden="true" />{t("marketWallet.recheck")}
                      </Button> : null}
                      {showStop ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => setAbandonConfirmId(order.id)}>
                        {t("marketWallet.abandonTopup")}
                      </Button> : null}
                      {order.status === "pending" ? <Button size="sm" variant="ghost" isIconOnly aria-label={t("marketWallet.refreshGatewayStatus")} isDisabled={statusBusyId === order.id || !!busy} onPress={() => void refreshGatewayStatus(order.id)}>
                        <RefreshCw className={`size-4 ${statusBusyId === order.id ? "animate-spin" : ""}`} aria-hidden="true" />
                      </Button> : null}
                    </div> : null}
                    {order.buyerRecheckResult === "queued" ? <p role="status" className="font-medium text-primary">{t("marketWallet.recheckQueued")}</p>
                      : order.buyerRecheckResult === "retrying" ? <p role="status" className="text-amber-700">{t("marketWallet.recheckRetrying")}</p>
                        : order.buyerRecheckCompletedAt && order.buyerRecheckResult && recheckResultIsCurrent ? <p role="status">{t(`marketWallet.recheckResult.${order.buyerRecheckResult}` as Parameters<typeof t>[0], { time: dateText(order.buyerRecheckCompletedAt, locale) })}</p> : null}
                    {order.platformFeeMinor > 0 ? <p>{t("walletPolicy.orderBreakdown", { fee: formatMinorUsd(order.platformFeeMinor), total: formatMinorUsd(order.amountMinor) })}</p> : null}
                    {neverCreated ? <p>{t("marketWallet.neverCreatedHelp")}</p> : null}
                    {order.paidAt ? <p>{t("marketWallet.paidAt", { time: dateText(order.paidAt, locale) })}</p> : null}
                    {deadline && order.status === "pending" && !abandoned ? <p>{t("marketWallet.gatewayDeadline", { time: dateText(deadline, locale) })}</p> : null}
                    {countdown ? <p className={`font-medium tabular-nums ${countdown.phase === "soon" ? "text-amber-700" : ""}`}>
                      {countdown.phase === "checking"
                        ? t("marketWallet.autoCheckPending")
                        : t("marketWallet.autoCheckCountdown", { time: formatTopupCountdown(countdown.remainingSeconds) })}
                    </p> : null}
                    {countdown?.phase === "soon" ? <p className="text-amber-700">{t("marketWallet.autoCheckSoon")}</p> : null}
                    {gateway?.gatewayStatus === "awaiting_payment" && gateway.canPay && order.status === "pending" && !abandoned ? <p>{t("marketWallet.awaitingMethodHelp")}</p> : null}
                    {order.status === "pending" && gateway && !gateway.canPay && !abandoned ? <p className="text-amber-700">{t(`marketWallet.payability.${gateway.payabilityReason ?? "gateway_unknown"}` as Parameters<typeof t>[0])}</p> : null}
                    {abandoned && !CLOSED_TOPUP_STATUS.has(order.status) ? <p>{t("marketWallet.abandonedHelp")}</p> : null}
                    {order.status === "review_required" ? <p className="text-amber-700">{t(order.reviewReason === "terminal_payment_marker" ? "marketWallet.reviewTerminalMarker" : order.reviewReason === "paid_receipt_invalid" ? "marketWallet.reviewInvalidReceipt" : order.reviewReason === "paid_terms_review" ? "marketWallet.reviewPaidTerms" : "marketWallet.reviewHelp")}</p> : null}
                    {order.lastGatewayCheckedAt ? <p>{t("marketWallet.lastGatewayCheck", { time: dateText(order.lastGatewayCheckedAt, locale), status: lastGatewayLabel })}</p> : null}
                    {order.status === "pending" && gateway?.canCancel ? <div className="pt-1">
                      <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => void chooseCancelAction(order)}>
                        <Ban className="size-4" aria-hidden="true" />{t("marketWallet.cancelPayment")}
                      </Button>
                    </div> : null}
                  </div>
                </details>
              </li>;
            })}
            {loaded && !topups.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTopups")}</li> : null}
          </ul>
        </WalletHistorySection>
        <WalletHistorySection title={t("marketWallet.transactions")} collapsed={!historyExpanded.transactions && transactions.length > HISTORY_PREVIEW_LIMIT}
          hasMore={!!transactionCursor} loading={busy === "transactions"} disabled={!!busy}
          onShowEarlier={() => setHistoryExpanded((current) => ({ ...current, transactions: true }))} onLoadMore={() => loadMore("transactions")}>
          <ul>
            {(historyExpanded.transactions ? transactions : transactions.slice(0, HISTORY_PREVIEW_LIMIT)).map((entry) => {
              const delta = safeBigInt(entry.deltaUnits);
              const heldDelta = safeBigInt(entry.heldDeltaUnits);
              // A hold or release moves money between available and held without
              // changing the posted balance; show that movement instead of $0.00.
              const holdOnly = delta === 0n && heldDelta !== 0n;
              const entryKey = walletEntryKey(entry);
              const entryLabel = t(entryKey, entry.counterpartyEmail ? { email: entry.counterpartyEmail } : undefined);
              const amountTone = holdOnly ? "text-muted-foreground" : delta > 0n ? "text-emerald-800" : delta < 0n ? "text-foreground" : "text-muted-foreground";
              const counterparty = entry.counterpartyEmail && !entryKey.startsWith("marketWallet.entry.transfer") ? entry.counterpartyEmail : null;
              const holdNote = holdOnly ? t(heldDelta > 0n ? "marketWallet.entryHeld" : "marketWallet.entryReleased") : null;
              const entryMain = <>
                {entry.c2cOrderId ? <button type="button" onClick={() => { setTask("c2c"); setC2COrderId(entry.c2cOrderId ?? null); }} className="min-w-0 truncate text-left font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">{entryLabel}</button> : <span className="min-w-0 truncate font-medium">{entryLabel}</span>}
                <span className="ml-auto shrink-0 text-xs text-muted-foreground">{dateText(entry.createdAt, locale)}</span>
                {holdOnly ? <span className="shrink-0 tabular-nums text-muted-foreground">{formatWalletUnits((heldDelta < 0n ? -heldDelta : heldDelta).toString())}</span>
                  : <span className={`shrink-0 tabular-nums ${amountTone}`}>{signedWalletUnits(entry.deltaUnits)}</span>}
              </>;
              return <li key={entry.id} className="border-b border-border/70 text-sm last:border-b-0">
                {counterparty || holdNote ? <details className="group">
                  <summary className="flex min-h-9 cursor-pointer list-none items-center gap-2 py-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
                    <ChevronDown className="size-3.5 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" aria-hidden="true" />
                    <span className="sr-only">{t("marketWallet.entryDetails")}</span>
                    {entryMain}
                  </summary>
                  <div className="grid gap-1 pb-2 pl-5 text-xs text-muted-foreground">
                    {counterparty ? <p className="break-all">{counterparty}</p> : null}
                    {holdNote ? <p>{holdNote}</p> : null}
                  </div>
                </details> : <div className="flex min-h-9 items-center gap-2 py-1 pl-5">{entryMain}</div>}
              </li>;
            })}
            {loaded && !transactions.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTransactions")}</li> : null}
          </ul>
        </WalletHistorySection>
      </div> : null}
      {wallet.hasWalletActivity ? <MarketBuyerDisputesList /> : null}
      <ConfirmAlertDialog
        open={reviewRetryConfirm}
        title={t("marketWallet.reviewRetryTitle")}
        description={t("marketWallet.reviewRetryWarning")}
        confirmLabel={t("marketWallet.startSeparateTopup")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "topup"}
        onConfirm={() => void startTopup()}
        onOpenChange={(open) => { if (busy !== "topup") setReviewRetryConfirm(open); }}
      />
      <ConfirmAlertDialog
        open={!!cancelConfirmId}
        title={t("marketWallet.cancelPaymentTitle")}
        description={t("marketWallet.cancelPaymentNote")}
        confirmLabel={t("marketWallet.cancelPayment")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "cancelTopup"}
        onConfirm={() => void cancelSelectedTopup()}
        onOpenChange={(open) => { if (busy !== "cancelTopup" && !open) setCancelConfirmId(null); }}
      />
      <ConfirmAlertDialog
        open={!!abandonConfirmId}
        title={t("marketWallet.abandonTopupTitle")}
        description={t("marketWallet.abandonTopupNote")}
        confirmLabel={t("marketWallet.abandonTopup")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "abandonTopup"}
        onConfirm={() => void abandonSelectedTopup()}
        onOpenChange={(open) => { if (busy !== "abandonTopup" && !open) setAbandonConfirmId(null); }}
      />
      <ConfirmAlertDialog
        open={!!cancelUnavailableId}
        title={t("marketWallet.cancelUnavailableTitle")}
        description={cancelUnavailableDescription}
        confirmLabel={t("marketWallet.recheck")}
        cancelLabel={t("common.close")}
        tone="warning"
        busy={!!statusBusyId}
        onConfirm={() => {
          const id = cancelUnavailableId;
          setCancelUnavailableId(null);
          if (id) void requestRecheck(id);
        }}
        onOpenChange={(open) => { if (!open) setCancelUnavailableId(null); }}
      />
    </main>
  );
}
