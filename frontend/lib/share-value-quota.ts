import { authFetch, readAuthState } from "./auth";
import { parseJson } from "./api";
import type { ShareUserPolicy, ShareUserGrant } from "./types";

export function usdLimitToMicros(input: string): string | undefined {
  if (!/^(0|[1-9]\d*)(?:\.\d{1,6})?$/.test(input)) return undefined;
  const [whole, fraction = ""] = input.split(".");
  const amount = BigInt(whole) * 1_000_000n + BigInt(fraction.padEnd(6, "0"));
  return amount > 0n && amount <= 1_000_000_000_000_000n ? String(amount) : undefined;
}
export function microsToUsdInput(input: string): string {
  if (!/^(0|[1-9]\d*)$/.test(input)) return "";
  const n = BigInt(input);
  const fraction = String(n % 1_000_000n).padStart(6, "0").replace(/0+$/, "");
  return String(n / 1_000_000n) + (fraction ? "." + fraction : "");
}
export function formatQuotaPicos(input: string | undefined): string {
  if (!input || !/^(0|[1-9]\d*)$/.test(input)) return "—";
  const n = BigInt(input), scale = 1_000_000_000_000n;
  if (n > 0n && n < 1_000_000n) return "< US$0.000001";
  const fraction = String(n % scale / 1_000_000n).padStart(6, "0").replace(/0+$/, "").padEnd(2, "0");
  return "US$" + String(n / scale) + "." + fraction;
}
export function valueQuotaAvailability(usedPicos?: string, limitMicros?: string) {
  if (!usedPicos || !limitMicros || !/^(0|[1-9]\d{0,38})$/.test(usedPicos) || !/^[1-9]\d{0,15}$/.test(limitMicros)) return undefined;
  const used = BigInt(usedPicos), limit = BigInt(limitMicros) * 1_000_000n;
  return {remainingPicos: (used >= limit ? 0n : limit-used).toString(), exhausted: used >= limit, nearLimit: used*5n >= limit*4n};
}
export type ValueQuotaStatus = {
  email: string; policyEpoch: string; windowId: string; windowStartsAtMs: number; resetsAtMs?: number | null;
  meteredFromMs: number; activePricingRevision: string; usagePricingRevisionsDigest: string; usageWatermark: number;
  limitUsdMicros?: string | null; usedPicos: string; pendingRiskPicos: string; meteringStatus: string; syncedAtMs: number;
  observedPicos: string; manualOffsetPicos: string;
};
export type ValueQuotaPreview = {
  ready: boolean; reason?: string | null; activePricingRevision?: string | null; configRevision: number;
  capabilityReady?: boolean;
  checks?: ValueReadinessCheck[];
  pricing?: {publishedRevision: string | null; effectiveFromMs: number | null; acknowledgedAtMs: number | null; version: number};
  rows: Array<{email: string; usedPicos?: string | null; effectiveFromMs: number; meteredFromMs: number; reason?: string | null}>;
};
export type ValueReadinessCheck = {code: string; ready: boolean; state?: string; app?: string; profileId?: string | null; driverId?: string | null};
export function valueCapabilityReady(preview: ValueQuotaPreview | undefined) {
  return preview?.capabilityReady === true && Array.isArray(preview.checks)
    && ["router_protocol", "sqlite_authority", "metering_storage", "acknowledged_tariff", "text_surface"]
      .every(code => preview.checks!.some(check => check.code === code))
    && preview.checks.length > 0 && preview.checks.every(check => check.ready === true);
}
export type ValueReadinessError = "conflict"|"offline"|"timeout"|"upgrade"|"controlAuth"|"invalidResponse"|"failed";
export function valueReadinessError(error: unknown):ValueReadinessError {
  const candidate = error as {code?: string; status?: number; details?: {readinessCode?: string}};
  if (candidate?.code === "quota_revision_conflict" || candidate?.status === 409) return "conflict";
  const code = candidate?.details?.readinessCode ?? candidate?.code;
  const codes = {quota_server_offline:"offline",quota_server_timeout:"timeout",quota_server_upgrade_required:"upgrade",
    quota_control_unauthorized:"controlAuth",quota_server_invalid_response:"invalidResponse"} as const;
  return codes[code as keyof typeof codes] ?? "failed";
}
export function compatibleValueStatus(items: ValueQuotaStatus[] | undefined, grant: ShareUserGrant | undefined) {
  if (!grant?.active || !grant.valueMetering) return undefined;
  return items?.find(item => item.email === grant.email.trim().toLowerCase()
    && item.policyEpoch === grant.valueMetering!.quotaPolicyEpoch
    && (item.limitUsdMicros ?? null) === (grant.policy.valueQuota?.limitUsdMicros ?? null));
}
export function mergeValueStatuses(previous: ValueQuotaStatus[], incoming: ValueQuotaStatus[]) {
  return incoming.map(item => {
    const old = previous.find(p => p.email === item.email && p.policyEpoch === item.policyEpoch);
    return old && (old.windowStartsAtMs > item.windowStartsAtMs
      || (old.windowId === item.windowId && old.usageWatermark > item.usageWatermark)) ? old : item;
  });
}
export function valueStatusTextKey(status: ValueQuotaStatus | undefined, now = Date.now()) {
  if (!status) return "dashboard.userLimit.value.unsynced";
  if (now - status.syncedAtMs > 90_000) return "dashboard.userLimit.value.stale";
  const states = ["observed", "conservative", "pending", "unavailable", "scheduled"] as const;
  type State = typeof states[number];
  const code: State = states.includes(status.meteringStatus as State) ? status.meteringStatus as State : "unavailable";
  return `dashboard.userLimit.value.state.${code}` as const;
}
export async function previewValueQuota(shareId: string, expectedConfigRevision: number, policies: Record<string, ShareUserPolicy> = {}, signal?: AbortSignal) {
  const result = await parseJson<ValueQuotaPreview>(await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/value-quota-preview`, {
    method: "POST", headers: {"Content-Type":"application/json"}, body: JSON.stringify({expectedConfigRevision,policies}),
    signal, cache: "no-store",
  }, readAuthState()));
  if (!result || typeof result.ready !== "boolean" || result.configRevision !== expectedConfigRevision || !Array.isArray(result.rows)
    || (result.checks != null && (!Array.isArray(result.checks) || result.checks.some(c => !c || typeof c.code !== "string" || typeof c.ready !== "boolean")))) {
    throw {code:"quota_server_invalid_response"};
  }
  return result;
}
export async function getValueQuotaStatus(shareId: string, signal?: AbortSignal) {
  return parseJson<{items: ValueQuotaStatus[]}>(await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/value-quota-status`, {signal}));
}
export async function getMyValueQuotaStatus(shareId: string, signal?: AbortSignal) {
  return parseJson<{items: ValueQuotaStatus[]}>(await authFetch(`/v1/me/share-value-quotas?shareId=${encodeURIComponent(shareId)}`, {signal}));
}
export type ValueUsageEdit = {
  action: "set" | "clear"; targetPicos?: string; expectedGrantRevision: number; windowId: string;
  expectedWatermark: number; operationId: string; reason: string;
};
export async function editValueUsage(shareId: string, expectedConfigRevision: number, edits: Record<string, ValueUsageEdit>) {
  return parseJson<{items: ValueQuotaStatus[]}>(await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/value-usage-edits`, {
    method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({expectedConfigRevision,edits}),
  }));
}
export function usdUsageToPicos(input: string): string | undefined {
  if (!/^(0|[1-9]\d*)(?:\.\d{1,12})?$/.test(input)) return undefined;
  const [whole,fraction=""] = input.split(".");
  const picos = BigInt(whole)*1_000_000_000_000n+BigInt(fraction.padEnd(12,"0"));
  return picos <= 1_000_000_000_000_000_000_000n ? String(picos) : undefined;
}
