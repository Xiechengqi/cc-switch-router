/*
 * Share Market rental audit (Router owner only). Same envelope, ledger money,
 * periods and events as the Client audit (`client-rental-audit.ts`); the record
 * describes a Share seat rental instead of a Client.
 */
import { authFetch } from "./auth";
import { parseJson } from "./api";
import {
  AUDIT_LIST_PAGE_SIZE,
  AUDIT_MAX_PAGE,
  auditShape,
  validAuditEvents,
  validAuditMoney,
  validAuditPageEnvelope,
  validAuditPeriods,
  type AuditEvent,
  type AuditFilters,
  type AuditMoney,
  type AuditPeriod,
} from "./client-rental-audit";

export const SHARE_AUDIT_STATUSES = ["opening", "grant_failed", "trialing", "paid", "free", "paused", "ending", "ended", "unknown"] as const;
export const SHARE_AUDIT_ATTENTION = ["none", "finance_review", "disputed", "revoke_pending", "revoke_failed", "billing_control_failed", "settlement_pending"] as const;

export type ShareAuditRecord = {
  id: string; shareId: string; shareName: string | null; shareSubdomain: string | null; appType: string | null;
  seatId: string | null; seatLabel: string | null; seatPosition: number | null; installationId: string | null;
  buyerEmail: string | null; providerEmail: string | null;
  provenance: "order_snapshot" | "historical_record";
  createdAt: string; activatedAt: string | null; endedAt: string | null;
  contractId: string | null; status: typeof SHARE_AUDIT_STATUSES[number]; attention: typeof SHARE_AUDIT_ATTENTION[number];
  terminationId: string | null; sharePresent: boolean;
  pricingModel: string | null; dailyRateMinor: number | null; offerRevision: number | null; renewalPolicy: string | null;
  expiresAt: string | null; releaseReason: string | null;
  money: AuditMoney | null;
};
export type ShareAuditPage = { items: ShareAuditRecord[]; total: number; page: number; pageSize: typeof AUDIT_LIST_PAGE_SIZE; totalPages: number; serverNow: string };
export type ShareAuditTrial = {
  phase: string; tokenAllowance: string | null; tokenBudget: string | null;
  valueAllowanceUsdMicros: string | null; valueBudgetUsdMicros: string | null;
};
/** From the termination payload; an amount the payload never recorded is null, not zero. */
export type ShareAuditSettlement = {
  status: string; reason: string | null; heldUnits: string | null; returnedUnits: string | null; compensationUnits: string | null;
  responsibility: string | null; cutoffAt: string;
};
export type ShareAuditDetail = {
  record: ShareAuditRecord; periods: AuditPeriod[]; periodsNextCursor: number | null;
  events: AuditEvent[]; eventsNextCursor: string | null;
  trial: ShareAuditTrial | null; settlement: ShareAuditSettlement | null; serverNow: string;
};

const { object, text, nullableText, time, nullableTime, nullableInteger, units } = auditShape;
const nullableUnits = (v: unknown) => v === null || units(v);

function validRecord(v: unknown): v is ShareAuditRecord {
  return object(v) && text(v.id) && text(v.shareId)
    && ["shareName", "shareSubdomain", "appType", "seatId", "seatLabel", "installationId", "buyerEmail", "providerEmail", "contractId", "terminationId", "pricingModel", "renewalPolicy", "releaseReason"].every(k => nullableText(v[k]))
    && ["dailyRateMinor", "offerRevision", "seatPosition"].every(k => nullableInteger(v[k]))
    && time(v.createdAt) && ["activatedAt", "endedAt", "expiresAt"].every(k => nullableTime(v[k]))
    && SHARE_AUDIT_STATUSES.includes(v.status as ShareAuditRecord["status"])
    && SHARE_AUDIT_ATTENTION.includes(v.attention as ShareAuditRecord["attention"])
    && ["order_snapshot", "historical_record"].includes(v.provenance as string) && typeof v.sharePresent === "boolean"
    && validAuditMoney(v.money);
}

export function parseShareAuditPage(v: unknown): ShareAuditPage {
  if (!object(v) || !Array.isArray(v.items) || v.items.length > AUDIT_LIST_PAGE_SIZE || !v.items.every(validRecord)
    || new Set(v.items.map(i => i.id)).size !== v.items.length || !validAuditPageEnvelope(v, v.items)) throw new Error("Invalid Share rental audit page");
  return v as ShareAuditPage;
}

export function parseShareAuditDetail(v: unknown, id: string): ShareAuditDetail {
  if (!object(v) || !validRecord(v.record) || v.record.id !== id
    || !validAuditPeriods(v.periods, v.periodsNextCursor) || !validAuditEvents(v.events, v.eventsNextCursor) || !time(v.serverNow)
    || !(v.trial === null || object(v.trial) && text(v.trial.phase)
      && ["tokenAllowance", "tokenBudget", "valueAllowanceUsdMicros", "valueBudgetUsdMicros"].every(k => nullableUnits((v.trial as Record<string, unknown>)[k])))
    || !(v.settlement === null || object(v.settlement) && text(v.settlement.status)
      && ["heldUnits", "returnedUnits", "compensationUnits"].every(k => nullableUnits((v.settlement as Record<string, unknown>)[k]))
      && (v.settlement.reason === undefined || nullableText(v.settlement.reason))
      && nullableText(v.settlement.responsibility) && time(v.settlement.cutoffAt))) throw new Error("Invalid Share rental audit detail");
  return v as ShareAuditDetail;
}

const ROOT = "/v1/admin/share-market/rental-audit";
export async function getShareRentalAudit(filters: AuditFilters, page = 1, signal?: AbortSignal) {
  if (!Number.isInteger(page) || page < 1 || page > AUDIT_MAX_PAGE) throw new Error("Invalid rental audit page number");
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(filters)) if (value?.trim()) query.set(key, value.trim());
  query.set("page", String(page));
  return parseShareAuditPage(await parseJson<unknown>(await authFetch(`${ROOT}?${query}`, { cache: "no-store", signal })));
}
export async function getShareRentalAuditDetail(id: string, options: { periodCursor?: number; eventCursor?: string } = {}, signal?: AbortSignal) {
  const query = new URLSearchParams();
  if (options.periodCursor != null) query.set("periodCursor", String(options.periodCursor));
  if (options.eventCursor) query.set("eventCursor", options.eventCursor);
  return parseShareAuditDetail(await parseJson<unknown>(await authFetch(`${ROOT}/${encodeURIComponent(id)}?${query}`, { cache: "no-store", signal })), id);
}
