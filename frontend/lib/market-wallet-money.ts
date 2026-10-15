const UNITS_PER_DOLLAR = 8_640_000n;

export function formatWalletUnits(raw: string, currencySymbol = "$", locale = "en-US") {
  const units = BigInt(raw);
  const negative = units < 0n;
  const absolute = negative ? -units : units;
  const whole = absolute / UNITS_PER_DOLLAR;
  const remainder = absolute % UNITS_PER_DOLLAR;
  const millionths = remainder * 1_000_000n / UNITS_PER_DOLLAR;
  const fraction = millionths.toString().padStart(6, "0").replace(/0+$/, "");
  const decimals = fraction.length < 2 ? fraction.padEnd(2, "0") : fraction;
  if (whole === 0n && remainder > 0n && millionths === 0n) {
    return `${negative ? "−" : ""}<${currencySymbol}0.000001`;
  }
  return `${negative ? "−" : ""}${currencySymbol}${whole.toLocaleString(locale)}.${decimals}`;
}

export function parseUsdMinor(raw: string): number | null {
  const value = raw.trim();
  if (!/^(?:0|[1-9]\d{0,3})(?:\.\d{1,2})?$/.test(value)) return null;
  const [dollars, fraction = ""] = value.split(".");
  const minor = Number(dollars) * 100 + Number(fraction.padEnd(2, "0"));
  return minor >= 100 && minor <= 100_000 ? minor : null;
}

export function safeCheckoutUrl(raw: string | null) {
  if (!raw) return null;
  try {
    const url = new URL(raw);
    if (url.origin !== "https://www.dujiaopay.com" || url.username || url.password) return null;
    return url.href;
  } catch {
    return null;
  }
}
