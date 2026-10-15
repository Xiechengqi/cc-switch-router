export const MARKET_CURRENCY = "USD" as const;
export const USD_CNY_RATE_SCALE = 1_000_000;
export const DEFAULT_USD_CNY_RATE_MICROS = 7 * USD_CNY_RATE_SCALE;
export const DISPLAY_CURRENCY_STORAGE_KEY = "cc_switch_router_display_currency_v1";
export type DisplayCurrency = "USD" | "CNY";

// Set only by the client LocaleProvider after hydration. Server rendering always
// starts in USD, so one user's display preference cannot leak into another SSR.
let displayCurrency: DisplayCurrency = "USD";
let displayRateMicros = DEFAULT_USD_CNY_RATE_MICROS;

export function setMarketMoneyDisplay(currency: DisplayCurrency, rateMicros: number, rateFresh: boolean) {
  displayCurrency = currency === "CNY" && rateFresh ? "CNY" : "USD";
  if (Number.isSafeInteger(rateMicros) && rateMicros > 0) displayRateMicros = rateMicros;
}

export function readStoredDisplayCurrency(): DisplayCurrency {
  if (typeof localStorage === "undefined") return "USD";
  try {
    return localStorage.getItem(DISPLAY_CURRENCY_STORAGE_KEY) === "CNY" ? "CNY" : "USD";
  } catch {
    return "USD";
  }
}

export function writeStoredDisplayCurrency(currency: DisplayCurrency) {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(DISPLAY_CURRENCY_STORAGE_KEY, currency);
  } catch {
    // Private browsing may reject storage; the in-memory choice still works.
  }
}

function formatCurrencyMinor(valueMinor: number, currency: "USD" | "CNY", locale: string) {
  return new Intl.NumberFormat(locale, { style: "currency", currency }).format(valueMinor / 100);
}

export function usdMinorToCnyMinor(
  amountUsdMinor: number,
  usdCnyRateMicros: number,
) {
  const amount = BigInt(Math.trunc(amountUsdMinor));
  const rate = BigInt(Math.trunc(usdCnyRateMicros));
  const scale = BigInt(USD_CNY_RATE_SCALE);
  const rounded = amount >= 0
    ? (amount * rate + scale / BigInt(2)) / scale
    : (amount * rate - scale / BigInt(2)) / scale;
  return Number(rounded);
}

export function formatUsdCnyRate(usdCnyRateMicros: number) {
  const normalized = Math.max(0, Math.trunc(usdCnyRateMicros));
  const whole = Math.floor(normalized / USD_CNY_RATE_SCALE);
  const fraction = normalized % USD_CNY_RATE_SCALE;
  if (!fraction) return String(whole);
  return `${whole}.${String(fraction).padStart(6, "0").replace(/0+$/, "")}`;
}

export function formatUsdMoney(amountUsdMinor: number, locale: string) {
  return formatCurrencyMinor(amountUsdMinor, "USD", locale);
}

export function formatMarketPrice(amountUsdMinor: number, locale: string) {
  return formatUsdMoneyInCurrency(amountUsdMinor, locale, displayCurrency, displayRateMicros);
}

export function formatUsdMoneyInCurrency(
  amountUsdMinor: number,
  locale: string,
  currency: DisplayCurrency,
  rateMicros: number,
) {
  return currency === "CNY"
    ? `≈${formatCurrencyMinor(usdMinorToCnyMinor(amountUsdMinor, rateMicros), "CNY", locale)}`
    : formatCurrencyMinor(amountUsdMinor, "USD", locale);
}

export function formatUsdCnyMoney(
  amountUsdMinor: number,
  locale: string,
  amountCnyMinor: number,
) {
  // Invoice CNY amounts are frozen when issued, so never recompute this
  // supplied amount using the current quote or hide its USD counterpart.
  return `${formatCurrencyMinor(amountUsdMinor, "USD", locale)} / ${formatCurrencyMinor(amountCnyMinor, "CNY", locale)}`;
}
