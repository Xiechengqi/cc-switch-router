import { authFetch } from "./auth";
import { parseJson } from "./api";
import { AUDIT_ATTENTION, AUDIT_STATUSES, type AuditRecord } from "./client-rental-audit";
import type { ClientMarketRental } from "./types";

export const CLIENT_RENTAL_PAGE_SIZE = 5;
export const CLIENT_RENTAL_MONEY_FIELDS = ["chargedUnits", "returnedUnits", "unfulfilledUnits", "serviceUnits", "compensationUnits", "netUnits"] as const;
export type ClientRentalView = "current" | "history";
export type ClientRentalMoney = Record<typeof CLIENT_RENTAL_MONEY_FIELDS[number], string>;
export type ClientRentalRecord = Pick<AuditRecord,
  "id" | "installationId" | "providerEmail" | "clientSubdomain" | "countryCode" | "provenance" |
  "createdAt" | "activatedAt" | "endedAt" | "cleanupConfirmedAt" | "contractId" | "status" | "cleanupStatus" |
  "attention" | "terminationId" | "pricingModel" | "cyclePriceMinor" | "dailyRateMinor" | "offerRevision" |
  "trialSecondsRemaining" | "renewalPolicy"> & {
    group: ClientRentalView; periodEnd: string | null; expiresAt: string | null;
    freeDurationDays: number | null; contractStatus: string | null; money: ClientRentalMoney | null;
  };
