"use client";

import * as React from "react";
import { ArrowUpRight, ChevronLeft, ChevronRight, Loader2, Undo2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletBadge, type WalletBadgeTone } from "@/components/market/wallet-primitives";
import { messages, type MessageKey } from "@/lib/i18n";
import type { MarketTopupBuyerState, MarketWalletTopup, MarketWalletTransaction } from "@/lib/market-wallet-api";
import { formatMinorUsd, formatWalletUnits, walletTopupCheckoutHref } from "@/lib/market-wallet-money";
import { cn } from "@/lib/utils";

export type LedgerFilter = "all" | "topup" | "transfer" | "c2c" | "market";
const FILTERS: LedgerFilter[] = ["all", "topup", "transfer", "c2c", "market"];
/** Rows per page. Every row is h-16, so the card never changes size. */
export const LEDGER_PAGE_SIZE = 8;
const ROW = "flex h-16 min-w-0 items-center gap-3";
/** LEDGER_PAGE_SIZE rows of h-16 (4rem). */
const LIST_HEIGHT = "h-[32rem]";
const PAGER_BUTTON = "inline-flex size-11 items-center justify-center rounded-xl border border-border text-foreground transition-all duration-200 hover:-translate-y-0.5 hover:border-accent/30 hover:shadow-sm active:scale-[0.98] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-40 motion-reduce:transition-none motion-reduce:hover:translate-y-0";

function big(raw: string) {
  try { return BigInt(raw); } catch { return 0n; }
}

function category(entry: MarketWalletTransaction): Exclude<LedgerFilter, "all" | "topup"> | "topup" {
  if (entry.c2cOrderId) return "c2c";
  if (entry.sourceKind.startsWith("wallet_transfer")) return "transfer";
  if (entry.sourceKind === "topup" || entry.sourceKind.startsWith("topup")) return "topup";
  return "market";
}

/** Entry label keys stay compatible with the existing ledger vocabulary. */
function entryKey(entry: MarketWalletTransaction): MessageKey {
  if (entry.c2cOrderId) {
    if (entry.action.startsWith("reserve:")) return "marketWallet.entry.c2cHold";
    if (entry.sourceKind === "hold" || entry.action === "release_hold") return "marketWallet.entry.c2cReleaseHold";
    if (entry.action === "release") return big(entry.deltaUnits) > 0n ? "marketWallet.entry.c2cReleaseIn" : "marketWallet.entry.c2cReleaseOut";
  }
  if (entry.sourceKind === "wallet_transfer") {
    return big(entry.deltaUnits) > 0n ? "marketWallet.entry.transferIn" : "marketWallet.entry.transferOut";
  }
  if (entry.action.startsWith("reserve:")) return "marketWallet.entry.holdPlaced";
  const key = `marketWallet.entry.${entry.sourceKind}`;
  return (key in messages.en ? key : "marketWallet.entry.other") as MessageKey;
}

export { unitsToDollarInput } from "@/lib/wallet-view";

const STATE_TONE: Record<MarketTopupBuyerState, WalletBadgeTone> = {
  awaiting_payment: "accent",
  confirming: "warning",
  credited: "success",
  closed: "neutral",
};

function topupState(order: MarketWalletTopup): MarketTopupBuyerState {
  if (order.buyerView) return order.buyerView.state;
  if (order.status === "credited") return "credited";
  if (["expired", "cancelled"].includes(order.status)) return "closed";
  return order.status === "pending" && !order.buyerAbandonedAt ? "awaiting_payment" : "confirming";
}

/**
 * One activity list with plain filters instead of separate history cards.
 * Top-ups list orders (including unpaid ones); the other filters list balance
 * movements. A received transfer offers "send back" as an ordinary transfer.
 *
 * Pages are fixed-height and fixed-size. Server pages load on demand when a
 * page needs rows that have not been fetched yet.
 */
