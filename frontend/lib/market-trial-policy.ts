import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

export type MarketTrialPolicy = { version: number; clientHours: number; shareHours: number; shareTokens: number; canEdit?: boolean };
export function validateTrialPolicy(value: MarketTrialPolicy) {
  if (!value || !Number.isSafeInteger(value.version) || value.version < 1
    || ![value.clientHours, value.shareHours].every((n) => Number.isInteger(n) && n >= 0 && n <= 720)
    || !Number.isSafeInteger(value.shareTokens) || value.shareTokens < 0) throw new Error("Market trial policy unavailable; reload to retry.");
  return value;
}
export async function getMarketTrialPolicy(admin = false) {
  return validateTrialPolicy(await parseJson<MarketTrialPolicy>(await authFetch(admin ? "/v1/admin/market/trial-policy" : "/v1/market-trials/policy", { cache: "no-store" })));
}
export async function saveMarketTrialPolicy(value: MarketTrialPolicy) {
  return validateTrialPolicy(await parseJson<MarketTrialPolicy>(await authFetch("/v1/admin/market/trial-policy", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ expectedVersion: value.version, clientHours: value.clientHours, shareHours: value.shareHours, shareTokens: value.shareTokens }) })));
}
