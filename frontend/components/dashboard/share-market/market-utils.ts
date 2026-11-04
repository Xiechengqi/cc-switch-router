import type { AppLocale, MessageKey } from "@/lib/i18n";
import { ApiError } from "@/lib/api";
import { formatMarketPrice as formatUsdMoney } from "@/lib/market-money";
import {
  isApiProviderRuntime,
  providerQuotaStatusLine,
  providerStatusIdentity,
} from "@/components/dashboard/share-dashboard-utils";
import {
  marketProviderHealthTone,
  type ShareProviderStatusPanelView,
} from "@/components/dashboard/share-provider-status-panel";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import { formatUsdLimit, USD_LIMIT_MAX_MICROS } from "@/lib/share-value-quota";
import type {
  ShareMarketAppCapability,
  ShareMarketListing,
  ShareMarketSeat,
  ShareMarketSubscription,
  ShareTokenPeriod,
  ShareUpstreamProvider,
} from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";
import { modelHealthPresentation } from "@/lib/share-health";

export const CORE_SHARE_APPS = ["claude", "codex", "gemini"] as const;
export type CoreShareApp = (typeof CORE_SHARE_APPS)[number];

type MarketTranslate = (key: MessageKey, values?: Record<string, string | number>) => string;

export function shareMarketMutationError(reason: unknown, t: MarketTranslate) {
  if (!(reason instanceof ApiError)) {
    return t("shareMarket.error.requestFailed");
  }

  const coded: Partial<Record<string, MessageKey>> = {

    DATABASE_UNAVAILABLE: "shareMarket.error.temporarilyUnavailable",
    share_market_client_upgrade_required: "shareMarket.error.clientUpgradeRequired",
    share_market_trial_upgrade_required: "marketTrial.upgradeRequired",
    share_market_share_tokens_exhausted: "shareMarket.error.shareTokensExhausted",
    share_market_share_offline: "shareMarket.error.shareOffline",
    share_market_share_expired: "shareMarket.error.shareExpired",
    share_market_runtime_stale: "shareMarket.error.runtimeStale",
    share_market_runtime_sync_required: "shareMarket.error.runtimeStale",
    share_market_model_unavailable: "shareMarket.error.modelUnavailable",
    share_market_contract_incompatible: "shareMarket.error.contractIncompatible",
    share_market_required_app_unavailable: "shareMarket.error.requiredAppUnavailable",
    share_market_service_term_unfulfillable: "shareMarket.error.termUnfulfillable",
    share_market_seat_capacity_invalid: "shareMarket.error.parallelExceedsShare",
    share_market_contract_settings_protected: "shareMarket.error.settingsProtected",
    share_market_reopen_required: "shareMarket.error.reopenRequired",
    share_market_other_active_listing: "shareMarket.error.otherActiveListing",
    share_market_listing_not_closed: "shareMarket.error.listingNotClosed",
    share_market_seat_not_reopenable: "shareMarket.error.seatNotReopenable",
    share_market_offer_changed: "shareMarket.error.offerChanged",
    share_market_value_quota_unavailable: "shareMarket.error.valueQuotaUnavailable",
  };
  const codeKey = reason.code ? coded[reason.code] : undefined;
  if (codeKey) return t(codeKey);

  const message = reason.message.toLowerCase();
  const known: Array<[string, MessageKey]> = [

    ["share must be active", "shareMarket.error.shareInactive"],
    ["disable public free access", "shareMarket.error.publicAccessEnabled"],
    ["pending public free access edit", "shareMarket.error.pendingShareEdit"],
    ["share is already listed", "shareMarket.error.alreadyListed"],
    ["still has active share market rentals", "shareMarket.error.activeRentals"],
    ["token period is unsupported", "shareMarket.error.unsupportedPeriod"],
    ["token period unsupported", "shareMarket.error.unsupportedPeriod"],
    ["seat uses a token period unsupported", "shareMarket.error.unsupportedPeriod"],
    ["exceeds share concurrency", "shareMarket.error.parallelExceedsShare"],
    ["listing seat limit reached", "shareMarket.error.seatLimit"],
    ["seat offer changed", "shareMarket.error.offerChanged"],
    ["must be reclaimed before editing", "shareMarket.error.seatNotEditable"],
    ["listing share is no longer", "shareMarket.error.shareChanged"],
    ["only the share owner", "shareMarket.error.ownerRequired"],
    ["only listing owner", "shareMarket.error.ownerRequired"],
  ];
  const messageKey = known.find(([fragment]) => message.includes(fragment))?.[1];
  if (messageKey) return t(messageKey);
  if (reason.status === 401) return t("shareMarket.loginRequired");
  if (reason.status === 403) return t("shareMarket.error.ownerRequired");
  if (reason.status === 404) return t("shareMarket.error.notFound");
  if (reason.status === 409) return t("shareMarket.error.conflict");
  if (reason.status >= 500) return t("shareMarket.error.temporarilyUnavailable");
  return t("shareMarket.error.requestFailed");
}

