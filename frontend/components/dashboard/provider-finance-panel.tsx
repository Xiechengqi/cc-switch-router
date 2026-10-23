"use client";

import * as React from "react";
import Link from "next/link";
import { Button } from "@heroui/react";
import { ShieldAlert } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletBadge } from "@/components/market/wallet-primitives";
import { messages, type MessageKey } from "@/lib/i18n";
import {
  getMarketProviderEarnings, getMarketProviderFinance, getMarketProviderHolds,
  type MarketEarningHold, type MarketProviderFinance,
} from "@/lib/market-wallet-api";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { useAuth } from "@/components/auth/auth-provider";
import { subscribeMarketBalanceChanged } from "@/lib/market-balance-events";
import { earningLotState, earningUnits, refreshEarningHistory, type EarningHistory } from "@/lib/earning-settlement";

function holdReasonKey(kind: string): MessageKey {
  const key = `marketProviderFinance.holdReason.${kind}`;
  return (key in messages.en ? key : "marketProviderFinance.holdReason.other") as MessageKey;
}

export function ProviderFinancePanel({ walletMode }: { walletMode: "disabled" | "shadow" | "enabled" }) {
  const { session } = useAuth();
  const { locale, t } = useLocaleText();
  const [finance, setFinance] = React.useState<MarketProviderFinance | null>(null);
  const [holds, setHolds] = React.useState<MarketEarningHold[]>([]);
  const [history, setHistory] = React.useState<EarningHistory>({ items: [], nextCursor: null });
  const historyRef = React.useRef<EarningHistory | null>(null);
  const [loadedActor, setLoadedActor] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const generation = React.useRef(0);
  const inFlight = React.useRef(false);
  const actor = session?.user?.id;

  React.useEffect(() => {
    let active = true;
    historyRef.current = null;
    setFinance(null); setHolds([]); setHistory({ items: [], nextCursor: null });
    setLoadedActor(undefined); setBusy(false); setError("");
    const refresh = async () => {
      if (!actor || !active || document.visibilityState !== "visible" || inFlight.current) return;
      const seq = ++generation.current;
      inFlight.current = true;
      try {
        const [next, firstPage, nextHolds] = await Promise.all([
          getMarketProviderFinance(), getMarketProviderEarnings(), getMarketProviderHolds(),
        ]);
        const merged = await refreshEarningHistory(historyRef.current, firstPage, getMarketProviderEarnings,
          () => active && seq === generation.current);
        if (!merged) return;
        historyRef.current = merged; setHistory(merged);
        setFinance(next); setHolds(nextHolds); setLoadedActor(actor); setError("");
      } catch (cause) {
        if (active && seq === generation.current) setError(cause instanceof Error ? cause.message : String(cause));
      } finally { if (active && seq === generation.current) inFlight.current = false; }
    };
    const onVisible = () => { void refresh(); };
    const off = actor ? subscribeMarketBalanceChanged(actor, onVisible) : () => {};
    const timer = window.setInterval(onVisible, 15_000);
    window.addEventListener("focus", onVisible);
    document.addEventListener("visibilitychange", onVisible);
    onVisible();
    return () => { active = false; generation.current++; inFlight.current = false; window.clearInterval(timer); off(); window.removeEventListener("focus", onVisible); document.removeEventListener("visibilitychange", onVisible); };
  }, [actor]);

  const more = async () => {
    if (loadedActor !== actor || !history.nextCursor || busy || inFlight.current) return;
    const seq = ++generation.current;
    inFlight.current = true; setBusy(true);
    try {
      const page = await getMarketProviderEarnings(history.nextCursor);
      if (seq !== generation.current) return;
      const previous = historyRef.current ?? history;
      const merged = { items: [...previous.items, ...page.items.filter(v => !previous.items.some(n => n.id === v.id))], nextCursor: page.nextCursor };
      historyRef.current = merged; setHistory(merged); setError("");
    } catch (cause) { if (seq === generation.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { if (seq === generation.current) { setBusy(false); inFlight.current = false; } }
  };
  const dateText = (value: string) => {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
  };
  const linkClass = "inline-flex min-h-11 items-center whitespace-nowrap rounded-lg px-3 text-sm font-medium text-primary hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

  return <section className="mx-auto grid max-w-5xl gap-5 px-4 pb-8 sm:px-6" aria-label={t("marketProviderFinance.title")}>
    <header className="grid gap-2">
      <h2 className="text-lg font-semibold">{t("marketProviderFinance.title")}</h2>
      <p className="text-sm text-muted-foreground">{t("earningSettlement.help")}</p>
      {walletMode !== "enabled" ? <p className="text-sm text-muted-foreground">{t("earningSettlement.notOpen")}</p> : null}
      <div className="flex flex-wrap gap-2">
        <Link href="/account/wallet/" className={linkClass}>{t("earningSettlement.wallet")}</Link>
        <Link href="/account/wallet/?c2cSell=1#c2c-orders" className={linkClass}>{t("c2c.seller")}</Link>
        <Link href="/account/wallet/#wallet-transfer" className={linkClass}>{t("walletTransfer.title")}</Link>
      </div>
    </header>
    {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
    {!finance || loadedActor !== actor ? <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p> : <>
      <dl className="grid gap-3 sm:grid-cols-3">
        {([
          ["marketProviderFinance.pending", finance.summary.pendingUnits],
          ["earningSettlement.total", finance.summary.creditedUnits],
          ["marketProviderFinance.holdsTitle", finance.summary.restrictedUnits],
        ] as const).map(([key, value]) => <div key={key} className="min-w-0 rounded-lg bg-muted/50 p-4">
          <dt className="text-xs text-muted-foreground">{t(key)}</dt>
          <dd className="mt-1 break-all text-xl font-semibold tabular-nums">{formatWalletUnits(value, "$", locale)}</dd>
        </div>)}
      </dl>
      {earningUnits(finance.summary.readyUnits) > 0n ? <p role="status" className="text-sm text-muted-foreground">{t("earningSettlement.waiting", { amount: formatWalletUnits(finance.summary.readyUnits, "$", locale) })}</p> : null}
      <details className="min-w-0 text-sm">
        <summary className="min-h-11 cursor-pointer py-3 font-medium focus-visible:outline-2 focus-visible:outline-ring">{t("commission.earningsDetail")}</summary>
        <dl className="grid gap-2 pb-3 tabular-nums sm:grid-cols-3">{([
          ["commission.gross",finance.summary.grossEarnedUnits], ["commission.deducted",finance.summary.commissionUnits], ["commission.net",finance.summary.netEarnedUnits],
        ] as const).map(([key,value]) => <div key={key}><dt className="text-muted-foreground">{t(key)}</dt><dd>{formatWalletUnits(value,"$",locale)}</dd></div>)}</dl>
        <p className="text-xs text-muted-foreground">{t("commission.providerHelp")}</p>
      </details>
      {holds.length ? <aside className="grid gap-3 rounded-lg bg-amber-50 p-4" aria-label={t("marketProviderFinance.holdsTitle")}>
        <h3 className="flex items-center gap-2 text-sm font-semibold text-amber-950"><ShieldAlert className="size-4" aria-hidden="true" />{t("marketProviderFinance.holdsTitle")}</h3>
        <p className="text-xs text-amber-900">{t("marketProviderFinance.holdsHelp")}</p>
        <ul className="grid gap-2">{holds.map(hold => <li key={`${hold.reasonKind}:${hold.reasonRef}`} className="flex flex-wrap items-center justify-between gap-2 text-sm">
          <span>{t(holdReasonKey(hold.reasonKind))} · {t("marketProviderFinance.holdLots", { count: hold.lots })}</span>
          <span className="tabular-nums">{formatWalletUnits(hold.heldUnits,"$",locale)}</span>
        </li>)}</ul>
      </aside> : null}
      <section className="grid min-w-0 gap-3" aria-labelledby="provider-earnings-title">
        <h3 id="provider-earnings-title" className="font-semibold">{t("marketProviderFinance.earningsTitle")}</h3>
        {history.items.length ? <div role="region" aria-labelledby="provider-earnings-title" tabIndex={0} className="max-h-96 overflow-auto overscroll-contain rounded-sm focus-visible:outline-2 focus-visible:outline-ring">
          <table className="w-full min-w-[34rem] text-left text-xs">
            <thead className="sticky top-0 bg-background text-muted-foreground"><tr>
              {(["marketProviderFinance.lot.fulfilled","marketProviderFinance.lot.unlocks","commission.net","earningSettlement.credited","marketProviderFinance.lot.state"] as const).map(key => <th key={key} scope="col" className="py-3 pr-3 font-medium">{t(key)}</th>)}
            </tr></thead>
            <tbody>{history.items.map(lot => { const state = earningLotState(lot); return <tr key={lot.id}>
              <td className="py-3 pr-3">{dateText(lot.fulfilledAt)}</td><td className="py-3 pr-3">{dateText(lot.unlockAt)}</td>
              <td className="py-3 pr-3 tabular-nums">
                {formatWalletUnits((earningUnits(lot.amountUnits)-earningUnits(lot.reversedUnits)).toString(),"$",locale)}
                {lot.commissionPolicyVersion > 0 ? <details className="text-muted-foreground">
                  <summary className="min-h-11 cursor-pointer whitespace-nowrap rounded-lg py-3 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">{t("commission.breakdown")}</summary>
                  <span className="block">{t("commission.lotGross", { amount: formatWalletUnits(lot.grossUnits,"$",locale) })}</span>
                  <span className="block">{t("commission.previewFee", { rate: (lot.commissionRateBps / 100).toString(), fee: formatWalletUnits(lot.commissionUnits,"$",locale) })}</span>
                  {earningUnits(lot.commissionReversedUnits) > 0n ? <span className="block">{t("commission.lotReversed", { amount: formatWalletUnits(lot.commissionReversedUnits,"$",locale) })}</span> : null}
                </details> : null}
              </td>
              <td className="py-3 pr-3 tabular-nums">{formatWalletUnits(lot.creditedUnits,"$",locale)}</td>
              <td className="py-3"><WalletBadge tone={state.tone}>{t(state.key)}</WalletBadge></td>
            </tr>; })}</tbody>
          </table>
          {history.nextCursor ? <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void more()}>{t(busy ? "common.loading" : "marketWallet.more")}</Button> : null}
        </div> : <p className="text-sm text-muted-foreground">{t("marketProviderFinance.noEarnings")}</p>}
      </section>
    </>}
  </section>;
}
