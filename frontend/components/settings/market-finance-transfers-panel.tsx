"use client";

/* Hallmark · component: offline transfer operations · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P5 H5 E4 S5 R5 V4
 */

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { exactTransferMinor, marketFinanceActionsForMode, validTronTransactionId } from "@/lib/market-finance-actions";
import {
  getMarketFinanceTransfers,
  transitionMarketFinanceTransfer,
  validMarketReasonCode,
  type MarketFinanceTransfer,
  type MarketFinanceTransferAction,
} from "@/lib/market-wallet-api";
import { formatMinorUsd, formatWalletUnits, parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";
import { localDateTimeToIso } from "@/lib/market-explorer";
import { CopyTextButton, TronTransactionLink } from "@/components/market/wallet-primitives";
import { OwnerField, ownerErrorText } from "@/components/settings/market-finance-shared";

type TransferKind = "withdrawals";
// Admin rows may carry the recorded transfer hash once one exists.
type Selection = { kind: TransferKind; transfer: MarketFinanceTransfer };

export function MarketFinanceTransfersPanel({ financeMode, withdrawalsPaused = false }: { financeMode: "disabled" | "shadow" | "enabled"; withdrawalsPaused?: boolean }) {
  const { locale, t } = useLocaleText();
  const [withdrawals, setWithdrawals] = React.useState<MarketFinanceTransfer[]>([]);
  const [selection, setSelection] = React.useState<Selection | null>(null);
  const [action, setAction] = React.useState<MarketFinanceTransferAction>("approve");
  const [reference, setReference] = React.useState("");
  const [reason, setReason] = React.useState("");
  const [confirmed, setConfirmed] = React.useState(false);
  const [actualAmount, setActualAmount] = React.useState("");
  const [transferredAt, setTransferredAt] = React.useState("");
  const [networkFee, setNetworkFee] = React.useState("");
  const [varianceReason, setVarianceReason] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");

  const reload = React.useCallback(async () => {
    setWithdrawals(await getMarketFinanceTransfers("withdrawals"));
  }, []);

  React.useEffect(() => {
    let active = true;
    getMarketFinanceTransfers("withdrawals")
      .then((nextWithdrawals) => {
        if (!active) return;
        setWithdrawals(nextWithdrawals);
      })
      .catch((cause) => { if (active) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, []);

  const open = (kind: TransferKind, transfer: MarketFinanceTransfer) => {
    const first = marketFinanceActionsForMode(transfer.status, kind, financeMode, withdrawalsPaused)[0];
    if (!first) return;
    setSelection({ kind, transfer });
    setAction(first);
    setReference("");
    setReason("");
    setConfirmed(false);
    // The Owner types what was actually sent; never prefill the request.
    setActualAmount("");
    setTransferredAt("");
    setNetworkFee("");
    setVarianceReason("");
    setError("");
  };

  const needsReason = action === "reject" || action === "mark_failed" || action === "review";
  const needsAttestation = action === "reject" || action === "mark_failed" || action === "mark_paid";
  const reasonReady = !needsReason || /^[a-z0-9_]{1,80}$/.test(reason.trim());
  const amountMinor = selection ? exactTransferMinor(selection.transfer.amountUnits) : null;
  const referenceReady = action !== "mark_paid" || validTronTransactionId(reference);
  const actualMinor = parseNonNegativeUsdMinor(actualAmount);
  const actualReady = actualMinor != null && actualMinor > 0;
  const transferredAtIso = localDateTimeToIso(transferredAt);
  const transferredAtReady = !!transferredAtIso && Date.parse(transferredAtIso) <= Date.now() + 5 * 60_000;
  const feeMinor = networkFee.trim() ? parseNonNegativeUsdMinor(networkFee) : null;
  const feeReady = !networkFee.trim() || feeMinor != null;
  const hasVariance = actualReady && amountMinor != null && actualMinor !== amountMinor;
  const varianceReady = !hasVariance || validMarketReasonCode(varianceReason);
  const paidDetailsReady = action !== "mark_paid" || (actualReady && transferredAtReady && feeReady && varianceReady);
  const canSubmit = !!selection && !busy && amountMinor != null
    && marketFinanceActionsForMode(selection.transfer.status, selection.kind, financeMode, withdrawalsPaused).includes(action)
    && reasonReady && referenceReady && paidDetailsReady && (!needsAttestation || confirmed);

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!canSubmit || !selection || amountMinor == null) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await transitionMarketFinanceTransfer(selection.kind, selection.transfer.id, action, selection.transfer.version, {
        ...(action === "mark_paid" && actualMinor != null && transferredAtIso ? {
          externalReference: reference.trim(),
          actualAmountMinor: actualMinor,
          transferredAt: transferredAtIso,
          ...(feeMinor != null ? { networkFeeMinor: feeMinor } : {}),
          ...(hasVariance ? { varianceReason: varianceReason.trim() } : {}),
        } : {}),
        ...(needsReason ? { reason: reason.trim() } : {}),
      });
      await reload();
      setSelection(null);
      setNotice(t("settings.marketFinance.transferUpdated"));
    } catch (cause) {
      setError(ownerErrorText(t, cause));
      // On a version conflict the Owner must refresh the current transfer
      // before choosing another transition. Never retry an offline payment
      // action against a stale snapshot.
      await reload().catch(() => {});
      setSelection(null);
    } finally {
      setBusy(false);
    }
  };

  const renderList = (kind: TransferKind, items: MarketFinanceTransfer[]) => <div className="grid min-w-0 content-start gap-2">
    <h5 className="text-sm font-semibold">{t("settings.marketFinance.pendingWithdrawals", { count: items.length })}</h5>
    {items.length ? <ul className="grid gap-2">{items.map((item) => <li key={item.id} className="grid min-w-0 gap-2 rounded-md border border-border px-3 py-2 text-sm">
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
        <span className="font-semibold tabular-nums">{formatWalletUnits(item.amountUnits, "$", locale)}</span>
        <span className="text-xs text-muted-foreground">{t(`marketProviderFinance.status.${item.status}` as Parameters<typeof t>[0])}</span>
      </div>
      <div className="flex min-w-0 items-center gap-2">
        <p className="min-w-0 flex-1 break-all font-mono text-xs text-muted-foreground">{item.providerUserId ?? item.userId} · {item.address}</p>
        <CopyTextButton value={item.address} label={t("settings.marketFinance.transfer.copyAddress")} />
      </div>
      {item.externalReference ? <p className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">{t("settings.marketFinance.transfer.reference")}: <TronTransactionLink hash={item.externalReference} /></p> : null}
      <Button size="sm" variant="outline" className="min-h-11 justify-self-start whitespace-nowrap" isDisabled={busy || !marketFinanceActionsForMode(item.status, kind, financeMode, withdrawalsPaused).length} onPress={() => open(kind, item)}>
        {t("settings.marketFinance.processTransfer")}
      </Button>
    </li>)}</ul> : <p className="text-sm text-muted-foreground">{t("settings.marketFinance.noPendingTransfers")}</p>}
  </div>;

  return <section className="grid min-w-0 gap-4" aria-label={t("settings.marketFinance.offlineTransfers")}>
    <div className="grid gap-1">
      <h4 className="text-sm font-semibold">{t("settings.marketFinance.offlineTransfers")}</h4>
      <p className="text-xs text-muted-foreground">{t("settings.marketFinance.offlineTransfersNote")}</p>
    </div>
    {error && !selection ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
    {notice ? <p role="status" className="text-sm text-emerald-800">{notice}</p> : null}
    <div className="grid min-w-0 gap-4">
      {renderList("withdrawals", withdrawals)}
    </div>
    <Button variant="ghost" size="sm" className="min-h-11 justify-self-start whitespace-nowrap" isDisabled={busy} onPress={() => void reload().catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))}>
      {t("common.reload")}
    </Button>

    <Modal.Backdrop isOpen={!!selection} onOpenChange={(open) => { if (!open && !busy) setSelection(null); }}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(520px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.CloseTrigger />
          <Modal.Header><Modal.Heading>{t("settings.marketFinance.processTransfer")}</Modal.Heading></Modal.Header>
          <form onSubmit={(event) => void submit(event)}>
            <Modal.Body className="grid max-h-[65vh] min-w-0 gap-3 overflow-y-auto text-sm">
              {selection ? <>
                <p className="font-semibold tabular-nums">{formatWalletUnits(selection.transfer.amountUnits, "$", locale)}</p>
                <div className="flex min-w-0 items-center gap-2">
                  <p className="min-w-0 flex-1 break-all font-mono text-xs">USDT / TRON · {selection.transfer.address}</p>
                  <CopyTextButton value={selection.transfer.address} label={t("settings.marketFinance.transfer.copyAddress")} />
                </div>
                <p className="text-xs text-muted-foreground">{t("settings.marketFinance.transferStatus")}: {t(`marketProviderFinance.status.${selection.transfer.status}` as Parameters<typeof t>[0])}</p>
                <label className="grid gap-1">
                  <span>{t("settings.marketFinance.transferAction")}</span>
                  <select value={action} onChange={(event) => { setAction(event.target.value as MarketFinanceTransferAction); setConfirmed(false); }}
                    className="h-11 min-w-0 rounded-md border border-input bg-background px-3 focus-visible:outline-2 focus-visible:outline-ring">
                    {marketFinanceActionsForMode(selection.transfer.status, selection.kind, financeMode, withdrawalsPaused).map((choice) => <option key={choice} value={choice}>{t(`settings.marketFinance.transferAction.${choice}`)}</option>)}
                  </select>
                </label>
                {action === "mark_paid" ? <label className="grid gap-1">
                  <span>{t("settings.marketFinance.transactionId")}</span>
                  <input value={reference} onChange={(event) => setReference(event.target.value)} autoComplete="off" spellCheck={false}
                    aria-invalid={!!reference && !referenceReady} minLength={64} maxLength={64}
                    className="h-11 min-w-0 rounded-md border border-input bg-background px-3 font-mono text-xs focus-visible:outline-2 focus-visible:outline-ring aria-invalid:border-destructive" />
                  <span className="text-xs text-muted-foreground">{t("settings.marketFinance.transactionIdHelp")}</span>
                  {referenceReady && reference ? <TronTransactionLink hash={reference} /> : null}
                </label> : null}
                {action === "mark_paid" ? <div className="grid gap-3 rounded-xl border border-border p-3">
                  <p className="text-xs font-medium">{t("settings.marketFinance.transfer.requested", { amount: amountMinor != null ? formatMinorUsd(amountMinor) : formatWalletUnits(selection.transfer.amountUnits, "$", locale) })}</p>
                  <OwnerField label={t("settings.marketFinance.transfer.actualAmount")} help={t("settings.marketFinance.transfer.actualHelp")} inputMode="decimal" value={actualAmount} onChange={(event) => setActualAmount(event.target.value)} invalid={!!actualAmount && !actualReady} placeholder="0.00" />
                  <OwnerField type="datetime-local" label={t("settings.marketFinance.transfer.transferredAt")} value={transferredAt} onChange={(event) => setTransferredAt(event.target.value)} invalid={!!transferredAt && !transferredAtReady} />
                  <OwnerField label={t("settings.marketFinance.transfer.networkFee")} inputMode="decimal" value={networkFee} onChange={(event) => setNetworkFee(event.target.value)} invalid={!feeReady} placeholder="0.00" />
                  {hasVariance ? <OwnerField label={t("settings.marketFinance.transfer.varianceReason")} help={t("settings.marketFinance.transfer.varianceHelp")} value={varianceReason} onChange={(event) => setVarianceReason(event.target.value)} invalid={!!varianceReason && !varianceReady} maxLength={64} placeholder="network_fee_deducted" inputClassName="font-mono" /> : null}
                </div> : null}
                {needsReason ? <label className="grid gap-1">
                  <span>{t("settings.marketFinance.transferReason")}</span>
                  <input value={reason} onChange={(event) => setReason(event.target.value)} placeholder="no_external_transfer"
                    aria-invalid={!!reason && !reasonReady} maxLength={80}
                    className="h-11 min-w-0 rounded-md border border-input bg-background px-3 font-mono text-xs focus-visible:outline-2 focus-visible:outline-ring aria-invalid:border-destructive" />
                  <span className="text-xs text-muted-foreground">{t("settings.marketFinance.transferReasonHelp")}</span>
                </label> : null}
                {needsAttestation ? <label className="flex items-start gap-2 text-xs">
                  <input type="checkbox" className="mt-0.5" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
                  {t(action === "mark_paid" ? "settings.marketFinance.confirmPaid" : "settings.marketFinance.confirmNoPayment")}
                </label> : null}
              </> : null}
              {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
            </Modal.Body>
            <Modal.Footer>
              <Button type="button" variant="outline" isDisabled={busy} onPress={() => setSelection(null)}>{t("common.cancel")}</Button>
              <Button type="submit" isDisabled={!canSubmit} className="whitespace-nowrap">{busy ? t("common.loading") : t("settings.marketFinance.recordTransition")}</Button>
            </Modal.Footer>
          </form>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  </section>;
}
