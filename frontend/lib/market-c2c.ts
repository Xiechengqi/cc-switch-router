import { authFetch } from "@/lib/auth";
import { ApiError, parseJson } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";

export const C2C_TERMS_VERSION = 3;
export type C2CPolicy = { version: number; enabled: boolean; admissionMinor: number; minMinor: number; maxMinor: number; paymentMinutes: number };
export const C2C_BLOCKERS = ["C2C_DISABLED", "C2C_TRANSFERS_DISABLED", "C2C_TRANSFERS_PAUSED", "C2C_ACCOUNT_INACTIVE", "C2C_IDENTITY", "C2C_FINANCE_HOLD", "C2C_SUSPENDED", "C2C_ADMISSION", "C2C_OVERDUE_ORDERS"] as const;
export type C2CBlocker = (typeof C2C_BLOCKERS)[number];
export type C2CPolicyView = { policy: C2CPolicy; available: boolean; blockers: C2CBlocker[]; canManagePolicy: boolean; verified: boolean; termsVersion: number; claimGraceSeconds: number; firstPurchaseMaxMinor: number; independentDisputeHandlerAvailable: boolean };
export type C2CSeller = {
  userId: string; version: number; enabled: boolean; suspended: boolean; displayName: string; description: string;
  currency: "USD" | "CNY"; priceBps: number; minMinor: number; maxMinor: number;
  autoPauseReason: "acceptance_timeout" | "paid_order_overdue" | null; autoPausedAt: string | null; autoPauseOrderId: string | null;
  autoAcceptTermsVersion: 0 | 3; autoAcceptConsentedAt: string | null;
};
export type C2CSellerInput = Omit<C2CSeller, "userId" | "version" | "suspended" | "displayName" | "autoPauseReason" | "autoPausedAt" | "autoPauseOrderId" | "autoAcceptTermsVersion" | "autoAcceptConsentedAt"> & { expectedVersion: number; acknowledgeExternalPayment: boolean; autoAcceptTermsVersion: 3 };
export const C2C_SELLER_STATES = ["not_selling", "selling", "paused", "confirmation_required", "auto_paused", "suspended", "insufficient_stock", "unavailable", "order_limit"] as const;
export type C2CSellerSnapshot = { seller: C2CSeller | null; admissionBalanceUnits: string; availableUnits: string; blockers: C2CBlocker[]; canEnable: boolean; state: (typeof C2C_SELLER_STATES)[number]; pendingOrders: number };
export type C2COffer = Pick<C2CSeller, "version" | "displayName" | "description" | "currency" | "priceBps" | "minMinor" | "maxMinor"> & {
  sellerId: string; availableMinor: number; completedOrders: number;
};
export type C2CQuote = { id: string; sellerId: string; sellerName: string; sellerVersion: number; creditMinor: number; payMinor: number; currency: "USD" | "CNY"; fxMicros: number; priceBps: number; description: string; expiresAt: string; flowVersion: 1 | 2 | 3; acceptanceMinutes?: number; paymentMinutes?: number };
export const C2C_STATUSES = ["awaiting_seller", "awaiting_payment", "paid", "disputed", "released", "cancelled", "expired"] as const;
export type C2CStatus = (typeof C2C_STATUSES)[number];
export type C2COrder = {
  id: string; quoteId: string; buyerId: string; sellerId: string; amountUnits: string; status: C2CStatus; version: number;
  paymentDeadline: string | null; flowVersion: 1 | 2 | 3; acceptanceDeadline: string | null; acceptedAt: string | null; paidAt: string | null; releasedAt: string | null; latePaymentReported: boolean; latePaymentResolved: boolean;
  createdAt: string; updatedAt: string; sellerName: string; payMinor: number; currency: "USD" | "CNY"; fxMicros: number; priceBps: number;
  description: string | null;
  legacyPayment: { contact: string; paymentMethod: string; paymentDetails: string } | null;
};
export type C2CEvent = { id: string; actorId: string; action: string; note: string; createdAt: string };
export type C2CPage<T> = { items: T[]; next: string | null };
export type C2CPending = { actor: string; key: string; quote: C2CQuote };
export const C2C_PENDING_PREFIX = "router_c2c_order_v1:";
const BASE = "/v1/market-wallet/c2c";
const ADMIN = "/v1/admin/market-finance/c2c";
// Anchor countdowns to Router time, then advance with the monotonic clock.
// Changing the computer's wall clock must not reopen a payment window.
export class C2CClock {
  private anchor: { server: number; monotonic: number } | null = null;
  update(header: string | null, sent: number, received: number) {
    const parsed = header ? Date.parse(header) : NaN;
    if (!Number.isFinite(parsed) || received < sent) return;
    const estimate = parsed + Math.min((received - sent) / 2, 2000);
    this.anchor = { server: this.anchor ? Math.max(estimate, this.now(received)) : estimate, monotonic: received };
  }
  now(monotonic = performance.now(), fallback = Date.now()) {
    return this.anchor ? this.anchor.server + Math.max(0, monotonic - this.anchor.monotonic) : fallback;
  }
}
const clock = new C2CClock();
export const c2cNow = () => clock.now();
// C2C policy limits are independent from the official gateway's $1–$1000
// top-up amount parser. The owner may allow $0.01–$100,000 C2C orders.
export function parseC2CMinor(raw: string): number | null {
  const value = parseNonNegativeUsdMinor(raw);
  return value !== null && value >= 1 && value <= 10_000_000 ? value : null;
}
const id = (v: unknown): v is string => typeof v === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(v);
const integer = (v: unknown): v is number => Number.isSafeInteger(v) && Number(v) > 0;
const date = (v: unknown) => typeof v === "string" && Number.isFinite(Date.parse(v));
const currency = (v: unknown) => v === "USD" || v === "CNY";
const nonnegative = (v: unknown): v is number => Number.isSafeInteger(v) && Number(v) >= 0;
const boundedText = (v: unknown, max: number, empty = false): v is string => typeof v === "string" && (empty || !!v.trim()) && [...v].length <= max;
export const C2C_DESCRIPTION_MAX = 300;
export function validC2CDescription(value: unknown, empty = false): value is string {
  return boundedText(value, C2C_DESCRIPTION_MAX, empty) && !/[\u0000-\u001f\u007f-\u009f]/u.test(value);
}
export function c2cDescriptionParts(value: string): { text: string; href?: string }[] {
  const parts: { text: string; href?: string }[] = [];
  let from = 0;
  for (const match of value.matchAll(/https?:\/\/[^\s<>]+/giu)) {
    const text = match[0].replace(/[.,;:!?，。；：！？)\]）]+$/u, "");
    try {
      const url = new URL(text);
      if (!url.hostname || url.username || url.password || !["https:", "http:"].includes(url.protocol)) continue;
      parts.push({ text: value.slice(from, match.index) }, { text, href: url.href });
      from = match.index + text.length;
    } catch { /* Keep malformed URLs as inert text. */ }
  }
  parts.push({ text: value.slice(from) });
  return parts;
}
function validUnits(v: unknown): v is string {
  return typeof v === "string" && /^(0|[1-9]\d{0,18})$/.test(v) && BigInt(v) <= 9_223_372_036_854_775_807n;
}
function invalid(): never { throw new Error("Invalid C2C response; reload to retry"); }
export function validC2CCapability(blockers: C2CBlocker[], available: boolean) {
  if (!Array.isArray(blockers) || blockers.some(code => !C2C_BLOCKERS.includes(code))
    || new Set(blockers).size !== blockers.length || typeof available !== "boolean" || available !== !blockers.length) invalid();
}
function validTerms(creditMinor: number, payMinor: number, code: string, fx: number, bps: number) {
  if (!integer(creditMinor) || creditMinor > 10_000_000 || !integer(payMinor) || !currency(code) || !integer(fx)
    || (code === "USD" && fx !== 1_000_000) || !integer(bps) || bps < 5000 || bps > 20000) invalid();
  if ((BigInt(creditMinor) * BigInt(fx) * BigInt(bps) + 5_000_000_000n) / 10_000_000_000n !== BigInt(payMinor)) invalid();
}
export function validC2CPolicy(p: C2CPolicy): C2CPolicy {
  if (!p || !integer(p.version) || typeof p.enabled !== "boolean" || !integer(p.admissionMinor) || !integer(p.minMinor)
    || p.admissionMinor < 100 || p.admissionMinor > 10_000_000 || !integer(p.maxMinor) || p.maxMinor > 10_000_000
    || p.maxMinor < p.minMinor || !integer(p.paymentMinutes) || p.paymentMinutes < 5 || p.paymentMinutes > 60) invalid();
  return p;
}
export function validC2CQuote(q: C2CQuote): C2CQuote {
  if (!q || !id(q.id) || !id(q.sellerId) || !integer(q.sellerVersion) || !boundedText(q.sellerName, 60)
    || !validC2CDescription(q.description) || !date(q.expiresAt) || ![1, 2, 3].includes(q.flowVersion)
    || (q.flowVersion === 3 ? !integer(q.paymentMinutes) || q.paymentMinutes < 5 || q.paymentMinutes > 60
      : !integer(q.acceptanceMinutes) || q.acceptanceMinutes < 5 || q.acceptanceMinutes > 30)) invalid();
  validTerms(q.creditMinor, q.payMinor, q.currency, q.fxMicros, q.priceBps);
  return q;
}
export function validC2COrder(o: C2COrder): C2COrder {
  if (!o || !id(o.id) || !id(o.quoteId) || !id(o.buyerId) || !id(o.sellerId) || o.buyerId === o.sellerId || !integer(o.version)
    || !C2C_STATUSES.includes(o.status) || typeof o.amountUnits !== "string" || !/^[1-9]\d{0,18}$/.test(o.amountUnits)
    || BigInt(o.amountUnits) > 9_223_372_036_854_775_807n || BigInt(o.amountUnits) % 86_400n !== 0n
    || ![1, 2, 3].includes(o.flowVersion) || !date(o.createdAt) || !date(o.updatedAt) || !boundedText(o.sellerName, 60)
    || (o.flowVersion !== 2 ? (!date(o.paymentDeadline) || o.acceptanceDeadline !== null || o.acceptedAt !== null || o.status === "awaiting_seller")
      : (!date(o.acceptanceDeadline) || (o.acceptedAt === null
        ? o.paymentDeadline !== null || !["awaiting_seller", "disputed", "released", "cancelled", "expired"].includes(o.status)
        : !date(o.acceptedAt) || !date(o.paymentDeadline) || o.status === "awaiting_seller" || Date.parse(o.acceptedAt) >= Date.parse(o.acceptanceDeadline!) || Date.parse(o.paymentDeadline!) <= Date.parse(o.acceptedAt))))
    || typeof o.latePaymentReported !== "boolean" || typeof o.latePaymentResolved !== "boolean"
    || (o.latePaymentResolved && !o.latePaymentReported)
    || (o.description !== null && !validC2CDescription(o.description))
    || (o.legacyPayment !== null && (!o.legacyPayment || o.description !== null || !boundedText(o.legacyPayment.contact, 300)
      || !boundedText(o.legacyPayment.paymentMethod, 60) || !boundedText(o.legacyPayment.paymentDetails, 1500)))
    || (o.paidAt !== null && !date(o.paidAt)) || (["awaiting_seller", "awaiting_payment"].includes(o.status) && o.paidAt !== null)
    || (["paid", "disputed", "released"].includes(o.status) && !date(o.paidAt))
    || (o.status === "released" ? !date(o.releasedAt) : o.releasedAt !== null)) invalid();
  validTerms(Number(BigInt(o.amountUnits) / 86_400n), o.payMinor, o.currency, o.fxMicros, o.priceBps);
  return o;
}
export function validC2CSeller(s: C2CSeller): C2CSeller {
  if (!s || !id(s.userId) || !integer(s.version) || typeof s.enabled !== "boolean" || typeof s.suspended !== "boolean"
    || !boundedText(s.displayName, 60) || !validC2CDescription(s.description, !s.enabled)
    || !currency(s.currency) || !integer(s.priceBps) || s.priceBps < 5000 || s.priceBps > 20000 || !integer(s.minMinor)
    || !integer(s.maxMinor) || s.maxMinor < s.minMinor || s.maxMinor > 10_000_000
    || ![0, C2C_TERMS_VERSION].includes(s.autoAcceptTermsVersion)
    || (s.autoAcceptTermsVersion === 0 ? s.autoAcceptConsentedAt !== null : !date(s.autoAcceptConsentedAt))
    || (s.autoPauseReason === null ? s.autoPausedAt !== null || s.autoPauseOrderId !== null
      : !["acceptance_timeout", "paid_order_overdue"].includes(s.autoPauseReason) || !date(s.autoPausedAt) || !id(s.autoPauseOrderId))) invalid();
  return s;
}
export function validC2CEvent(e: C2CEvent): C2CEvent {
  if (!e || !id(e.id) || !id(e.actorId) || !id(e.action) || !boundedText(e.note, 1500, true) || !date(e.createdAt)) invalid();
  return e;
}
function validPage<T>(p: C2CPage<T>, check?: (item: T) => T): C2CPage<T> {
  if (!p || !Array.isArray(p.items) || (p.next !== null && !id(p.next))) invalid();
  if (check) p.items.forEach(check);
  return p;
}
async function request<T>(path: string, body?: unknown, method = "POST", key?: string): Promise<T> {
  const sent = performance.now();
  const response = await authFetch(path, body === undefined ? { cache: "no-store" } : {
    method, cache: "no-store", headers: { "Content-Type": "application/json", ...(key ? { "Idempotency-Key": key } : {}) }, body: JSON.stringify(body),
  });
  clock.update(response.headers.get("x-router-server-time"), sent, performance.now());
  return parseJson<T>(response);
}
export async function getC2CPolicy() {
  const p = await request<C2CPolicyView>(`${BASE}/policy`); validC2CPolicy(p?.policy);
  if (typeof p.available !== "boolean" || typeof p.verified !== "boolean" || p.termsVersion !== C2C_TERMS_VERSION || p.claimGraceSeconds !== 120 || p.firstPurchaseMaxMinor !== 10000 || typeof p.independentDisputeHandlerAvailable !== "boolean") invalid();
  validC2CCapability(p.blockers, p.available);
  if (typeof p.canManagePolicy !== "boolean") invalid();
  return p;
}
export async function getC2CSeller() {
  const p = await request<C2CSellerSnapshot>(`${BASE}/seller`);
  if (!p || !validUnits(p.admissionBalanceUnits) || !validUnits(p.availableUnits) || !C2C_SELLER_STATES.includes(p.state) || !nonnegative(p.pendingOrders)) invalid();
  validC2CCapability(p.blockers, p.canEnable);
  if (p.seller !== null) validC2CSeller(p.seller);
  return p;
}
export const saveC2CSeller = async (s: C2CSellerInput) => validC2CSeller(await request<C2CSeller>(`${BASE}/seller`, s, "PUT"));
export const pauseC2CSeller = async (s: C2CSeller) => validC2CSeller(await request<C2CSeller>(`${BASE}/seller/pause`, { expectedVersion: s.version }));
export const resumeC2CSeller = async (s: C2CSeller) => validC2CSeller(await request<C2CSeller>(`${BASE}/seller/resume`, { expectedVersion: s.version }));
export async function getC2COffers(before = "") {
  const p = await request<C2CPage<C2COffer> & { emptyReason: "platform_paused" | "no_matching_sellers" | "no_other_sellers" | null }>(`${BASE}/sellers?before=${encodeURIComponent(before)}`);
  validPage(p);
  if (p.emptyReason !== null && !["platform_paused", "no_matching_sellers", "no_other_sellers"].includes(p.emptyReason)) invalid();
  p.items.forEach(o => { if (!id(o.sellerId) || !integer(o.version) || !integer(o.availableMinor) || !integer(o.minMinor) || !integer(o.maxMinor)
    || o.maxMinor < o.minMinor || o.maxMinor > o.availableMinor || o.maxMinor > 10_000_000 || !currency(o.currency)
    || !boundedText(o.displayName, 60) || !validC2CDescription(o.description) || !integer(o.priceBps) || o.priceBps < 5000 || o.priceBps > 20000
    || !nonnegative(o.completedOrders)) invalid(); });
  return p;
}
export const quoteC2C = async (sellerId: string, amountMinor: number) => { const q = validC2CQuote(await request<C2CQuote>(`${BASE}/quotes`, { sellerId, amountMinor })); if (q.flowVersion !== C2C_TERMS_VERSION || q.sellerId !== sellerId || q.creditMinor !== amountMinor) invalid(); return q; };
export async function createC2C(p: C2CPending) {
  return matchC2CPendingOrder(p, await request<C2COrder>(`${BASE}/orders`, { quoteId: p.quote.id, termsVersion: p.quote.flowVersion, acknowledgeExternalPayment: true }, "POST", p.key));
}
export function matchC2CPendingOrder(p: C2CPending, order: C2COrder) {
  const result = validC2COrder(order);
  if (result.buyerId !== p.actor || result.quoteId !== p.quote.id || result.sellerId !== p.quote.sellerId || BigInt(result.amountUnits) !== BigInt(p.quote.creditMinor) * 86_400n
    || result.payMinor !== p.quote.payMinor || result.currency !== p.quote.currency || result.fxMicros !== p.quote.fxMicros || result.priceBps !== p.quote.priceBps
    || result.description !== p.quote.description || result.sellerName !== p.quote.sellerName || result.flowVersion !== p.quote.flowVersion
    || (p.quote.flowVersion === 3 && Date.parse(result.paymentDeadline!) - Date.parse(result.createdAt) !== p.quote.paymentMinutes! * 60_000)) invalid();
  return result;
}
export function matchC2COrder(before: C2COrder, after: C2COrder): C2COrder {
  validC2COrder(after);
  if (after.id !== before.id || after.quoteId !== before.quoteId || after.buyerId !== before.buyerId || after.sellerId !== before.sellerId
    || after.amountUnits !== before.amountUnits || after.payMinor !== before.payMinor || after.currency !== before.currency
    || after.fxMicros !== before.fxMicros || after.priceBps !== before.priceBps || after.version < before.version || after.flowVersion !== before.flowVersion
    || after.acceptanceDeadline !== before.acceptanceDeadline || (before.acceptedAt !== null && after.acceptedAt !== before.acceptedAt)
    || (before.paymentDeadline !== null && after.paymentDeadline !== before.paymentDeadline)
    || (before.description !== null && after.description !== before.description)) invalid();
  return after;
}
export async function getC2COrders(filter: { before?: string; status?: string; role?: string; quoteId?: string } = {}, admin = false) {
  return validPage(await request<C2CPage<C2COrder>>(`${admin ? ADMIN : BASE}/orders?${new URLSearchParams(filter)}`), validC2COrder);
}
export const getC2COrder = async (orderId: string) => { const o = validC2COrder(await request<C2COrder>(`${BASE}/orders/${encodeURIComponent(orderId)}`)); if (o.id !== orderId) invalid(); return o; };
export const getC2CEvents = async (orderId: string, before = "", admin = false) => validPage(await request<C2CPage<C2CEvent>>(`${admin ? ADMIN : BASE}/orders/${encodeURIComponent(orderId)}/events?before=${encodeURIComponent(before)}`), validC2CEvent);
export const actC2C = async (o: C2COrder, action: "accept" | "reject" | "paid" | "cancel" | "dispute" | "late-payment" | "notes", note = "") => matchC2COrder(o, await request<C2COrder>(`${BASE}/orders/${o.id}/${action}`, { expectedVersion: o.version, note, ...(action === "cancel" ? { acknowledgeUnpaid: true } : {}) }));
export const releaseC2C = async (o: C2COrder) => matchC2COrder(o, await request<C2COrder>(`${BASE}/orders/${o.id}/release`, { expectedVersion: o.version, acknowledgeReceived: true }));
export const getAdminC2CPolicy = async () => { const p = await request<{ policy: C2CPolicy; canManage: boolean }>(`${ADMIN}/policy`); validC2CPolicy(p?.policy); if (typeof p.canManage !== "boolean") invalid(); return p; };
export const saveAdminC2CPolicy = async (p: C2CPolicy) => validC2CPolicy(await request<C2CPolicy>(`${ADMIN}/policy`, p, "PUT"));
export const getAdminC2COrder = async (orderId: string) => { const p = await request<{ order: C2COrder; events: C2CPage<C2CEvent>; evidenceValid: boolean; canResolve: boolean }>(`${ADMIN}/orders/${orderId}`); validC2COrder(p?.order); validPage(p.events, validC2CEvent); if (p.order.id !== orderId || typeof p.evidenceValid !== "boolean" || typeof p.canResolve !== "boolean") invalid(); return p; };
export const resolveC2C = async (o: C2COrder, action: string, evidence: string) => matchC2COrder(o, await request<C2COrder>(`${ADMIN}/orders/${o.id}/resolve`, { expectedVersion: o.version, action, evidence, attestExternalEvidence: true }));
export type C2CAdminSeller = Pick<C2CSeller, "userId" | "version" | "displayName" | "enabled" | "suspended" | "autoPauseReason">;
export const getAdminC2CSellers = async (before = "") => validPage(await request<C2CPage<C2CAdminSeller>>(`${ADMIN}/sellers?before=${encodeURIComponent(before)}`), s => {
  if (!s || !id(s.userId) || !integer(s.version) || !boundedText(s.displayName, 60) || typeof s.enabled !== "boolean" || typeof s.suspended !== "boolean" || (s.autoPauseReason !== null && !["acceptance_timeout", "paid_order_overdue"].includes(s.autoPauseReason))) invalid(); return s;
});
export const suspendC2CSeller = (s: C2CAdminSeller, reason: string) => request(`${ADMIN}/sellers/${s.userId}/suspension`, { expectedVersion: s.version, suspended: !s.suspended, reason });
export type C2CDisputeHandler = { userId: string; email: string; version: number; enabled: boolean; isAdmin: boolean; updatedAt: string };
export const getC2CDisputeHandlers = async (before = "") => validPage(await request<C2CPage<C2CDisputeHandler>>(`${ADMIN}/dispute-handlers?before=${encodeURIComponent(before)}`), h => {
  if (!h || !id(h.userId) || !boundedText(h.email, 320) || !integer(h.version) || typeof h.enabled !== "boolean" || typeof h.isAdmin !== "boolean" || !date(h.updatedAt)) invalid(); return h;
});
export const saveC2CDisputeHandler = (email: string, expectedVersion: number, enabled: boolean) => request(`${ADMIN}/dispute-handlers`, { email, expectedVersion, enabled }, "PUT");

