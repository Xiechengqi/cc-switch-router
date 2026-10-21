"use client";

/* "Wallet & trading" notification preferences. Mandatory kinds (security,
 * money movement, contract state) are always on and shown locked.
 */

import * as React from "react";
import { Lock } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletSectionLabel } from "@/components/market/wallet-primitives";
import { messages, type MessageKey } from "@/lib/i18n";
import {
  getMarketNotificationPreferences,
  updateMarketNotificationPreferences,
  type MarketNotificationPreference,
} from "@/lib/market-wallet-api";
import { cn } from "@/lib/utils";

function kindKey(kind: string): MessageKey {
  const key = `marketNotifications.kind.${kind}`;
  return (key in messages.en ? key : "marketNotifications.kind.other") as MessageKey;
}

export function MarketNotificationPreferences() {
  const { t } = useLocaleText();
  const [items, setItems] = React.useState<MarketNotificationPreference[] | null>(null);
  const [savingKind, setSavingKind] = React.useState<string | null>(null);
  const [error, setError] = React.useState("");
  const [status, setStatus] = React.useState("");

  React.useEffect(() => {
    let active = true;
    getMarketNotificationPreferences()
      .then((next) => { if (active) setItems(next); })
      .catch((cause) => { if (active) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, []);

  const toggle = async (item: MarketNotificationPreference) => {
    if (item.mandatory || savingKind) return;
    const enabled = !item.enabled;
    setSavingKind(item.eventKind);
    setError("");
    setStatus("");
    setItems((current) => current?.map((entry) => entry.eventKind === item.eventKind ? { ...entry, enabled } : entry) ?? current);
    try {
      setItems(await updateMarketNotificationPreferences([{ eventKind: item.eventKind, enabled }]));
      setStatus(t("marketNotifications.preferenceSaved"));
    } catch (cause) {
      setItems((current) => current?.map((entry) => entry.eventKind === item.eventKind ? { ...entry, enabled: item.enabled } : entry) ?? current);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSavingKind(null);
    }
  };

  const optional = items?.filter((item) => !item.mandatory) ?? [];
  const mandatory = items?.filter((item) => item.mandatory) ?? [];

  const row = (item: MarketNotificationPreference) => {
    const id = `market-notification-pref-${item.eventKind}`;
    return (
      <li key={item.eventKind} className="flex min-h-11 items-center justify-between gap-3 py-1.5">
        <label htmlFor={id} className={cn("min-w-0 text-sm", item.mandatory ? "text-muted-foreground" : "cursor-pointer text-foreground")}>
          {t(kindKey(item.eventKind))}
        </label>
        {item.mandatory ? (
          <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <Lock className="size-3.5" aria-hidden="true" />{t("marketNotifications.required")}
            <input id={id} type="checkbox" checked readOnly disabled className="sr-only" aria-describedby="market-notification-mandatory-note" />
          </span>
        ) : (
          <span className="relative inline-flex size-11 items-center justify-center">
            <input
              id={id}
              type="checkbox"
              role="switch"
              aria-checked={item.enabled}
              checked={item.enabled}
              disabled={savingKind === item.eventKind}
              onChange={() => void toggle(item)}
              className="peer absolute inset-0 z-10 cursor-pointer opacity-0 disabled:cursor-wait"
            />
            <span aria-hidden="true" className="pointer-events-none relative h-6 w-10 rounded-full bg-slate-300 transition-colors duration-200 peer-checked:bg-gradient-to-r peer-checked:from-[#0052FF] peer-checked:to-[#4D7CFF] peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-ring peer-disabled:opacity-60 motion-reduce:transition-none" />
            <span aria-hidden="true" className="pointer-events-none absolute left-[0.4375rem] size-5 rounded-full bg-white shadow-sm transition-transform duration-200 peer-checked:translate-x-4 motion-reduce:transition-none" />
          </span>
        )}
      </li>
    );
  };

  return (
    <section className="grid gap-3 rounded-xl border border-border bg-card p-4 sm:p-5" aria-labelledby="market-notification-prefs-title">
      <div className="grid gap-2">
        <WalletSectionLabel>{t("marketNotifications.sectionLabel")}</WalletSectionLabel>
        <h3 id="market-notification-prefs-title" className="text-sm font-semibold text-foreground">{t("marketNotifications.preferencesTitle")}</h3>
        <p className="text-sm text-muted-foreground">{t("marketNotifications.preferencesHint")}</p>
      </div>
      {error ? <p role="alert" className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</p> : null}
      <p className="sr-only" aria-live="polite">{status}</p>
      {!items && !error ? <p className="text-sm text-muted-foreground">{t("common.loading")}</p> : null}
      {items ? (
        <div className="grid gap-4 md:grid-cols-2">
          <div>
            <h4 className="font-mono text-[0.68rem] uppercase tracking-[0.15em] text-muted-foreground">{t("marketNotifications.optional")}</h4>
            <ul className="mt-1 divide-y divide-border">{optional.map(row)}</ul>
          </div>
          <div>
            <h4 className="font-mono text-[0.68rem] uppercase tracking-[0.15em] text-muted-foreground">{t("marketNotifications.mandatory")}</h4>
            <ul className="mt-1 divide-y divide-border">{mandatory.map(row)}</ul>
            <p id="market-notification-mandatory-note" className="mt-2 text-xs text-muted-foreground">{t("marketNotifications.mandatoryNote")}</p>
          </div>
        </div>
      ) : null}
    </section>
  );
}
