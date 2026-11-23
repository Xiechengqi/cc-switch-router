"use client";

/* Platform balance: top-up first.
 * Layout: balance → top up (primary) → needs attention (only when something
 * does) → send / C2C (secondary, in drawers) → one activity list.
 * Security, limits and corrections are fixed rules enforced by the server;
 * nothing here asks people to manage them. */

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Button } from "@heroui/react";
import { ArrowRightLeft, ArrowUpRight, CheckCircle2, ChevronRight, Clock3, RefreshCw, Users, WalletCards } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { AccountSectionHeader } from "@/components/dashboard/account-section";
import type { C2CView } from "@/components/dashboard/c2c-orders-panel";
import { MarketBuyerDisputesList } from "@/components/dashboard/market-buyer-disputes-list";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletSectionLabel } from "@/components/market/wallet-primitives";
import { WalletTopupForm } from "@/components/market/wallet-topup-form";
import { WalletC2CDrawer, WalletTransferDrawer, type TransferPrefill } from "@/components/wallet/wallet-action-drawers";
import { WalletBalanceHero } from "@/components/wallet/wallet-balance-hero";
import { WalletLedger, unitsToDollarInput } from "@/components/wallet/wallet-ledger";
import { WalletPendingActions } from "@/components/wallet/wallet-pending-actions";
import { useWalletData } from "@/components/wallet/use-wallet-data";
import { ApiError } from "@/lib/api";
import { WALLET_ACTIONS_ID } from "@/lib/dashboard-nav";
import { getMarketWalletTopup, recheckMarketWalletTopup, type MarketWallet, type MarketWalletTopup } from "@/lib/market-wallet-api";
import { safeMarketReturnPath } from "@/lib/market-wallet-money";

export { WalletActionButton } from "@/components/market/wallet-topup-form";

const C2C_VIEWS: C2CView[] = ["buy", "sell", "orders"];

function linkedC2CView(params: { get(name: string): string | null }): C2CView | null {
  if (params.get("c2cOrder")) return "orders";
  if (params.get("c2cSell") === "1") return "sell";
  const requested = params.get("c2c") as C2CView | null;
  return requested && C2C_VIEWS.includes(requested) ? requested : null;
}

const SECONDARY_CARD = "group flex min-h-16 min-w-0 items-center gap-4 rounded-2xl border border-border bg-card p-4 text-left shadow-sm transition-all duration-300 ease-out hover:-translate-y-0.5 hover:border-accent/30 hover:shadow-lg focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring motion-reduce:transition-none motion-reduce:hover:translate-y-0";
const ICON_TILE = "grid size-11 shrink-0 place-items-center rounded-xl bg-gradient-to-br from-[#0052FF] to-[#4D7CFF] text-white shadow-[0_4px_14px_rgba(0,82,255,0.25)] transition-transform duration-300 group-hover:scale-105 motion-reduce:transition-none";

