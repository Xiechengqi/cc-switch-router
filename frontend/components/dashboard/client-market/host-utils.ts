import type {
  ClientMarketHost,
  ClientMarketHostTransferDocument,
  ClientMarketProvider,
  CreateClientSelectionPersist,
  HostIpIntel,
  MarketProviderRecommendationMode,
} from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";
import { formatMarketPrice as formatUsdMoney, MARKET_CURRENCY } from "@/lib/market-money";
import { hostHealthRecordRatio } from "@/lib/host-health-view";

/**
 * Pure helpers, constants and types shared by the Client Market surfaces.
 *
 * Extracted from `client-market-page.tsx`, which had grown to ~3000 lines holding
 * eight unrelated concerns. Everything here is side-effect free and independently
 * testable; anything needing React state stays in a component file.
 */

export const CLIENT_MARKET_POLL_MS = 20_000;

export function parseClientTrialHours(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (!/^\d+$/.test(trimmed) || Number(trimmed) > 720) throw new Error("invalid_client_trial_hours");
  return Number(trimmed);
}

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

/** One line for a shop card: reported os-arch, vCPU, and GiB memory. Unknown values stay off the line. */
export function hostShopSpecLine(host: Pick<ClientMarketHost, "hostHealth">) {
  const snapshot = host.hostHealth?.snapshot;
  if (!snapshot) return "";
  const platform = [snapshot.os, snapshot.arch].filter(Boolean).join("-");
  const cpu = snapshot.effectiveVcpu.value;
  const memory = snapshot.memoryTotalBytes.value;
  const gib = (bytes: number) => `${new Intl.NumberFormat("en", { maximumFractionDigits: 1 }).format(bytes / 2 ** 30)}G`;
  return [platform, cpu != null ? `${new Intl.NumberFormat("en", { maximumFractionDigits: 1 }).format(cpu)}C` : "", memory != null ? gib(memory) : ""].filter(Boolean).join(" · ");
}

export function hostDisplayLabel(host: ClientMarketHost) {
  return host.hostname || host.ip || host.id.slice(0, 8);
}

export function hostMatchesMarketProvider(
  host: Pick<ClientMarketHost, "marketProviderId">,
  marketProviderId?: string,
) {
  return !marketProviderId || host.marketProviderId === marketProviderId;
}

function providerFreeIdle(provider: ClientMarketProvider) {
  if (typeof provider.countries?.[0]?.freeIdle === "number") {
    return provider.countries.reduce((sum, country) => sum + (country.freeIdle || 0), 0);
  }
  return 0;
}

export function recommendedClientMarketProviderId(
  providers: ClientMarketProvider[],
  officialProviderId: string | undefined,
  recommendationMode: MarketProviderRecommendationMode | string | undefined,
) {
  if (recommendationMode === "on") {
    return providers.find((provider) => providerFreeIdle(provider) > 0)?.providerId;
  }
  return officialProviderId && providers.some((provider) => provider.providerId === officialProviderId)
    ? officialProviderId
    : undefined;
}

export function persistedClientMarketProviderSelection(
  providerIds: string[],
  providers: Pick<ClientMarketProvider, "providerId">[],
): CreateClientSelectionPersist {
  if (providerIds.length === 0) {
    return {
      mode: "custom",
      providerIds: providers.map((provider) => provider.providerId),
    };
  }
  // Every interaction is durable user intent. `official_default` is reserved
  // for an untouched preference so a later off/shadow -> on rollout cannot
  // silently replace a Provider the user explicitly selected beforehand.
  return { mode: "custom", providerIds };
}

export function hostCanManage(host: ClientMarketHost, viewerEmail?: string | null) {
  if (host.isHostOwner === true) return true;
  // Fallback when API ownership flags lag behind host_owner_email.
  const viewer = viewerEmail?.trim().toLowerCase();
  const owner = host.hostOwnerEmail?.trim().toLowerCase();
  return !!viewer && !!owner && viewer === owner;
}

/** Provider-only. Renters release via `client_release` (see hostCanClientRelease). */
export function hostCanCleanup(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    !!host.installationId &&
    (host.status === "allocated" || host.status === "unreachable" || host.status === "draining") &&
    hostCanManage(host, viewerEmail)
  );
}