export type ClientRentalPage = {
  items: ClientRentalRecord[]; view: ClientRentalView; total: number; page: number;
  pageSize: 5; totalPages: number; focusedId: string | null; serverNow: string;
};
export type ClientRentalPeriod = {
  id: string; sequence: number; startedAt: string; endedAt: string; debitedAt: string;
  amountUnits: string; serviceUnits: string; returnedUnits: string; compensationUnits: string;
  status: string; terminationRateBps: number | null;
};
export type ClientRentalEvent = { id: string; action: string; createdAt: string; note: string | null };
export type ClientRentalDetail = {
  record: ClientRentalRecord; rental: ClientMarketRental | null;
  periods: ClientRentalPeriod[]; periodsNextCursor: number | null;
  events: ClientRentalEvent[]; eventsNextCursor: string | null;
  trialConsumedSeconds: number; serverNow: string;
  settlement: { heldUnits: string; returnedUnits: string; compensationUnits: string; reason: string;
    responsibility: string; cutoffAt: string; cleanupConfirmedAt: string | null } | null;
};
const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const text = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 2048;
const nullableText = (v: unknown) => v === null || text(v);
const time = (v: unknown) => text(v) && Number.isFinite(Date.parse(v));
const nullableTime = (v: unknown) => v === null || time(v);
const integer = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const nullableInteger = (v: unknown) => v === null || integer(v);
const units = (v: unknown): v is string => typeof v === "string" && /^(0|[1-9]\d{0,18})$/.test(v) && BigInt(v) <= 9223372036854775807n;
function validLiveRental(v: unknown, record: ClientRentalRecord): boolean {
  if (v === null) return true;
  if (!object(v) || record.group !== "current" || v.isClientOwner !== true || v.installationId !== record.installationId
    || !["hostId", "providerId", "hostOwnerEmail", "clientOwnerEmail", "status", "pricingModel"].every(k => text(v[k]))
    || !integer(v.offerRevision) || !time(v.updatedAt)
    || !["canRelease", "canFinalizeRelease", "hostCleanupPending", "providerTerminalAccessActive", "canManageProviderTerminal"].every(k => typeof v[k] === "boolean")
    || !["activatedAt", "expiresAt", "nextCleanupProbeAt", "providerTerminalAuthorizedUntil"].every(k => v[k] == null || time(v[k]))
    || !["dailyRateMinor", "cyclePriceMinor", "freeDurationDays", "cleanupProbeAttempts"].every(k => v[k] == null || integer(v[k]))
    || !(v.walletContractId == null || text(v.walletContractId) && v.walletContractId === record.contractId)) return false;
  if (v.walletContract == null) return true;
  const wallet = v.walletContract;
  if (!object(wallet) || wallet.id !== record.contractId || !text(wallet.status) || !text(wallet.renewalPolicy)
    || !integer(wallet.trialSecondsRemaining) || !nullableTime(wallet.periodEnd)) return false;
  if (wallet.nextRentalTrial == null) return true;
  const trial = wallet.nextRentalTrial;
  return object(trial) && ["epoch", "allowanceSeconds", "consumedSeconds", "remainingSeconds"].every(k => integer(trial[k]))
    && nullableTime(trial.lastResetAt);
}
function validRecord(v: unknown): v is ClientRentalRecord {
  if (!object(v) || !text(v.id) || !["current", "history"].includes(v.group as string)
    || !["installationId", "providerEmail", "clientSubdomain", "countryCode", "contractId", "terminationId", "pricingModel", "renewalPolicy", "contractStatus"].every(k => nullableText(v[k]))
    || !["cyclePriceMinor", "dailyRateMinor", "offerRevision", "trialSecondsRemaining", "freeDurationDays"].every(k => nullableInteger(v[k]))
    || !time(v.createdAt) || !["activatedAt", "endedAt", "cleanupConfirmedAt", "periodEnd", "expiresAt"].every(k => nullableTime(v[k]))
    || !AUDIT_STATUSES.includes(v.status as ClientRentalRecord["status"]) || !AUDIT_ATTENTION.includes(v.attention as ClientRentalRecord["attention"])
    || !["not_required", "verified", "failed", "pending", "unknown"].includes(v.cleanupStatus as string)
    || !["order_snapshot", "historical_record"].includes(v.provenance as string)
    || (v.group === "history") !== ["ended", "activation_failed"].includes(v.status as string)) return false;
  if (v.money === null) return true;
  return object(v.money) && CLIENT_RENTAL_MONEY_FIELDS.every(k => units(v.money && (v.money as Record<string, unknown>)[k]))
    && BigInt(v.money.chargedUnits as string) - BigInt(v.money.returnedUnits as string) === BigInt(v.money.netUnits as string);
}
export function parseClientRentalPage(v: unknown, view: ClientRentalView): ClientRentalPage {
  if (!object(v) || v.view !== view || !Array.isArray(v.items) || !v.items.every(i => validRecord(i) && i.group === view)
    || new Set(v.items.map(i => i.id)).size !== v.items.length || !integer(v.total) || !time(v.serverNow)
    || v.pageSize !== 5 || !integer(v.page) || v.page < 1 || v.page > 4294967295
    || !integer(v.totalPages) || v.totalPages !== Math.max(1, Math.ceil(v.total / 5)) || v.page > v.totalPages
    || v.items.length !== Math.min(5, v.total - (v.page - 1) * 5)
    || !nullableText(v.focusedId) || v.focusedId !== null && !v.items.some(i => i.id === v.focusedId)) throw new Error("Invalid personal rental page");
  return v as ClientRentalPage;
}
export function parseClientRentalDetail(v: unknown, id: string): ClientRentalDetail {
  if (!object(v) || !validRecord(v.record) || v.record.id !== id || !time(v.serverNow) || !integer(v.trialConsumedSeconds)
    || !validLiveRental(v.rental, v.record)
    || !Array.isArray(v.periods) || v.periods.length > 50 || !v.periods.every(p => object(p) && text(p.id) && integer(p.sequence) && p.sequence > 0 && text(p.status)
      && ["startedAt", "endedAt", "debitedAt"].every(k => time(p[k])) && ["amountUnits", "serviceUnits", "returnedUnits", "compensationUnits"].every(k => units(p[k]))
      && nullableInteger(p.terminationRateBps) && (p.terminationRateBps === null || p.terminationRateBps <= 10000))
    || !nullableInteger(v.periodsNextCursor) || v.periodsNextCursor !== null && v.periodsNextCursor !== v.periods.at(-1)?.sequence
    || !Array.isArray(v.events) || v.events.length > 50 || !v.events.every(e => object(e) && text(e.id) && text(e.action) && time(e.createdAt) && (e.note === null || typeof e.note === "string" && Array.from(e.note).length <= 2000))
    || !nullableText(v.eventsNextCursor) || v.eventsNextCursor !== null && v.eventsNextCursor !== v.events.at(-1)?.id
    || !(v.settlement === null || object(v.settlement) && ["heldUnits", "returnedUnits", "compensationUnits"].every(k => units((v.settlement as Record<string, unknown>)[k]))
      && text(v.settlement.reason) && text(v.settlement.responsibility) && time(v.settlement.cutoffAt) && nullableTime(v.settlement.cleanupConfirmedAt))) throw new Error("Invalid personal rental detail");
  return v as ClientRentalDetail;
}
const ROOT = "/v1/client-market/me/rentals";
export async function getClientRentals(view: ClientRentalView, page: number, focus?: string | null, signal?: AbortSignal) {
  if (!Number.isInteger(page) || page < 1 || page > 4294967295) throw new Error("Invalid personal rental page number");
  const query = new URLSearchParams({ view, page: String(page) });
  if (focus) query.set("focus", focus);
  return parseClientRentalPage(await parseJson<unknown>(await authFetch(`${ROOT}?${query}`, { cache: "no-store", signal })), view);
}
export async function getClientRentalDetail(id: string, options: { periodCursor?: number; eventCursor?: string } = {}, signal?: AbortSignal) {
  const query = new URLSearchParams();
  if (options.periodCursor != null) query.set("periodCursor", String(options.periodCursor));
  if (options.eventCursor) query.set("eventCursor", options.eventCursor);
  return parseClientRentalDetail(await parseJson<unknown>(await authFetch(`${ROOT}/${encodeURIComponent(id)}?${query}`, { cache: "no-store", signal })), id);
}
export const clientRentalLabel = (r: ClientRentalRecord) => r.clientSubdomain || r.installationId || r.id;
