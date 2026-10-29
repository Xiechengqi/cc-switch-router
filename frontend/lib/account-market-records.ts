/** Stable account-table order: work first, current service by expiry, history
 * newest first. Invalid/missing dates never poison Array.sort with NaN. */
export type AccountRecordOrder = { id: string; attention?: boolean; ended?: boolean; expiresAt?: string | null; updatedAt?: string | null };
const date = (raw: string | null | undefined, fallback: number) => {
  const value = raw ? Date.parse(raw) : NaN;
  return Number.isFinite(value) ? value : fallback;
};
export function compareAccountRecords(left: AccountRecordOrder, right: AccountRecordOrder): number {
  return Number(!!right.attention) - Number(!!left.attention)
    || Number(!!left.ended) - Number(!!right.ended)
    || (left.ended && right.ended
      ? date(right.updatedAt, 0) - date(left.updatedAt, 0)
      : date(left.expiresAt, Number.MAX_SAFE_INTEGER) - date(right.expiresAt, Number.MAX_SAFE_INTEGER))
    || left.id.localeCompare(right.id);
}
