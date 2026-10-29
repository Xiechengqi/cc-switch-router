"use client";

/* Hallmark · component: quick balance dialog · genre: modern-minimal · theme: existing Router system
 * states: loading · ready · error · busy (top-up in flight blocks closing)
 * Top-up, C2C purchase and sending without leaving the current page. The
 * balance page stays the place for history, order follow-up and selling.
 */

import * as React from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Button, Modal } from "@heroui/react";
import { ArrowUpRight, Loader2, WalletCards } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { C2CBuyPanel } from "@/components/dashboard/c2c-buy-panel";
import { WalletTransferPanel } from "@/components/dashboard/wallet-transfer-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletTopupForm } from "@/components/market/wallet-topup-form";
import { C2CStatusLabel } from "@/components/market/c2c-primitives";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { notifyMarketBalanceChanged, subscribeMarketBalanceChanged } from "@/lib/market-balance-events";
import { c2cErrorKey, getC2COrders, type C2COrder } from "@/lib/market-c2c";
import {
  getMarketWallet,
  getMarketWalletTopups,
  type MarketWallet,
  type MarketWalletTopup,
} from "@/lib/market-wallet-api";
import { formatWalletUnits, formatWalletUnitsAsCny, safeMarketReturnPath } from "@/lib/market-wallet-money";

const TASKS = ["topup", "c2c", "transfer"] as const;
type Task = (typeof TASKS)[number];

