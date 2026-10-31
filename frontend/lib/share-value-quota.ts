import { authFetch, readAuthState } from "./auth";
import { parseJson } from "./api";
import type { ShareUserPolicy, ShareUserGrant } from "./types";
import type { valueQuotaMessagesEn } from "./share-value-quota-messages";
import { sharePolicyFingerprint } from "./share-settings";

export type ValueQuotaMessageKey = keyof typeof valueQuotaMessagesEn;
type ValueQuotaT = (key: ValueQuotaMessageKey, values?: Record<string, string | number>) => string;

/** 1,000,000,000 USD, the domain upper bound shared with the Rust validator. */
export const USD_LIMIT_MAX_MICROS = 1_000_000_000_000_000n;
export type UsdAmountError = "empty" | "format" | "too_many_decimals" | "too_small" | "too_large";
export type UsdAmountParse = { ok: true; micros: string } | { ok: false; error: UsdAmountError };

/**
 * Lenient for people, exact for the wire: accepts `$10.00`, `US$ 1,250.5`,
 * ` 10`, `.5`, `05` and `10.`, but never a sign, an exponent, misgrouped
 * thousands separators or more than six decimals. BigInt only.
 */
export function parseUsdLimit(input: string): UsdAmountParse {
  let text = input.trim();
  if (!text) return { ok: false, error: "empty" };
  const symbol = /^(?:US\$|\$)\s*/i.exec(text);
  if (symbol) text = text.slice(symbol[0].length);
  if (!text) return { ok: false, error: "empty" };
  const match = /^(\d{1,3}(?:,\d{3})+|\d*)(?:\.(\d*))?$/.exec(text);
  if (!match || (!match[1] && !match[2])) return { ok: false, error: "format" };
  const fraction = match[2] ?? "";
  if (fraction.length > 6) return { ok: false, error: "too_many_decimals" };
  const whole = match[1].replaceAll(",", "") || "0";
  const micros = BigInt(whole) * 1_000_000n + BigInt(fraction.padEnd(6, "0"));
  if (micros <= 0n) return { ok: false, error: "too_small" };
  if (micros > USD_LIMIT_MAX_MICROS) return { ok: false, error: "too_large" };
  return { ok: true, micros: String(micros) };
}
export function usdLimitToMicros(input: string): string | undefined {
  const parsed = parseUsdLimit(input);
  return parsed.ok ? parsed.micros : undefined;
}
export function usdAmountErrorKey(error: UsdAmountError): ValueQuotaMessageKey {
  return `dashboard.userLimit.value.amountError.${error}`;
}
export function microsToUsdInput(input: string): string {
  if (!/^(0|[1-9]\d*)$/.test(input)) return "";
  const n = BigInt(input);
  const fraction = String(n % 1_000_000n).padStart(6, "0").replace(/0+$/, "");
  return String(n / 1_000_000n) + (fraction ? "." + fraction : "");
}
const grouped = (whole: bigint) => whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
/** Limits read as `US$10`, `US$10.50`, `US$1,000`. */
export function formatUsdLimit(micros: string | null | undefined): string {
  if (!micros || !/^(0|[1-9]\d*)$/.test(micros)) return "—";
  const n = BigInt(micros);
  const fraction = String(n % 1_000_000n).padStart(6, "0").replace(/0+$/, "");
  return "US$" + grouped(n / 1_000_000n) + (fraction ? "." + fraction.padEnd(2, "0") : "");
}
/** Usage keeps at least cents and never rounds a tiny positive amount to zero. */
export function formatQuotaPicos(input: string | null | undefined): string {
  if (!input || !/^(0|[1-9]\d*)$/.test(input)) return "—";
  const n = BigInt(input), scale = 1_000_000_000_000n;
  if (n > 0n && n < 1_000_000n) return "< US$0.000001";
  const fraction = String(n % scale / 1_000_000n).padStart(6, "0").replace(/0+$/, "").padEnd(2, "0");
  return "US$" + grouped(n / scale) + "." + fraction;
}
export function valueQuotaAvailability(usedPicos?: string | null, limitMicros?: string | null) {
  if (!usedPicos || !limitMicros || !/^(0|[1-9]\d{0,38})$/.test(usedPicos) || !/^[1-9]\d{0,15}$/.test(limitMicros)) return undefined;
  const used = BigInt(usedPicos), limit = BigInt(limitMicros) * 1_000_000n;
  return {remainingPicos: (used >= limit ? 0n : limit-used).toString(), exhausted: used >= limit, nearLimit: used*5n >= limit*4n,
    /** Basis points, capped at 100%, for a progress bar. */
    basisPoints: Number(used >= limit ? 10_000n : used * 10_000n / limit)};
}
/** Dates in USD quota lines follow the app locale, not the browser default. */
export function formatQuotaTime(ms: number | null | undefined, locale: string, style: "short" | "full" = "full") {
  const date = new Date(ms ?? Number.NaN);
  if (!Number.isFinite(date.getTime())) return "—";
  const tag = locale === "zh-CN" ? "zh-CN" : "en-US";
  return style === "short"
    ? date.toLocaleString(tag, {month: "short", day: "numeric", hour: "2-digit", minute: "2-digit"})
    : date.toLocaleString(tag);
}
export type ValueMeteringStatus = "observed" | "conservative" | "pending" | "blocked" | "unavailable" | "scheduled";
export type ValueQuotaStatus = {
  email: string; policyEpoch: string; windowId: string; windowStartsAtMs: number; resetsAtMs?: number | null;
  meteredFromMs: number; activePricingRevision: string; usagePricingRevisionsDigest: string; usageWatermark: number;
  limitUsdMicros?: string | null; usedPicos: string; pendingRiskPicos: string; meteringStatus: string; syncedAtMs: number;
  observedPicos: string; manualOffsetPicos: string;
  /** Present on self-contained rows while a USD policy waits for its next window. */
  pendingLimitUsdMicros?: string | null; pendingEffectiveFromMs?: number | null;
  tokenPeriod?: string | null; pendingTokenPeriod?: string | null;
  /** Why the window is not plainly observed, e.g. on `unavailable` rows. */
  reasonCode?: string | null;
};
/** A per-user failure returned in place of a status row (e.g. by a correction). */
export type ValueStatusRowError = {email: string; code: string};