export function isCoreShareApp(value: string): value is CoreShareApp {
  return CORE_SHARE_APPS.includes(value as CoreShareApp);
}

export function isTerminalSubscription(status: string) {
  return status === "released" || status === "grant_failed";
}

export function activeSubscriptionForShare(
  subscriptions: ShareMarketSubscription[],
  shareId: string,
) {
  return subscriptions.find(
    (subscription) =>
      subscription.shareId === shareId && !isTerminalSubscription(subscription.status),
  );
}

export function isSeatIdle(seat: Pick<ShareMarketSeat, "status" | "readOnly">) {
  return seat.status === "available" && !seat.readOnly;
}

export function listingIdleCount(listing: { seats: Array<Pick<ShareMarketSeat, "status" | "readOnly">> }) {
  return listing.seats.filter(isSeatIdle).length;
}

export function listingLowestDailyRate(listing: {
  seats: Array<Pick<ShareMarketSeat, "status" | "readOnly" | "dailyRateMinor" | "cyclePriceMinor" | "isFree">>;
}) {
  const idle = listing.seats.filter(isSeatIdle);
  const seats = idle.length ? idle : listing.seats;
  if (!seats.length) return Number.POSITIVE_INFINITY;
  return Math.min(...seats.map(seatComparableMonthlyMinor));
}

/** Quota windows in 30 days as a fraction; cumulative quotas never repeat. */
const WINDOWS_PER_30_DAYS: Partial<Record<ShareTokenPeriod, readonly [bigint, bigint]>> = {
  day: [30n, 1n],
  week: [30n, 7n],
  sevenDays: [30n, 7n],
  calendarMonth: [1n, 1n],
  thirtyDays: [1n, 1n],
};

export type SeatValueDiscount = {
  /** Rent as a share of the full-use API list-price value, in basis points. */
  ratioBps: number;
  /** API list-price value of using the whole quota for 30 days, in micros. */
  monthlyValueMicros: string;
  monthlyPriceMinor: number;
};

/**
 * Nominal discount of a paid USD-equivalent seat: 30 days of rent against
 * 30 days of fully used quota at API list prices. A seat without a USD
 * quota, a free seat or a cumulative quota has none. Exact BigInt maths.
 */
export function seatValueDiscount(
  seat: Pick<ShareMarketSeat, "isFree" | "valueQuota" | "tokenPeriod" | "dailyRateMinor" | "cyclePriceMinor">,
): SeatValueDiscount | null {
  const windows = WINDOWS_PER_30_DAYS[seat.tokenPeriod];
  const limit = seat.valueQuota?.limitUsdMicros;
  const monthlyPriceMinor = seatComparableMonthlyMinor(seat);
  if (seat.isFree || !windows || !limit || !/^[1-9]\d{0,15}$/.test(limit)
    || BigInt(limit) > USD_LIMIT_MAX_MICROS || !Number.isSafeInteger(monthlyPriceMinor) || monthlyPriceMinor <= 0) return null;
  const [count, days] = windows;
  const valueMicros = BigInt(limit) * count;
  // price micros / (value micros / days) = price * 10_000 * days / valueMicros
  const rounded = (BigInt(monthlyPriceMinor) * 10_000n * days * 10_000n + valueMicros / 2n) / valueMicros;
  // Never turn a positive rent into "100% off" through basis-point rounding.
  const ratioBps = rounded < 1n ? 1n : rounded;
  if (ratioBps > BigInt(Number.MAX_SAFE_INTEGER)) return null;
  return {
    ratioBps: Number(ratioBps),
    monthlyValueMicros: (valueMicros / days).toString(),
    monthlyPriceMinor,
  };
}

/**
 * Chinese buyers read discounts as 折 (6.67% → 0.7 折), others as percent off.
 * The off-percentage rounds down so a discount is never overstated. Null
 * when the rent is not below the API list-price value.
 */
