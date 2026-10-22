import { authFetch } from "@/lib/auth";
import { ApiError, parseJson } from "@/lib/api";
import type { MarketWalletPage } from "@/lib/market-wallet-api";

export type EarningConversionQuote = { id: string; amountUnits: string; termsVersion: number; expiresAt: string };
export type EarningWalletConversion = {
  id: string; quoteId: string; amountUnits: string; termsVersion: number; journalId: string; createdAt: string;
};
export type PendingEarningConversion = { actor: string; key: string; quote: EarningConversionQuote };
export const EARNING_CONVERSION_PENDING_PREFIX = "router_earning_conversion_v1:";
const ENDPOINT = "/v1/market-provider-finance/wallet-conversions";

function validQuote(value: EarningConversionQuote) {
  if (!value || typeof value.id !== "string" || !value.id || value.id.length > 128
    || typeof value.amountUnits !== "string" || !/^[1-9]\d{0,18}$/.test(value.amountUnits)
    || BigInt(value.amountUnits) > 9_223_372_036_854_775_807n || value.termsVersion !== 1
    || typeof value.expiresAt !== "string" || !Number.isFinite(Date.parse(value.expiresAt))) {
    throw new Error("Invalid earning conversion preview");
  }
  return value;
}
export function readPendingEarningConversion(storage: Pick<Storage, "getItem">, actor: string): PendingEarningConversion | null {
  const raw = storage.getItem(EARNING_CONVERSION_PENDING_PREFIX + actor);
  if (!raw) return null;
  const value = JSON.parse(raw) as PendingEarningConversion;
  if (!value || value.actor !== actor || typeof value.key !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(value.key)) {
    throw new Error("Invalid pending earning conversion");
  }
  validQuote(value.quote);
  return value;
}
export function writePendingEarningConversion(storage: Pick<Storage, "setItem" | "getItem">, value: PendingEarningConversion) {
  validQuote(value.quote);
  storage.setItem(EARNING_CONVERSION_PENDING_PREFIX + value.actor, JSON.stringify(value));
  if (readPendingEarningConversion(storage, value.actor)?.key !== value.key) throw new Error("Conversion recovery storage unavailable");
}
export async function quoteEarningConversion(amountMinor: number | null, all: boolean, batch = false) {
  return validQuote(await parseJson<EarningConversionQuote>(await authFetch(
    "/v1/market-provider-finance/wallet-conversion-quotes", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify(batch ? { batch: true } : all ? { all: true } : { amountMinor }),
    },
  )));
}
export async function confirmEarningConversion(value: PendingEarningConversion) {
  return parseJson<EarningWalletConversion>(await authFetch(ENDPOINT, {
    method: "POST", headers: { "Content-Type": "application/json", "Idempotency-Key": value.key },
    body: JSON.stringify({ quoteId: value.quote.id, termsVersion: value.quote.termsVersion, acknowledgeNonWithdrawable: true }),
  }));
}
export async function getEarningConversions(cursor?: string, quoteId?: string) {
  const query = new URLSearchParams();
  if (cursor) query.set("cursor", cursor);
  if (quoteId) query.set("quoteId", quoteId);
  return parseJson<MarketWalletPage<EarningWalletConversion>>(await authFetch(`${ENDPOINT}?${query}`, { cache: "no-store" }));
}
export function earningConversionResultMatches(value: EarningWalletConversion, pending: PendingEarningConversion) {
  return !!value?.id && !!value.journalId && value.quoteId === pending.quote.id
    && value.termsVersion === pending.quote.termsVersion && value.amountUnits === pending.quote.amountUnits;
}

/** Only an explicit business rejection proves this request did not commit.
 * Proxy timeouts (including HTTP 408), expired sessions and unknown failures
 * retain the original key and quote for lookup or idempotent retry. */
export function earningConversionDefinitelyRejected(cause: unknown) {
  if (cause instanceof ApiError && cause.status === 404 && cause.code === "EARNING_CONVERSION_QUOTE_NOT_FOUND") return true;
  return cause instanceof ApiError && cause.status === 409 && [
    "EARNING_CONVERSION_QUOTE_EXPIRED", "EARNING_CONVERSION_BATCH_LIMIT",
    "EARNING_CONVERSION_INSUFFICIENT", "EARNING_CONVERSION_TERMS_REQUIRED",
    "EARNING_CONVERSION_UNAVAILABLE", "EARNING_CONVERSION_IDEMPOTENCY_CONFLICT",
    "MARKET_FINANCE_HOLD",
  ].includes(cause.code ?? "");
}

const CHANGE_EVENT = "router-market-balance-changed";
const CHANGE_KEY = "router_market_balance_revision_v1";
/** No amounts or credentials in the cross-tab invalidation signal. */
export function notifyMarketBalanceChanged(actor: string) {
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT, { detail: actor }));
  try { localStorage.setItem(CHANGE_KEY, JSON.stringify({ actor, nonce: crypto.randomUUID() })); } catch { /* Same-tab refresh still works. */ }
}
export function subscribeMarketBalanceChanged(actor: string, refresh: () => void) {
  const onLocal = (event: Event) => { if ((event as CustomEvent).detail === actor) refresh(); };
  const onStorage = (event: StorageEvent) => {
    if (event.key !== CHANGE_KEY || !event.newValue) return;
    try { if (JSON.parse(event.newValue).actor === actor) refresh(); } catch { /* Ignore malformed browser storage. */ }
  };
  window.addEventListener(CHANGE_EVENT, onLocal);
  window.addEventListener("storage", onStorage);
  return () => { window.removeEventListener(CHANGE_EVENT, onLocal); window.removeEventListener("storage", onStorage); };
}