/**
 * Status rows and per-row errors, apart. An error row (`{email, error:{code}}`)
 * must never replace the last good status of that user.
 */
export function splitValueStatusRows(items: unknown): {items: ValueQuotaStatus[]; errors: ValueStatusRowError[]} {
  const errors: ValueStatusRowError[] = [];
  if (Array.isArray(items)) for (const item of items) {
    const raw = item as Record<string, unknown> | null;
    if (!raw || typeof raw !== "object" || typeof raw.email !== "string" || raw.error == null) continue;
    const error = raw.error as {code?: unknown};
    errors.push({email: raw.email.trim().toLowerCase(), code: typeof error === "object" && typeof error.code === "string" ? error.code : "failed"});
  }
  return {items: normalizeValueStatuses(items), errors};
}
/** Accept `syncedAt` as an alias and drop rows that cannot be attributed or carry an error. */
export function normalizeValueStatuses(items: unknown): ValueQuotaStatus[] {
  if (!Array.isArray(items)) return [];
  return items.flatMap(item => {
    if (!item || typeof item !== "object") return [];
    const raw = item as Record<string, unknown>;
    if (typeof raw.email !== "string" || raw.error != null) return [];
    const synced = typeof raw.syncedAtMs === "number" ? raw.syncedAtMs : typeof raw.syncedAt === "number" ? raw.syncedAt
      : typeof raw.syncedAt === "string" && Number.isFinite(Date.parse(raw.syncedAt)) ? Date.parse(raw.syncedAt) : 0;
    return [{...(raw as unknown as ValueQuotaStatus), email: raw.email.trim().toLowerCase(), syncedAtMs: synced}];
  });
}

