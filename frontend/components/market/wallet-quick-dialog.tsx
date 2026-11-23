"use client";

/* Hallmark · component: quick balance dialog · genre: modern-minimal · theme: existing Router system
 * states: loading · ready · error · busy (top-up in flight blocks closing)
 * Top up without leaving the current page. Sending, C2C and history live on
 * the balance page, one tap away.
 */

import * as React from "react";
import Link from "next/link";
import { Button, Modal } from "@heroui/react";
import { ArrowUpRight, Loader2, WalletCards } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletTopupForm } from "@/components/market/wallet-topup-form";
import { useWalletSnapshot } from "@/components/market/in-place-topup";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { formatWalletUnits, formatWalletUnitsAsCny, safeMarketReturnPath } from "@/lib/market-wallet-money";


export function WalletQuickDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const { actor, current, error, ready, load, reload, addOrder } = useWalletSnapshot(open);
  const [busy, setBusy] = React.useState(false);
  const [returnPath, setReturnPath] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (open) setReturnPath(safeMarketReturnPath(`${window.location.pathname}${window.location.search}`));
  }, [open]);

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
          <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" isDisabled={busy} />
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
                <Button size="sm" variant="outline" onPress={() => void load()}>{t("common.retry")}</Button>
              </div>
            ) : null}
            {current && actor ? (
              <div className="grid min-w-0 gap-3">
                <WalletTopupForm
                  wallet={current.wallet}
                  topups={current.topups}
                  ready={ready}
                  active={open}
                  returnPath={returnPath}
                  idPrefix="wallet-quick-topup"
                  onBusyChange={setBusy}
                  onOrderCreated={addOrder}
                  onReload={reload}
                />
                <p className="text-xs leading-5 text-muted-foreground">{t("walletPage.topupNote")}</p>
              </div>
            ) : null}
          </Modal.Body>
          <Modal.Footer className="flex-wrap justify-between">
            <Link href={DASHBOARD_ACCOUNT_WALLET_PATH} tabIndex={busy ? -1 : undefined} onClick={(event) => { if (busy) event.preventDefault(); else close(); }} aria-disabled={busy} className="inline-flex min-h-11 items-center gap-1 text-sm font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-disabled:pointer-events-none aria-disabled:opacity-50">
              {t("walletPage.quickMore")}<ArrowUpRight className="size-4" aria-hidden />
            </Link>
            <Button variant="ghost" className="min-h-11" isDisabled={busy} onPress={close}>{t("common.close")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