export function formatSeatValueDiscount(discount: SeatValueDiscount, locale: string) {
  if (!Number.isSafeInteger(discount.ratioBps) || discount.ratioBps <= 0 || discount.ratioBps >= 10_000) return null;
  if (locale.startsWith("zh")) {
    const zhe = discount.ratioBps / 1_000;
    const digits = zhe >= 1 ? 1 : 2;
    const value = Math.max(Math.ceil(zhe * 10 ** digits) / 10 ** digits, 10 ** -digits);
    return `${new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value)} 折`;
  }
  return `${Math.floor((10_000 - discount.ratioBps) / 100)}% off`;
}

/** Rent as a percentage of full-use API value, e.g. `6.7%` or `120%`. */
export function formatSeatValueRatio(discount: SeatValueDiscount, locale: string) {
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: discount.ratioBps < 1_000 ? 2 : 1 }).format(discount.ratioBps / 100)}%`;
}

/** Best (lowest) nominal ratio among a listing's rentable USD seats. */
export function listingBestValueRatio(listing: {
  seats: Array<Pick<ShareMarketSeat, "status" | "readOnly" | "isFree" | "valueQuota" | "tokenPeriod" | "dailyRateMinor" | "cyclePriceMinor">>;
}) {
  const idle = listing.seats.filter(isSeatIdle);
  const ratios = (idle.length ? idle : listing.seats)
    .map((seat) => seatValueDiscount(seat)?.ratioBps)
    .filter((ratio): ratio is number => ratio != null);
  return ratios.length ? Math.min(...ratios) : Number.POSITIVE_INFINITY;
}

/** Comparable monthly key for mixed legacy-daily and calendar-month catalog rows. */
export function seatComparableMonthlyMinor(
  seat: Pick<ShareMarketSeat, "isFree" | "dailyRateMinor" | "cyclePriceMinor">,
) {
  if (seat.isFree) return 0;
  return seat.cyclePriceMinor ?? (seat.dailyRateMinor == null ? 0 : seat.dailyRateMinor * 30);
}

export type ListingPriceSummary = {
  /** Lowest effective daily rate in minor units, or null when the Share has no seats. */
  rateMinor: number | null;
  /** Formatted lowest price, e.g. "$2.50 / day", or the free label when the floor is 0. */
  price: string;
  /** True when the candidate seats do not all share the same rate, so `price` is a floor. */
  isFrom: boolean;
};

/**
 * Price floor shown on the catalog card. Mirrors `listingLowestDailyRate`: idle seats
 * decide the number a buyer can actually act on, and only when none are idle does the
 * whole Share stand in, so a full Share still advertises the level it sells at.
 */
export function listingPriceSummary(
  listing: { seats: Array<Pick<ShareMarketSeat, "status" | "readOnly" | "dailyRateMinor" | "cyclePriceMinor" | "isFree">> },
  locale: string,
  freeLabel: string,
  dayLabel: string,
  monthLabel?: string,
): ListingPriceSummary {
  const idle = listing.seats.filter(isSeatIdle);
  const seats = idle.length ? idle : listing.seats;
  if (!seats.length) return { rateMinor: null, price: "-", isFrom: false };
  const comparableRates = seats.map(seatComparableMonthlyMinor);
  const lowestComparable = Math.min(...comparableRates);
  const selected = seats[comparableRates.indexOf(lowestComparable)];
  const rateMinor = selected.isFree
    ? 0
    : selected.cyclePriceMinor ?? selected.dailyRateMinor ?? 0;
  return {
    rateMinor,
    price: formatSeatPrice(selected, locale, freeLabel, dayLabel, monthLabel),
    isFrom: comparableRates.some((rate) => rate !== lowestComparable),
  };
}

export function formatSeatPrice(
  seat: Pick<ShareMarketSeat, "isFree" | "dailyRateMinor" | "cyclePriceMinor">,
  locale: string,
  freeLabel: string,
  dayLabel: string,
  monthLabel = locale.startsWith("zh") ? "月" : "month",
) {
  if (seat.isFree) return freeLabel;
  if (seat.cyclePriceMinor != null) {
    return `${formatUsdMoney(seat.cyclePriceMinor, locale)} / ${monthLabel}`;
  }
  return seat.dailyRateMinor == null
    ? freeLabel
    : `${formatUsdMoney(seat.dailyRateMinor, locale)} / ${dayLabel}`;
}

/**
 * A seat's usage limit in its own unit: Tokens, or a USD-equivalent amount
 * that `valueLabel` marks as API value so it never reads as a price.
 */
export function formatSeatQuota(
  seat: Pick<ShareMarketSeat, "tokenLimit" | "tokenPeriod" | "valueQuota">,
  locale: string,
  unlimitedLabel: string,
  periodLabel: (period: ShareTokenPeriod) => string,
  valueLabel: (amount: string) => string,
) {
  if (seat.valueQuota) {
    return `${valueLabel(formatUsdLimit(seat.valueQuota.limitUsdMicros))} · ${periodLabel(seat.tokenPeriod)}`;
  }
  if (seat.tokenLimit == null) return unlimitedLabel;
  return `${formatTokenMillions(seat.tokenLimit, locale)} · ${periodLabel(seat.tokenPeriod)}`;
}

export function capabilityModels(capability: Pick<ShareMarketAppCapability, "models">) {
  return capability.models ?? [];
}

export function capabilityModelLabel(
  capability: ShareMarketAppCapability,
  passthroughLabel: string,
  unknownLabel: string,
) {
  const models = capabilityModels(capability);
  if (capability.modelMode === "passthrough") return passthroughLabel;
  if (capability.modelMode === "fixed") {
    return capability.upstreamModel || models[0] || unknownLabel;
  }
  return models.join(" / ") || unknownLabel;
}

function capabilityDetailScore(capability: ShareMarketAppCapability) {
  return (
    Number(Boolean(capability.quota)) * 4 +
    Number(Boolean(capability.subscriptionLevel)) * 3 +
    Number(Boolean(capability.accountHint)) * 2 +
    Number(Boolean(capability.providerName || capability.providerType))
  );
}

export function primaryMarketCapability(
  capabilities: ShareMarketAppCapability[],
) {
  const core = CORE_SHARE_APPS.flatMap((app) => {
    const capability = capabilities.find((item) => item.app === app);
    return capability ? [capability] : [];
  });
  const candidates = core.length ? core : capabilities;
  return candidates.reduce<ShareMarketAppCapability | undefined>(
    (best, capability) =>
      !best || capabilityDetailScore(capability) > capabilityDetailScore(best)
        ? capability
        : best,
    undefined,
  );
}

export function enabledMarketCapabilities(
  listing: Pick<ShareMarketListing, "appCapabilities" | "supportedApps">,
) {
  const enabledApps = new Set(listing.supportedApps);
  return listing.appCapabilities.filter((capability) => enabledApps.has(capability.app));
}

export function marketCapabilityRuntime(
  capability: ShareMarketAppCapability,
): ShareUpstreamProvider {
  const models = capabilityModels(capability);
  const upstreamModel = capability.upstreamModel || models[0] || "";
  return {
    app: capability.app,
    kind: capability.providerType,
    providerType: capability.providerType,
    providerName: capability.providerName,
    accountEmail: capability.accountHint,
    subscriptionLevel: capability.subscriptionLevel,
    quota: capability.quota
      ? {
          status: capability.quota.status,
          plan: capability.quota.plan,
          subscriptionPeriodEnd: capability.quota.subscriptionPeriodEnd,
          tiers: capability.quota.tiers,
          unobservedTiers: capability.quota.unobservedTiers,
        }
      : capability.subscriptionLevel
        ? {
            status: "ok",
            plan: capability.subscriptionLevel,
            tiers: [],
          }
        : undefined,
    models: models.map((actualModel) => ({ actualModel })),
    modelPolicy:
      capability.modelMode === "passthrough"
        ? { mode: "passthrough" }
        : capability.modelMode === "fixed" && upstreamModel
          ? { mode: "single", upstreamModel }
          : undefined,
  };
}

export function marketProviderStatusView(
  listing: Pick<ShareMarketListing, "appCapabilities" | "supportedApps" | "modelHealthStatus">,
  locale: AppLocale,
  labels: { unknown: string; passthrough: string },
): ShareProviderStatusPanelView {
  const capabilities = enabledMarketCapabilities(listing);
  const primary = primaryMarketCapability(capabilities);
  if (!primary) {
    return {
      primaryLine: labels.unknown,
      identityLine: "-",
      modelsLine: "-",
      toneClassName: marketProviderHealthTone("unknown"),
    };
  }
  const runtime = marketCapabilityRuntime(primary);
  const isApiProvider =
    primary.providerFamily === "api" || isApiProviderRuntime(runtime);
  const accountLine = providerStatusIdentity(runtime);
  const modelsLine = CORE_SHARE_APPS.flatMap((app) => {
    const item = capabilities.find((capability) => capability.app === app);
    return item
      ? [`${SHARE_APP_LABELS[app]}: ${capabilityModelLabel(item, labels.passthrough, labels.unknown)}`]
      : [];
  })
    .join(" · ");
  // Identity/price detail richness is not a health signal. Never let a healthy
  // primary App conceal another enabled App's failure on a legacy Router.
  const healthState = capabilities.some((item) => item.healthState === "unavailable")
    ? "unavailable" : capabilities.some((item) => item.healthState === "degraded")
      ? "degraded" : capabilities.every((item) => item.healthState === "healthy")
        ? "healthy" : "unknown";
  const health = listing.modelHealthStatus
    ? modelHealthPresentation(listing.modelHealthStatus, locale) : undefined;
  return {
    primaryLine: isApiProvider
      ? primary.providerName || primary.providerType || labels.unknown
      : providerQuotaStatusLine(runtime, locale),
    identityLine: isApiProvider ? "-" : accountLine,
    modelsLine:
      modelsLine || capabilityModelLabel(primary, labels.passthrough, labels.unknown),
    panelTitle: health?.title,
    toneClassName: health?.className ?? marketProviderHealthTone(healthState),
  };
}

export function subscriptionStatusKey(status: string): MessageKey | null {
  const keys: Record<string, MessageKey> = {
    grant_pending: "shareMarket.subscription.grantPending",
    active_free: "shareMarket.subscription.activeFree",
    active_postpaid: "shareMarket.subscription.activePaid",
    billing_suspend_pending: "shareMarket.subscription.billingSuspendPending",
    billing_suspended: "shareMarket.subscription.billingSuspended",
    billing_resume_pending: "shareMarket.subscription.billingResumePending",
    billing_control_failed: "shareMarket.subscription.billingControlRetry",
    revoke_pending: "shareMarket.subscription.revokePending",
    revoke_failed: "shareMarket.subscription.revokeFailed",
    grant_failed: "shareMarket.subscription.grantFailed",
    released: "shareMarket.subscription.released",
  };
  return keys[status] || null;
}

export function integrityStatusKey(status: string): MessageKey {
  const keys: Record<string, MessageKey> = {
    compatible: "shareMarket.integrity.compatible",
    violated: "shareMarket.integrity.violated",
    remediating: "shareMarket.integrity.remediating",
    terminated: "shareMarket.integrity.terminated",
  };
  return keys[status] || "shareMarket.integrity.violated";
}

export function integrityReasonText(reason: string | undefined, t: MarketTranslate) {
  if (!reason) return "";
  const keys: Record<string, MessageKey> = {
    share_missing: "shareMarket.integrity.reason.shareMissing",
    share_inactive: "shareMarket.integrity.reason.shareInactive",
    share_contract_upgrade_required: "shareMarket.integrity.reason.clientUpgradeRequired",
    contract_apps_missing: "shareMarket.integrity.reason.contractAppsMissing",
    contract_apps_changed: "shareMarket.integrity.reason.contractAppsChanged",
    required_app_disabled: "shareMarket.integrity.reason.requiredAppDisabled",
    share_parallel_capacity_reduced: "shareMarket.integrity.reason.parallelReduced",
    fixed_term_not_covered: "shareMarket.integrity.reason.fixedTermNotCovered",
    upstream_fixed_term_not_covered: "shareMarket.integrity.reason.upstreamTermNotCovered",
    binding_missing: "shareMarket.integrity.reason.bindingMissing",
    provider_disabled: "shareMarket.integrity.reason.providerDisabled",
    binding_unresolved: "shareMarket.integrity.reason.bindingUnresolved",
    provider_binding_changed: "shareMarket.integrity.reason.providerBindingChanged",
    provider_model_changed: "shareMarket.integrity.reason.providerModelChanged",
    required_app_unavailable: "shareMarket.integrity.reason.requiredAppUnavailable",
    app_scope_not_enforced: "shareMarket.integrity.reason.appScopeNotEnforced",
    entitlement_missing: "shareMarket.integrity.reason.entitlementMissing",
    contract_integrity_repair_timeout: "shareMarket.integrity.reason.repairTimeout",
  };
  const messages = reason
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean)
    .map((item) => t(keys[item] || "shareMarket.integrity.reason.unknown"));
  return [...new Set(messages)].join(" · ");
}

export function grantFailureMessageKey(code?: string): MessageKey {
  switch (code) {
    case "cc_switch_share_revision_conflict":
      return "shareMarket.authorizationFailure.revisionConflict";
    case "cc_switch_share_policy_divergent":
      return "shareMarket.authorizationFailure.policyDivergent";
    case "cc_switch_share_binding_immutable":
      return "shareMarket.authorizationFailure.bindingImmutable";
    case "control_ack_timeout":
      return "shareMarket.authorizationFailure.controlTimeout";
    case "share_market_grant_contract_violation":
      return "shareMarket.authorizationFailure.contractViolation";
    default:
      return "shareMarket.authorizationFailure.generic";
  }
}
