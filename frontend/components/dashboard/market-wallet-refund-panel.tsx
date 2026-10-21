"use client";

/* Hallmark · component: wallet balance refund · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P4 H5 E4 S5 R5 V4
 */

import * as React from "react";
import { Button } from "@heroui/react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { CopyTextButton, TronTransactionLink } from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import {
  cancelMarketBuyerRefund,
  getMarketBuyerRefunds,
  requestMarketBuyerRefund,
  type MarketBuyerRefund,
} from "@/lib/market-wallet-api";
import { formatWalletUnits, parseWithdrawalMinor } from "@/lib/market-wallet-money";

const UNITS_PER_MINOR = 86_400n;

export function MarketWalletRefundPanel({ availableUnits, onSubmitted }: {
  availableUnits: string;
  onSubmitted: () => Promise<void>;
}) {
  const { locale, t } = useLocaleText();
  const [refunds, setRefunds] = React.useState<MarketBuyerRefund[]>([]);
  const [nextCursor, setNextCursor] = React.useState<string | null>(null);
  const [amount, setAmount] = React.useState("");
  const [address, setAddress] = React.useState("");
  const [confirmed, setConfirmed] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [cancelTarget, setCancelTarget] = React.useState<MarketBuyerRefund | null>(null);
  const idempotencyRef = React.useRef<{ fingerprint: string; key: string } | null>(null);

  React.useEffect(() => {
    let active = true;
    getMarketBuyerRefunds().then((page) => {
      if (!active) return;
      setRefunds(page.items);
      setNextCursor(page.nextCursor);
    }).catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => { active = false; };
  }, []);

  const amountMinor = parseWithdrawalMinor(amount);
  const amountReady = amountMinor != null
    && BigInt(amountMinor) * UNITS_PER_MINOR <= BigInt(availableUnits);
  const addressReady = /^T[1-9A-HJ-NP-Za-km-z]{33}$/.test(address.trim());

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || !confirmed || !amountReady || !addressReady || amountMinor == null) return;
    setBusy(true);
    setError("");
    setNotice("");
    const fingerprint = `${amountMinor}:${address.trim()}`;
    if (idempotencyRef.current?.fingerprint !== fingerprint) {
      idempotencyRef.current = { fingerprint, key: crypto.randomUUID() };
    }
    try {
      const result = await requestMarketBuyerRefund(amountMinor, address.trim(), idempotencyRef.current.key);
      setRefunds((current) => [result, ...current.filter((item) => item.id !== result.id)]);
      setAmount("");
      setAddress("");
      setConfirmed(false);
      idempotencyRef.current = null;
      setNotice(t("marketWallet.refundRequested"));
      await onSubmitted();
    } catch (cause) {
      setError(refundErrorText(cause));
    } finally {
      setBusy(false);
    }
  };

  const refundErrorText = (cause: unknown) => {
    if (cause instanceof ApiError && cause.code === "MARKET_REFUND_EXCEEDS_FREE_BALANCE") {
      const free = typeof cause.details?.freeUnits === "string" ? cause.details.freeUnits : "0";
      const committed = typeof cause.details?.committedUnits === "string" ? cause.details.committedUnits : "0";
      return t("marketWallet.refundExceedsFree", {
        free: formatWalletUnits(free, "$", locale),
        committed: formatWalletUnits(committed, "$", locale),
      });
    }
    if (cause instanceof ApiError && cause.status === 503) return t("marketWallet.refundWalletClosed");
    return cause instanceof Error ? cause.message : String(cause);
  };

  const cancelRefund = async () => {
    const target = cancelTarget;
    if (!target || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const updated = await cancelMarketBuyerRefund(target.id, target.version);
      setRefunds((current) => current.map((item) => item.id === updated.id ? updated : item));
      setCancelTarget(null);
      setNotice(t("marketWallet.refundCancelled"));
      await onSubmitted();
    } catch (cause) {
      setCancelTarget(null);
      setError(cause instanceof ApiError && cause.status === 409 ? t("marketWallet.refundCancelChanged") : refundErrorText(cause));
      const page = await getMarketBuyerRefunds().catch(() => null);
      if (page) { setRefunds(page.items); setNextCursor(page.nextCursor); }
    } finally {
      setBusy(false);
    }
  };

  const loadMore = async () => {
    if (busy || !nextCursor) return;
    setBusy(true);
    setError("");
    try {
      const page = await getMarketBuyerRefunds(nextCursor);
      setRefunds((current) => [...current, ...page.items.filter((item) => !current.some((prior) => prior.id === item.id))]);
      setNextCursor(page.nextCursor);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return <section className="grid min-w-0 gap-4 rounded-xl border border-border bg-card p-5" aria-label={t("marketWallet.refundTitle")}>
    <div className="grid gap-1">
      <h2 className="text-base font-semibold">{t("marketWallet.refundTitle")}</h2>
      <p className="text-sm text-muted-foreground">{t("marketWallet.refundHelp")}</p>
    </div>
    {error ? <p role="alert" className="rounded-md bg-destructive/5 px-3 py-2 text-sm text-destructive">{error}</p> : null}
    {notice ? <p role="status" className="rounded-md bg-emerald-50 px-3 py-2 text-sm text-emerald-800">{notice}</p> : null}
    <form className="grid min-w-0 gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]" onSubmit={(event) => void submit(event)}>
      <label className="grid min-w-0 gap-1 text-sm">
        <span>{t("marketWallet.refundAmount")}</span>
        <input inputMode="decimal" value={amount} onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }}
          placeholder="10.00" aria-invalid={!!amount && !amountReady} aria-describedby="wallet-refund-amount-help"
          className="h-11 min-w-0 rounded-md border border-input bg-background px-3 outline-2 outline-transparent focus-visible:outline-ring aria-invalid:border-destructive" />
        <span id="wallet-refund-amount-help" className="min-h-[1lh] text-xs text-muted-foreground">{t("marketWallet.refundAmountHelp", { amount: formatWalletUnits(availableUnits, "$", locale) })}</span>
      </label>
      <label className="grid min-w-0 gap-1 text-sm">
        <span>{t("marketWallet.refundAddress")}</span>
        <input value={address} onChange={(event) => { setAddress(event.target.value); setConfirmed(false); idempotencyRef.current = null; }}
          autoComplete="off" spellCheck={false} aria-invalid={!!address && !addressReady} aria-describedby="wallet-refund-address-help"
          className="h-11 min-w-0 rounded-md border border-input bg-background px-3 font-mono text-xs outline-2 outline-transparent focus-visible:outline-ring aria-invalid:border-destructive" />
        <span id="wallet-refund-address-help" className="min-h-[1lh] text-xs text-muted-foreground">{t("marketWallet.refundAddressHelp")}</span>
      </label>
      <label className="flex items-start gap-2 text-xs text-muted-foreground sm:col-span-2">
        <input type="checkbox" className="mt-0.5" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
        {t("marketWallet.refundConfirm")}
      </label>
      <Button type="submit" isDisabled={busy || !confirmed || !amountReady || !addressReady} className="min-h-11 whitespace-nowrap sm:justify-self-start">
        {busy ? t("common.loading") : t("marketWallet.refundRequest")}
      </Button>
    </form>
    {refunds.length ? <div className="grid gap-2">
      <h3 className="text-sm font-semibold">{t("marketWallet.refundHistory")}</h3>
      <ul className="grid gap-2">{refunds.map((item) => <li key={item.id} className="grid min-w-0 gap-1 rounded-md bg-muted/50 px-3 py-2 text-xs">
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
          <span className="font-medium tabular-nums">{formatWalletUnits(item.amountUnits, "$", locale)}</span>
          <span className="flex items-center gap-2">
            <span>{t(`marketProviderFinance.status.${item.status}` as Parameters<typeof t>[0])}</span>
            {item.status === "requested" ? <Button size="sm" variant="outline" className="min-h-11 whitespace-nowrap" isDisabled={busy} onPress={() => setCancelTarget(item)}>
              {t("marketWallet.refundCancel")}
            </Button> : null}
          </span>
        </div>
        <span className="flex min-w-0 items-center gap-1 text-muted-foreground">
          <span className="min-w-0 truncate font-mono" title={item.address}>{item.address}</span>
          <CopyTextButton value={item.address} size="sm" label={t("marketWallet.copyAddress")} />
        </span>
        {item.externalReference ? <TronTransactionLink hash={item.externalReference} /> : null}
      </li>)}</ul>
      {nextCursor ? <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void loadMore()}>{t("marketWallet.more")}</Button> : null}
    </div> : null}
    <ConfirmAlertDialog
      open={!!cancelTarget}
      title={t("marketWallet.refundCancelTitle")}
      description={t("marketWallet.refundCancelNote", { amount: cancelTarget ? formatWalletUnits(cancelTarget.amountUnits, "$", locale) : "" })}
      confirmLabel={t("marketWallet.refundCancel")}
      cancelLabel={t("common.close")}
      tone="warning"
      busy={busy}
      onConfirm={() => void cancelRefund()}
      onOpenChange={(open) => { if (!open && !busy) setCancelTarget(null); }}
    />
  </section>;
}
