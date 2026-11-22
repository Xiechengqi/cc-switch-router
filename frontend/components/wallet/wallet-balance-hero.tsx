"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatWalletUnits, formatWalletUnitsAsCny } from "@/lib/market-wallet-money";
import type { MarketWallet } from "@/lib/market-wallet-api";

function units(raw: string | undefined) {
  try { return BigInt(raw ?? "0"); } catch { return 0n; }
}

/**
 * The one number people come for. Reserved and due amounts appear only when
 * they exist, as one plain sentence each, never as an extra dashboard.
 */
export function WalletBalanceHero({ wallet }: { wallet: MarketWallet }) {
  const { t, locale, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const cny = displayCurrency === "CNY" && fxRateFresh;
  const money = (raw: string) => cny ? formatWalletUnitsAsCny(raw, fxRateMicros, locale) : formatWalletUnits(raw);
  const held = units(wallet.heldUnits);
  const owed = units(wallet.committedUnits);
  return (
    <section
      aria-label={t("walletPage.balance")}
      className="relative isolate min-w-0 overflow-hidden rounded-2xl bg-foreground px-6 py-7 text-white shadow-xl sm:px-8 sm:py-9"
    >
      {/* Texture: dot grid and a soft accent glow, felt more than seen. */}
      <span aria-hidden className="pointer-events-none absolute inset-0 -z-10 opacity-[0.04] [background-image:radial-gradient(circle,white_1px,transparent_1px)] [background-size:24px_24px]" />
      <span aria-hidden className="pointer-events-none absolute -right-16 -top-24 -z-10 size-72 rounded-full bg-[#4D7CFF] opacity-25 blur-[90px]" />
      <p className="font-mono text-[0.7rem] uppercase tracking-[0.15em] text-white/60">{t("walletPage.balance")}</p>
      <p className="mt-2 break-all text-4xl font-semibold tabular-nums tracking-tight sm:text-5xl" aria-live="polite">
        {money(wallet.availableUnits)}
      </p>
      {cny ? <p className="mt-1 text-xs text-white/60">{t("walletPage.cnyNote", { amount: formatWalletUnits(wallet.availableUnits) })}</p> : null}
      {held > 0n || owed > 0n ? (
        <div className="mt-5 grid gap-1 border-t border-white/10 pt-4 text-sm leading-6 text-white/75">
          {held > 0n ? <p>{t("walletPage.frozen", { amount: money(wallet.heldUnits) })}</p> : null}
          {owed > 0n ? <p>{t("walletPage.owed", { amount: money(wallet.committedUnits ?? "0") })}</p> : null}
        </div>
      ) : null}
    </section>
  );
}