export function WalletLedger({
  topups,
  transactions,
  hasMoreTopups,
  hasMoreTransactions,
  loadMore,
  onSendBack,
  onOpenC2COrder,
}: {
  topups: MarketWalletTopup[];
  transactions: MarketWalletTransaction[];
  hasMoreTopups: boolean;
  hasMoreTransactions: boolean;
  loadMore: (kind: "topups" | "transactions") => Promise<void>;
  onSendBack: (email: string, amountUnits: string) => void;
  onOpenC2COrder: (id: string) => void;
}) {
  const { t, locale } = useLocaleText();
  const [filter, setFilter] = React.useState<LedgerFilter>("all");
  const [page, setPage] = React.useState(0);
  const [loading, setLoading] = React.useState(false);
  const [loadError, setLoadError] = React.useState(false);
  const inFlight = React.useRef(false);
  const date = (value: string) => {
    const parsed = new Date(value);
    return Number.isNaN(parsed.getTime()) ? value : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(parsed);
  };
  const entries = filter === "all" ? transactions : transactions.filter((entry) => category(entry) === filter);
  const total = filter === "topup" ? topups.length : entries.length;
  const more = filter === "topup" ? hasMoreTopups : hasMoreTransactions;
  const first = page * LEDGER_PAGE_SIZE;
  const needed = first + LEDGER_PAGE_SIZE;
  const lastPage = Math.max(0, Math.ceil(total / LEDGER_PAGE_SIZE) - 1);
  const hasNext = total > needed || more;

  // Fetch older server pages until this page is full or history ends. Each
  // load re-renders with a new cursor, so this runs again with fresh counts.
  React.useEffect(() => {
    if (inFlight.current || loadError || !more || total >= needed) return;
    inFlight.current = true;
    setLoading(true);
    loadMore(filter === "topup" ? "topups" : "transactions")
      .catch(() => setLoadError(true))
      .finally(() => {
        inFlight.current = false;
        setLoading(false);
      });
  }, [filter, loadError, loadMore, loading, more, needed, total]);

  // A refresh can shorten history (for example after a filter change); never
  // stay on a page past the end.
  React.useEffect(() => {
    if (!more && page > lastPage) setPage(lastPage);
  }, [lastPage, more, page]);

  const choose = (next: LedgerFilter) => {
    setFilter(next);
    setPage(0);
    setLoadError(false);
  };
  const turn = (delta: number) => {
    setLoadError(false);
    setPage((current) => Math.max(0, current + delta));
  };
  const shown = Math.min(total, needed);
  // The total is known only once the whole history has been fetched.
  const range = total <= first ? "" : more
    ? t("walletPage.pageRange", { from: first + 1, to: shown })
    : t("walletPage.pageRangeTotal", { from: first + 1, to: shown, total });
  const waiting = total <= first && (loading || more) && !loadError;

  return (
    <section aria-labelledby="wallet-ledger-title" className="grid min-w-0 gap-4 rounded-2xl border border-border bg-card p-5 shadow-sm sm:p-6">
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
        <h2 id="wallet-ledger-title" className="text-lg font-semibold tracking-tight">{t("walletPage.ledger")}</h2>
        <div role="tablist" aria-label={t("walletPage.ledger")} className="-mx-1 flex max-w-full gap-1 overflow-x-auto px-1">
          {FILTERS.map((item) => (
            <button
              key={item}
              type="button"
              role="tab"
              id={`wallet-ledger-tab-${item}`}
              aria-controls="wallet-ledger-panel"
              aria-selected={filter === item}
              onClick={() => choose(item)}
              className={cn(
                "min-h-9 whitespace-nowrap rounded-full px-3 text-sm font-medium transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
                filter === item ? "bg-foreground text-white" : "text-muted-foreground hover:bg-muted hover:text-foreground",
              )}
            >
              {t(`walletPage.filter.${item}`)}
            </button>
          ))}
        </div>
      </div>
      <div
        id="wallet-ledger-panel"
        role="tabpanel"
        aria-labelledby={`wallet-ledger-tab-${filter}`}
        aria-busy={loading}
        className={cn(LIST_HEIGHT, "min-w-0 overflow-y-auto overscroll-contain")}
      >
        {waiting ? (
          <div className="flex h-full items-center justify-center text-muted-foreground">
            <Loader2 className="size-5 animate-spin motion-reduce:animate-none" aria-hidden />
            <span className="sr-only">{t("common.loading")}</span>
          </div>
        ) : loadError && total <= first ? (
          <div role="alert" className="flex h-full flex-col items-center justify-center gap-3 text-center text-sm text-muted-foreground">
            <p>{t("walletPage.pageFailed")}</p>
            <button type="button" onClick={() => setLoadError(false)} className={cn(PAGER_BUTTON, "w-auto px-4 font-medium")}>
              {t("common.retry")}
            </button>
          </div>
        ) : total <= first ? (
          <p className="flex h-full items-center justify-center text-center text-sm text-muted-foreground">{t("walletPage.empty")}</p>
        ) : (
          <ul className="divide-y divide-border">
            {filter === "topup"
              ? topups.slice(first, needed).map((order) => {
                const state = topupState(order);
                const fee = order.platformFeeMinor > 0 ? t("walletPage.feeIncluded", { fee: formatMinorUsd(order.platformFeeMinor), total: formatMinorUsd(order.amountMinor) }) : null;
                return (
                  <li key={order.id} className={ROW}>
                    <div className="grid min-w-0 flex-1 gap-0.5">
                      <p className="text-sm font-semibold tabular-nums">{formatMinorUsd(order.creditAmountMinor ?? order.amountMinor)}</p>
                      <p className="truncate text-xs text-muted-foreground">{date(order.createdAt)}{fee ? ` · ${fee}` : ""}</p>
                    </div>
                    {state === "awaiting_payment" && order.buyerView?.canPay !== false ? (
                      <a href={walletTopupCheckoutHref(order.id)} target="_blank" rel="noopener noreferrer" className="inline-flex min-h-11 items-center gap-1 text-sm font-semibold text-accent hover:underline focus-visible:outline-2 focus-visible:outline-ring">
                        {t("walletPage.continuePay")}<ArrowUpRight className="size-4" aria-hidden />
                      </a>
                    ) : null}
                    <WalletBadge tone={STATE_TONE[state]}>{t(`walletPage.topupState.${state}`)}</WalletBadge>
                  </li>
                );
              })
              : entries.slice(first, needed).map((entry) => {
                const delta = big(entry.deltaUnits);
                const held = big(entry.heldDeltaUnits);
                const holdOnly = delta === 0n && held !== 0n;
                const incomingTransfer = entry.sourceKind === "wallet_transfer" && delta > 0n && !!entry.counterpartyEmail;
                const counterparty = entry.counterpartyEmail
                  ? t(delta > 0n ? "walletPage.paidFrom" : "walletPage.paidTo", { email: entry.counterpartyEmail })
                  : null;
                const label = t(entryKey(entry), entry.counterpartyEmail ? { email: entry.counterpartyEmail } : undefined);
                return (
                  <li key={entry.id} className={ROW}>
                    <div className="grid min-w-0 flex-1 gap-0.5">
                      {entry.c2cOrderId ? (
                        <button type="button" onClick={() => onOpenC2COrder(entry.c2cOrderId!)} className="min-w-0 truncate text-left text-sm font-medium text-accent hover:underline focus-visible:outline-2 focus-visible:outline-ring">
                          {label}
                        </button>
                      ) : <p className="truncate text-sm font-medium">{label}</p>}
                      <p className="truncate text-xs text-muted-foreground">{date(entry.createdAt)}{counterparty && !label.includes(entry.counterpartyEmail ?? "\u0000") ? ` · ${counterparty}` : ""}</p>
                    </div>
                    {incomingTransfer ? (
                      <button type="button" onClick={() => onSendBack(entry.counterpartyEmail!, entry.deltaUnits)}
                        className="inline-flex min-h-11 shrink-0 items-center gap-1 rounded-xl px-2 text-xs font-semibold text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring">
                        <Undo2 className="size-3.5" aria-hidden />{t("walletPage.sendBack")}
                      </button>
                    ) : null}
                    <span className={cn("shrink-0 text-sm font-semibold tabular-nums", holdOnly ? "text-muted-foreground" : delta > 0n ? "text-emerald-700" : "text-foreground")}>
                      {holdOnly
                        ? formatWalletUnits((held < 0n ? -held : held).toString())
                        : `${delta > 0n ? "+" : ""}${formatWalletUnits(entry.deltaUnits)}`}
                    </span>
                  </li>
                );
              })}
          </ul>
        )}
      </div>
      {page > 0 || hasNext ? (
        <nav aria-label={t("walletPage.pages")} className="flex min-w-0 items-center justify-between gap-3 border-t border-border pt-4">
          <p aria-live="polite" className="font-mono text-xs uppercase tracking-[0.15em] text-muted-foreground tabular-nums">{range}</p>
          <div className="flex gap-2">
            <button type="button" className={PAGER_BUTTON} aria-label={t("walletPage.prevPage")} disabled={page === 0} onClick={() => turn(-1)}>
              <ChevronLeft className="size-4" aria-hidden />
            </button>
            <button type="button" className={PAGER_BUTTON} aria-label={t("walletPage.nextPage")} disabled={!hasNext || waiting} onClick={() => turn(1)}>
              <ChevronRight className="size-4" aria-hidden />
            </button>
          </div>
        </nav>
      ) : null}
    </section>
  );
}
