/** Chain-explorer helpers for wallet transfer references. */

const TRON_TX = /^[a-fA-F0-9]{64}$/;

/** TRON transaction URL, or null when `hash` is not a TRON transaction id. */
export function tronTransactionUrl(hash: string | null | undefined) {
  const value = hash?.trim() ?? "";
  return TRON_TX.test(value) ? `https://tronscan.org/#/transaction/${value.toLowerCase()}` : null;
}

/** "datetime-local" input value → RFC 3339 (UTC) string, or null if invalid. */
export function localDateTimeToIso(value: string) {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}
