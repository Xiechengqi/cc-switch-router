import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

export type ClientTerminationPolicy = { version: number; rateBps: number; canEdit?: boolean };
export type ClientTerminationPreview = { previewId: string; expiresAt: string; policyVersion: number; rateBps: number; periodAmountUnits: string; remainingUnits: string; compensationUnits: string; returnUnits: string; nextRentalTrial?: import("./types").ClientTrialState; trialSecondsRemaining?: number | null };
export type ClientTerminationRequest = { previewId: string; reason: "voluntary" | "provider_issue" | "platform_issue"; claim: string };
export type ClientTerminationResolution = "provider_waiver" | "owner_decision" | "buyer_withdrawal" | "timeout_waiver";
export type ClientTermination = {
  id: string; installationId: string; hostId: string; contractId: string | null; periodId: string | null;
  buyerUserId: string; providerUserId: string; policyVersion: number; rateBps: number;
  periodAmountUnits: string; remainingUnits: string; compensationCapUnits: string; returnedUnits: string;
  compensationUnits: string; heldUnits: string; reason: string; claim: string; responsibility: string;
  status: "cleanup_pending" | "cleanup_failed" | "finance_pending" | "disputed" | "completed"; cutoffAt: string;
  cleanupConfirmedAt: string | null; cleanupJobId: string | null; disputeUsed: boolean; version: number;
  disputeOpenedAt: string | null; disputeDeadlineAt: string | null; providerRespondedAt: string | null;
  resolution: ClientTerminationResolution | null; chatAvailable: boolean;
  updatedAt: string; unlockAt: string | null; creditedUnits: string;
  clientSubdomain?: string | null; buyerEmail?: string | null; providerEmail?: string | null; decisionNote?: string | null;
};
export type ClientTerminationPage = { items: ClientTermination[]; nextCursor: string | null };
export type ClientTerminationRole = "buyer" | "provider" | "owner";
export type ClientTerminationAction = "dispute" | "decision" | "respond" | "withdraw";
export type ClientTerminationAttention = "provider_response" | "owner_decision" | "cleanup_failed" | "finance_review" | "waiting_provider" | "waiting_owner" | "timeout_pending" | "processing" | "completed";
export type ClientTerminationPermissions = {
  buyer: boolean; provider: boolean; owner: boolean;
  dispute: boolean; withdraw: boolean; waive: boolean; respond: boolean; resolve: boolean; retryCleanup: boolean;
};
export type ClientTerminationEvent = { id: string; action: string; actorRole: "buyer" | "provider" | "owner" | "system"; note: string | null; awardUnits: string | null; createdAt: string };
export type ClientTerminationDetail = ClientTermination & {
  permissions: ClientTerminationPermissions; attention: ClientTerminationAttention; actionRequired: boolean;
  serverNow: string; events: ClientTerminationEvent[]; eventsNextCursor: string | null;
};
export type ClientTerminationHostSummary = { hostId: string; terminationId: string; attention: ClientTerminationAttention; actionRequiredCount: number; responseDeadlineAt: string | null };
export type ClientTerminationWorkspace = { hosts: ClientTerminationHostSummary[]; actionRequiredCount: number; canAdjudicate: boolean; serverNow: string };
export type ClientTerminationWorkPage = { items: (ClientTermination & Pick<ClientTerminationDetail, "permissions" | "attention" | "actionRequired" | "serverNow">)[]; nextCursor: string | null; serverNow: string };

const ATTENTION = ["provider_response", "owner_decision", "cleanup_failed", "finance_review", "waiting_provider", "waiting_owner", "timeout_pending", "processing", "completed"];
const unitString = (value: unknown): value is string => typeof value === "string" && /^(?:0|[1-9]\d{0,18})$/.test(value) && BigInt(value) <= 9223372036854775807n;
const timestamp = (value: unknown): value is string => typeof value === "string" && Number.isFinite(Date.parse(value));
const cursor = (value: unknown) => value === null || typeof value === "string" && value.length > 0;
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
function validWorkRecord(value: unknown): boolean {
  if (!object(value) || !object(value.permissions)) return false;
  return ["id", "installationId", "hostId", "buyerUserId", "providerUserId"].every(key => typeof value[key] === "string" && value[key])
    && ["periodAmountUnits", "remainingUnits", "compensationCapUnits", "returnedUnits", "compensationUnits", "heldUnits", "creditedUnits"].every(key => unitString(value[key]))
    && ["cutoffAt", "updatedAt", "serverNow"].every(key => timestamp(value[key]))
    && ["cleanupConfirmedAt", "disputeOpenedAt", "disputeDeadlineAt", "providerRespondedAt", "unlockAt"].every(key => value[key] === null || timestamp(value[key]))
    && ["buyer", "provider", "owner", "dispute", "withdraw", "waive", "respond", "resolve", "retryCleanup"].every(key => typeof (value.permissions as Record<string, unknown>)[key] === "boolean")
    && (value.permissions.buyer === true || value.permissions.provider === true || value.permissions.owner === true)
    && ["cleanup_pending", "cleanup_failed", "finance_pending", "disputed", "completed"].includes(value.status as string)
    && ATTENTION.includes(value.attention as string) && typeof value.actionRequired === "boolean"
    && typeof value.disputeUsed === "boolean" && typeof value.chatAvailable === "boolean"
    && typeof value.reason === "string" && typeof value.claim === "string" && typeof value.responsibility === "string"
    && Number.isSafeInteger(value.version) && (value.version as number) >= 0;
}
/** Reject malformed money/capability responses before rendering BigInt or dates.
 * Client validation is defensive only; authorization remains server-owned. */
