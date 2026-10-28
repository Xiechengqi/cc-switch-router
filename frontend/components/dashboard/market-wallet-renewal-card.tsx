"use client";

import { walletLifecycleKey, trialHours } from "@/lib/client-checkout";
import { ClientNextTrialNotice } from "@/components/dashboard/client-market/client-next-trial-notice";
import * as React from "react";
import Link from "next/link";
import { Button } from "@heroui/react";
import { CalendarClock, Loader2, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import {
  getMarketWalletRenewal,
  updateMarketWalletRenewal,
  type MarketWalletRenewal,
} from "@/lib/market-wallet-api";

export function MarketWalletRenewalCard({
  contractId,
  onChanged,
  compact = false,
  refreshKey,
}: {
  contractId: string;
  onChanged?: () => Promise<void> | void;
  compact?: boolean;
  refreshKey?: string;
}) {
  const { locale, t } = useLocaleText();
  const [contract, setContract] = React.useState<MarketWalletRenewal | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [attempt, setAttempt] = React.useState(0);

  React.useEffect(() => {
    let active = true;
    setContract(null);
    setError("");
    void getMarketWalletRenewal(contractId).then((value) => {
      if (active) setContract(value);
    }).catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => { active = false; };
  }, [contractId, attempt, refreshKey]);

  const toggle = async () => {
    if (!contract || busy) return;
    setBusy(true);
    setError("");
    try {
      const next = await updateMarketWalletRenewal(contractId, contract.renewalPolicy !== "automatic", contract.renewalPolicy);
      setContract(next);
      await onChanged?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setContract(null);
    } finally {
      setBusy(false);
    }
  };

  if (!contract && !error) return <span className="inline-flex items-center gap-2 text-xs text-muted-foreground"><Loader2 className="h-3.5 w-3.5 animate-spin" />{t("common.loading")}</span>;
  if (!contract) return <div className="flex flex-wrap items-center gap-2 text-xs text-destructive"><span role="alert">{error}</span><Button size="sm" variant="ghost" onPress={() => setAttempt((value) => value + 1)}><RefreshCw className="h-3.5 w-3.5" />{t("common.retry")}</Button></div>;

  const canChange = contract.status === "trialing" || contract.status === "active"
    || (contract.status === "suspended" && contract.renewalPolicy === "automatic");
  const nextDate = contract.periodEnd && Number.isFinite(Date.parse(contract.periodEnd))
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(new Date(contract.periodEnd))
    : null;
  return <div className={`flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 rounded-md border border-border bg-card ${compact ? "px-2 py-1.5 text-xs" : "px-3 py-2 text-sm"}`} data-no-card-open>
    <span className="inline-flex items-center gap-1.5"><CalendarClock className="h-4 w-4 text-muted-foreground" />
      <strong>{formatUsdMoney(contract.cyclePriceMinor, locale)} / {t("market.month")}</strong>
    </span>
    <span className="text-muted-foreground">{t(walletLifecycleKey(contract), { hours: trialHours(contract.trialSecondsRemaining, locale), date: nextDate || "—" })}</span>
    <span className="text-muted-foreground">{contract.renewalPolicy === "automatic"
      ? t("marketWallet.renewal.auto") : t("marketWallet.renewal.manual")}</span>
    {canChange ? <Button size="sm" variant="outline" isDisabled={busy} onPress={() => void toggle()}>
      {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
      {t(contract.renewalPolicy === "automatic" ? "marketWallet.renewal.stop" : "marketWallet.renewal.start")}
    </Button> : null}
    <Link className="font-medium text-primary hover:underline" href={DASHBOARD_ACCOUNT_WALLET_PATH}>{t("marketWallet.renewal.wallet")}</Link>
    {!compact ? <ClientNextTrialNotice trial={contract.nextRentalTrial} className="basis-full" /> : null}
    {error ? <span role="alert" className="basis-full text-xs text-destructive">{error}</span> : null}
  </div>;
}
