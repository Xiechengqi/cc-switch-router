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
