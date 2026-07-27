import type { ClientMarketHost, ClientMarketHostTransferDocument, HostIpIntel } from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";

/**
 * Pure helpers, constants and types shared by the Client Market surfaces.
 *
 * Extracted from `client-market-page.tsx`, which had grown to ~3000 lines holding
 * eight unrelated concerns. Everything here is side-effect free and independently
 * testable; anything needing React state stays in a component file.
 */

export const CLIENT_MARKET_POLL_MS = 20_000;

export const ROUTER_OPEN_LOGIN_EVENT = "router-open-login";
export const ADD_HOST_SSH_KEY_OPEN_KEY = "cc-switch.client-market.add-host.ssh-key-open";
export const ADD_HOST_MODE_KEY = "cc-switch.client-market.add-host.mode";

export type AddHostMode = "password" | "manual";
export type StepKey = "installKey" | "connectivity" | "ipInfo" | "register";
export type StepStatus = "pending" | "running" | "done" | "failed";
export type StepStatusMap = Record<StepKey, StepStatus>;

export const IDLE_STEP_STATUS: StepStatusMap = {
  installKey: "pending",
  connectivity: "pending",
  ipInfo: "pending",
  register: "pending",
};

export const IP_RISK_LABEL_KEYS: Record<string, MessageKey> = {
  中性: "clientMarket.ipRisk.neutral",
  轻微风险: "clientMarket.ipRisk.low",
  低风险: "clientMarket.ipRisk.low",
  稍高风险: "clientMarket.ipRisk.elevated",
  中风险: "clientMarket.ipRisk.medium",
  高风险: "clientMarket.ipRisk.high",
  极高风险: "clientMarket.ipRisk.critical",
  风险: "clientMarket.ipRisk.risky",
  neutral: "clientMarket.ipRisk.neutral",
  low: "clientMarket.ipRisk.low",
  "low risk": "clientMarket.ipRisk.low",
  elevated: "clientMarket.ipRisk.elevated",
  medium: "clientMarket.ipRisk.medium",
  high: "clientMarket.ipRisk.high",
  critical: "clientMarket.ipRisk.critical",
  risky: "clientMarket.ipRisk.risky",
};

export const IP_CLASS_LABEL_KEYS: Record<string, MessageKey> = {
  "IDC 机房 IP": "clientMarket.ipClass.idc",
  "IDC机房IP": "clientMarket.ipClass.idc",
  数据中心: "clientMarket.ipClass.datacenter",
  "住宅 IP": "clientMarket.ipClass.residential",
  住宅IP: "clientMarket.ipClass.residential",
  "VPN 出口节点": "clientMarket.ipClass.vpnExit",
  VPN出口节点: "clientMarket.ipClass.vpnExit",
  代理: "clientMarket.ipClass.proxy",
  VPN: "clientMarket.ipClass.vpn",
  托管: "clientMarket.ipClass.hosting",
  Tor: "clientMarket.ipClass.tor",
  business: "clientMarket.ipClass.business",
  hosting: "clientMarket.ipClass.hosting",
  datacenter: "clientMarket.ipClass.datacenter",
  residential: "clientMarket.ipClass.residential",
  proxy: "clientMarket.ipClass.proxy",
  vpn: "clientMarket.ipClass.vpn",
  tor: "clientMarket.ipClass.tor",
  idc: "clientMarket.ipClass.idc",
};

export function containsCjk(value: string) {
  return /[\u3400-\u9fff]/.test(value);
}

export function hostDisplayLabel(host: ClientMarketHost) {
  return host.hostname || host.ip || host.id.slice(0, 8);
}

export function hostCanManage(host: ClientMarketHost, viewerEmail?: string | null) {
  if (host.isHostOwner === true) return true;
  // Fallback when API ownership flags lag behind host_owner_email.
  const viewer = viewerEmail?.trim().toLowerCase();
  const owner = host.hostOwnerEmail?.trim().toLowerCase();
  return !!viewer && !!owner && viewer === owner;
}

/** Provider-only. Renters release their own Client from Account → Client rentals,
 *  which posts `client_release` instead. */
export function hostCanCleanup(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    !!host.installationId &&
    (host.status === "allocated" || host.status === "unreachable" || host.status === "draining") &&
    hostCanManage(host, viewerEmail)
  );
}

export function hostCanReverify(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    hostCanManage(host, viewerEmail) &&
    (host.status === "unreachable" || host.status === "disabled" || host.status === "abnormal")
  );
}

