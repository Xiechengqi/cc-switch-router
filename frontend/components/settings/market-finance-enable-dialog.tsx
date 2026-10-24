"use client";

/* Official recharge confirmation. Closing recharge never closes the wallet. */

import { Alert, Button, Modal } from "@heroui/react";
import { AlertTriangle, Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MarketFinanceReadiness } from "@/lib/market-wallet-api";
import { codeLabel, OWNER_DIALOG_CLASS } from "@/components/settings/market-finance-shared";

export function MarketFinanceEnableDialog({ open, readiness, busy, error, onConfirm, onClose }: {
  open: boolean;
  readiness: MarketFinanceReadiness;
  busy: boolean;
  error: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const { t } = useLocaleText();
  const canSubmit = readiness.canEnable && readiness.mode === "shadow" && !busy;
  const rows: [string, number, boolean][] = [
    [t("settings.marketFinance.verifiedTopups", { count: readiness.verifiedLiveTopups }), readiness.verifiedLiveTopups, readiness.verifiedLiveTopups === 0],
    [t("settings.marketFinance.channelBlockers", { count: readiness.paymentChannelBlockers.length }), readiness.paymentChannelBlockers.length, readiness.paymentChannelBlockers.length > 0],
    [t("settings.marketFinance.criticalCases", { count: readiness.criticalCases }), readiness.criticalCases, readiness.criticalCases > 0],
    [t("settings.marketFinance.staleTopupReviews", { count: readiness.staleTopupReviews }), readiness.staleTopupReviews, readiness.staleTopupReviews > 0],
  ];

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => { if (!next && !busy) onClose(); }}>
      <Modal.Container placement="center">
        <Modal.Dialog className={OWNER_DIALOG_CLASS}>
          <Modal.CloseTrigger />
          <Modal.Header><Modal.Heading>{t("settings.marketFinance.enable.title")}</Modal.Heading></Modal.Header>
          <form onSubmit={(event) => { event.preventDefault(); if (canSubmit) onConfirm(); }}>
            <Modal.Body className="grid max-h-[65vh] gap-4 overflow-y-auto text-sm">
              <p className="flex items-start gap-2 text-muted-foreground">
                <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                {t("settings.marketFinance.enable.note")}
              </p>
              <div className="grid gap-2">
                <p className="font-medium">{t("settings.marketFinance.enable.snapshot")}</p>
                <ul className="grid gap-2 text-sm">
                  <li className="flex justify-between gap-3">
                    <span>{t("settings.marketFinance.mode")}</span>
                    <span className="font-medium">{t(`settings.marketFinance.mode.${readiness.mode}`)}</span>
                  </li>
                  {rows.map(([label, , bad]) => (
                    <li key={label} className={bad ? "font-medium text-amber-800" : "text-muted-foreground"}>{label}</li>
                  ))}
                  {readiness.paymentChannelBlockers.map((blocker) => (
                    <li key={blocker.code} className="flex justify-between gap-3 pl-3 text-amber-800">
                      <span>{codeLabel(t, "settings.marketFinance.blocker.", blocker.code)}</span>
                      <span className="tabular-nums">{blocker.count}</span>
                    </li>
                  ))}
                </ul>
              </div>
              {!readiness.canEnable ? <Alert status="warning">{t("settings.marketFinance.enable.notReady")}</Alert> : null}
              {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
            </Modal.Body>
            <Modal.Footer>
              <Button type="button" variant="outline" isDisabled={busy} onPress={onClose}>{t("common.cancel")}</Button>
              <Button type="submit" variant="primary" isDisabled={!canSubmit}>
                {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                {t("settings.marketFinance.enable.confirm")}
              </Button>
            </Modal.Footer>
          </form>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