export function WalletQuickDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { session } = useAuth();
  const router = useRouter();
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const actor = session?.authenticated ? session.user?.id : undefined;
  const [snapshot, setSnapshot] = React.useState<{ actor: string; wallet: MarketWallet; topups: MarketWalletTopup[] } | null>(null);
  const [error, setError] = React.useState("");
  const [task, setTask] = React.useState<Task>("topup");
  const [busy, setBusy] = React.useState(false);
  const [ready, setReady] = React.useState(false);
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const loadVersion = React.useRef(0);

  const load = React.useCallback(async () => {
    if (!actor) return;
    const version = ++loadVersion.current;
    let wallet: MarketWallet;
    let topups: Awaited<ReturnType<typeof getMarketWalletTopups>>;
    try {
      [wallet, topups] = await Promise.all([getMarketWallet(), getMarketWalletTopups()]);
    } catch (cause) {
      if (version === loadVersion.current) { setReady(false); setError(cause instanceof Error ? cause.message : String(cause)); }
      return;
    }
    if (version !== loadVersion.current) return;
    setSnapshot((current) => ({
      actor,
      wallet,
      // Keep orders created in this dialog that the first page may not list yet.
      topups: current?.actor === actor
        ? [...topups.items, ...current.topups.filter((order) => !topups.items.some((item) => item.id === order.id))]
        : topups.items,
    }));
    setError("");
    setReady(true);
  }, [actor]);

  React.useEffect(() => {
    if (!open) return;
    // Reopening is a new read barrier: cached history must not authorize a payment.
    setReady(false);
    setReturnPath(safeMarketReturnPath(`${window.location.pathname}${window.location.search}`));
    const refresh = () => { if (document.visibilityState === "visible") void load(); };
    refresh();
    const timer = window.setInterval(refresh, 30_000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    const off = actor ? subscribeMarketBalanceChanged(actor, refresh) : () => {};
    return () => { loadVersion.current++; window.clearInterval(timer); off(); window.removeEventListener("focus", refresh); document.removeEventListener("visibilitychange", refresh); };
  }, [actor, load, open]);

  const reload = React.useCallback(async () => {
    if (actor) notifyMarketBalanceChanged(actor);
    await load();
  }, [actor, load]);

  const current = snapshot?.actor === actor ? snapshot : null;
  const amount = current
    ? displayCurrency === "CNY" && fxRateFresh
      ? formatWalletUnitsAsCny(current.wallet.availableUnits, fxRateMicros, locale)
      : formatWalletUnits(current.wallet.availableUnits, "$", locale)
    : null;

  const close = () => { if (!busy) onOpenChange(false); };
  if (!open) return null;

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => { if (next || !busy) onOpenChange(next); }}>
      <Modal.Container placement="center" className="p-0 sm:p-4">
        <Modal.Dialog className="light flex h-dvh w-full max-w-none flex-col overflow-hidden rounded-none bg-card text-card-foreground sm:h-auto sm:max-h-[calc(100dvh-2rem)] sm:max-w-[34rem] sm:rounded-xl">
          <Modal.CloseTrigger isDisabled={busy} />
          <Modal.Header>
            <Modal.Heading className="flex items-center gap-2">
              <WalletCards className="size-4 text-muted-foreground" aria-hidden />
              {t("walletQuick.title")}
            </Modal.Heading>
          </Modal.Header>
          <Modal.Body className="grid min-h-0 min-w-0 flex-1 content-start gap-4 overflow-y-auto overscroll-contain sm:max-h-[76dvh]">
            <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-4 gap-y-1 rounded-lg bg-muted/50 px-4 py-3">
              <span className="text-sm text-muted-foreground">{t("walletQuick.available")}</span>
              <span className="break-all text-2xl font-semibold tabular-nums tracking-tight" aria-live="polite">
                {amount ?? (error ? "—" : <Loader2 className="size-5 animate-spin motion-reduce:animate-none" aria-label={t("common.loading")} />)}
              </span>
            </div>
            {error ? (
              <div className="grid justify-items-start gap-2">
                <p role="alert" className="text-sm text-destructive">{t("walletQuick.unavailable")} {error}</p>
                <Button size="sm" variant="outline" onPress={() => void load().catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))}>{t("common.retry")}</Button>
              </div>
            ) : null}
            {current && actor ? (
              <>
                <SegmentedControl
                  value={task}
                  onChange={setTask}
                  ariaLabel={t("marketWallet.actions")}
                  size="md"
                  fullWidth
                  disabled={busy}
                  items={TASKS.map((id) => ({ id, label: t(`marketWallet.action.${id}`) }))}
                />
                <div role="tabpanel" aria-label={t(`marketWallet.action.${task}`)} className="min-w-0">
                  <div hidden={task !== "topup"}>
                    <WalletTopupForm
                      wallet={current.wallet}
                      topups={current.topups}
                      ready={ready}
                      active={open && task === "topup"}
                      returnPath={returnPath}
                      idPrefix="wallet-quick-topup"
                      onBusyChange={setBusy}
                      onOrderCreated={(order) => { loadVersion.current++; setSnapshot((value) => value && value.actor === actor
                        ? { ...value, topups: [order, ...value.topups.filter((item) => item.id !== order.id)] }
                        : value); }}
                      onReload={reload}
                    />
                  </div>
                  {task === "c2c" ? (
                    <div className="grid min-w-0 gap-3">
                      <p className="text-sm text-muted-foreground">{t("walletQuick.c2cHelp")}</p>
                      <C2CBuyPanel
                        actor={actor}
                        inlineConfirmation
                        onBusyChange={setBusy}
                        onOrder={(order) => {
                          onOpenChange(false);
                          router.push(`${DASHBOARD_ACCOUNT_WALLET_PATH}?c2cOrder=${encodeURIComponent(order.id)}`);
                        }}
                      />
                      <QuickC2CBuyerOrders actor={actor} onSelect={(id) => {
                        if (busy) return;
                        onOpenChange(false); router.push(`${DASHBOARD_ACCOUNT_WALLET_PATH}?c2cOrder=${encodeURIComponent(id)}`);
                      }} disabled={busy} />
                      <Link href={`${DASHBOARD_ACCOUNT_WALLET_PATH}?task=c2c`} aria-disabled={busy} tabIndex={busy ? -1 : undefined} onClick={(event) => { if (busy) event.preventDefault(); else close(); }} className="inline-flex min-h-11 items-center gap-1 justify-self-start text-sm font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-disabled:opacity-50">
                        {t("walletQuick.c2cOrders")}<ArrowUpRight className="size-4" aria-hidden />
                      </Link>
                    </div>
                  ) : task === "transfer" ? (
                    <WalletTransferPanel embedded inlineConfirmation onBusyChange={setBusy} walletVersion={current.wallet.version} onTransferred={reload} />
                  ) : null}
                </div>
              </>
            ) : null}
          </Modal.Body>
          <Modal.Footer className="flex-wrap justify-between">
            <Link href={DASHBOARD_ACCOUNT_WALLET_PATH} tabIndex={busy ? -1 : undefined} onClick={(event) => { if (busy) event.preventDefault(); else close(); }} aria-disabled={busy} className="inline-flex min-h-11 items-center gap-1 text-sm font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-disabled:pointer-events-none aria-disabled:opacity-50">
              {t("walletQuick.fullPage")}<ArrowUpRight className="size-4" aria-hidden />
            </Link>
            <Button variant="ghost" className="min-h-11" isDisabled={busy} onPress={close}>{t("common.close")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function QuickC2CBuyerOrders({ actor, disabled, onSelect }: { actor: string; disabled: boolean; onSelect: (id: string) => void }) {
  const { t, locale } = useLocaleText();
  const [orders, setOrders] = React.useState<C2COrder[]>([]);
  const [error, setError] = React.useState("");
  const [loaded, setLoaded] = React.useState(false);
  const [retry, setRetry] = React.useState(0);
  React.useEffect(() => {
    let active = true;
    let reading = false;
    const load = async () => {
      if (reading || document.visibilityState !== "visible") return;
      reading = true;
      try {
        const page = await getC2COrders({ role: "buyer", status: "active" });
        if (page.items.some(item => item.buyerId !== actor)) throw new Error("C2C account changed");
        if (active) { setOrders(page.items.slice(0, 3)); setError(""); setLoaded(true); }
      } catch (cause) { if (active) setError(t(c2cErrorKey(cause))); }
      finally { reading = false; }
    };
    void load();
    const timer = window.setInterval(() => void load(), 15_000);
    return () => { active = false; window.clearInterval(timer); };
  }, [actor, retry, t]);
  return <section className="grid min-w-0 gap-2 border-t border-border pt-3" aria-label={t("walletQuick.activeC2C")}>
    <h3 className="text-sm font-semibold">{t("walletQuick.activeC2C")}</h3>
    {error ? <div className="grid gap-2"><p role="alert" className="text-sm text-destructive">{error}</p><Button variant="ghost" isDisabled={disabled} onPress={() => setRetry(value => value + 1)}>{t("common.retry")}</Button></div>
      : !loaded ? <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p>
        : !orders.length ? <p className="text-sm text-muted-foreground">{t("walletQuick.noActiveC2C")}</p>
          : orders.map(order => <button key={order.id} type="button" disabled={disabled} onClick={() => onSelect(order.id)} className="grid min-h-11 min-w-0 gap-1 rounded-md bg-muted/50 p-3 text-left hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-50">
            <span className="flex min-w-0 flex-wrap items-center justify-between gap-2 text-sm"><span className="min-w-0 truncate">{order.sellerName}</span><span className="tabular-nums">{formatWalletUnits(order.amountUnits, "$", locale)}</span></span>
            <C2CStatusLabel status={order.status} />
          </button>)}
  </section>;
}
