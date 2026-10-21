"use client";

/* Irreversible cutover confirmation: shows the readiness snapshot and requires
 * the Owner to type the exact phrase before switching the wallet on. */

import * as React from "react";
import { Alert, Button, Modal } from "@heroui/react";
import { AlertTriangle, Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MarketFinanceReadiness } from "@/lib/market-wallet-api";
import { codeLabel, OWNER_DIALOG_CLASS } from "@/components/settings/market-finance-shared";

export const ENABLE_PLATFORM_WALLET_PHRASE = "ENABLE PLATFORM WALLET";

export function MarketFinanceEnableDialog({ open, readiness, busy, error, onConfirm, onClose }: {
  open: boolean;
  readiness: MarketFinanceReadiness;
  busy: boolean;
  error: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const { t } = useLocaleText();
  const [phrase, setPhrase] = React.useState("");
  React.useEffect(() => { if (open) setPhrase(""); }, [open]);
  const matches = phrase === ENABLE_PLATFORM_WALLET_PHRASE;
  const canSubmit = matches && readiness.canEnable && readiness.mode === "shadow" && !busy;
  const rows: [string, number, boolean][] = [
    [t("settings.marketFinance.verifiedTopups", { count: readiness.verifiedLiveTopups }), readiness.verifiedLiveTopups, readiness.verifiedLiveTopups === 0],
    [t("settings.marketFinance.blockers", { count: readiness.legacyBlockers.length }), readiness.legacyBlockers.length, readiness.legacyBlockers.length > 0],
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
              <p className="flex items-start gap-2 rounded-xl border border-red-200 bg-red-50 p-3 text-red-900">
                <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                {t("settings.marketFinance.enable.irreversible")}
              </p>
              <div className="grid gap-2">
                <p className="font-mono text-[0.68rem] uppercase tracking-[0.15em] text-muted-foreground">{t("settings.marketFinance.enable.snapshot")}</p>
                <ul className="grid gap-1 rounded-xl border border-border p-3 text-xs">
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
                  {readiness.legacyBlockers.map((blocker) => (
                    <li key={blocker.code} className="flex justify-between gap-3 pl-3 text-amber-800">
                      <span className="break-all">{blocker.code}</span>
                      <span className="tabular-nums">{blocker.count}</span>
                    </li>
                  ))}
                </ul>
              </div>
              {!readiness.canEnable ? <Alert status="warning">{t("settings.marketFinance.enable.notReady")}</Alert> : null}
              <label className="grid gap-1">
                <span className="font-medium">{t("settings.marketFinance.enable.typePrompt", { phrase: ENABLE_PLATFORM_WALLET_PHRASE })}</span>
                <input
                  value={phrase}
                  onChange={(event) => setPhrase(event.target.value)}
                  autoFocus
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={ENABLE_PLATFORM_WALLET_PHRASE}
                  aria-invalid={!!phrase && !matches}
                  className="h-11 min-w-0 rounded-lg border border-input bg-background px-3 font-mono text-sm outline-2 outline-transparent focus-visible:outline-ring aria-invalid:border-destructive"
                />
              </label>
              {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
            </Modal.Body>
            <Modal.Footer>
              <Button type="button" variant="outline" isDisabled={busy} onPress={onClose}>{t("common.cancel")}</Button>
              <Button type="submit" variant="danger" isDisabled={!canSubmit}>
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
