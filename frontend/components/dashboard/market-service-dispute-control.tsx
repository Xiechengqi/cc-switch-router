"use client";

/* Hallmark · component: service dispute request · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P4 H5 E4 S5 R5 V4
 */

import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { openMarketServiceDispute } from "@/lib/market-wallet-api";

const REASONS = ["service_unavailable", "billing_incorrect", "service_quality"] as const;

export function MarketServiceDisputeControl({ contractId, initiallyOpen, onChanged }: {
  contractId?: string;
  initiallyOpen?: boolean;
  onChanged?: () => Promise<void> | void;
}) {
  const { t } = useLocaleText();
  const [reason, setReason] = React.useState<(typeof REASONS)[number]>(REASONS[0]);
  const [opened, setOpened] = React.useState(!!initiallyOpen);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => { setOpened(!!initiallyOpen); }, [contractId, initiallyOpen]);
  if (!contractId) return null;
  if (opened) return <p role="status" className="text-xs text-amber-800">{t("marketWallet.disputeOpen")}</p>;

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await openMarketServiceDispute(contractId, reason);
      setOpened(true);
      await onChanged?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return <details className="min-w-0 text-xs">
    <summary className="inline-flex min-h-11 cursor-pointer items-center whitespace-nowrap text-muted-foreground underline-offset-2 hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
      {t("marketWallet.reportServiceIssue")}
    </summary>
    <form className="grid max-w-sm gap-2 pb-2" onSubmit={(event) => void submit(event)}>
      <p className="text-muted-foreground">{t("marketWallet.disputeHelp")}</p>
      <label className="grid gap-1">
        <span>{t("marketWallet.disputeReason")}</span>
        <select value={reason} onChange={(event) => setReason(event.target.value as (typeof REASONS)[number])}
          className="h-11 min-w-0 rounded-md border border-input bg-background px-3 focus-visible:outline-2 focus-visible:outline-ring">
          {REASONS.map((item) => <option key={item} value={item}>{t(`marketWallet.disputeReason.${item}`)}</option>)}
        </select>
      </label>
      {error ? <p role="alert" className="text-destructive">{error}</p> : null}
      <Button type="submit" size="sm" variant="outline" isDisabled={busy} className="min-h-11 justify-self-start whitespace-nowrap">
        {busy ? t("common.loading") : t("marketWallet.submitDispute")}
      </Button>
    </form>
  </details>;
}