export function hostCanDelete(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    hostCanManage(host, viewerEmail) &&
    !host.installationId &&
    (host.status === "idle" || host.status === "disabled" || host.status === "abnormal")
  );
}

export function hostCanExport(host: ClientMarketHost, viewerEmail?: string | null) {
  return hostCanManage(host, viewerEmail) && !!host.ip && host.port != null;
}

export function hostExportKey(host: { ip?: string | null; port?: number | null }) {
  if (!host.ip || host.port == null) return "";
  return formatHostEndpoint(host.ip, host.port);
}

/** Fixed line format: ip:port|note|priceCents|periodDays|fingerprint */
export type HostTransferLineEntry = ClientMarketHostTransferDocument["hosts"][number];

export function formatHostEndpoint(ip: string, port: number) {
  return ip.includes(":") ? `[${ip}]:${port}` : `${ip}:${port}`;
}

export function splitHostEndpoint(endpoint: string): { ip: string; port: number } | null {
  const trimmed = endpoint.trim();
  if (!trimmed) return null;
  if (trimmed.startsWith("[")) {
    const close = trimmed.indexOf("]");
    if (close < 1 || trimmed[close + 1] !== ":") return null;
    const ip = trimmed.slice(1, close).trim();
    const port = Number(trimmed.slice(close + 2));
    if (!ip || !Number.isInteger(port) || port <= 0 || port > 65535) return null;
    return { ip, port };
  }
  const idx = trimmed.lastIndexOf(":");
  if (idx <= 0) return null;
  const ip = trimmed.slice(0, idx).trim();
  const port = Number(trimmed.slice(idx + 1));
  if (!ip || !Number.isInteger(port) || port <= 0 || port > 65535) return null;
  return { ip, port };
}

export function encodeHostTransferLine(entry: HostTransferLineEntry): string {
  const endpoint = formatHostEndpoint(entry.ip, entry.port);
  const note = entry.note?.trim() || "";
  const price = entry.priceCents != null ? String(entry.priceCents) : "";
  const period = entry.rentalPeriodDays != null ? String(entry.rentalPeriodDays) : "";
  const fingerprint = entry.expectedFingerprint?.trim() || "";
  const status = entry.informationalStatus?.trim();
  const line = `${endpoint}|${note}|${price}|${period}|${fingerprint}`;
  return status ? `${line} # ${status}` : line;
}

export function encodeHostTransferDocument(document: ClientMarketHostTransferDocument): string {
  return document.hosts.map(encodeHostTransferLine).join("\n");
}

