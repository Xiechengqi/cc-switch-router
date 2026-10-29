import { authFetch } from "./auth";
import { parseJson } from "./api";

export const AUDIT_STATUSES = ["opening", "activation_failed", "trialing", "paid", "free", "paused", "ended", "unknown"] as const;
export const AUDIT_ATTENTION = ["none", "finance_review", "waiting_provider", "owner_decision", "cleanup_failed", "cleanup_pending", "settlement_pending", "cleanup_unknown"] as const;
export const AUDIT_MONEY_FIELDS = ["chargedUnits", "returnedUnits", "unfulfilledUnits", "serviceUnits", "providerNetUnits", "platformFeeUnits", "providerCreditedUnits", "compensationUnits", "compensationCreditedUnits"] as const;
export type AuditMoney = Record<typeof AUDIT_MONEY_FIELDS[number], string>;
export type AuditRecord = {
  id: string; jobId: string | null; installationId: string | null; hostId: string | null;
  buyerEmail: string | null; providerEmail: string | null; clientSubdomain: string | null;
  hostIp: string | null; hostPort: number | null; countryCode: string | null;
  provenance: "order_snapshot" | "historical_record";
  createdAt: string; activatedAt: string | null; endedAt: string | null; cleanupConfirmedAt: string | null;
  contractId: string | null; status: typeof AUDIT_STATUSES[number];
  cleanupStatus: "not_required" | "verified" | "failed" | "pending" | "unknown";
  attention: typeof AUDIT_ATTENTION[number]; terminationId: string | null; hostPresent: boolean;
  pricingModel: string | null; cyclePriceMinor: number | null; dailyRateMinor: number | null;
  offerRevision: number | null; trialSecondsRemaining: number | null; renewalPolicy: string | null;
  money: AuditMoney | null;
};
export type AuditFilters = {
  search?: string; buyerEmail?: string; providerEmail?: string; status?: string;
  attention?: string; from?: string; until?: string;
};
export const AUDIT_LIST_PAGE_SIZE = 5;
const MAX_PAGE = 4_294_967_295;
export type AuditPage = { items: AuditRecord[]; total: number; page: number; pageSize: typeof AUDIT_LIST_PAGE_SIZE; totalPages: number; serverNow: string };
export type AuditPeriod = {
  id: string; sequence: number; startedAt: string; endedAt: string; debitedAt: string;
  amountUnits: string; serviceUnits: string; returnedUnits: string; compensationUnits: string;
  journalId: string; status: string; terminationRateBps: number | null; commissionRateBps: number | null;
};
export type AuditEvent = { id: string; action: string; actorEmail: string | null; createdAt: string; note: string | null };
export type AuditDetail = {
  record: AuditRecord; periods: AuditPeriod[]; periodsNextCursor: number | null;
  events: AuditEvent[]; eventsNextCursor: string | null; trialConsumedSeconds: number; serverNow: string;
  settlement: { heldUnits: string; returnedUnits: string; compensationUnits: string; reason: string; responsibility: string; cutoffAt: string; cleanupConfirmedAt: string | null } | null;
};
const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const text = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 2048;
const nullableText = (v: unknown) => v === null || text(v);
const time = (v: unknown) => text(v) && Number.isFinite(Date.parse(v));
const nullableTime = (v: unknown) => v === null || time(v);
const integer = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const nullableInteger = (v: unknown) => v === null || integer(v);
const units = (v: unknown) => typeof v === "string" && /^(0|[1-9]\d{0,18})$/.test(v) && BigInt(v) <= 9223372036854775807n;
function validRecord(v: unknown): v is AuditRecord {
  return object(v) && text(v.id)
    && ["jobId", "installationId", "hostId", "buyerEmail", "providerEmail", "clientSubdomain", "hostIp", "countryCode", "contractId", "terminationId", "pricingModel", "renewalPolicy"].every(k => nullableText(v[k]))
    && ["hostPort", "cyclePriceMinor", "dailyRateMinor", "offerRevision", "trialSecondsRemaining"].every(k => nullableInteger(v[k]))
    && time(v.createdAt) && ["activatedAt", "endedAt", "cleanupConfirmedAt"].every(k => nullableTime(v[k]))
    && AUDIT_STATUSES.includes(v.status as AuditRecord["status"]) && AUDIT_ATTENTION.includes(v.attention as AuditRecord["attention"])
    && ["not_required", "verified", "failed", "pending", "unknown"].includes(v.cleanupStatus as string)
    && ["order_snapshot", "historical_record"].includes(v.provenance as string) && typeof v.hostPresent === "boolean"
    && (v.money === null || object(v.money) && AUDIT_MONEY_FIELDS.every(k => units((v.money as Record<string, unknown>)[k])));
}
export function parseAuditPage(v: unknown): AuditPage {
  if (!object(v) || !Array.isArray(v.items) || v.items.length > AUDIT_LIST_PAGE_SIZE || !v.items.every(validRecord)
    || new Set(v.items.map(i => i.id)).size !== v.items.length || !integer(v.total) || !time(v.serverNow)
    || v.pageSize !== AUDIT_LIST_PAGE_SIZE || !integer(v.page) || v.page < 1 || v.page > MAX_PAGE
    || !integer(v.totalPages) || v.totalPages !== Math.max(1, Math.ceil(v.total / AUDIT_LIST_PAGE_SIZE)) || v.page > v.totalPages
    || v.items.length !== Math.min(AUDIT_LIST_PAGE_SIZE, v.total - (v.page - 1) * AUDIT_LIST_PAGE_SIZE)) throw new Error("Invalid rental audit page");
  return v as AuditPage;
}

