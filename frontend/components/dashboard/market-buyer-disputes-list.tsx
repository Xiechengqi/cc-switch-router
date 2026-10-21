"use client";

/* Buyer's own service disputes: status, resolution and refunded amount. */

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletBadge } from "@/components/market/wallet-primitives";
import { messages, type MessageKey } from "@/lib/i18n";
import { getMarketServiceDisputes, type MarketServiceDispute } from "@/lib/market-wallet-api";
import { formatWalletUnits } from "@/lib/market-wallet-money";

function knownKey(key: string): MessageKey | null {
  return key in messages.en ? key as MessageKey : null;
}

export function MarketBuyerDisputesList() {
  const { locale, t } = useLocaleText();
  const [items, setItems] = React.useState<MarketServiceDispute[]>([]);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    let active = true;
    getMarketServiceDisputes()
      .then((next) => { if (active) setItems(next); })
      .catch((cause) => { if (active) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, []);

  if (!items.length && !error) return null;
  const date = (value: string) => new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(new Date(value));

  return (
    <section className="grid min-w-0 gap-3 rounded-xl border border-border bg-card p-5" aria-labelledby="market-buyer-disputes-title">
      <div className="grid gap-1">
        <h2 id="market-buyer-disputes-title" className="text-base font-semibold">{t("marketWallet.disputes.title")}</h2>
        <p className="text-sm text-muted-foreground">{t("marketWallet.disputes.help")}</p>
      </div>
      {error ? <p role="alert" className="rounded-md bg-destructive/5 px-3 py-2 text-sm text-destructive">{error}</p> : null}
      <ul className="grid gap-2">
        {items.map((item) => {
          const reasonKey = knownKey(`marketWallet.disputeReason.${item.reasonCode}`);
          const refunded = (() => { try { return BigInt(item.refundedUnits) > 0n; } catch { return false; } })();
          return (
            <li key={item.id} className="grid min-w-0 gap-1 rounded-md bg-muted/50 px-3 py-2 text-sm">
              <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
                <span className="font-medium">{reasonKey ? t(reasonKey) : item.reasonCode}</span>
                <WalletBadge tone={item.status === "open" ? "warning" : "success"}>
                  {t(item.status === "open" ? "marketWallet.disputes.statusOpen" : "marketWallet.disputes.statusResolved")}
                </WalletBadge>
              </div>
              <p className="break-all font-mono text-xs text-muted-foreground">{item.contractId}</p>
              <p className="text-xs text-muted-foreground">
                {t("marketWallet.disputes.opened", { date: date(item.createdAt) })}
                {item.resolvedAt ? ` · ${t("marketWallet.disputes.resolved", { date: date(item.resolvedAt) })}` : ""}
              </p>
              {item.status === "resolved" ? (
                <p className="text-xs">
                  {item.resolutionCode ? <span className="font-mono text-muted-foreground">{item.resolutionCode} · </span> : null}
                  <span className={refunded ? "font-medium tabular-nums text-emerald-800" : "text-muted-foreground"}>
                    {refunded
                      ? t("marketWallet.disputes.refunded", { amount: formatWalletUnits(item.refundedUnits, "$", locale) })
                      : t("marketWallet.disputes.noRefund")}
                  </span>
                </p>
              ) : null}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
