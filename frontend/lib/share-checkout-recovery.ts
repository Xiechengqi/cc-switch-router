/** Session-only, login-scoped recovery. Never store credentials, quotes,
 * wallet balances, contacts, or subscription data here. */
export type ShareCheckoutDraft = { shareId: string; seatId: string; autoRenew: boolean };
export type SharePendingPurchase = ShareCheckoutDraft & { quoteId: string; idempotencyKey: string };
type Recovery = { scope: string; draft: ShareCheckoutDraft; pending?: SharePendingPurchase };
const KEY = "router_share_checkout_v1";
const identifier = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 256 && !/[\s\u0000-\u001f\u007f,]/.test(v);
export function parseShareCheckoutRecovery(value: unknown, scope: string): Recovery | null {
  if (!value || typeof value !== "object" || !scope) return null;
  const v = value as Recovery;
  const d = v.draft;
  if (v.scope !== scope || !d || !identifier(d.shareId) || !identifier(d.seatId) || typeof d.autoRenew !== "boolean") return null;
  const p = v.pending;
  if (p !== undefined && (!p || typeof p !== "object" || !identifier(p.quoteId) || !identifier(p.idempotencyKey) || p.shareId !== d.shareId || p.seatId !== d.seatId || p.autoRenew !== d.autoRenew)) return null;
  return { scope, draft: { shareId: d.shareId, seatId: d.seatId, autoRenew: d.autoRenew },
    ...(p ? { pending: { shareId: p.shareId, seatId: p.seatId, autoRenew: p.autoRenew, quoteId: p.quoteId, idempotencyKey: p.idempotencyKey } } : {}) };
}
export function readShareCheckoutRecovery(scope: string) {
  try { return parseShareCheckoutRecovery(JSON.parse(sessionStorage.getItem(KEY) || "null"), scope); } catch { return null; }
}
export function saveShareCheckoutRecovery(scope: string, draft: ShareCheckoutDraft, pending?: SharePendingPurchase) {
  const recovery = parseShareCheckoutRecovery({ scope, draft, pending }, scope);
  if (!recovery) return false;
  try { sessionStorage.setItem(KEY, JSON.stringify(recovery)); return true; } catch { return false; }
}
export function clearShareCheckoutRecovery(scope: string) {
  try { if (readShareCheckoutRecovery(scope)) sessionStorage.removeItem(KEY); } catch { /* unavailable storage */ }
}