/** Explicit submission only; never coerce decimals, exponents or unsafe pages. */
export function parseAuditPageNumber(value: string, totalPages: number): number | null {
  const trimmed = value.trim();
  if (!/^[1-9]\d{0,9}$/.test(trimmed) || !integer(totalPages) || totalPages < 1) return null;
  const page = Number(trimmed);
  return page <= Math.min(totalPages, MAX_PAGE) ? page : null;
}
export function parseAuditDetail(v: unknown, id: string): AuditDetail {
  if (!object(v) || !validRecord(v.record) || v.record.id !== id || !Array.isArray(v.periods) || v.periods.length > 50
    || !v.periods.every(p => object(p) && text(p.id) && integer(p.sequence) && p.sequence > 0 && text(p.journalId) && text(p.status)
      && ["startedAt", "endedAt", "debitedAt"].every(k => time(p[k])) && ["amountUnits", "serviceUnits", "returnedUnits", "compensationUnits"].every(k => units(p[k]))
      && ["terminationRateBps", "commissionRateBps"].every(k => nullableInteger(p[k]) && (p[k] === null || (p[k] as number) <= 10000)))
    || !nullableInteger(v.periodsNextCursor) || v.periodsNextCursor !== null && v.periodsNextCursor !== v.periods.at(-1)?.sequence
    || !Array.isArray(v.events) || v.events.length > 50 || !v.events.every(e => object(e) && text(e.id) && text(e.action) && nullableText(e.actorEmail) && time(e.createdAt) && (e.note === null || typeof e.note === "string" && e.note.length <= 2000))
    || !nullableText(v.eventsNextCursor) || v.eventsNextCursor !== null && v.eventsNextCursor !== v.events.at(-1)?.id
    || !integer(v.trialConsumedSeconds) || !time(v.serverNow)
    || !(v.settlement === null || object(v.settlement) && ["heldUnits", "returnedUnits", "compensationUnits"].every(k => units((v.settlement as Record<string, unknown>)[k]))
      && text(v.settlement.reason) && text(v.settlement.responsibility) && time(v.settlement.cutoffAt) && nullableTime(v.settlement.cleanupConfirmedAt))) throw new Error("Invalid rental audit detail");
  return v as AuditDetail;
}
export function auditFiltersFromDates(value: AuditFilters & { startDate?: string; endDate?: string }): AuditFilters {
  const { startDate, endDate, ...rest } = value;
  const midnight = (day: string) => {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(day)) throw new Error("Invalid audit date");
    const date = new Date(`${day}T00:00:00Z`);
    if (!Number.isFinite(date.getTime()) || date.toISOString().slice(0, 10) !== day) throw new Error("Invalid audit date");
    return date;
  };
  const from = startDate ? midnight(startDate).toISOString() : undefined;
  const until = endDate ? new Date(midnight(endDate).getTime() + 86400000).toISOString() : undefined;
  if (from && until && from >= until) throw new Error("Invalid audit date range");
  return { ...rest, from, until };
}
const ROOT = "/v1/admin/client-market/rental-audit";
export async function getRentalAudit(filters: AuditFilters, page = 1, signal?: AbortSignal) {
  if (!Number.isInteger(page) || page < 1 || page > MAX_PAGE) throw new Error("Invalid rental audit page number");
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(filters)) if (value?.trim()) query.set(key, value.trim());
  query.set("page", String(page));
  return parseAuditPage(await parseJson<unknown>(await authFetch(`${ROOT}?${query}`, { cache: "no-store", signal })));
}
export async function getRentalAuditDetail(id: string, options: { periodCursor?: number; eventCursor?: string } = {}, signal?: AbortSignal) {
  const query = new URLSearchParams();
  if (options.periodCursor != null) query.set("periodCursor", String(options.periodCursor));
  if (options.eventCursor) query.set("eventCursor", options.eventCursor);
  return parseAuditDetail(await parseJson<unknown>(await authFetch(`${ROOT}/${encodeURIComponent(id)}?${query}`, { cache: "no-store", signal })), id);
}
