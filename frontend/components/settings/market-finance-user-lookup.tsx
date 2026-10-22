"use client";

/* Owner lookup of one user's wallet, Provider balances and recent activity. */

import * as React from "react";
import { Alert, Button } from "@heroui/react";
import { Loader2, Search } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import { lookupMarketFinanceUser, type MarketFinanceTopup, type MarketFinanceUserLookup, type MarketWalletTopup } from "@/lib/market-wallet-api";
import { MarketFinanceTopupOwnerActionDialog } from "@/components/settings/market-finance-topup-owner-actions";
import { formatMinorUsd, formatWalletUnits } from "@/lib/market-wallet-money";
import { paymentTokenLabel } from "@/lib/market-payment-tokens";
import { messages, type MessageKey } from "@/lib/i18n";
import { codeLabel, OwnerField, ownerDateText, ownerErrorText } from "@/components/settings/market-finance-shared";

export function MarketFinanceUserLookup() {
  const { locale, t } = useLocaleText();
  const [email, setEmail] = React.useState("");
  const [result, setResult] = React.useState<MarketFinanceUserLookup | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [reverseTarget, setReverseTarget] = React.useState<MarketFinanceTopup | null>(null);
  // Credited top-ups are not in the recovery queue, so reversal starts here.
  const toFinanceTopup = (order: MarketWalletTopup): MarketFinanceTopup => ({
    id: order.id, userId: result?.userId ?? "", channelId: order.channelId, merchantOrderId: order.id,
    gatewayOrderId: null, amountMinor: order.amountMinor, status: order.status,
    createdAt: order.createdAt, updatedAt: order.createdAt, buyerAbandonedAt: order.buyerAbandonedAt,
    userEmail: result?.email ?? null, orderMode: order.orderMode,
  });
  const units = (raw: string) => formatWalletUnits(raw, "$", locale);
  const isZero = (raw: string) => { try { return BigInt(raw) === 0n; } catch { return false; } };
  const entryLabel = (kind: string) => {
    const key = `marketWallet.entry.${kind}`;
    return t((key in messages.en ? key : "marketWallet.entry.other") as MessageKey);
  };

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || !email.trim()) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      setResult(await lookupMarketFinanceUser(email));
    } catch (cause) {
      setResult(null);
      setError(cause instanceof ApiError && cause.status === 404 ? t("settings.marketFinance.lookup.notFound") : ownerErrorText(t, cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="grid min-w-0 gap-3" aria-labelledby="market-finance-lookup-title">
      <div className="grid gap-1">
        <h4 id="market-finance-lookup-title" className="text-sm font-semibold">{t("settings.marketFinance.lookup.title")}</h4>
        <p className="text-xs text-muted-foreground">{t("settings.marketFinance.lookup.note")}</p>
      </div>
      <form className="flex min-w-0 flex-col gap-2 sm:flex-row sm:items-end" onSubmit={(event) => void submit(event)}>
        <OwnerField className="flex-1" type="email" label={t("settings.marketFinance.lookup.email")} value={email} onChange={(event) => setEmail(event.target.value)} maxLength={320} placeholder="buyer@example.com" />
        <Button type="submit" variant="outline" className="min-h-11 whitespace-nowrap" isDisabled={busy || !email.trim()}>
          {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Search className="size-4" aria-hidden="true" />}
          {t("settings.marketFinance.lookup.search")}
        </Button>
      </form>
      <div aria-live="polite" className="grid gap-3">
        {error ? <Alert status="danger">{error}</Alert> : null}
        {notice ? <Alert status="success">{notice}</Alert> : null}
        {result ? (
          <div className="grid min-w-0 gap-4 rounded-2xl border border-border bg-card p-4 shadow-sm">
            <div className="min-w-0">
              <p className="break-all font-medium">{result.email}</p>
              <p className="break-all font-mono text-xs text-muted-foreground">{result.userId}</p>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <dl className="grid gap-1 rounded-xl bg-muted/50 p-3 text-xs">
                <dt className="mb-1 font-mono text-[0.68rem] uppercase tracking-[0.15em] text-accent">{t("settings.marketFinance.lookup.wallet")}</dt>
                {([["lookup.posted", result.wallet.postedUnits], ["lookup.held", result.wallet.heldUnits], ["lookup.available", result.wallet.availableUnits]] as const).map(([key, value]) => (
                  <div key={key} className="flex justify-between gap-3"><span className="text-muted-foreground">{t(`settings.marketFinance.${key}`)}</span><span className="font-medium tabular-nums">{units(value)}</span></div>
                ))}
              </dl>
              <dl className="grid gap-1 rounded-xl bg-muted/50 p-3 text-xs">
                <dt className="mb-1 font-mono text-[0.68rem] uppercase tracking-[0.15em] text-accent">{t("settings.marketFinance.lookup.provider")}</dt>
                {([
                  [t("marketProviderFinance.pending"), result.provider.pendingUnits],
                  [t("settings.marketFinance.lookup.providerAvailable"), result.provider.availableUnits],
                  [t("marketProviderFinance.withdrawable"), result.provider.withdrawableUnits],
                  [t("marketProviderFinance.held"), result.provider.withdrawalHeldUnits],
                  [t("settings.marketFinance.lookup.providerPaid"), result.provider.paidUnits],
                  [t("earningConversion.total"), result.provider.convertedUnits ?? "0"],
                ] as const).map(([label, value]) => (
                  <div key={label} className="flex justify-between gap-3"><span className="text-muted-foreground">{label}</span><span className="font-medium tabular-nums">{units(value)}</span></div>
                ))}
              </dl>
            </div>
            <div className="grid gap-4 lg:grid-cols-2">
              <div className="grid content-start gap-2">
                <h5 className="text-xs font-semibold">{t("settings.marketFinance.lookup.transactions")}</h5>
                {result.transactions.length ? <ul className="grid gap-1">{result.transactions.slice(0, 20).map((entry) => (
                  <li key={entry.id} className="flex min-w-0 justify-between gap-2 rounded-lg bg-muted/40 px-2 py-1.5 text-xs">
                    <span className="min-w-0"><span className="font-medium">{entryLabel(entry.sourceKind)}</span> <span className="text-muted-foreground">· {ownerDateText(entry.createdAt, locale)}</span></span>
                    <span className="shrink-0 tabular-nums">{isZero(entry.deltaUnits) && !isZero(entry.heldDeltaUnits)
                      ? t("settings.marketFinance.lookup.heldDelta", { amount: units(entry.heldDeltaUnits) })
                      : units(entry.deltaUnits)}</span>
                  </li>
                ))}</ul> : <p className="text-xs text-muted-foreground">{t("settings.marketFinance.lookup.none")}</p>}
              </div>
              <div className="grid content-start gap-2">
                <h5 className="text-xs font-semibold">{t("settings.marketFinance.lookup.topups")}</h5>
                {result.topups.length ? <ul className="grid gap-1">{result.topups.slice(0, 20).map((order) => (
                  <li key={order.id} className="flex min-w-0 flex-wrap justify-between gap-2 rounded-lg bg-muted/40 px-2 py-1.5 text-xs">
                    <span className="min-w-0"><span className="font-medium tabular-nums">{formatMinorUsd(order.amountMinor)}</span> <span className="text-muted-foreground">· {ownerDateText(order.createdAt, locale)}{order.paymentTokenId ? ` · ${paymentTokenLabel(order.paymentTokenId).label}` : ""}</span></span>
                    <span className="flex items-center gap-2">{codeLabel(t, "marketWallet.status.", order.status)}
                      {order.status === "credited" ? <Button size="sm" variant="outline" className="min-h-11 whitespace-nowrap" onPress={() => { setNotice(""); setReverseTarget(toFinanceTopup(order)); }}>
                        {t("settings.marketFinance.topupRecovery.reverse")}
                      </Button> : null}
                    </span>
                  </li>
                ))}</ul> : <p className="text-xs text-muted-foreground">{t("settings.marketFinance.lookup.none")}</p>}
              </div>
            </div>
            <div className="grid gap-2">
              <h5 className="text-xs font-semibold">{t("settings.marketFinance.lookup.holds")}</h5>
              {result.earningHolds.length ? <ul className="grid gap-1">{result.earningHolds.map((hold) => (
                <li key={`${hold.reasonKind}:${hold.reasonRef}`} className="flex min-w-0 flex-wrap justify-between gap-2 rounded-lg bg-muted/40 px-2 py-1.5 text-xs">
                  <span>{codeLabel(t, "settings.marketFinance.holds.reason.", hold.reasonKind)} · <span className="break-all font-mono">{hold.reasonRef}</span></span>
                  <span className="tabular-nums">{units(hold.heldUnits)}</span>
                </li>
              ))}</ul> : <p className="text-xs text-muted-foreground">{t("settings.marketFinance.lookup.none")}</p>}
            </div>
          </div>
        ) : null}
      </div>
      <MarketFinanceTopupOwnerActionDialog
        action={reverseTarget ? "reverse" : null}
        topup={reverseTarget}
        onClose={() => setReverseTarget(null)}
        onDone={async (message) => {
          setNotice(message);
          setReverseTarget(null);
          if (result) setResult(await lookupMarketFinanceUser(result.email).catch(() => result));
        }}
      />
    </section>
  );
}
