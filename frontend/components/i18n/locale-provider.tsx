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

type LocaleContextValue = {
  locale: AppLocale;
  setLocale: (locale: AppLocale) => void;
  t: (key: MessageKey, values?: Record<string, string | number>) => string;
};

const LocaleContext = React.createContext<LocaleContextValue | null>(null);

function initialLocale(): AppLocale {
  if (typeof window === "undefined") return "en";
  return readStoredLocale() || detectBrowserLocale();
}

export function LocaleProvider({ children }: { children: React.ReactNode }) {
  const [locale, setLocaleState] = React.useState<AppLocale>(initialLocale);

  React.useEffect(() => {
    const next = readStoredLocale() || detectBrowserLocale();
    setLocaleState((current) => (current === next ? current : next));
  }, []);

  const setLocale = React.useCallback((nextLocale: AppLocale) => {
    writeStoredLocale(nextLocale);
    setLocaleState(nextLocale);
  }, []);

  const t = React.useCallback(
    (key: MessageKey, values?: Record<string, string | number>) => {
      const template = messages[locale][key] || messages.en[key] || key;
      return interpolate(template, values);
    },
    [locale],
  );

  const value = React.useMemo(() => ({ locale, setLocale, t }), [locale, setLocale, t]);

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
