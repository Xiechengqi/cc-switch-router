export const MARKET_CURRENCY = "USD" as const;
export const USD_CNY_RATE_SCALE = 1_000_000;
export const DEFAULT_USD_CNY_RATE_MICROS = 7 * USD_CNY_RATE_SCALE;

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

export function formatUsdCnyMoney(
  amountUsdMinor: number,
  locale: string,
  amountCnyMinor: number,
) {
  return `${formatUsdMoney(amountUsdMinor, locale)} / ${formatCurrencyMinor(amountCnyMinor, "CNY", locale)}`;
}
