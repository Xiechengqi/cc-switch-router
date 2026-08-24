import type { AppLocale, MessageKey } from "@/lib/i18n";
import { ApiError } from "@/lib/api";
import { formatUsdMoney } from "@/lib/market-money";
import {
  isApiProviderRuntime,
  providerQuotaStatusLine,
} from "@/components/dashboard/share-dashboard-utils";
import {
  marketProviderHealthTone,
  type ShareProviderStatusPanelView,
} from "@/components/dashboard/share-provider-status-panel";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type {
  ShareMarketAppCapability,
  ShareMarketListing,
  ShareMarketProviderFamily,
  ShareMarketSeat,
  ShareMarketSubscription,
  ShareTokenPeriod,
  ShareUpstreamProvider,
} from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";

export const PROVIDER_FAMILY_ORDER: ShareMarketProviderFamily[] = [
  "anthropic",
  "openai",
  "google",
  "xai",
  "cursor",
  "kiro",
  "copilot",
  "api",
  "multi",
  "other",
];

export const PROVIDER_FAMILY_KEYS: Record<ShareMarketProviderFamily, MessageKey> = {
  anthropic: "shareMarket.family.anthropic",
  openai: "shareMarket.family.openai",
  google: "shareMarket.family.google",
  xai: "shareMarket.family.xai",
  cursor: "shareMarket.family.cursor",
  kiro: "shareMarket.family.kiro",
  copilot: "shareMarket.family.copilot",
  api: "shareMarket.family.api",
  multi: "shareMarket.family.multi",
  other: "shareMarket.family.other",
};

export const CORE_SHARE_APPS = ["claude", "codex", "gemini"] as const;
export type CoreShareApp = (typeof CORE_SHARE_APPS)[number];

type MarketTranslate = (key: MessageKey, values?: Record<string, string | number>) => string;

export function shareMarketMutationError(reason: unknown, t: MarketTranslate) {
  if (!(reason instanceof ApiError)) {
    return t("shareMarket.error.requestFailed");
  }

  const coded: Partial<Record<string, MessageKey>> = {
    SHARE_MARKET_PAYMENT_PROFILE_REQUIRED: "shareMarket.error.paymentRequired",
    MARKET_SUPPLIER_SETTLEMENT_PROFILE_REQUIRED: "shareMarket.error.settlementRequired",
    DATABASE_UNAVAILABLE: "shareMarket.error.temporarilyUnavailable",
    share_market_client_upgrade_required: "shareMarket.error.clientUpgradeRequired",
    share_market_share_tokens_exhausted: "shareMarket.error.shareTokensExhausted",
    share_market_share_offline: "shareMarket.error.shareOffline",
    share_market_share_expired: "shareMarket.error.shareExpired",
    share_market_runtime_stale: "shareMarket.error.runtimeStale",
    share_market_model_unavailable: "shareMarket.error.modelUnavailable",
    share_market_contract_incompatible: "shareMarket.error.contractIncompatible",
    share_market_required_app_unavailable: "shareMarket.error.requiredAppUnavailable",
    share_market_service_term_unfulfillable: "shareMarket.error.termUnfulfillable",
    share_market_seat_capacity_invalid: "shareMarket.error.parallelExceedsShare",
    share_market_contract_settings_protected: "shareMarket.error.settingsProtected",
    share_market_fixed_term_required: "shareMarket.error.fixedTermRequired",
    share_market_termination_quote_required: "shareMarket.error.terminationQuoteRequired",
  };
  const codeKey = reason.code ? coded[reason.code] : undefined;
  if (codeKey) return t(codeKey);

  const message = reason.message.toLowerCase();
  const known: Array<[string, MessageKey]> = [
    ["configure account payment details", "shareMarket.error.paymentRequired"],
    ["configure usd settlement terms", "shareMarket.error.settlementRequired"],
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
  seats: Array<Pick<ShareMarketSeat, "status" | "readOnly" | "dailyRateMinor" | "isFree">>;
}) {
  const idle = listing.seats.filter(isSeatIdle);
  const seats = idle.length ? idle : listing.seats;
  if (!seats.length) return Number.POSITIVE_INFINITY;
  return Math.min(...seats.map((seat) => seat.dailyRateMinor ?? 0));
}

