export const MARKET_CURRENCY = "USD" as const;
export const USD_CNY_RATE = 7;

function formatCurrencyMinor(valueMinor: number, currency: "USD" | "CNY", locale: string) {
  return new Intl.NumberFormat(locale, { style: "currency", currency }).format(valueMinor / 100);
}

export function usdMinorToCnyMinor(amountUsdMinor: number) {
  return amountUsdMinor * USD_CNY_RATE;
}

export function formatUsdMoney(amountUsdMinor: number, locale: string) {
  return formatCurrencyMinor(amountUsdMinor, "USD", locale);
}

export function formatUsdCnyMoney(
  amountUsdMinor: number,
  locale: string,
  amountCnyMinor = usdMinorToCnyMinor(amountUsdMinor),
) {
  return `${formatUsdMoney(amountUsdMinor, locale)} / ${formatCurrencyMinor(amountCnyMinor, "CNY", locale)}`;
}