/** Renter (or self-renter) may release their Client with reason `client_release`. */
export function hostCanClientRelease(
  host: ClientMarketHost,
  rental?: {
    isClientOwner?: boolean;
    canRelease?: boolean;
    status?: string;
    recurring?: { cancelAtPeriodEnd?: boolean };
  } | null,
) {
  if (host.isClientOwner !== true || !host.installationId) return false;
  if (
    host.status !== "allocated" &&
    host.status !== "unreachable" &&
    host.status !== "draining"
  ) {
    return false;
  }
  if (!rental) return true;
  if (rental.isClientOwner === false) return false;
  if (rental.status === "released" || rental.status === "releasing") return false;
  return rental.canRelease !== false;
}

/** Within「我的」, rented rows (isClientOwner) sort ahead of hosted-only rows. */
export function prioritizeMineClientOwned<T extends { isClientOwner?: boolean }>(hosts: T[]) {
  return [...hosts].sort((left, right) => {
    const leftRank = left.isClientOwner === true ? 0 : 1;
    const rightRank = right.isClientOwner === true ? 0 : 1;
    return leftRank - rightRank;
  });
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
    (host.status === "onboarding" || host.status === "maintaining" || host.status === "idle" || host.status === "disabled" || host.status === "abnormal")
  );
}

export function hostCanRetireUnreachable(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    host.canRetireUnreachable === true &&
    hostCanManage(host, viewerEmail) &&
    host.status === "unreachable" &&
    !!host.installationId
  );
}

export function hostCanExport(host: ClientMarketHost, viewerEmail?: string | null) {
  return hostCanManage(host, viewerEmail) && !!host.ip && host.port != null;
}

export function hostExportKey(host: { ip?: string | null; port?: number | null }) {
  if (!host.ip || host.port == null) return "";
  return formatHostEndpoint(host.ip, host.port);
}

/**
 * Transfer line format:
 * ip:port|note|priceMinor|USD|freeDurationDays|fingerprint|pricingModel|trialHours
 *
 * The seventh field was added for calendar-month offers.  A missing field is
 * intentionally interpreted as `legacy_metered_daily`, so every transfer file
 * produced before the recurring-billing migration keeps its original meaning.
 */
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
  const price = entry.cyclePriceMinor != null
    ? String(entry.cyclePriceMinor)
    : entry.dailyRateMinor != null
      ? String(entry.dailyRateMinor)
      : "";
  const currency = entry.currency?.trim().toUpperCase() || "";
  const freeDurationDays = entry.freeDurationDays != null ? String(entry.freeDurationDays) : "";
  const fingerprint = entry.expectedFingerprint?.trim() || "";
  const pricingModel = price
    ? entry.cyclePriceMinor != null
      ? "prepaid_calendar_month"
      : "legacy_metered_daily"
    : "";
  const status = entry.informationalStatus?.trim();
  const trial = entry.trialHours == null ? "" : `|${entry.trialHours}`;
  const line = `${endpoint}|${note}|${price}|${currency}|${freeDurationDays}|${fingerprint}|${pricingModel}${trial}`;
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
    const [
      endpointPart,
      note = "",
      priceRaw = "",
      currencyRaw = "",
      freeDurationRaw = "",
      fingerprint = "",
      pricingModelRaw = "",
      trialHoursRaw = "",
    ] = line
      .split("|")
      .map((part) => part.trim());
    const endpoint = splitHostEndpoint(endpointPart);
    if (!endpoint) return { errorLine: trimmed };
    const key = formatHostEndpoint(endpoint.ip, endpoint.port);
    if (seen.has(key)) continue;
    seen.add(key);
    // Version-1 transfer files accepted `0` as the free-offer sentinel. Keep
    // that meaning only when no explicit pricing model is present; a version-2
    // row that names a paid model must still carry a positive price.
    const normalizedPriceRaw = !pricingModelRaw && priceRaw === "0" ? "" : priceRaw;
    const pricingModel = pricingModelRaw || (normalizedPriceRaw ? "legacy_metered_daily" : "free");
    if (!["free", "legacy_metered_daily", "prepaid_calendar_month"].includes(pricingModel)) {
      return { errorLine: trimmed };
    }
    let priceMinor: number | undefined;
    if (normalizedPriceRaw) {
      if (!/^\d+$/.test(normalizedPriceRaw)) return { errorLine: trimmed };
      priceMinor = Number(normalizedPriceRaw);
      if (!Number.isSafeInteger(priceMinor) || priceMinor < 1 || priceMinor > 100_000_000) {
        return { errorLine: trimmed };
      }
    }
    if ((pricingModel === "free") !== (priceMinor == null)) return { errorLine: trimmed };
    const currency = currencyRaw ? currencyRaw.toUpperCase() : undefined;
    if (currency && currency !== MARKET_CURRENCY) return { errorLine: trimmed };
    let freeDurationDays: number | undefined;
    if (freeDurationRaw) {
      freeDurationDays = Number(freeDurationRaw);
      if (!Number.isInteger(freeDurationDays) || freeDurationDays < 1 || freeDurationDays > 365) {
        return { errorLine: trimmed };
      }
    }
    if (priceMinor && freeDurationDays != null) return { errorLine: trimmed };
    let trialHours: number | null;
    try { trialHours = parseClientTrialHours(trialHoursRaw); } catch { return { errorLine: trimmed }; }
    hosts.push({
      ip: endpoint.ip,
      port: endpoint.port,
      note: note || undefined,
      dailyRateMinor: pricingModel === "legacy_metered_daily" ? priceMinor : undefined,
      cyclePriceMinor: pricingModel === "prepaid_calendar_month" ? priceMinor : undefined,
      pricingModel,
      billingInterval: pricingModel === "prepaid_calendar_month" ? "calendar_month" : undefined,
      currency: currency ? MARKET_CURRENCY : undefined,
      freeDurationDays,
      ...(trialHours == null ? {} : { trialHours }),
      expectedFingerprint: fingerprint || undefined,
    });
  }
  if (!hosts.length) return {};
  return {
    document: {
      version: hosts.some((host) => host.cyclePriceMinor != null) ? 2 : 1,
      hosts,
    },
  };
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
    onboarding: "clientMarket.status.onboarding",
    maintaining: "clientMarket.status.maintaining",
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

