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
