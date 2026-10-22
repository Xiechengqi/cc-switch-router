"use client";

/* Owner-only top-up actions with a mandatory dry-run preview:
 * manual credit (matched on-chain transfer) and reversal of a credited top-up. */

import * as React from "react";
import { Alert, Button, Modal } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  MARKET_PAYMENT_TOKEN_IDS,
  manualCreditMarketFinanceTopup,
  reverseMarketFinanceTopup,
  validMarketReasonCode,
  type MarketFinanceTopup,
  type MarketTopupManualCreditResult,
  type MarketTopupReverseResult,
} from "@/lib/market-wallet-api";
import { formatMinorUsd, parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";
import { paymentTokenLabel } from "@/lib/market-payment-tokens";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { OWNER_DIALOG_CLASS, OwnerField, ownerErrorText } from "@/components/settings/market-finance-shared";

export type TopupOwnerAction = "credit" | "reverse";

const EVIDENCE = /^[A-Za-z0-9_.:/#-]{3,200}$/;

export function MarketFinanceTopupOwnerActionDialog({ action, topup, onClose, onDone }: {
  action: TopupOwnerAction | null;
  topup: MarketFinanceTopup | null;
  onClose: () => void;
  onDone: (message: string) => Promise<unknown>;
}) {
  const { t } = useLocaleText();
  const [txHash, setTxHash] = React.useState("");
  const [tokenId, setTokenId] = React.useState("");
  const [evidence, setEvidence] = React.useState("");
  const [reason, setReason] = React.useState("");
  const [amount, setAmount] = React.useState("");
  const [creditPreview, setCreditPreview] = React.useState<MarketTopupManualCreditResult | null>(null);
  const [reversePreview, setReversePreview] = React.useState<MarketTopupReverseResult | null>(null);
  const [previewFingerprint, setPreviewFingerprint] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const open = !!action && !!topup;

  React.useEffect(() => {
    if (!open) return;
    setTxHash(""); setTokenId(""); setEvidence(""); setReason("");
    setAmount(topup ? (topup.amountMinor / 100).toFixed(2) : "");
    setCreditPreview(null); setReversePreview(null); setPreviewFingerprint(""); setError("");
  }, [open, action, topup]);

  const amountMinor = parseNonNegativeUsdMinor(amount);
  const amountValid = amountMinor != null && amountMinor > 0 && (!topup || amountMinor <= topup.amountMinor);
  const txValid = /^[A-Za-z0-9x]{16,128}$/.test(txHash.trim());
  const evidenceValid = EVIDENCE.test(evidence.trim());
  const reasonValid = validMarketReasonCode(reason);
  const fingerprint = [action, txHash.trim(), tokenId, evidence.trim(), reason.trim(), amountMinor].join("|");
  const inputsValid = evidenceValid && reasonValid && (action === "credit" ? txValid : amountValid);
  const hasPreview = action === "credit" ? !!creditPreview : !!reversePreview;
  const previewCurrent = hasPreview && previewFingerprint === fingerprint;

  const run = async (dryRun: boolean) => {
    if (!topup || !action || busy || !inputsValid) return;
    setBusy(true);
    setError("");
    try {
      if (action === "credit") {
        const result = await manualCreditMarketFinanceTopup(topup.id, {
          expectedUpdatedAt: topup.updatedAt,
          txHash: txHash.trim(),
          tokenId: tokenId || undefined,
          evidenceReference: evidence.trim(),
          reasonCode: reason.trim(),
          dryRun,
        });
        if (dryRun) { setCreditPreview(result); setPreviewFingerprint(fingerprint); }
        else { onClose(); await onDone(t("settings.marketFinance.topupRecovery.creditDone")); }
      } else if (amountMinor != null) {
        const result = await reverseMarketFinanceTopup(topup.id, {
          amountMinor,
          evidenceReference: evidence.trim(),
          reasonCode: reason.trim(),
          dryRun,
          expectedShortfallMinor: dryRun ? undefined : reversePreview?.shortfallMinor,
        });
        if (dryRun) { setReversePreview(result); setPreviewFingerprint(fingerprint); }
        else { onClose(); await onDone(t("settings.marketFinance.topupRecovery.reverseDone")); }
      }
    } catch (cause) {
      setCreditPreview(null);
      setReversePreview(null);
      setError(ownerErrorText(t, cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => { if (!next && !busy) onClose(); }}>
      <Modal.Container placement="center">
        <Modal.Dialog className={OWNER_DIALOG_CLASS}>
          <Modal.CloseTrigger />
          <Modal.Header>
            <Modal.Heading>{t(action === "reverse" ? "settings.marketFinance.topupRecovery.reverseTitle" : "settings.marketFinance.topupRecovery.manualCreditTitle")}</Modal.Heading>
          </Modal.Header>
          <form onSubmit={(event) => { event.preventDefault(); void run(true); }}>
            <Modal.Body className="grid max-h-[65vh] gap-3 overflow-y-auto text-sm">
              {topup ? <p className="font-medium tabular-nums">{formatMinorUsd(topup.amountMinor)} · <span className="break-all font-mono text-xs text-muted-foreground">{topup.merchantOrderId}</span></p> : null}
              <p className="text-xs text-muted-foreground">{t(action === "reverse" ? "settings.marketFinance.topupRecovery.reverseNote" : "settings.marketFinance.topupRecovery.manualCreditNote")}</p>
              {action === "credit" ? <>
                <OwnerField autoFocus label={t("settings.marketFinance.topupRecovery.txHash")} value={txHash} onChange={(event) => setTxHash(event.target.value)} invalid={!!txHash && !txValid} maxLength={128} inputClassName="font-mono text-xs" />
                <label className="grid gap-1">
                  <span className="font-medium">{t("settings.marketFinance.topupRecovery.tokenId")}</span>
                  <select value={tokenId} onChange={(event) => setTokenId(event.target.value)} className={WALLET_INPUT}>
                    <option value="">{t("settings.marketFinance.topupRecovery.tokenAuto")}</option>
                    {MARKET_PAYMENT_TOKEN_IDS.map((id) => <option key={id} value={id}>{paymentTokenLabel(id).label}</option>)}
                  </select>
                </label>
              </> : (
                <OwnerField autoFocus label={t("settings.marketFinance.topupRecovery.reverseAmount")} inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} invalid={!!amount && !amountValid} />
              )}
              <OwnerField label={t("settings.marketFinance.topupRecovery.evidence")} help={t("settings.marketFinance.topupRecovery.evidenceHelp")} value={evidence} onChange={(event) => setEvidence(event.target.value)} invalid={!!evidence && !evidenceValid} maxLength={200} inputClassName="font-mono text-xs" />
              <OwnerField label={t("settings.marketFinance.owner.reasonCode")} help={t("settings.marketFinance.owner.reasonCodeHelp")} value={reason} onChange={(event) => setReason(event.target.value)} invalid={!!reason && !reasonValid} maxLength={64} inputClassName="font-mono" />
              <div aria-live="polite" className="grid gap-2">
                {creditPreview && action === "credit" ? <Alert status={creditPreview.credited ? "warning" : "success"}>
                  <div className="grid gap-1">
                    <span>{t("settings.marketFinance.topupRecovery.creditPreview", {
                      status: creditPreview.gatewayStatus,
                      source: creditPreview.paidSource ?? "—",
                      amount: formatMinorUsd(creditPreview.amountMinor),
                      token: paymentTokenLabel(creditPreview.tokenId).label,
                    })}</span>
                    <span>{t("walletPolicy.ownerCreditSplit", { credit: formatMinorUsd(creditPreview.creditMinor), fee: formatMinorUsd(creditPreview.feeMinor) })}</span>
                    <span className="text-xs">{t("settings.marketFinance.owner.previewNote")}</span>
                  </div>
                </Alert> : null}
                {reversePreview && action === "reverse" ? <Alert status={reversePreview.shortfallMinor > 0 ? "warning" : "success"}>
                  <div className="grid gap-1">
                    <span>{t("settings.marketFinance.topupRecovery.reversePreview", {
                      amount: formatMinorUsd(reversePreview.amountMinor),
                      fromWallet: formatMinorUsd(reversePreview.fromWalletMinor),
                      shortfall: formatMinorUsd(reversePreview.shortfallMinor),
                    })}</span>
                    <span>{t("walletPolicy.ownerReverseSplit", { principal: formatMinorUsd(reversePreview.principalMinor), fee: formatMinorUsd(reversePreview.feeMinor) })}</span>
                    <span className="text-xs">{t("settings.marketFinance.owner.previewNote")}</span>
                  </div>
                </Alert> : null}
                {hasPreview && !previewCurrent ? <p className="text-xs text-amber-800">{t("settings.marketFinance.owner.previewStale")}</p> : null}
                {error ? <Alert status="danger">{error}</Alert> : null}
              </div>
            </Modal.Body>
            <Modal.Footer>
              <Button type="button" variant="outline" isDisabled={busy} onPress={onClose}>{t("common.cancel")}</Button>
              <Button type="submit" variant="outline" isDisabled={busy || !inputsValid}>
                {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                {t("settings.marketFinance.owner.preview")}
              </Button>
              <Button type="button" variant={action === "reverse" ? "danger" : "primary"} isDisabled={busy || !inputsValid || !previewCurrent} onPress={() => void run(false)}>
                {t("settings.marketFinance.owner.confirm")}
              </Button>
            </Modal.Footer>
          </form>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
