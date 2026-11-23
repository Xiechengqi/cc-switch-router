import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

/**
 * Wallet confirmation status. Risky wallet operations are confirmed with the
 * login password; a newly set password waits before it can confirm money that
 * moves immediately.
 */
export type WalletSecurity = {
  hasPassword: boolean;
  passwordSetAt: string | null;
  /** Set while a fresh password cannot yet confirm US$500+ payments. */
  immediateUsableAt: string | null;
  canManageRiskPolicy: boolean;
  largeAmountMinor: number;
  passwordCooldownHours: number;
};
export type FinancialRiskPolicy = { version: number; topupUserDayMinor: number; topupChannelDayMinor: number; topupGlobalDayMinor: number; topupGlobalHourMinor: number; incomingUserDayMinor: number; chainVerificationRequired: boolean; topupCircuitOpen: boolean };

const isTime = (value: unknown) => value === null || (typeof value === "string" && Number.isFinite(Date.parse(value)));

export async function getWalletSecurity(): Promise<WalletSecurity> {
  const value = await parseJson<WalletSecurity>(await authFetch("/v1/market-wallet/security", { cache: "no-store" }));
  if (!value || typeof value.hasPassword !== "boolean" || typeof value.canManageRiskPolicy !== "boolean"
    || !isTime(value.passwordSetAt) || !isTime(value.immediateUsableAt)
    || ![value.largeAmountMinor, value.passwordCooldownHours].every(Number.isSafeInteger)) {
    throw new Error("Wallet security unavailable; reload.");
  }
  return value;
}

function validatePolicy(p: FinancialRiskPolicy) {
  if (!p || !Number.isSafeInteger(p.version) || p.version < 1 || [p.topupUserDayMinor, p.topupChannelDayMinor, p.topupGlobalDayMinor, p.topupGlobalHourMinor, p.incomingUserDayMinor].some(v => !Number.isSafeInteger(v) || v < 100 || v > 1_000_000_000) || typeof p.chainVerificationRequired !== "boolean" || typeof p.topupCircuitOpen !== "boolean") throw new Error("Financial risk policy unavailable; reload.");
  return p;
}
export async function getFinancialRiskPolicy() { return validatePolicy(await parseJson<FinancialRiskPolicy>(await authFetch("/v1/admin/market-finance/risk-policy", { cache: "no-store" }))); }
export async function saveFinancialRiskPolicy(policy: FinancialRiskPolicy) { return validatePolicy(await parseJson<FinancialRiskPolicy>(await authFetch("/v1/admin/market-finance/risk-policy", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(policy) }))); }