export function formatSeatPrice(
  seat: Pick<ShareMarketSeat, "isFree" | "dailyRateMinor">,
  locale: string,
  freeLabel: string,
  dayLabel: string,
) {
  return seat.isFree || seat.dailyRateMinor == null
    ? freeLabel
    : `${formatUsdMoney(seat.dailyRateMinor, locale)} / ${dayLabel}`;
}

export function formatTokenLimit(
  seat: Pick<ShareMarketSeat, "tokenLimit" | "tokenPeriod">,
  locale: string,
  unlimitedLabel: string,
  periodLabel: (period: ShareTokenPeriod) => string,
) {
  if (seat.tokenLimit == null) return unlimitedLabel;
  return `${formatTokenMillions(seat.tokenLimit, locale)} · ${periodLabel(seat.tokenPeriod)}`;
}

export function capabilityModelLabel(
  capability: ShareMarketAppCapability,
  passthroughLabel: string,
  unknownLabel: string,
) {
  if (capability.modelMode === "passthrough") return passthroughLabel;
  if (capability.modelMode === "fixed") {
    return capability.upstreamModel || capability.models[0] || unknownLabel;
  }
  return capability.models.join(" / ") || unknownLabel;
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
  const upstreamModel =
    capability.upstreamModel || capability.models[0] || "";
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
        }
      : capability.subscriptionLevel
        ? {
            status: "ok",
            plan: capability.subscriptionLevel,
            tiers: [],
          }
        : undefined,
    models: capability.models.map((actualModel) => ({ actualModel })),
    modelPolicy:
      capability.modelMode === "passthrough"
        ? { mode: "passthrough" }
        : capability.modelMode === "fixed" && upstreamModel
          ? { mode: "single", upstreamModel }
          : undefined,
  };
}

export function marketProviderStatusView(
  listing: Pick<ShareMarketListing, "appCapabilities" | "supportedApps">,
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
  const providerIdentity = [primary.providerName, primary.providerType]
    .map((value) => value?.trim())
    .filter((value): value is string => Boolean(value))
    .filter((value, index, values) => values.indexOf(value) === index)
    .join(" · ");
  const modelsLine = CORE_SHARE_APPS.flatMap((app) => {
    const item = capabilities.find((capability) => capability.app === app);
    return item
      ? [`${SHARE_APP_LABELS[app]}: ${capabilityModelLabel(item, labels.passthrough, labels.unknown)}`]
      : [];
  })
    .join(" · ");
  const healthState =
    primary.healthState ||
    (primary.available === false
      ? "unavailable"
      : primary.available === true
        ? "healthy"
        : "unknown");
  return {
    primaryLine: isApiProvider
      ? primary.providerName || primary.providerType || labels.unknown
      : providerQuotaStatusLine(runtime, locale),
    identityLine: providerIdentity || "-",
    modelsLine:
      modelsLine || capabilityModelLabel(primary, labels.passthrough, labels.unknown),
    toneClassName: marketProviderHealthTone(healthState),
  };
}

export function subscriptionStatusKey(status: string): MessageKey | null {
  const keys: Record<string, MessageKey> = {
    grant_pending: "shareMarket.subscription.grantPending",
    active_free: "shareMarket.subscription.activeFree",
    active_postpaid: "shareMarket.subscription.activePostpaid",
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

export function refundStatusKey(status: string): MessageKey {
  const keys: Record<string, MessageKey> = {
    applied: "shareMarket.refund.status.applied",
    refund_due: "shareMarket.refund.status.refund_due",
    settled: "shareMarket.refund.status.settled",
  };
  return keys[status] || "shareMarket.refund.status.refund_due";
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