export const VALUE_PREVIEW_REASON_CODES = ["ready", "scheduled_next_window", "metered_from_now", "unverified_surface",
  "pricing_unavailable", "storage_unavailable", "router_upgrade_required", "blocked_needs_reconcile", "invalid_policy"] as const;
export type ValuePreviewReasonCode = typeof VALUE_PREVIEW_REASON_CODES[number];
const BLOCKING_REASONS: ReadonlySet<string> = new Set(VALUE_PREVIEW_REASON_CODES.slice(3));
export function knownReasonCode(code: unknown): ValuePreviewReasonCode | undefined {
  return VALUE_PREVIEW_REASON_CODES.find(known => known === code);
}
export type ValuePreviewRow = {email: string; usedPicos?: string | null; effectiveFromMs: number; meteredFromMs: number;
  reason?: string | null; reasonCode?: string | null; ready?: boolean | null};
/** A requested policy the Server rejected outright (bad amount, period, user…). */
export type ValuePreviewInvalidRow = {email: string; reasonCode?: string | null; reason?: string | null};
export type ValueQuotaPreview = {
  ready: boolean;
  invalidRows?: ValuePreviewInvalidRow[]; reason?: string | null; reasonCode?: string | null; activePricingRevision?: string | null; configRevision: number;
  capabilityReady?: boolean;
  checks?: ValueReadinessCheck[];
  pricing?: {publishedRevision: string | null; effectiveFromMs: number | null; acknowledgedAtMs: number | null; version: number};
  rows: ValuePreviewRow[];
};
export type ValueRowOutcome = "scheduled" | "meteredFromNow" | "ready" | "blocked";
/** Old Servers send no reason code; fall back to the window times they always sent. */
export function valuePreviewRowOutcome(row: ValuePreviewRow, now = Date.now()): ValueRowOutcome {
  const code = row.reasonCode ?? undefined;
  if (row.ready === false || (code && BLOCKING_REASONS.has(code))) return "blocked";
  if (code === "scheduled_next_window") return "scheduled";
  if (code === "metered_from_now") return "meteredFromNow";
  if (code === "ready") return "ready";
  if (code) return "blocked";
  return row.effectiveFromMs > now + 5_000 ? "scheduled" : "ready";
}
/** A scheduled start or carried-over usage must be seen before it is applied. */
export function valuePreviewNeedsAcknowledgement(preview: ValueQuotaPreview | undefined, now = Date.now()) {
  return !!preview?.rows.some(row => {
    const outcome = valuePreviewRowOutcome(row, now);
    return outcome === "scheduled" || (outcome !== "blocked" && !!row.usedPicos && /^[1-9]\d*$/.test(row.usedPicos));
  });
}
export type ValueQuotaMessage = {key: ValueQuotaMessageKey; values?: Record<string, string | number>} | {raw: string};
function reasonMessage(code: string | null | undefined, reason: string | null | undefined): ValueQuotaMessage {
  const known = knownReasonCode(code);
  if (known) return {key: `dashboard.userLimit.value.reason.${known}`};
  return reason?.trim() ? {raw: reason.trim()} : {key: "dashboard.userLimit.value.notReady"};
}
/** The first reason that keeps this preview from being applied, translated per row when possible. */
export function valuePreviewBlocker(preview: ValueQuotaPreview, now = Date.now()): {email?: string; message: ValueQuotaMessage} | undefined {
  const invalid = preview.invalidRows?.[0];
  if (invalid) return {email: invalid.email, message: reasonMessage(invalid.reasonCode ?? "invalid_policy", invalid.reason)};
  const row = preview.rows.find(item => valuePreviewRowOutcome(item, now) === "blocked");
  if (row) return {email: row.email, message: reasonMessage(row.reasonCode, row.reason)};
  if (!preview.ready) return {message: preview.capabilityReady === false && !preview.reasonCode
    ? {key: "dashboard.userLimit.value.notReady"} : reasonMessage(preview.reasonCode, preview.reason)};
  return undefined;
}
/** Why a status row is not plainly observed; nothing for confirmed usage. */
export function valueStatusReason(status: ValueQuotaStatus | undefined): ValueQuotaMessage | undefined {
  const code = status?.reasonCode?.trim();
  if (!status || !code || valueMeteringState(status) === "observed") return undefined;
  if (knownReasonCode(code)) return {key: `dashboard.userLimit.value.reason.${knownReasonCode(code)!}`};
  if (code === "price_unavailable") return {key: "dashboard.userLimit.value.reason.pricing_unavailable"};
  return {raw: code};
}
export function renderValueQuotaMessage(message: ValueQuotaMessage, t: ValueQuotaT) {
  return "raw" in message ? message.raw : t(message.key, message.values);
}

