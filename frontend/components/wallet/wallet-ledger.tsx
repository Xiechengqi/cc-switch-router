"use client";

import * as React from "react";
import { ArrowUpRight, Loader2, Undo2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletBadge, type WalletBadgeTone } from "@/components/market/wallet-primitives";
import { messages, type MessageKey } from "@/lib/i18n";
import type { MarketTopupBuyerState, MarketWalletTopup, MarketWalletTransaction } from "@/lib/market-wallet-api";
import { formatMinorUsd, formatWalletUnits, walletTopupCheckoutHref } from "@/lib/market-wallet-money";
import { cn } from "@/lib/utils";

export type LedgerFilter = "all" | "topup" | "transfer" | "c2c" | "market";
const FILTERS: LedgerFilter[] = ["all", "topup", "transfer", "c2c", "market"];

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
  const [loading, setLoading] = React.useState(false);
  const date = (value: string) => {
    const parsed = new Date(value);
    return Number.isNaN(parsed.getTime()) ? value : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(parsed);
  };
  const entries = filter === "all" ? transactions : transactions.filter((entry) => category(entry) === filter);
  const more = filter === "topup" ? hasMoreTopups : hasMoreTransactions;
  const showMore = async () => {
    if (loading) return;
    setLoading(true);
    try { await loadMore(filter === "topup" ? "topups" : "transactions"); } finally { setLoading(false); }
  };

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
              aria-selected={filter === item}
              onClick={() => setFilter(item)}
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
      <ul role="tabpanel" className="divide-y divide-border">
        {filter === "topup"
          ? topups.map((order) => {
            const state = topupState(order);
            const fee = order.platformFeeMinor > 0 ? t("walletPage.feeIncluded", { fee: formatMinorUsd(order.platformFeeMinor), total: formatMinorUsd(order.amountMinor) }) : null;
            return (
              <li key={order.id} className="flex min-w-0 items-center gap-3 py-3">
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
          : entries.map((entry) => {
            const delta = big(entry.deltaUnits);
            const held = big(entry.heldDeltaUnits);
            const holdOnly = delta === 0n && held !== 0n;
            const incomingTransfer = entry.sourceKind === "wallet_transfer" && delta > 0n && !!entry.counterpartyEmail;
            const counterparty = entry.counterpartyEmail
              ? t(delta > 0n ? "walletPage.paidFrom" : "walletPage.paidTo", { email: entry.counterpartyEmail })
              : null;
            const label = t(entryKey(entry), entry.counterpartyEmail ? { email: entry.counterpartyEmail } : undefined);
            return (
              <li key={entry.id} className="flex min-w-0 items-center gap-3 py-3">
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
        {(filter === "topup" ? topups.length : entries.length) === 0 ? (
          <li className="py-8 text-center text-sm text-muted-foreground">{t("walletPage.empty")}</li>
        ) : null}
      </ul>
      {more ? (
        <button type="button" onClick={() => void showMore()} disabled={loading}
          className="inline-flex min-h-11 items-center justify-center gap-2 justify-self-center rounded-xl px-4 text-sm font-medium text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50">
          {loading ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
          {t(loading ? "common.loading" : "walletPage.loadMore")}
        </button>
      ) : null}
    </section>
  );
}
