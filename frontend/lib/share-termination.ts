import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

export type ShareTerminationPolicy = { version: number; rateBps: number; canEdit?: boolean };
export type ShareTerminationReason = "voluntary" | "provider_issue" | "platform_issue";
export type ShareTerminationEvent = { id: string; action: string; createdAt: string; detail: Record<string, unknown> };
export type ShareTermination = {
  id: string; subscriptionId: string; shareId: string; seatId: string; shareName: string; installationId: string;
  buyerUserId: string; providerUserId: string; buyerEmail: string; providerEmail: string;
  periodAmountUnits: string; timeEarnedUnits: string; quotaEarnedUnits: string; remainingUnits: string;
  compensationCapUnits: string; compensationUnits: string; returnedUnits: string; heldUnits: string;
  rateBps: number; policyVersion: number; version: number; reason: ShareTerminationReason; claim: string;
  status: "revoke_pending" | "finance_pending" | "disputed" | "completed";
  responsibility: string; cutoffAt: string; revokedAt: string | null; updatedAt: string;
  responseDeadlineAt: string | null; providerRespondedAt: string | null; decisionNote: string | null; serverNow: string;
  actionRequired: boolean;
  permissions: Record<"buyer" | "provider" | "owner" | "withdraw" | "waive" | "respond" | "resolve" | "retryRevoke", boolean>;
  events: ShareTerminationEvent[]; eventsNextCursor: string | null;
};
export type ShareTerminationPreview = Pick<ShareTermination, "subscriptionId" | "periodAmountUnits" | "remainingUnits" | "compensationCapUnits" | "rateBps" | "policyVersion"> & {
  previewId: string; expiresAt: string; returnUnits: string; usagePending: boolean; serverNow: string;
};
const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
export const shareTerminationUnits = (v: unknown): v is string => typeof v === "string" && /^(0|[1-9]\d{0,18})$/.test(v) && BigInt(v) <= 9223372036854775807n;
const date = (v: unknown) => typeof v === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/.test(v) && Number.isFinite(Date.parse(v));
const cursor = (v: unknown) => v === null || typeof v === "string" && v.length > 0;
const integer = (v: unknown, min: number, max = Number.MAX_SAFE_INTEGER) => Number.isSafeInteger(v) && (v as number) >= min && (v as number) <= max;
function event(v: unknown): v is ShareTerminationEvent {
  return object(v) && typeof v.id === "string" && !!v.id && typeof v.action === "string" && !!v.action && date(v.createdAt) && object(v.detail);
}
export const shareTerminationNoteValid = (v: string) => new TextEncoder().encode(v).length <= 2000;
export function parseShareTermination(v: unknown): ShareTermination {
  if (!object(v) || !object(v.permissions) || !["id", "subscriptionId", "shareId", "seatId", "installationId", "buyerUserId", "providerUserId"].every(k => typeof v[k] === "string" && v[k])
    || !["shareName", "buyerEmail", "providerEmail", "claim"].every(k => typeof v[k] === "string") || !(v.decisionNote === null || typeof v.decisionNote === "string")
    || !["periodAmountUnits", "timeEarnedUnits", "quotaEarnedUnits", "remainingUnits", "compensationCapUnits", "compensationUnits", "returnedUnits", "heldUnits"].every(k => shareTerminationUnits(v[k]))
    || !["cutoffAt", "updatedAt", "serverNow"].every(k => date(v[k])) || !["revokedAt", "responseDeadlineAt", "providerRespondedAt"].every(k => v[k] === null || date(v[k]))
    || !integer(v.version, 1) || !integer(v.policyVersion, 0) || !integer(v.rateBps, 0, 10000)
    || !["revoke_pending", "finance_pending", "disputed", "completed"].includes(v.status as string)
    || !["voluntary", "provider_issue", "platform_issue"].includes(v.reason as string)
    || !["buyer", "undetermined", "none"].includes(v.responsibility as string)
    || !["buyer", "provider", "owner", "withdraw", "waive", "respond", "resolve", "retryRevoke"].every(k => typeof (v.permissions as Record<string, unknown>)[k] === "boolean")
    || typeof v.actionRequired !== "boolean" || !Array.isArray(v.events) || !v.events.every(event) || !cursor(v.eventsNextCursor)) throw new Error("Invalid Share termination response");
  return v as ShareTermination;
}
export function parseShareTerminationPreview(v: unknown): ShareTerminationPreview {
  if (!object(v) || typeof v.previewId !== "string" || !v.previewId || !date(v.expiresAt) || !date(v.serverNow) || typeof v.usagePending !== "boolean"
    || typeof v.subscriptionId !== "string" || !v.subscriptionId || !integer(v.policyVersion, 0)
    || !["periodAmountUnits", "remainingUnits", "compensationCapUnits", "returnUnits"].every(k => shareTerminationUnits(v[k]))
    || !integer(v.rateBps, 0, 10000)) throw new Error("Invalid Share termination preview");
  if (BigInt(v.remainingUnits as string) > BigInt(v.periodAmountUnits as string)
    || BigInt(v.compensationCapUnits as string) > BigInt(v.remainingUnits as string)
    || BigInt(v.returnUnits as string) + BigInt(v.compensationCapUnits as string) !== BigInt(v.remainingUnits as string)
    || Date.parse(v.expiresAt as string) <= Date.parse(v.serverNow as string)) throw new Error("Invalid Share termination preview allocation");
  return v as ShareTerminationPreview;
}
async function request(path: string, method = "GET", body?: unknown, signal?: AbortSignal) {
  return parseJson<unknown>(await authFetch(path, { method, cache: "no-store", signal, ...(body === undefined ? {} : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }) }));
}
const root = "/v1/share-market";
export const shareTerminationHref = (id: string) => `/share-market/?terminationId=${encodeURIComponent(id)}`;
export async function previewShareTermination(id: string, signal?: AbortSignal) {
  const v = parseShareTerminationPreview(await request(`${root}/subscriptions/${encodeURIComponent(id)}/termination-preview`, "POST", undefined, signal));
  if (v.subscriptionId !== id) throw new Error("Share termination preview identity mismatch");
  return v;
}
export async function terminateShare(id: string, previewId: string, reason: ShareTerminationReason, claim: string, signal?: AbortSignal) {
  const v = parseShareTermination(await request(`${root}/subscriptions/${encodeURIComponent(id)}/terminate`, "POST", { previewId, reason, claim }, signal));
  if (v.subscriptionId !== id) throw new Error("Share termination subscription identity mismatch");
  return v;
}
export async function getShareTermination(id: string, signal?: AbortSignal) {
  const record = parseShareTermination(await request(`${root}/terminations/${encodeURIComponent(id)}`, "GET", undefined, signal));
  if (record.id !== id) throw new Error("Share termination identity mismatch");
  return record;
}
export async function getShareTerminations(options: { shareId?: string; seatId?: string; cursor?: string; actionOnly?: boolean } = {}, signal?: AbortSignal) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(options)) if (v !== undefined) q.set(k, String(v));
  const v = await request(`${root}/terminations?${q}`, "GET", undefined, signal);
  if (!object(v) || !Array.isArray(v.items) || !cursor(v.nextCursor)) throw new Error("Invalid Share termination list");
  const items = v.items.map(parseShareTermination);
  if (items.some(i => options.shareId && i.shareId !== options.shareId || options.seatId && i.seatId !== options.seatId)) throw new Error("Share termination resource identity mismatch");
  return { items, nextCursor: v.nextCursor as string | null };
}
export async function getShareTerminationEvents(id: string, after: string, signal?: AbortSignal) {
  const v = await request(`${root}/terminations/${encodeURIComponent(id)}/events?cursor=${encodeURIComponent(after)}`, "GET", undefined, signal);
  if (!object(v) || !Array.isArray(v.items) || !v.items.every(event) || !cursor(v.nextCursor)) throw new Error("Invalid Share termination history");
  return { items: v.items as ShareTerminationEvent[], nextCursor: v.nextCursor as string | null };
}
export async function actOnShareTermination(record: ShareTermination, action: "respond" | "waive" | "resolve" | "withdraw" | "retry-revoke", note: string, awardUnits?: string, signal?: AbortSignal) {
  const v = parseShareTermination(await request(`${root}/terminations/${encodeURIComponent(record.id)}/${action}`, "POST", { expectedVersion: record.version, note, ...(awardUnits === undefined ? {} : { awardUnits }) }, signal));
  if (v.id !== record.id || v.subscriptionId !== record.subscriptionId) throw new Error("Share termination decision identity mismatch");
  return v;
}
export async function getShareTerminationPolicy(signal?: AbortSignal): Promise<ShareTerminationPolicy> {
  return parsePolicy(await request("/v1/admin/market/share-termination-policy", "GET", undefined, signal));
}
export async function saveShareTerminationPolicy(p: ShareTerminationPolicy, rateBps: number, signal?: AbortSignal): Promise<ShareTerminationPolicy> {
  return parsePolicy(await request("/v1/admin/market/share-termination-policy", "PUT", { expectedVersion: p.version, rateBps }, signal));
}
function parsePolicy(v: unknown): ShareTerminationPolicy {
  if (!object(v) || !integer(v.version, 1) || !integer(v.rateBps, 0, 10000) || !(v.canEdit === undefined || typeof v.canEdit === "boolean")) throw new Error("Invalid Share termination policy");
  return v as ShareTerminationPolicy;
}