/** `rememberKey` keeps the latest balance for the next visit to paint at once. */
export function PlatformWalletPanel({ initialWallet, rememberKey }: { initialWallet: MarketWallet; rememberKey?: string }) {
  const { session } = useAuth();
  const { t } = useLocaleText();
  const params = useSearchParams();
  const actorId = session?.user?.id || "";
  const data = useWalletData(actorId, initialWallet, rememberKey);
  const { wallet } = data;

  const [transferOpen, setTransferOpen] = React.useState(params.get("task") === "transfer");
  const [prefill, setPrefill] = React.useState<TransferPrefill>(null);
  const [c2cView, setC2CView] = React.useState<C2CView>(() => linkedC2CView(params) ?? "buy");
  const [c2cOpen, setC2COpen] = React.useState(() => params.get("task") === "c2c" || linkedC2CView(params) !== null);
  const [c2cOrderId, setC2COrderId] = React.useState<string | null>(params.get("c2cOrder"));
  const [formBusy, setFormBusy] = React.useState(false);
  const [refreshError, setRefreshError] = React.useState("");

  // Links from notifications and other pages land on the matching drawer.
  const linkedOrder = params.get("c2cOrder");
  const linkedTask = params.get("task");
  React.useEffect(() => {
    if (linkedOrder) { setC2COrderId(linkedOrder); setC2CView("orders"); setC2COpen(true); }
  }, [linkedOrder]);
  React.useEffect(() => {
    if (linkedTask === "transfer") setTransferOpen(true);
    if (linkedTask === "c2c") setC2COpen(true);
  }, [linkedTask]);

  // Returning from checkout: follow that order until it settles.
  const returnedTopupId = params.get("topup");
  const [returnedTopup, setReturnedTopup] = React.useState<MarketWalletTopup | null>(null);
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const suggestedAmount = params.get("amount");
  const { reload } = data;
  React.useEffect(() => {
    let saved: string | null = null;
    try {
      if (returnedTopupId) {
        const key = `marketWalletReturnTo:${returnedTopupId}`;
        saved = safeMarketReturnPath(window.sessionStorage.getItem(key)) ?? safeMarketReturnPath(window.localStorage.getItem(key));
      }
    } catch {
      // Storage is optional; the wallet works without it.
    }
    setReturnPath(safeMarketReturnPath(params.get("returnTo")) ?? saved);
  }, [params, returnedTopupId]);
  React.useEffect(() => {
    if (!returnedTopupId) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    // The buyer just paid or left checkout: ask for a fresh check once.
    void recheckMarketWalletTopup(returnedTopupId).catch(() => {});
    const poll = async () => {
      try {
        const order = await getMarketWalletTopup(returnedTopupId);
        if (!active) return;
        setReturnedTopup(order);
        data.replaceTopup(order);
        const state = order.buyerView?.state;
        const settled = state ? state === "credited" || state === "closed" : ["credited", "expired", "cancelled"].includes(order.status);
        if (settled) {
          try { window.localStorage.removeItem(`marketWalletReturnTo:${returnedTopupId}`); } catch { /* optional */ }
          if (order.status === "credited") await reload().catch(() => {});
          return;
        }
      } catch (cause) {
        if (!active) return;
        // A rejected order id is final; transient failures keep following.
        if (cause instanceof ApiError && cause.status < 500 && cause.status !== 429) return;
      }
      if (active) timer = setTimeout(poll, 5_000);
    };
    void poll();
    return () => { active = false; if (timer) clearTimeout(timer); };
  }, [returnedTopupId]);

  const refresh = () => {
    setRefreshError("");
    void reload().catch(() => setRefreshError(t("walletPage.refreshFailed")));
  };
  const credited = returnedTopup?.status === "credited";

  return (
    <div className="grid w-full min-w-0 gap-6">
      <AccountSectionHeader
        icon={WalletCards}
        title={t("account.nav.wallet")}
        hint={t("marketWallet.subtitle")}
        actions={(
          <Button variant="outline" size="sm" isDisabled={formBusy} onPress={refresh}>
            <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
          </Button>
        )}
      />

      {returnedTopup || returnPath ? (
        <div role="status" aria-live="polite" className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-xl border border-border bg-card px-4 py-3 text-sm">
          {returnedTopup ? (
            <span className="flex items-center gap-2">
              {credited ? <CheckCircle2 className="size-4 text-emerald-700" aria-hidden="true" /> : <Clock3 className="size-4 text-muted-foreground" aria-hidden="true" />}
              {t(`walletPage.topupState.${returnedTopup.buyerView?.state ?? (credited ? "credited" : "confirming")}`)}
            </span>
          ) : <span />}
          {returnPath ? (
            <Link href={returnPath} className="inline-flex min-h-11 items-center whitespace-nowrap font-medium text-accent underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t(credited ? "walletPage.returnedToMarket" : "walletPage.returnWhilePending")}<ArrowUpRight className="ml-1 size-4" aria-hidden="true" />
            </Link>
          ) : null}
        </div>
      ) : null}
      {refreshError || data.error ? <p role="alert" className="rounded-xl border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">{refreshError || data.error}</p> : null}

      <div className="grid min-w-0 gap-6 lg:grid-cols-[1.1fr_0.9fr] lg:items-start">
        <div className="grid min-w-0 gap-6">
          <WalletBalanceHero wallet={wallet} />
          <section
            id={WALLET_ACTIONS_ID}
            tabIndex={-1}
            aria-labelledby="wallet-topup-title"
            className="grid min-w-0 scroll-mt-20 gap-4 rounded-2xl border border-border bg-card p-5 shadow-md outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring sm:p-6"
          >
            <h2 id="wallet-topup-title" className="sr-only">{t("walletPage.topupLabel")}</h2>
            <WalletSectionLabel>{t("walletPage.topupLabel")}</WalletSectionLabel>
            <WalletTopupForm
              wallet={wallet}
              topups={data.topups}
              ready={data.ready}
              returnPath={returnPath}
              suggestedAmount={suggestedAmount}
              onBusyChange={setFormBusy}
              onOrderCreated={data.replaceTopup}
              onReload={reload}
            />
            <p className="text-xs leading-5 text-muted-foreground">{t("walletPage.topupNote")}</p>
          </section>
        </div>

        <div className="grid min-w-0 gap-6">
          <WalletPendingActions
            topups={data.topups}
            awaitingReturns={data.awaitingReturns}
            c2cActive={data.c2cActive}
            onChanged={reload}
            onTopupChanged={data.replaceTopup}
            onOpenC2C={() => { setC2CView("orders"); setC2COpen(true); }}
          />
          <section aria-label={t("walletPage.moreLabel")} className="grid min-w-0 gap-3">
            <button type="button" className={SECONDARY_CARD} onClick={() => { setPrefill(null); setTransferOpen(true); }}>
              <span className={ICON_TILE}><ArrowRightLeft className="size-5" aria-hidden /></span>
              <span className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-base font-semibold tracking-tight">{t("walletPage.send")}</span>
                <span className="text-sm text-muted-foreground">{t("walletPage.sendHint")}</span>
              </span>
              <ChevronRight className="size-5 shrink-0 text-muted-foreground transition-transform duration-200 group-hover:translate-x-1 motion-reduce:transition-none" aria-hidden />
            </button>
            <button type="button" className={SECONDARY_CARD} onClick={() => setC2COpen(true)}>
              <span className={ICON_TILE}><Users className="size-5" aria-hidden /></span>
              <span className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-base font-semibold tracking-tight">{t("walletPage.c2c")}</span>
                <span className="text-sm text-muted-foreground">{t("walletPage.c2cHint")}</span>
              </span>
              <ChevronRight className="size-5 shrink-0 text-muted-foreground transition-transform duration-200 group-hover:translate-x-1 motion-reduce:transition-none" aria-hidden />
            </button>
          </section>
        </div>
      </div>

      {wallet.canTopup || wallet.hasWalletActivity ? (
        <WalletLedger
          topups={data.topups}
          transactions={data.transactions}
          hasMoreTopups={!!data.topupCursor}
          hasMoreTransactions={!!data.transactionCursor}
          loadMore={data.loadMore}
          onSendBack={(email, amountUnits) => { setPrefill({ email, amount: unitsToDollarInput(amountUnits) }); setTransferOpen(true); }}
          onOpenC2COrder={(id) => { setC2COrderId(id); setC2CView("orders"); setC2COpen(true); }}
        />
      ) : null}
      {wallet.hasWalletActivity ? <MarketBuyerDisputesList /> : null}

      <WalletTransferDrawer
        open={transferOpen}
        onClose={() => setTransferOpen(false)}
        prefill={prefill}
        walletVersion={wallet.version}
        onTransferred={reload}
      />
      {actorId ? (
        <WalletC2CDrawer
          open={c2cOpen}
          onClose={() => setC2COpen(false)}
          actor={actorId}
          view={c2cView}
          onViewChange={setC2CView}
          orderId={c2cOrderId}
          onOrderChange={setC2COrderId}
          onActiveCountChange={data.setC2CActive}
        />
      ) : null}
    </div>
  );
}