export function parseClientTerminationDetail(value: unknown, id: string): ClientTerminationDetail {
  if (!object(value) || !validWorkRecord(value) || value.id !== id || !Array.isArray(value.events) || !cursor(value.eventsNextCursor)
    || !value.events.every(event => object(event) && typeof event.id === "string" && event.id && typeof event.action === "string"
      && ["buyer", "provider", "owner", "system"].includes(event.actorRole as string) && timestamp(event.createdAt)
      && (event.note === null || typeof event.note === "string") && (event.awardUnits === null || unitString(event.awardUnits)))) throw new Error("Invalid termination detail");
  return value as ClientTerminationDetail;
}
export function parseClientTerminationWorkspace(value: unknown): ClientTerminationWorkspace {
  if (!object(value) || !Array.isArray(value.hosts) || !timestamp(value.serverNow) || typeof value.canAdjudicate !== "boolean"
    || !Number.isSafeInteger(value.actionRequiredCount) || (value.actionRequiredCount as number) < 0
    || !value.hosts.every(host => object(host) && typeof host.hostId === "string" && host.hostId
      && typeof host.terminationId === "string" && host.terminationId && ATTENTION.includes(host.attention as string)
      && Number.isSafeInteger(host.actionRequiredCount) && (host.actionRequiredCount as number) >= 0
      && (host.responseDeadlineAt === null || timestamp(host.responseDeadlineAt)))) throw new Error("Invalid termination workspace");
  return value as ClientTerminationWorkspace;
}
export function parseClientTerminationWorkPage(value: unknown): ClientTerminationWorkPage {
  if (!object(value) || !Array.isArray(value.items) || !value.items.every(validWorkRecord) || !cursor(value.nextCursor) || !timestamp(value.serverNow)) throw new Error("Invalid termination records");
  return value as ClientTerminationWorkPage;
}

export const clientTerminationHref = (id: string) => `/client-market/?terminationId=${encodeURIComponent(id)}`;
export async function getClientTerminationDetail(id: string, signal?: AbortSignal, eventCursor?: string) {
  return parseClientTerminationDetail(await parseJson<unknown>(await authFetch(`/v1/client-market/terminations/${encodeURIComponent(id)}${eventCursor ? `?eventCursor=${encodeURIComponent(eventCursor)}` : ""}`, { cache: "no-store", signal })), id);
}
export async function getClientTerminationWorkspace(signal?: AbortSignal) {
  return parseClientTerminationWorkspace(await parseJson<unknown>(await authFetch("/v1/client-market/termination-workspace", { cache: "no-store", signal })));
}
export async function getClientTerminationWorkRecords(options: { hostId?: string; pendingOnly?: boolean; cursor?: string }, signal?: AbortSignal) {
  const query = new URLSearchParams();
  if (options.hostId) query.set("hostId", options.hostId);
  if (options.pendingOnly) query.set("pendingOnly", "true");
  if (options.cursor) query.set("cursor", options.cursor);
  return parseClientTerminationWorkPage(await parseJson<unknown>(await authFetch(`/v1/client-market/termination-workspace/records?${query}`, { cache: "no-store", signal })));
}