export function clientConnectionStateLabelKey(state: string): MessageKey {
  const known = {
    online: "clientMarket.connection.state.online",
    reconnecting: "clientMarket.connection.state.reconnecting",
    offline: "clientMarket.connection.state.offline",
    disabled: "clientMarket.connection.state.disabled",
  } as const;
  return (known[state as keyof typeof known] || "clientMarket.connection.state.offline") as MessageKey;
}

export const HOST_STATUS_GROUPS = ["all", "idle", "in_use", "needs_attention"] as const;
export type HostStatusFilter = (typeof HOST_STATUS_GROUPS)[number];
/** Left-rail tabs: optional "mine" (authed only) + status groups. */
export const HOST_LIST_TABS = ["mine", "all", "idle", "in_use", "needs_attention"] as const;
export type HostListTab = (typeof HOST_LIST_TABS)[number];

export function hostBelongsToViewer(host: {
  isHostOwner?: boolean;
  isClientOwner?: boolean;
}): boolean {
  return host.isHostOwner === true || host.isClientOwner === true;
}

export const STATUS_GROUP_MEMBERS: Record<Exclude<HostStatusFilter, "all">, readonly string[]> = {
  idle: ["idle"],
  in_use: ["allocated", "locked", "reserved"],
  needs_attention: ["onboarding", "maintaining", "draining", "unreachable", "abnormal", "disabled"],
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

export function hostMatchesListTab(
  host: { status: string; isHostOwner?: boolean; isClientOwner?: boolean },
  tab: HostListTab,
) {
  if (tab === "mine") return hostBelongsToViewer(host);
  return hostMatchesStatusFilter(host.status, tab);
}

export function statusGroupLabelKey(group: HostListTab): MessageKey {
  return `clientMarket.statusGroup.${group}` as MessageKey;
}

export function statusGroupHintKey(group: HostListTab): MessageKey {
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

export function formatHostOffer(
  dailyRateMinor: number | undefined,
  locale: string,
  freeDurationDays?: number,
  cyclePriceMinor?: number,
) {
  if (cyclePriceMinor != null) {
    const amount = formatUsdMoney(cyclePriceMinor, locale);
    return locale.startsWith("zh") ? `${amount} / 月` : `${amount} / month`;
  }
  if (dailyRateMinor == null) {
    if (freeDurationDays != null) {
      return locale.startsWith("zh")
        ? `免费 · ${freeDurationDays} 天`
        : `Free · ${freeDurationDays} ${freeDurationDays === 1 ? "day" : "days"}`;
    }
    return locale.startsWith("zh") ? "免费 · 永久" : "Free · permanent";
  }
  const amount = formatUsdMoney(dailyRateMinor, locale);
  return locale.startsWith("zh") ? `${amount} / 天` : `${amount} / day`;
}

export const SHOP_CONTINENTS = [
  "all",
  "asia",
  "europe",
  "north_america",
  "south_america",
  "oceania",
  "africa",
  "other",
] as const;
export type ShopContinent = (typeof SHOP_CONTINENTS)[number];

export const SHOP_UNKNOWN_COUNTRY = "unknown";

const SHOP_CONTINENT_MEMBERS: Record<Exclude<ShopContinent, "all" | "other">, readonly string[]> = {
  africa: ["DZ", "AO", "BJ", "BW", "BF", "BI", "CV", "CM", "CF", "TD", "KM", "CG", "CD", "CI", "DJ", "EG", "GQ", "ER", "SZ", "ET", "GA", "GM", "GH", "GN", "GW", "KE", "LS", "LR", "LY", "MG", "MW", "ML", "MR", "MU", "MA", "MZ", "NA", "NE", "NG", "RW", "ST", "SN", "SC", "SL", "SO", "ZA", "SS", "SD", "TZ", "TG", "TN", "UG", "ZM", "ZW", "EH", "RE", "YT", "SH"],
  asia: ["AF", "AM", "AZ", "BH", "BD", "BT", "BN", "KH", "CN", "CY", "GE", "HK", "IN", "ID", "IR", "IQ", "IL", "JP", "JO", "KZ", "KW", "KG", "LA", "LB", "MO", "MY", "MV", "MN", "MM", "NP", "KP", "OM", "PK", "PS", "PH", "QA", "SA", "SG", "KR", "LK", "SY", "TW", "TJ", "TH", "TL", "TR", "TM", "AE", "UZ", "VN", "YE", "IO"],
  europe: ["AX", "AL", "AD", "AT", "BY", "BE", "BA", "BG", "HR", "CZ", "DK", "EE", "FO", "FI", "FR", "DE", "GI", "GR", "GG", "VA", "HU", "IS", "IE", "IM", "IT", "JE", "LV", "LI", "LT", "LU", "MT", "MD", "MC", "ME", "NL", "MK", "NO", "PL", "PT", "RO", "RU", "SM", "RS", "SK", "SI", "ES", "SJ", "SE", "CH", "UA", "GB", "XK"],
  north_america: ["AI", "AG", "AW", "BS", "BB", "BZ", "BM", "BQ", "VG", "CA", "KY", "CR", "CU", "CW", "DM", "DO", "SV", "GL", "GD", "GP", "GT", "HT", "HN", "JM", "MQ", "MX", "MS", "NI", "PA", "PR", "BL", "KN", "LC", "MF", "PM", "VC", "SX", "TT", "TC", "US", "VI"],
  south_america: ["AR", "BO", "BR", "CL", "CO", "EC", "FK", "GF", "GY", "PY", "PE", "SR", "UY", "VE"],
  oceania: ["AS", "AU", "CX", "CC", "CK", "FJ", "PF", "GU", "HM", "KI", "MH", "FM", "NR", "NC", "NZ", "NU", "NF", "MP", "PW", "PG", "PN", "WS", "SB", "TK", "TO", "TV", "VU", "WF"],
};

const SHOP_COUNTRY_CONTINENT = new Map<string, Exclude<ShopContinent, "all" | "other">>(
  (Object.entries(SHOP_CONTINENT_MEMBERS) as Array<[Exclude<ShopContinent, "all" | "other">, readonly string[]]>)
    .flatMap(([continent, codes]) => codes.map((code) => [code, continent] as const)),
);

export function shopContinentForCountry(code: string | null | undefined): Exclude<ShopContinent, "all"> {
  const normalized = (code || "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(normalized) || normalized === SHOP_UNKNOWN_COUNTRY) return "other";
  return SHOP_COUNTRY_CONTINENT.get(normalized) || "other";
}

export type ShopCountrySort = "name" | "idle" | "price";

export type ShopCountryCard = {
  code: string;
  continent: Exclude<ShopContinent, "all">;
  idle: number;
  total: number;
  minMonthlyMinor: number | null;
  hasFreeIdle: boolean;
};

/** Monthly minor units, with a legacy daily offer scaled by 30. Matches `compareHostOffer`. */
export function hostShopPriceMinor(host: Pick<ClientMarketHost, "cyclePriceMinor" | "dailyRateMinor">) {
  if (typeof host.cyclePriceMinor === "number") return host.cyclePriceMinor;
  if (typeof host.dailyRateMinor === "number") return host.dailyRateMinor * 30;
  return null;
}

export function shopCountryKey(code: string | null | undefined) {
  const normalized = (code || "").trim().toUpperCase();
  return /^[A-Z]{2}$/.test(normalized) ? normalized : SHOP_UNKNOWN_COUNTRY;
}

export function aggregateShopCountries(hosts: readonly ClientMarketHost[]): ShopCountryCard[] {
  const cards = new Map<string, ShopCountryCard>();
  for (const host of hosts) {
    const code = shopCountryKey(host.countryCode);
    const card = cards.get(code) || {
      code,
      continent: shopContinentForCountry(code === SHOP_UNKNOWN_COUNTRY ? "" : code),
      idle: 0,
      total: 0,
      minMonthlyMinor: null,
      hasFreeIdle: false,
    };
    card.total += 1;
    if (host.status === "idle") {
      card.idle += 1;
      const price = hostShopPriceMinor(host);
      if (price == null) card.hasFreeIdle = true;
      else if (card.minMonthlyMinor == null || price < card.minMonthlyMinor) card.minMonthlyMinor = price;
    }
    cards.set(code, card);
  }
  return Array.from(cards.values());
}

export function sortShopCountries(
  cards: readonly ShopCountryCard[],
  sort: ShopCountrySort,
  nameOf: (code: string) => string,
) {
  const ranked = (card: ShopCountryCard) => (card.minMonthlyMinor == null ? Number.POSITIVE_INFINITY : card.minMonthlyMinor);
  return [...cards].sort((left, right) => {
    if (sort === "idle" && left.idle !== right.idle) return right.idle - left.idle;
    if (sort === "price" && ranked(left) !== ranked(right)) return ranked(left) - ranked(right);
    const name = nameOf(left.code).localeCompare(nameOf(right.code));
    if (name !== 0) return name;
    return left.code.localeCompare(right.code);
  });
}

export function hostMatchesShopFilters(
  host: ClientMarketHost,
  filters: { onlyIdle: boolean; arch: string; minimumCpu: string; minimumMemory: string },
) {
  if (filters.onlyIdle && host.status !== "idle") return false;
  const snapshot = host.hostHealth?.snapshot;
  if (filters.arch && snapshot?.arch !== filters.arch) return false;
  const cpu = Number(filters.minimumCpu);
  if (filters.minimumCpu && !(snapshot?.effectiveVcpu.value != null && snapshot.effectiveVcpu.value >= cpu)) return false;
  const memory = Number(filters.minimumMemory);
  if (filters.minimumMemory && !(snapshot?.memoryTotalBytes.value != null && snapshot.memoryTotalBytes.value >= memory * 2 ** 30)) return false;
  return true;
}

export function parseFreeDurationDays(value: string, t: Translate) {
  if (!/^\d+$/.test(value.trim())) {
    throw new Error(t("clientMarket.freeDurationInvalid"));
  }
  const days = Number(value);
  if (!Number.isInteger(days) || days < 1 || days > 365) {
    throw new Error(t("clientMarket.freeDurationInvalid"));
  }
  return days;
}

export function parseHostOffer(priceValue: string, t: Translate) {
  const price = priceValue.trim();
  if (!price) return { cyclePriceMinor: undefined, currency: undefined as string | undefined };
  if (!/^\d{1,7}(?:\.\d{1,2})?$/.test(price)) {
    throw new Error(t("clientMarket.offerInvalid"));
  }
  const [whole, fraction = ""] = price.split(".");
  const cyclePriceMinor = Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
  if (cyclePriceMinor < 1 || cyclePriceMinor > 100_000_000) {
    throw new Error(t("clientMarket.offerRange"));
  }
  return { cyclePriceMinor, currency: MARKET_CURRENCY };
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
  if (failureCode.startsWith("cleanup_ssh_unreachable")) {
    return "clientMarket.cleanupFailedGuidance.unreachable";
  }
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

/** Column multi-select for host owner emails (empty = all owners). */
export const OWNER_FILTER_KEY = "cc_switch_router_client_market_owner_filter_v3";
/** @deprecated migrated into OWNER_FILTER_KEY / "mine" tab */
export const OWNER_SCOPE_KEY = "cc_switch_router_client_market_owner_scope_v2";

export type OwnerScope = { mode: "mine" } | { mode: "custom"; emails: string[] };

export function normalizeOwnerFilters(value: unknown): string[] {
  if (Array.isArray(value) && value.every((item) => typeof item === "string")) {
    return value.map((item) => item.trim()).filter(Boolean);
  }
  // Migrate legacy mine/custom owner scope object.
  if (value && typeof value === "object" && "mode" in value) {
    const scope = value as OwnerScope;
    if (scope.mode === "custom" && Array.isArray(scope.emails)) {
      return scope.emails.map((item) => String(item).trim()).filter(Boolean);
    }
  }
  return [];
}

export const REGION_FILTER_KEY = "cc_switch_router_client_market_region_filter_v1";
export const STATUS_FILTER_KEY = "cc_switch_router_client_market_status_filter_v3";
export const SORT_PREFS_KEY = "cc_switch_router_client_market_sort_v2";
export const HOST_PAGE_SIZE = 10;

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

export const HOST_SORT_KEYS = ["status", "region", "owner", "offer", "subdomain", "ip", "health"] as const;
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
  const leftFree = left.dailyRateMinor == null && left.cyclePriceMinor == null;
  const rightFree = right.dailyRateMinor == null && right.cyclePriceMinor == null;
  if (leftFree !== rightFree) return leftFree ? -1 : 1;
  // Legacy daily offers remain comparable while new supply is monthly.  A
  // 30-day normalization is only a sorting key; the UI always keeps the true
  // billing unit visible.
  const normalized = (host: ClientMarketHost) =>
    host.cyclePriceMinor ?? (host.dailyRateMinor == null ? 0 : host.dailyRateMinor * 30);
  const priceCmp = normalized(left) - normalized(right);
  if (priceCmp !== 0) return priceCmp;
  return (left.currency || "USD").localeCompare(right.currency || "USD");
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
    case "health":
      return hostHealthRecordRatio(left.hostHealth) - hostHealthRecordRatio(right.hostHealth)
        || (left.hostHealth?.observable24h || 0) - (right.hostHealth?.observable24h || 0);
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

/** Persistable list tab; "mine" collapses to "all" when logged out. */
export function normalizeHostListTab(value: unknown, authed: boolean): HostListTab {
  if (value === "mine") return authed ? "mine" : "all";
  return normalizeHostStatusFilter(value);
}

export function hostStatusTabTone(status: HostListTab, active: boolean) {
  if (active) return "bg-white font-medium text-foreground shadow-sm";
  switch (status) {
    case "needs_attention":
      return "text-amber-700";
    case "idle":
      return "text-emerald-700";
    case "in_use":
      return "text-slate-700";
    case "mine":
      return "text-primary";
    default:
      return "text-muted-foreground";
  }
}

export const HOST_SORT_COLUMN_LABELS: Record<HostSortKey, MessageKey> = {
  health: "hostHealth.sort",
  status: "clientMarket.col.status",
  region: "clientMarket.col.region",
  owner: "clientMarket.col.owner",
  offer: "clientMarket.col.offer",
  subdomain: "clientMarket.col.subdomain",
  ip: "clientMarket.col.ip",
};