export function parseHostTransferLines(text: string): { document?: ClientMarketHostTransferDocument; errorLine?: string } {
  const hosts: HostTransferLineEntry[] = [];
  const seen = new Set<string>();
  for (const raw of text.split(/\r?\n/)) {
    const trimmed = raw.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const line = trimmed.replace(/\s+#.*$/, "").trim();
    if (!line) continue;
    const [endpointPart, note = "", priceRaw = "", periodRaw = "", fingerprint = ""] = line
      .split("|")
      .map((part) => part.trim());
    const endpoint = splitHostEndpoint(endpointPart);
    if (!endpoint) return { errorLine: trimmed };
    const key = formatHostEndpoint(endpoint.ip, endpoint.port);
    if (seen.has(key)) continue;
    seen.add(key);
    let priceCents: number | undefined;
    let rentalPeriodDays: number | undefined;
    if (priceRaw) {
      if (!/^\d+$/.test(priceRaw)) return { errorLine: trimmed };
      priceCents = Number(priceRaw);
    }
    if (periodRaw) {
      if (!/^\d+$/.test(periodRaw)) return { errorLine: trimmed };
      rentalPeriodDays = Number(periodRaw);
    }
    hosts.push({
      ip: endpoint.ip,
      port: endpoint.port,
      note: note || undefined,
      priceCents,
      rentalPeriodDays,
      expectedFingerprint: fingerprint || undefined,
    });
  }
  if (!hosts.length) return {};
  return { document: { version: 1, hosts } };
}

/** The host table only exposes cleanup to the Host owner, so this is always a
 *  Provider-initiated release. */
export function cleanupReasonForHost(_host: ClientMarketHost) {
  return "provider_release" as const;
}

export type BatchItemStatus = "queued" | "running" | "succeeded" | "failed" | "skipped";
export type BatchProgressItem = {
  hostId: string;
  label: string;
  status: BatchItemStatus;
  detail?: string;
};

export async function mapPool<T, R>(items: T[], concurrency: number, fn: (item: T, index: number) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(items.length);
  let cursor = 0;
  const workers = Array.from({ length: Math.min(Math.max(concurrency, 1), Math.max(items.length, 1)) }, async () => {
    while (true) {
      const index = cursor;
      cursor += 1;
      if (index >= items.length) return;
      results[index] = await fn(items[index], index);
    }
  });
  await Promise.all(workers);
  return results;
}

export function countBatchStatuses(items: BatchProgressItem[]) {
  let succeeded = 0;
  let skipped = 0;
  let failed = 0;
  for (const item of items) {
    if (item.status === "succeeded") succeeded += 1;
    else if (item.status === "skipped") skipped += 1;
    else if (item.status === "failed") failed += 1;
  }
  return { succeeded, skipped, failed };
}

export function translateMappedLabel(
  raw: string | undefined,
  map: Record<string, MessageKey>,
  t: (key: MessageKey) => string,
): string | null {
  const value = raw?.trim();
  if (!value) return null;
  const key = map[value] || map[value.toLowerCase()];
  return key ? t(key) : null;
}

export function formatHostIpIntelSecondary(
  intel: HostIpIntel | undefined,
  t: (key: MessageKey) => string,
): string[] {
  if (!intel) return [];
  const parts: string[] = [];
  const ispAsn = [intel.isp || intel.asName, intel.asn].filter(Boolean).join(" · ");
  if (ispAsn) parts.push(ispAsn);

  const risk = translateMappedLabel(intel.riskLevel, IP_RISK_LABEL_KEYS, t);
  if (risk) parts.push(risk);

  const classification =
    translateMappedLabel(intel.classificationType, IP_CLASS_LABEL_KEYS, t) ||
    translateMappedLabel(intel.networkType, IP_CLASS_LABEL_KEYS, t) ||
    (intel.vpn ? t("clientMarket.ipClass.vpn") : null) ||
    (intel.hosting ? t("clientMarket.ipClass.hosting") : null) ||
    (intel.proxy ? t("clientMarket.ipClass.proxy") : null) ||
    (intel.tor ? t("clientMarket.ipClass.tor") : null);
  if (classification) parts.push(classification);

  return parts;
}

export function formatHostIpLocation(
  intel: HostIpIntel | undefined,
  countryName: string,
  locale: string,
): string {
  if (!intel) return countryName;
  const preferLatin = locale.toLowerCase().startsWith("en");
  if (intel.location && !(preferLatin && containsCjk(intel.location))) {
    return intel.location;
  }
  const parts = [intel.city, intel.region, intel.country || countryName]
    .filter((part): part is string => !!part && !(preferLatin && containsCjk(part)));
  if (parts.length) return parts.join(" · ");
  return countryName;
}

export function statusLabelKey(status: string): MessageKey {
  const known = {
    idle: "clientMarket.status.idle",
    reserved: "clientMarket.status.reserved",
    allocated: "clientMarket.status.allocated",
    locked: "clientMarket.status.locked",
    draining: "clientMarket.status.draining",
    disabled: "clientMarket.status.disabled",
    unreachable: "clientMarket.status.unreachable",
    abnormal: "clientMarket.status.abnormal",
  } as const;
  return (known[status as keyof typeof known] || "clientMarket.status.idle") as MessageKey;
}

export const HOST_STATUS_GROUPS = ["all", "idle", "in_use", "needs_attention"] as const;
export type HostStatusFilter = (typeof HOST_STATUS_GROUPS)[number];

export const STATUS_GROUP_MEMBERS: Record<Exclude<HostStatusFilter, "all">, readonly string[]> = {
  idle: ["idle"],
  in_use: ["allocated", "locked", "reserved"],
  needs_attention: ["draining", "unreachable", "abnormal", "disabled"],
};

export function statusGroupForHost(status: string): Exclude<HostStatusFilter, "all"> | null {
  const normalized = status.trim().toLowerCase();
  for (const group of ["idle", "in_use", "needs_attention"] as const) {
    if (STATUS_GROUP_MEMBERS[group].includes(normalized)) return group;
  }
  return null;
}

export function hostMatchesStatusFilter(status: string, filter: HostStatusFilter) {
  if (filter === "all") return true;
  return statusGroupForHost(status) === filter;
}

export function statusGroupLabelKey(group: HostStatusFilter): MessageKey {
  return `clientMarket.statusGroup.${group}` as MessageKey;
}

export function statusGroupHintKey(group: HostStatusFilter): MessageKey {
  return `clientMarket.statusGroupHint.${group}` as MessageKey;
}

export function fineStatusHintKey(status: string): MessageKey | null {
  const known = {
    idle: "clientMarket.statusHint.idle",
    reserved: "clientMarket.statusHint.reserved",
    allocated: "clientMarket.statusHint.allocated",
    locked: "clientMarket.statusHint.locked",
    draining: "clientMarket.statusHint.draining",
    disabled: "clientMarket.statusHint.disabled",
    unreachable: "clientMarket.statusHint.unreachable",
    abnormal: "clientMarket.statusHint.abnormal",
  } as const;
  return known[status as keyof typeof known] ?? null;
}

export function authorizedKeysInstallCommand(line: string): string {
  const escaped = line.replace(/'/g, `'\\''`);
  return `echo '${escaped}' >> $HOME/.ssh/authorized_keys`;
}

export type Translate = (key: MessageKey, values?: Record<string, string | number>) => string;

export function formatHostOffer(priceCents: number | undefined, rentalPeriodDays: number | undefined, locale: string) {
  if (!priceCents || !rentalPeriodDays) return locale.startsWith("zh") ? "免费 · 永久" : "Free · forever";
  const amount = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" }).format(priceCents / 100);
  return locale.startsWith("zh") ? `${amount} · ${rentalPeriodDays} 天` : `${amount} · ${rentalPeriodDays}d`;
}

export function parseHostOffer(priceUsd: string, periodDays: string, t: Translate) {
  const price = priceUsd.trim();
  const period = periodDays.trim();
  if (!price && !period) return { priceCents: undefined, rentalPeriodDays: undefined };
  if (!price || !period || !/^\d{1,7}(?:\.\d{1,2})?$/.test(price) || !/^\d+$/.test(period)) {
    throw new Error(t("clientMarket.offerInvalid"));
  }
  const [whole, fraction = ""] = price.split(".");
  const priceCents = Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
  const rentalPeriodDays = Number(period);
  if (priceCents < 1 || priceCents > 100_000_000 || rentalPeriodDays < 4 || rentalPeriodDays > 3_650) {
    throw new Error(t("clientMarket.offerRange"));
  }
  return { priceCents, rentalPeriodDays };
}

export function isPaymentProfileRequiredError(message: string) {
  return message.toLowerCase().includes("configure payment details on the account page");
}

export function cleanupPhaseLabelKey(phase: string): MessageKey {
  switch (phase) {
    case "cleanup_stop":
      return "clientMarket.cleanupPhase.stop";
    case "cleanup_wipe":
      return "clientMarket.cleanupPhase.wipe";
    case "cleanup_purge":
      return "clientMarket.cleanupPhase.purge";
    case "complete":
      return "clientMarket.cleanupPhase.complete";
    case "cleanup_remote":
    default:
      return "clientMarket.cleanupPhase.remote";
  }
}

export function cleanupFailureGuidanceKey(failureCode?: string): MessageKey {
  if (!failureCode) return "clientMarket.cleanupFailedGuidance";
  if (failureCode.startsWith("cleanup_purge_failed")) return "clientMarket.cleanupFailedGuidance.purge";
  if (
    failureCode.startsWith("cleanup_ssh_timeout") ||
    failureCode.startsWith("cleanup_stop_failed") ||
    failureCode.startsWith("cleanup_wipe_failed")
  ) {
    return "clientMarket.cleanupFailedGuidance.remote";
  }
  if (
    failureCode.startsWith("cleanup_fingerprint_mismatch") ||
    failureCode.startsWith("cleanup_host_binding_mismatch")
  ) {
    return "clientMarket.cleanupFailedGuidance.safety";
  }
  return "clientMarket.cleanupFailedGuidance";
}

/** Human next-step copy for host list — never surface raw failure codes. */
export function hostStatusGuidanceKey(status: string, lastError?: string): MessageKey | null {
  const group = statusGroupForHost(status);
  if (group !== "needs_attention") return null;

  const code = (lastError || "").trim().toLowerCase();
  if (
    code.startsWith("provisioning_failed") ||
    code.startsWith("installer_failed") ||
    code.includes("provisioning failed")
  ) {
    return "clientMarket.hostErrorGuidance.provisioningFailed";
  }
  if (code.startsWith("rollback_failed") || code.includes("operator verification")) {
    return "clientMarket.hostErrorGuidance.rollbackFailed";
  }
  if (
    code.includes("already running") ||
    code.includes("cc-switch-server process")
  ) {
    return "clientMarket.hostErrorGuidance.abnormalProcess";
  }
  if (code.startsWith("cleanup_") || code.startsWith("cleanup ")) {
    return cleanupFailureGuidanceKey(lastError);
  }
  if (status === "draining") return "clientMarket.statusHint.draining";
  if (status === "disabled") return "clientMarket.statusHint.disabled";
  if (status === "abnormal") return "clientMarket.statusHint.abnormal";
  if (status === "unreachable") {
    return lastError
      ? "clientMarket.hostErrorGuidance.generic"
      : "clientMarket.statusHint.unreachable";
  }
  return lastError ? "clientMarket.hostErrorGuidance.generic" : null;
}

export const OWNER_SCOPE_KEY = "cc_switch_router_client_market_owner_scope_v2";

export type OwnerScope = { mode: "mine" } | { mode: "custom"; emails: string[] };
export const REGION_FILTER_KEY = "cc_switch_router_client_market_region_filter_v1";
export const PAYMENT_FILTER_KEY = "cc_switch_router_client_market_payment_filter_v1";
export const STATUS_FILTER_KEY = "cc_switch_router_client_market_status_filter_v2";
export const SORT_PREFS_KEY = "cc_switch_router_client_market_sort_v2";
export const HOST_PAGE_SIZE = 10;

export const PAYMENT_FILTER_KINDS = ["alipay", "wechat", "binance", "crypto", "custom"] as const;

export function paymentKindLabelKey(kind: string): MessageKey {
  switch (kind) {
    case "alipay":
      return "billing.payment.alipay";
    case "wechat":
      return "billing.payment.wechat";
    case "binance":
      return "billing.payment.binance";
    case "crypto":
      return "billing.payment.crypto";
    case "custom":
      return "billing.payment.custom";
    default:
      return "billing.payment.custom";
  }
}

export function hostSupportsPaymentKind(hostKinds: string[] | undefined, required: string): boolean {
  const kinds = new Set((hostKinds || []).map((kind) => kind.toLowerCase()));
  if (required === "crypto") {
    return kinds.has("crypto") || kinds.has("usdt") || kinds.has("usdc");
  }
  if (required === "usdt" || required === "usdc") {
    return kinds.has(required) || kinds.has("crypto");
  }
  return kinds.has(required);
}

/** Compact page list: 1 … 4 5 6 … 12 */
export function buildHostPageItems(current: number, total: number): Array<number | "ellipsis"> {
  if (total <= 7) return Array.from({ length: total }, (_, i) => i + 1);
  const pages = new Set<number>([1, total, current, current - 1, current + 1]);
  if (current <= 3) {
    pages.add(2);
    pages.add(3);
    pages.add(4);
  }
  if (current >= total - 2) {
    pages.add(total - 1);
    pages.add(total - 2);
    pages.add(total - 3);
  }
  const sorted = [...pages].filter((page) => page >= 1 && page <= total).sort((a, b) => a - b);
  const items: Array<number | "ellipsis"> = [];
  for (const page of sorted) {
    const prev = items[items.length - 1];
    if (typeof prev === "number" && page - prev > 1) items.push("ellipsis");
    items.push(page);
  }
  return items;
}

export const HOST_SORT_KEYS = ["status", "region", "owner", "offer", "subdomain", "ip"] as const;
export type HostSortKey = (typeof HOST_SORT_KEYS)[number];
export type HostSortDir = "asc" | "desc";
export type HostSortPrefs = { key: HostSortKey | null; dir: HostSortDir };

export const DEFAULT_HOST_SORT: HostSortPrefs = { key: "status", dir: "asc" };
export const CLEARED_HOST_SORT: HostSortPrefs = { key: null, dir: "asc" };

export function normalizeHostSortPrefs(value: unknown): HostSortPrefs {
  if (!value || typeof value !== "object") return DEFAULT_HOST_SORT;
  const record = value as { key?: unknown; dir?: unknown };
  if (record.key === null) return CLEARED_HOST_SORT;
  const key =
    typeof record.key === "string" && (HOST_SORT_KEYS as readonly string[]).includes(record.key)
      ? (record.key as HostSortKey)
      : DEFAULT_HOST_SORT.key;
  const dir = record.dir === "desc" ? "desc" : "asc";
  return { key, dir };
}

export function compareHostOffer(left: ClientMarketHost, right: ClientMarketHost) {
  const leftFree = !left.priceCents || !left.rentalPeriodDays;
  const rightFree = !right.priceCents || !right.rentalPeriodDays;
  if (leftFree !== rightFree) return leftFree ? -1 : 1;
  const priceCmp = (left.priceCents || 0) - (right.priceCents || 0);
  if (priceCmp !== 0) return priceCmp;
  return (left.rentalPeriodDays || 0) - (right.rentalPeriodDays || 0);
}

/**
 * Operational severity, lowest first. Sorting statuses alphabetically (abnormal,
 * allocated, disabled, draining, idle, …) put healthy and broken hosts in an order
 * that carried no meaning for the person on call. This ranks by "what needs a human
 * first" instead.
 *
 * `locked` and `reserved` are transient; a host that lingers in either is the
 * stranded-host case the Router reconciler sweeps up, so they outrank healthy states.
 */
const HOST_STATUS_SEVERITY: Record<string, number> = {
  unreachable: 0,
  abnormal: 1,
  draining: 2,
  locked: 3,
  reserved: 4,
  disabled: 5,
  allocated: 6,
  idle: 7,
};
const UNKNOWN_STATUS_SEVERITY = 8;

export function hostStatusSeverity(status: string): number {
  return HOST_STATUS_SEVERITY[status.trim().toLowerCase()] ?? UNKNOWN_STATUS_SEVERITY;
}

export function compareHostsBySortKey(left: ClientMarketHost, right: ClientMarketHost, key: HostSortKey) {
  switch (key) {
    case "status": {
      const severity = hostStatusSeverity(left.status) - hostStatusSeverity(right.status);
      // Fall back to the raw label so two unknown statuses still order deterministically.
      return severity !== 0 ? severity : left.status.localeCompare(right.status);
    }
    case "region":
      return (left.countryCode || "").localeCompare(right.countryCode || "");
    case "owner":
      return left.hostOwnerEmail.localeCompare(right.hostOwnerEmail);
    case "offer":
      return compareHostOffer(left, right);
    case "subdomain":
      return (left.clientSubdomain || "").localeCompare(right.clientSubdomain || "");
    case "ip":
      return `${left.ip || ""}:${left.port || 0}`.localeCompare(`${right.ip || ""}:${right.port || 0}`);
    default:
      return 0;
  }
}

export function compareHostsDefault(left: ClientMarketHost, right: ClientMarketHost) {
  const ownerCmp = left.hostOwnerEmail.localeCompare(right.hostOwnerEmail);
  if (ownerCmp !== 0) return ownerCmp;
  const ipCmp = `${left.ip || ""}:${left.port || 0}`.localeCompare(`${right.ip || ""}:${right.port || 0}`);
  if (ipCmp !== 0) return ipCmp;
  return left.id.localeCompare(right.id);
}

export function sortHosts(hosts: ClientMarketHost[], prefs: HostSortPrefs) {
  if (!prefs.key) {
    return [...hosts].sort(compareHostsDefault);
  }
  const dir = prefs.dir === "desc" ? -1 : 1;
  const key = prefs.key;
  return [...hosts].sort((left, right) => {
    const primary = compareHostsBySortKey(left, right, key);
    if (primary !== 0) return primary * dir;
    return compareHostsDefault(left, right);
  });
}

export function normalizeHostStatusFilter(value: unknown): HostStatusFilter {
  if (typeof value !== "string") return "all";
  if ((HOST_STATUS_GROUPS as readonly string[]).includes(value)) {
    return value as HostStatusFilter;
  }
  // Migrate legacy fine-grained tabs.
  const mapped = statusGroupForHost(value);
  return mapped ?? "all";
}

export function hostStatusTabTone(status: HostStatusFilter, active: boolean) {
  if (active) return "bg-white font-medium text-foreground shadow-sm";
  switch (status) {
    case "needs_attention":
      return "text-amber-700";
    case "idle":
      return "text-emerald-700";
    case "in_use":
      return "text-slate-700";
    default:
      return "text-muted-foreground";
  }
}

export const HOST_SORT_COLUMN_LABELS: Record<HostSortKey, MessageKey> = {
  status: "clientMarket.col.status",
  region: "clientMarket.col.region",
  owner: "clientMarket.col.owner",
  offer: "clientMarket.col.offer",
  subdomain: "clientMarket.col.subdomain",
  ip: "clientMarket.col.ip",
};