export function readC2CPending(storage: Pick<Storage, "getItem">, actor: string): C2CPending | null {
  const raw = storage.getItem(C2C_PENDING_PREFIX + actor); if (!raw) return null;
  const p = JSON.parse(raw) as C2CPending;
  // Old submissions can recover their matching order, never create a new one.
  if (p?.quote && !("flowVersion" in p.quote)) Object.assign(p.quote, { flowVersion: 1, acceptanceMinutes: 10 });
  if (!p || p.actor !== actor || !id(p.key) || p.key.length < 8) invalid(); validC2CQuote(p.quote); return p;
}
export function writeC2CPending(storage: Pick<Storage, "getItem" | "setItem">, p: C2CPending) {
  validC2CQuote(p.quote); storage.setItem(C2C_PENDING_PREFIX + p.actor, JSON.stringify(p));
  if (readC2CPending(storage, p.actor)?.key !== p.key) invalid();
}
export function c2cDefinitelyRejected(cause: unknown) {
  return cause instanceof ApiError && cause.status === 409 && [...C2C_BLOCKERS, "C2C_STALE", "C2C_STOCK", "C2C_PAUSED", "C2C_TERMS", "C2C_FIRST_ORDER_LIMIT", "C2C_ORDER_LIMIT", "C2C_SELLER_UNAVAILABLE", "MARKET_FINANCE_HOLD"].includes(cause.code ?? "");
}
export function c2cBlockerKey(code: C2CBlocker): MessageKey {
  const keys: Record<C2CBlocker, MessageKey> = {
    C2C_DISABLED: "c2c.error.disabled", C2C_TRANSFERS_DISABLED: "c2c.error.transfersDisabled", C2C_TRANSFERS_PAUSED: "c2c.error.transfersPaused",
    C2C_ACCOUNT_INACTIVE: "c2c.error.inactive", C2C_IDENTITY: "c2c.error.identity", C2C_FINANCE_HOLD: "c2c.error.hold",
    C2C_SUSPENDED: "c2c.error.suspended", C2C_ADMISSION: "c2c.error.admission", C2C_OVERDUE_ORDERS: "c2c.error.overdue",
  };
  return keys[code];
}
export function c2cErrorKey(cause: unknown): MessageKey {
  const code = cause instanceof ApiError ? cause.code : null;
  if (C2C_BLOCKERS.includes(code as C2CBlocker)) return c2cBlockerKey(code as C2CBlocker);
  if (cause instanceof ApiError && cause.status === 429) return "c2c.error.rate";
  const map: Record<string, MessageKey> = {
    C2C_STALE: "c2c.error.stale", C2C_STOCK: "c2c.error.stock", C2C_PAUSED: "c2c.error.paused", C2C_AUTO_PAUSED: "c2c.error.autoPaused", C2C_AUTO_ACCEPT_CONSENT: "c2c.confirmAutoAcceptHelp", C2C_TERMS: "c2c.error.stale", C2C_ADMISSION: "c2c.error.admission",
    C2C_SELLER_UNAVAILABLE: "c2c.error.offline", C2C_IDENTITY: "c2c.error.identity", C2C_SUSPENDED: "c2c.error.suspended",
    C2C_FX_UNAVAILABLE: "c2c.error.fx", C2C_PAYMENT_WINDOW: "c2c.error.window",
    C2C_FIRST_ORDER_LIMIT: "c2c.error.first", C2C_ORDER_LIMIT: "c2c.error.limit", MARKET_FINANCE_HOLD: "c2c.error.hold",
  };
  return map[code ?? ""] ?? "c2c.error.generic";
}
export function c2cPaymentWindow(o: C2COrder, now = c2cNow()) {
  const deadline = o.paymentDeadline ? Date.parse(o.paymentDeadline) : NaN;
  return { mayStartPayment: o.status === "awaiting_payment" && now < deadline, mayClaimPayment: o.status === "awaiting_payment" && now < deadline + 120_000 };
}
export function c2cPayLabel(minor: number, currency: string) { return `${currency === "CNY" ? "CN¥" : "US$"}${(minor / 100).toFixed(2)}`; }
