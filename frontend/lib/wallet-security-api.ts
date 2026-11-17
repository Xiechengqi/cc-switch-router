import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { passkeyCreationOptions, serializePasskey } from "@/lib/financial-auth";
export type WalletPasskey = { id: string; usableAfter: string; createdAt: string; revokedAt: string | null };
export type WalletSecurity = { sessionReference: string; canManageRiskPolicy: boolean; passkeys: WalletPasskey[]; coolingHours: number; largeTransferMinor: number; firstRecipientRequiresPasskey: boolean };
export type FinancialRiskPolicy = { version: number; topupUserDayMinor: number; topupChannelDayMinor: number; topupGlobalDayMinor: number; topupGlobalHourMinor: number; incomingUserDayMinor: number; chainVerificationRequired: boolean; topupCircuitOpen: boolean };
const base = "/v1/market-wallet/security";
export async function getWalletSecurity() {
  const value = await parseJson<WalletSecurity>(await authFetch(base, { cache: "no-store" }));
  if (!value || typeof value.sessionReference !== "string" || typeof value.canManageRiskPolicy !== "boolean" || !Array.isArray(value.passkeys) || value.passkeys.some(p => !p || !/^[a-f0-9-]{36}$/.test(p.id) || !Number.isFinite(Date.parse(p.usableAfter)) || !Number.isFinite(Date.parse(p.createdAt)) || (p.revokedAt !== null && !Number.isFinite(Date.parse(p.revokedAt))))) throw new Error("Wallet security unavailable; reload.");
  return value;
}
export async function registerWalletPasskey() {
  if (!globalThis.PublicKeyCredential || !navigator.credentials) throw new Error("Use a browser that supports Passkeys.");
  const context = (await import("@/lib/auth")).readAuthState();
  const options = await parseJson<{ challengeId: string; options: { publicKey: Record<string, unknown> } }>(await authFetch(`${base}/passkeys/registration`, { method: "POST" }, context));
  const credential = await navigator.credentials.create(passkeyCreationOptions(options.options)) as PublicKeyCredential | null;
  if (!credential) throw new Error("Passkey registration cancelled.");
  return parseJson<{ id: string; usableAfter: string }>(await authFetch(`${base}/passkeys/registration/finish`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ challengeId: options.challengeId, credential: serializePasskey(credential) }) }, context));
}
export async function revokeWalletPasskey(id: string) { return parseJson(await authFetch(`${base}/passkeys/${encodeURIComponent(id)}/revoke`, { method: "POST" })); }
function validatePolicy(p: FinancialRiskPolicy) {
  if (!p || !Number.isSafeInteger(p.version) || p.version < 1 || [p.topupUserDayMinor, p.topupChannelDayMinor, p.topupGlobalDayMinor, p.topupGlobalHourMinor, p.incomingUserDayMinor].some(v => !Number.isSafeInteger(v) || v < 100 || v > 1_000_000_000) || typeof p.chainVerificationRequired !== "boolean" || typeof p.topupCircuitOpen !== "boolean") throw new Error("Financial risk policy unavailable; reload.");
  return p;
}
export async function getFinancialRiskPolicy() { return validatePolicy(await parseJson<FinancialRiskPolicy>(await authFetch("/v1/admin/market-finance/risk-policy", { cache: "no-store" }))); }
export async function saveFinancialRiskPolicy(policy: FinancialRiskPolicy) { return validatePolicy(await parseJson<FinancialRiskPolicy>(await authFetch("/v1/admin/market-finance/risk-policy", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(policy) }))); }