/** Server-clock deadlines keep browser clock skew from offering an expired action. */
export function terminationServerNow(detail: Pick<ClientTerminationDetail, "serverNow">, receivedAt: number, now = Date.now()) {
  return Date.parse(detail.serverNow) + Math.max(0, now - receivedAt);
}
export function terminationActionAllowed(detail: ClientTerminationDetail, role: ClientTerminationRole, action: ClientTerminationAction, now: number) {
  const p = detail.permissions;
  if (!p[role]) return false;
  if (action === "respond") return role === "provider" && p.respond && terminationCanRespond(detail, now);
  if (action === "dispute") return role === "buyer" && p.dispute && terminationCanDispute(detail, now);
  if (action === "withdraw") return role === "buyer" && p.withdraw && terminationCanWithdraw(detail, now);
  return (role === "provider" ? p.waive : role === "owner" && p.resolve) && terminationCanDecide(detail, now);
}
export const getClientTerminationPolicy = (admin = false) => request<ClientTerminationPolicy>(admin ? "/v1/admin/market/client-termination-policy" : "/v1/client-market/termination-policy");
export const saveClientTerminationPolicy = (policy: ClientTerminationPolicy, rateBps: number) => request<ClientTerminationPolicy>("/v1/admin/market/client-termination-policy", "PUT", { expectedVersion: policy.version, rateBps });
export const previewClientTermination = (installationId: string) => request<ClientTerminationPreview>(`/v1/client-market/clients/${encodeURIComponent(installationId)}/termination-preview`, "POST");
export const getClientTermination = (installationId: string) => request<ClientTermination | null>(`/v1/client-market/clients/${encodeURIComponent(installationId)}/termination`);
export const getClientTerminations = (role: "buyer" | "provider" | "owner", cursor?: string) => request<ClientTerminationPage>(`${role === "owner" ? "/v1/admin/market/client-terminations" : "/v1/client-market/terminations"}?provider=${role === "provider"}&openOnly=${role === "owner"}&${cursor ? `cursor=${encodeURIComponent(cursor)}` : ""}`);
export const decideClientTermination = (termination: ClientTermination, owner: boolean, awardUnits: string, responsibility: string, note: string) => request<ClientTermination>(`${owner ? "/v1/admin/market/client-terminations" : "/v1/client-market/terminations"}/${encodeURIComponent(termination.id)}/${owner ? "resolve" : "waive"}`, "POST", { expectedVersion: termination.version, awardUnits, responsibility, note });
export const disputeClientTermination = (termination: ClientTermination, claim: string) => request<ClientTermination>(`/v1/client-market/terminations/${encodeURIComponent(termination.id)}/dispute`, "POST", { expectedVersion: termination.version, claim });
export const respondClientTermination = (termination: ClientTermination, note: string) => request<ClientTermination>(`/v1/client-market/terminations/${encodeURIComponent(termination.id)}/respond`, "POST", { expectedVersion: termination.version, note });
export const withdrawClientTerminationClaim = (termination: ClientTermination) => request<ClientTermination>(`/v1/client-market/terminations/${encodeURIComponent(termination.id)}/withdraw`, "POST", { expectedVersion: termination.version });

async function request<T>(path: string, method = "GET", body?: unknown) {
  return parseJson<T>(await authFetch(path, { method, cache: "no-store", ...(body === undefined ? {} : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }) }));
}

export function parseTerminationRate(raw: string): number | null {
  if (!/^(?:0|[1-9]\d{0,2})(?:\.\d{1,2})?$/.test(raw.trim())) return null;
  const [whole, fraction = ""] = raw.trim().split(".");
  const bps = Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
  return bps <= 10000 ? bps : null;
}
/** Optional partial awards use exact ledger units; full-cap/waive buttons do
 * not round the authoritative amounts through JS numbers or dollar cents. */
export function parseTerminationAward(raw: string): string | null {
  if (!/^(?:0|[1-9]\d{0,10})(?:\.\d{1,6})?$/.test(raw.trim())) return null;
  const [whole, fraction = ""] = raw.trim().split(".");
  const micros = BigInt(whole) * 1000000n + BigInt(fraction.padEnd(6, "0"));
  const units = (micros * 8640000n + 500000n) / 1000000n;
  return units <= 9223372036854775807n ? units.toString() : null;
}

export function terminationCanDispute(t: ClientTermination, now = Date.now()) {
  return t.status !== "finance_pending" && !t.disputeUsed && BigInt(t.compensationCapUnits) > 0n && BigInt(t.creditedUnits) === 0n
    && (!t.cleanupConfirmedAt || BigInt(t.compensationUnits) > 0n)
    && (!t.unlockAt || Date.parse(t.unlockAt) > now);
}
export function terminationCanDecide(t: ClientTermination, now = Date.now()) {
  const remaining = !t.cleanupConfirmedAt ? t.compensationCapUnits
    : BigInt(t.compensationUnits) > 0n ? t.compensationUnits : t.heldUnits;
  return !terminationTimeoutDue(t, now) && t.status !== "finance_pending" && BigInt(remaining) > 0n && BigInt(t.creditedUnits) === 0n
    && (!t.unlockAt || t.status === "disputed" || Date.parse(t.unlockAt) > now);
}
export function terminationRemainingCompensation(t: ClientTermination) {
  return !t.cleanupConfirmedAt || t.status === "finance_pending" ? t.compensationCapUnits : BigInt(t.compensationUnits) > 0n ? t.compensationUnits : t.heldUnits;
}
export function terminationTimeoutDue(t: ClientTermination, now = Date.now()) {
  return t.responsibility === "undetermined" && t.disputeUsed && !t.providerRespondedAt
    && !!t.disputeDeadlineAt && Date.parse(t.disputeDeadlineAt) <= now;
}
export function terminationCanWithdraw(t: ClientTermination, now = Date.now()) {
  return t.responsibility === "undetermined" && t.disputeUsed && terminationCanDecide(t, now);
}
export function terminationCanRespond(t: ClientTermination, now = Date.now()) {
  return t.responsibility === "undetermined" && t.disputeUsed && !t.providerRespondedAt
    && !terminationTimeoutDue(t, now) && BigInt(terminationRemainingCompensation(t)) > 0n;
}
export function terminationExplanationValid(raw: string, required = false) {
  return (!required || [...raw.trim()].length >= 3) && new TextEncoder().encode(raw).length <= 2000;
}
