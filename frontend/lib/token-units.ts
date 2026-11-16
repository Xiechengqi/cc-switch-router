export const TOKENS_PER_MILLION = 1_000_000;
const TOKENS_PER_WAN = 10_000;
const TOKENS_PER_YI = 100_000_000;

const MILLION_INPUT_PATTERN = /^(?:\d+(?:\.\d{0,6})?|\.\d{1,6})$/;

function isChineseLocale(locale?: string) {
  return Boolean(locale && /^zh\b/i.test(locale));
}

function formatExactScaledTokens(value: number, scale: number, unit: string, locale?: string) {
  const whole = Math.floor(value / scale);
  const remainder = value % scale;
  const formattedWhole = new Intl.NumberFormat(locale, {
    maximumFractionDigits: 0,
  }).format(whole);
  if (!remainder) return `${formattedWhole}${unit}`;

  const decimalSeparator = new Intl.NumberFormat(locale, {
    minimumFractionDigits: 1,
  }).formatToParts(1.1).find((part) => part.type === "decimal")?.value || ".";
  const fraction = String(remainder).padStart(String(scale).length - 1, "0").replace(/0+$/, "");
  return `${formattedWhole}${decimalSeparator}${fraction}${unit}`;
}

export function tokensToMillionsInput(value: number | null | undefined) {
  if (!Number.isSafeInteger(value) || value == null || value < 0) return "";
  const whole = Math.floor(value / TOKENS_PER_MILLION);
  const remainder = value % TOKENS_PER_MILLION;
  if (!remainder) return String(whole);
  const fraction = String(remainder).padStart(6, "0").replace(/0+$/, "");
  return `${whole}.${fraction}`;
}

export function millionsInputToTokens(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed || !MILLION_INPUT_PATTERN.test(trimmed)) return null;
  const normalized = trimmed.startsWith(".") ? `0${trimmed}` : trimmed;
  const [wholePart, fractionPart = ""] = normalized.split(".");
  const whole = Number(wholePart);
  const fraction = Number(fractionPart.padEnd(6, "0") || "0");
  if (!Number.isSafeInteger(whole) || !Number.isSafeInteger(fraction)) return null;
  const tokens = whole * TOKENS_PER_MILLION + fraction;
  return Number.isSafeInteger(tokens) ? tokens : null;
}

export function validTokenMillionsInput(value: string, options: { allowZero?: boolean } = {}) {
  const tokens = millionsInputToTokens(value);
  return tokens != null && (options.allowZero ? tokens >= 0 : tokens > 0);
}

export function formatTokenMillions(value: number, locale?: string) {
  if (!Number.isSafeInteger(value) || value < 0) return "-";
  if (isChineseLocale(locale)) {
    return value >= TOKENS_PER_YI
      ? formatExactScaledTokens(value, TOKENS_PER_YI, "亿", locale)
      : formatExactScaledTokens(value, TOKENS_PER_WAN, "万", locale);
  }
  return formatExactScaledTokens(value, TOKENS_PER_MILLION, " M", locale);
}

export function formatTokenMillionsFixed(value: number, locale?: string, fractionDigits = 1) {
  if (!Number.isSafeInteger(value) || value < 0 || !Number.isInteger(fractionDigits) || fractionDigits < 0) {
    return "-";
  }
  const [scale, unit] = isChineseLocale(locale)
    ? value >= TOKENS_PER_YI
      ? [TOKENS_PER_YI, "亿"] as const
      : [TOKENS_PER_WAN, "万"] as const
    : [TOKENS_PER_MILLION, " M"] as const;
  const formatted = new Intl.NumberFormat(locale, {
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  }).format(value / scale);
  return `${formatted}${unit}`;
}

/**
 * A Token amount for reading, in the same unit the locale shows everywhere
 * else (万/亿 in Chinese, M otherwise) with at most two decimals and no
 * trailing zeros. Digits are cut, not rounded, so usage never reads as more
 * than it is (99,999,999 stays under 1亿) and a small non-zero amount reads
 * "<0.01", never 0. Use `formatTokenMillions` where the exact value matters,
 * such as a configured limit.
 */
export function formatTokenAmount(value: number, locale?: string) {
  if (!Number.isSafeInteger(value) || value < 0) return "-";
  const [scale, unit] = isChineseLocale(locale)
    ? value >= TOKENS_PER_YI
      ? [TOKENS_PER_YI, "亿"] as const
      : [TOKENS_PER_WAN, "万"] as const
    : [TOKENS_PER_MILLION, " M"] as const;
  const hundredths = Math.floor(value / (scale / 100));
  const format = new Intl.NumberFormat(locale, { minimumFractionDigits: 0, maximumFractionDigits: 2 });
  if (value > 0 && hundredths === 0) return `<${format.format(0.01)}${unit}`;
  return `${format.format(hundredths / 100)}${unit}`;
}