export type ValueReadinessCheck = {code: string; ready: boolean; state?: string; app?: string; profileId?: string | null; driverId?: string | null};
export function valueCapabilityReady(preview: ValueQuotaPreview | undefined) {
  return preview?.capabilityReady === true && Array.isArray(preview.checks)
    && ["router_protocol", "sqlite_authority", "metering_storage", "acknowledged_tariff", "text_surface"]
      .every(code => preview.checks!.some(check => check.code === code))
    && preview.checks.length > 0 && preview.checks.every(check => check.ready === true);
}
/** A real Error, so callers never render "[object Object]". */
export class ValueQuotaClientError extends Error {
  readonly code: string;
  constructor(code: string) { super(code); this.name = "ValueQuotaClientError"; this.code = code; }
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
/** Every quota request failure becomes one translated sentence. */
export function valueQuotaErrorMessage(error: unknown): ValueQuotaMessage {
  const candidate = error as {code?: string; status?: number; details?: {readinessCode?: string; reasonCode?: string}};
  const code = candidate?.details?.reasonCode ?? candidate?.code;
  if (code === "quota_target_invalid") return {key: "dashboard.userLimit.value.error.targetInvalid"};
  if (code === "price_unavailable") return {key: "dashboard.userLimit.value.reason.pricing_unavailable"};
  if (code === "quota_unavailable" && !candidate?.details?.readinessCode) return {key: "dashboard.userLimit.value.error.unavailable"};
  if (code === "value_quota_not_active") return {key: "dashboard.userLimit.value.error.notActive"};
  const known = knownReasonCode(code);
  if (known) return {key: `dashboard.userLimit.value.reason.${known}`};
  return {key: `dashboard.userLimit.value.readiness.${valueReadinessError(error)}`};
}
export function compatibleValueStatus(items: ValueQuotaStatus[] | undefined, grant: ShareUserGrant | undefined) {
  if (!grant?.active || !grant.valueMetering) return undefined;
  return items?.find(item => item.email === grant.email.trim().toLowerCase()
    && item.policyEpoch === grant.valueMetering!.quotaPolicyEpoch
    && (item.limitUsdMicros ?? null) === (grant.policy.valueQuota?.limitUsdMicros ?? null));
}
export type ValueQuotaView = {
  /** The USD limit enforced now, with its authoritative status when synced. */
  enforced?: {limitMicros: string; status?: ValueQuotaStatus};
  /** A USD policy that starts later; the current policy stays enforced until then. */
  scheduled?: {limitMicros: string; effectiveFromMs: number};
};
const usdMicros = (value: unknown): value is string => typeof value === "string" && /^[1-9]\d*$/.test(value);
const scheduledPolicy = (grant: ShareUserGrant) => {
  const pending = grant.pendingValueQuota;
  return pending?.policy?.valueQuota && usdMicros(pending.policy.valueQuota.limitUsdMicros) && typeof pending.effectiveFromMs === "number"
    ? pending : undefined;
};
/**
 * The USD limit in force while a USD policy is scheduled. Server keeps sending
 * the real window status (with the pending fields alongside) as long as a USD
 * policy is enforced; without that row, a grant whose policy differs from the
 * schedule still names the enforced limit.
 */
function enforcedBeforeSchedule(grant: ShareUserGrant, pending: NonNullable<ShareUserGrant["pendingValueQuota"]>, statuses?: ValueQuotaStatus[]) {
  const key = grant.email.trim().toLowerCase();
  const live = statuses?.find(item => item.email === key && item.meteringStatus !== "scheduled" && usdMicros(item.limitUsdMicros)
    && (!grant.valueMetering || item.policyEpoch === grant.valueMetering.quotaPolicyEpoch));
  if (live) return {limitMicros: live.limitUsdMicros!, status: live};
  const quota = grant.policy.valueQuota;
  if (quota && usdMicros(quota.limitUsdMicros) && sharePolicyFingerprint(grant.policy) !== sharePolicyFingerprint(pending.policy)) {
    return {limitMicros: quota.limitUsdMicros};
  }
  return undefined;
}
/** What the grant itself says, for a row the status feed has not reported yet. */
function viewFromGrant(grant: ShareUserGrant, statuses?: ValueQuotaStatus[]): ValueQuotaView | undefined {
  const pending = scheduledPolicy(grant);
  if (pending) {
    const view: ValueQuotaView = {scheduled: {limitMicros: pending.policy.valueQuota!.limitUsdMicros, effectiveFromMs: pending.effectiveFromMs}};
    const enforced = enforcedBeforeSchedule(grant, pending, statuses);
    if (enforced) view.enforced = enforced;
    return view;
  }
  if (!grant.policy.valueQuota || !usdMicros(grant.policy.valueQuota.limitUsdMicros)) return undefined;
  return {enforced: {limitMicros: grant.policy.valueQuota.limitUsdMicros, status: compatibleValueStatus(statuses, grant)}};
}
/**
 * Owners read policy from grants and attach matching status. A grantee's rows
 * from `/v1/me/share-value-quotas` are self-contained and read alone; until
 * the first row arrives, the grantee's own grant still names the USD limit,
 * so a USD user never reads as "0 / ∞".
 */
export function valueQuotaView(grant: ShareUserGrant | undefined, email: string, statuses: ValueQuotaStatus[] | undefined,
  selfContained = false): ValueQuotaView | undefined {
  const key = email.trim().toLowerCase();
  if (selfContained) {
    const status = statuses?.find(item => item.email === key);
    const view: ValueQuotaView = {};
    if (status && usdMicros(status.limitUsdMicros)) view.enforced = {limitMicros: status.limitUsdMicros, status};
    if (status && usdMicros(status.pendingLimitUsdMicros) && typeof status.pendingEffectiveFromMs === "number") {
      view.scheduled = {limitMicros: status.pendingLimitUsdMicros, effectiveFromMs: status.pendingEffectiveFromMs};
    }
    if (view.enforced || view.scheduled) return view;
    if (!grant?.policy || grant.active === false) return undefined;
    const fallback = viewFromGrant(grant);
    if (fallback?.enforced) fallback.enforced = {limitMicros: fallback.enforced.limitMicros};
    return fallback;
  }
  if (!grant) return undefined;
  return viewFromGrant(grant, statuses);
}
export function mergeValueStatuses(previous: ValueQuotaStatus[], incoming: ValueQuotaStatus[]) {
  return incoming.map(item => {
    const old = previous.find(p => p.email === item.email && p.policyEpoch === item.policyEpoch);
    return old && (old.windowStartsAtMs > item.windowStartsAtMs
      || (old.windowId === item.windowId && old.usageWatermark > item.usageWatermark)) ? old : item;
  });
}
/** A mutation's rows replace only those users; everyone else keeps their row. */
export function upsertValueStatuses(previous: ValueQuotaStatus[], incoming: ValueQuotaStatus[]) {
  const emails = new Set(incoming.map(item => item.email));
  return [...previous.filter(item => !emails.has(item.email)), ...mergeValueStatuses(previous, incoming)];
}
export const VALUE_STATUS_STALE_MS = 180_000;
export function valueStatusTextKey(status: ValueQuotaStatus | undefined, now = Date.now()) {
  if (!status) return "dashboard.userLimit.value.unsynced";
  if (now - status.syncedAtMs > VALUE_STATUS_STALE_MS) return "dashboard.userLimit.value.stale";
  return `dashboard.userLimit.value.state.${valueMeteringState(status)}` as const;
}
export function valueMeteringState(status: ValueQuotaStatus): ValueMeteringStatus {
  const states = ["observed", "conservative", "pending", "blocked", "unavailable", "scheduled"] as const;
  return states.find(state => state === status.meteringStatus) ?? "unavailable";
}
export async function previewValueQuota(shareId: string, expectedConfigRevision: number, policies: Record<string, ShareUserPolicy> = {}, signal?: AbortSignal) {
  const result = await parseJson<ValueQuotaPreview>(await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/value-quota-preview`, {
    method: "POST", headers: {"Content-Type":"application/json"}, body: JSON.stringify({expectedConfigRevision,policies}),
    signal, cache: "no-store",
  }, readAuthState()));
  if (!result || typeof result.ready !== "boolean" || result.configRevision !== expectedConfigRevision || !Array.isArray(result.rows)
    || result.rows.some(row => !row || typeof row.email !== "string")
    || (result.invalidRows != null && (!Array.isArray(result.invalidRows) || result.invalidRows.some(row => !row || typeof row.email !== "string")))
    || (result.checks != null && (!Array.isArray(result.checks) || result.checks.some(c => !c || typeof c.code !== "string" || typeof c.ready !== "boolean")))) {
    throw new ValueQuotaClientError("quota_server_invalid_response");
  }
  return result;
}
export async function getValueQuotaStatus(shareId: string, signal?: AbortSignal) {
  const result = await parseJson<{items: unknown}>(await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/value-quota-status`, {signal}));
  return {items: normalizeValueStatuses(result?.items)};
}
export async function getMyValueQuotaStatus(shareId: string, signal?: AbortSignal) {
  const result = await parseJson<{items: unknown}>(await authFetch(`/v1/me/share-value-quotas?shareId=${encodeURIComponent(shareId)}`, {signal}));
  return {items: normalizeValueStatuses(result?.items)};
}
export type ValueUsageEdit = {
  action: "set" | "clear"; targetPicos?: string; expectedGrantRevision: number; windowId: string;
  expectedWatermark: number; operationId: string; reason: string;
};
export async function editValueUsage(shareId: string, expectedConfigRevision: number, edits: Record<string, ValueUsageEdit>) {
  const result = await parseJson<{items: unknown}>(await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/value-usage-edits`, {
    method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({expectedConfigRevision,edits}),
  }));
  return splitValueStatusRows(result?.items);
}
/** Usage targets share the limit parser's leniency but keep pico precision. */
export function usdUsageToPicos(input: string): string | undefined {
  let text = input.trim();
  const symbol = /^(?:US\$|\$)\s*/i.exec(text);
  if (symbol) text = text.slice(symbol[0].length);
  const match = /^(\d{1,3}(?:,\d{3})+|\d*)(?:\.(\d{0,12}))?$/.exec(text);
  if (!match || (!match[1] && !match[2])) return undefined;
  const picos = BigInt(match[1].replaceAll(",", "") || "0")*1_000_000_000_000n+BigInt((match[2] ?? "").padEnd(12,"0"));
  return picos <= 1_000_000_000_000_000_000_000n ? String(picos) : undefined;
}
