"use client";

import * as React from "react";
import { ArrowRight, ArrowUpRight, Clock3 } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_SECONDARY_BUTTON, WalletSectionLabel } from "@/components/market/wallet-primitives";
import { closeMarketWalletTopup, type MarketWalletTopup } from "@/lib/market-wallet-api";
import { formatMinorUsd, formatWalletUnits, walletTopupCheckoutHref } from "@/lib/market-wallet-money";
import { walletErrorText } from "@/lib/wallet-errors";
import { decideTransferReturn, type AwaitingReturn } from "@/lib/wallet-transfer-returns";
import { isOpenTopup } from "./use-wallet-data";

const ROW = "flex min-w-0 flex-wrap items-center justify-between gap-x-4 gap-y-2 py-3 first:pt-0 last:pb-0";
const LINK_BUTTON = "inline-flex min-h-11 items-center gap-1 whitespace-nowrap rounded-xl px-3 text-sm font-semibold text-accent transition-colors hover:bg-accent/5 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

/**
 * Only what needs the person now: an unpaid top-up, a top-up being confirmed,
 * a legacy return request, or C2C trades in progress. Renders nothing when there is nothing to do.
 */
export function WalletPendingActions({
  topups,
  awaitingReturns,
  c2cActive,
  onChanged,
  onOpenC2C,
  onTopupChanged,
}: {
  topups: MarketWalletTopup[];
  awaitingReturns: AwaitingReturn[];
  c2cActive: number;
  onChanged: () => Promise<void>;
  onOpenC2C: () => void;
  onTopupChanged: (order: MarketWalletTopup) => void;
}) {
  const { t, locale } = useLocaleText();
  const [closing, setClosing] = React.useState<MarketWalletTopup | null>(null);
  const [busy, setBusy] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [error, setError] = React.useState("");
  const time = (value: string) => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
  const open = topups.filter(isOpenTopup);
  const count = open.length + awaitingReturns.length + (c2cActive > 0 ? 1 : 0);
  if (!count && !notice && !error) return null;

  const run = async (id: string, work: () => Promise<unknown>, done?: string) => {
    if (busy) return;
    setBusy(id);
    setError("");
    setNotice("");
    try {
      await work();
      if (done) setNotice(done);
      await onChanged().catch(() => {});
    } catch (cause) {
      setError(walletErrorText(cause, t, locale));
    } finally {
      setBusy("");
    }
  };

  return (
    <section aria-labelledby="wallet-pending-title" className="grid min-w-0 gap-4 rounded-2xl border border-accent/20 bg-card p-5 shadow-sm sm:p-6">
      <h2 id="wallet-pending-title" className="sr-only">{t("walletPage.pendingLabel")}</h2>
      <WalletSectionLabel pulse={count > 0}>{t("walletPage.pendingLabel")}</WalletSectionLabel>
      {notice ? <p role="status" className="rounded-lg bg-emerald-50 px-3 py-2 text-sm text-emerald-800">{notice}</p> : null}
      {error ? <p role="alert" className="rounded-lg bg-destructive/5 px-3 py-2 text-sm text-destructive">{error}</p> : null}
      <ul className="divide-y divide-border">
        {open.map((order) => {
          const amount = formatMinorUsd(order.creditAmountMinor ?? order.amountMinor);
          const awaiting = order.buyerView ? order.buyerView.state === "awaiting_payment" : order.status === "pending";
          const canPay = order.buyerView?.canPay ?? awaiting;
          const canClose = order.buyerView?.canClose ?? awaiting;
          const until = order.buyerView?.payUntil ?? order.autoCloseAt;
          return (
            <li key={order.id} className={ROW}>
              <div className="grid min-w-0 gap-0.5">
                <p className="flex items-center gap-2 text-sm font-semibold tabular-nums">
                  {awaiting ? <span aria-hidden className="size-2 rounded-full bg-accent pulse-dot" /> : <Clock3 className="size-4 text-muted-foreground" aria-hidden />}
                  {t(awaiting ? "walletPage.topupAwaiting" : "walletPage.topupConfirming", { amount })}
                </p>
                <p className="text-xs text-muted-foreground">
                  {awaiting && until ? t("walletPage.topupAwaitingUntil", { time: time(until) }) : t("walletPage.topupConfirmingNote")}
                </p>
              </div>
              {awaiting ? (
                <div className="flex flex-wrap gap-1">
                  {canPay ? (
                    <a className={LINK_BUTTON} href={walletTopupCheckoutHref(order.id)} target="_blank" rel="noopener noreferrer">
                      {t("walletPage.continuePay")}<ArrowUpRight className="size-4" aria-hidden />
                    </a>
                  ) : null}
                  {canClose ? (
                    <button type="button" className={LINK_BUTTON + " text-muted-foreground"} disabled={!!busy} onClick={() => setClosing(order)}>
                      {t("walletPage.closeTopup")}
                    </button>
                  ) : null}
                </div>
              ) : null}
            </li>
          );
        })}
        {awaitingReturns.map((request) => (
          <li key={request.id} className={ROW}>
            <div className="grid min-w-0 gap-0.5">
              <p className="break-all text-sm font-semibold tabular-nums">
                {t("walletPage.returnRequest", { email: request.senderEmail, amount: formatWalletUnits(request.amountUnits) })}
              </p>
              <p className="whitespace-pre-wrap break-words text-xs text-muted-foreground">{request.reason}</p>
            </div>
            <div className="flex flex-wrap gap-1">
              <button type="button" className={WALLET_SECONDARY_BUTTON} disabled={!!busy}
                onClick={() => void run(request.id, () => decideTransferReturn(request.transferId, request.version, true))}>
                {t("walletPage.returnApprove", { amount: formatWalletUnits(request.amountUnits) })}
              </button>
              <button type="button" className={LINK_BUTTON + " text-muted-foreground"} disabled={!!busy}
                onClick={() => void run(request.id, () => decideTransferReturn(request.transferId, request.version, false))}>
                {t("walletPage.returnDecline")}
              </button>
            </div>
          </li>
        ))}
        {c2cActive > 0 ? (
          <li className={ROW}>
            <p className="text-sm font-semibold">{t("walletPage.c2cActive", { count: c2cActive })}</p>
            <button type="button" className={LINK_BUTTON} onClick={onOpenC2C}>
              {t("walletPage.view")}<ArrowRight className="size-4" aria-hidden />
            </button>
          </li>
        ) : null}
      </ul>
      <ConfirmAlertDialog
        open={!!closing}
        title={t("walletPage.closeTitle")}
        description={t("walletPage.closeBody")}
        confirmLabel={t("walletPage.closeTopup")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={!!closing && busy === closing.id}
        onConfirm={() => {
          const order = closing;
          if (!order) return;
          void run(order.id, async () => {
            onTopupChanged(await closeMarketWalletTopup(order.id));
            setClosing(null);
          }, t("walletPage.closed"));
        }}
        onOpenChange={(next) => { if (!next && !busy) setClosing(null); }}
      />
    </section>
  );
}
