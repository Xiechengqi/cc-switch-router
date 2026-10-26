"use client";

import * as React from "react";
import { I18nProvider } from "react-aria-components";
import {
  detectBrowserLocale,
  interpolate,
  messages,
  readStoredLocale,
  type AppLocale,
  type MessageKey,
  writeStoredLocale,
} from "@/lib/i18n";
import {
  DEFAULT_USD_CNY_RATE_MICROS,
  formatUsdCnyRate,
  readStoredDisplayCurrency,
  setMarketMoneyDisplay,
  writeStoredDisplayCurrency,
  type DisplayCurrency,
} from "@/lib/market-money";

type LocaleContextValue = {
  locale: AppLocale;
  setLocale: (locale: AppLocale) => void;
  displayCurrency: DisplayCurrency;
  setDisplayCurrency: (currency: DisplayCurrency) => void;
  fxRateFresh: boolean;
  fxRateMicros: number;
  fxRateLabel: string;
  t: (key: MessageKey, values?: Record<string, string | number>) => string;
};

const LocaleContext = React.createContext<LocaleContextValue | null>(null);

export function LocaleProvider({ children }: { children: React.ReactNode }) {
  // Match the server snapshot during hydration, then apply the browser preference.
  const [locale, setLocaleState] = React.useState<AppLocale>("en");
  const [displayCurrency, setDisplayCurrencyState] = React.useState<DisplayCurrency>("USD");
  const [fxRateFresh, setFxRateFresh] = React.useState(false);
  const [fxRateMicros, setFxRateMicros] = React.useState(DEFAULT_USD_CNY_RATE_MICROS);

  React.useEffect(() => {
    const next = readStoredLocale() || detectBrowserLocale();
    setLocaleState((current) => (current === next ? current : next));
    document.documentElement.lang = next;
  }, []);

  React.useEffect(() => {
    let active = true;
    const loadRate = async () => {
      try {
        const response = await fetch("/v1/market/config", { cache: "no-store" });
        if (!response.ok) throw new Error("market FX rate unavailable");
        const config = await response.json() as { usdCnyRateMicros?: number; fxRateFresh?: boolean };
        if (!active) return;
        const rate = config.usdCnyRateMicros;
        const fresh = config.fxRateFresh === true
          && Number.isSafeInteger(rate) && rate! >= 1_000_000 && rate! <= 20_000_000;
        const effectiveRate = fresh ? rate! : DEFAULT_USD_CNY_RATE_MICROS;
        const nextCurrency = fresh ? readStoredDisplayCurrency() : "USD";
        setMarketMoneyDisplay(nextCurrency, effectiveRate, fresh);
        setFxRateMicros(effectiveRate);
        setFxRateFresh(fresh);
        setDisplayCurrencyState(nextCurrency);
      } catch {
        if (!active) return;
        setMarketMoneyDisplay("USD", DEFAULT_USD_CNY_RATE_MICROS, false);
        setFxRateFresh(false);
        setDisplayCurrencyState("USD");
      }
    };
    void loadRate();
    const timer = window.setInterval(() => void loadRate(), 15 * 60_000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);

  const setDisplayCurrency = React.useCallback((currency: DisplayCurrency) => {
    if (currency === "CNY" && !fxRateFresh) return;
    writeStoredDisplayCurrency(currency);
    setMarketMoneyDisplay(currency, fxRateMicros, fxRateFresh);
    setDisplayCurrencyState(currency);
  }, [fxRateFresh, fxRateMicros]);

  const setLocale = React.useCallback((nextLocale: AppLocale) => {
    writeStoredLocale(nextLocale);
    document.documentElement.lang = nextLocale;
    setLocaleState(nextLocale);
  }, []);

  const t = React.useCallback(
    (key: MessageKey, values?: Record<string, string | number>) => {
      const template = messages[locale][key] || messages.en[key] || key;
      return interpolate(template, values);
    },
    [locale],
  );

  const value = React.useMemo(() => ({
    locale, setLocale, displayCurrency, setDisplayCurrency,
    fxRateFresh, fxRateMicros, fxRateLabel: formatUsdCnyRate(fxRateMicros), t,
  }), [locale, setLocale, displayCurrency, setDisplayCurrency, fxRateFresh, fxRateMicros, t]);

  return (
    <LocaleContext.Provider value={value}>
      <I18nProvider locale={locale}>{children}</I18nProvider>
    </LocaleContext.Provider>
  );
}

export function useLocaleText() {
  const value = React.useContext(LocaleContext);
  if (!value) throw new Error("useLocaleText must be used within LocaleProvider");
  return value;
}
