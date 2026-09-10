/**
 * Formatting for micro-USD amounts that arrive as decimal strings.
 *
 * The backend accumulates in i128 micro-USD (docs §7.2) and serialises as a
 * string precisely so the value never passes through a float. This module keeps
 * that promise on the client: parsing goes through BigInt, and `Number` is never
 * used for arithmetic on an amount. A cache-read line on a large window can
 * exceed 2^53 micro-USD, and silently losing the low digits there would make
 * the displayed total disagree with the sum of its own lines.
 */

const MICROS_PER_USD = 1_000_000n;
const MICROS_PATTERN = /^-?\d+$/;

/** Parses a micro-USD wire string. Returns `null` for anything malformed. */
export function parseUsdMicros(value: string | null | undefined): bigint | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (!trimmed || !MICROS_PATTERN.test(trimmed)) return null;
  try {
    return BigInt(trimmed);
  } catch {
    return null;
  }
}

function splitMicros(micros: bigint) {
  const negative = micros < 0n;
  const magnitude = negative ? -micros : micros;
  return {
    negative,
    whole: magnitude / MICROS_PER_USD,
    fraction: magnitude % MICROS_PER_USD,
  };
}

/**
 * Formats micro-USD as a currency amount.
 *
 * `maximumFractionDigits` defaults to 2, but small amounts keep enough digits to
 * stay distinguishable from zero: rendering a real $0.0003 line as "$0.00" would
 * read as "this cost nothing", which is exactly the wrong takeaway on a surface
 * whose whole point is cost visibility.
 */
export function formatUsdMicros(
  value: string | bigint | null | undefined,
  locale?: string,
  options: { maximumFractionDigits?: number } = {},
): string | null {
  const micros = typeof value === "bigint" ? value : parseUsdMicros(value);
  if (micros == null) return null;
  const { negative, whole, fraction } = splitMicros(micros);

  let digits = options.maximumFractionDigits ?? 2;
  if (options.maximumFractionDigits == null && whole === 0n && fraction !== 0n) {
    // Widen only as far as the first significant digit, up to the full
    // micro-USD precision the wire actually carries.
    const padded = String(fraction).padStart(6, "0");
    const firstSignificant = padded.search(/[1-9]/);
    digits = Math.min(6, Math.max(2, firstSignificant + 1));
  }

  const scale = 10n ** BigInt(digits);
  // Round half away from zero on the display value only. The underlying amount
  // is never mutated, so this cannot drift into a stored number.
  const scaled = (fraction * scale + MICROS_PER_USD / 2n) / MICROS_PER_USD;
  const carry = scaled / scale;
  const remainder = scaled % scale;
  const wholeWithCarry = whole + carry;

  const wholeText = new Intl.NumberFormat(locale, {
    maximumFractionDigits: 0,
    useGrouping: true,
  }).format(wholeWithCarry);
  const decimalSeparator =
    new Intl.NumberFormat(locale, { minimumFractionDigits: 1 })
      .formatToParts(1.1)
      .find((part) => part.type === "decimal")?.value || ".";
  const fractionText = digits > 0 ? String(remainder).padStart(digits, "0") : "";
  const body = fractionText ? `${wholeText}${decimalSeparator}${fractionText}` : wholeText;
  return `${negative ? "-" : ""}$${body}`;
}

/**
 * Formats the §7.5 estimate interval.
 *
 * Collapses to a single value when the bounds are equal — which is the common
 * case (no cache writes, or a model with no separate 1h price). Showing
 * "$1.20 ~ $1.20" would imply an uncertainty that is not there.
 */
export function formatUsdMicrosRange(
  lower: string | null | undefined,
  upper: string | null | undefined,
  locale?: string,
): string | null {
  const low = parseUsdMicros(lower);
  if (low == null) return null;
  const high = parseUsdMicros(upper);
  const lowText = formatUsdMicros(low, locale);
  if (lowText == null) return null;
  if (high == null || high <= low) return lowText;
  const highText = formatUsdMicros(high, locale);
  return highText == null ? lowText : `${lowText} ~ ${highText}`;
}

/**
 * Formats a per-1M-token rate. Rates are plain integers on the wire (they fit
 * comfortably in a JS number) but they share the micro-USD unit, so they share
 * the formatter to keep rounding identical between a rate and an amount.
 */
export function formatUsdMicrosPerMillion(
  ratePerMillion: number | string | null | undefined,
  locale?: string,
): string | null {
  if (ratePerMillion == null) return null;
  const text =
    typeof ratePerMillion === "number"
      ? Number.isFinite(ratePerMillion)
        ? String(Math.trunc(ratePerMillion))
        : null
      : ratePerMillion;
  if (text == null) return null;
  return formatUsdMicros(text, locale);
}

/** Percentage with one decimal, for the coverage/estimated footers. */
export function formatPercent(value: number, locale?: string): string {
  if (!Number.isFinite(value)) return "-";
  return new Intl.NumberFormat(locale, {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  }).format(value);
}
