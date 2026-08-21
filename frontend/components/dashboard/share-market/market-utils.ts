import type { MessageKey } from "@/lib/i18n";
import { formatUsdMoney } from "@/lib/market-money";
import type {
  ShareMarketAppCapability,
  ShareMarketProviderFamily,
  ShareMarketSeat,
  ShareMarketSubscription,
  ShareTokenPeriod,
} from "@/lib/types";

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
  return `${new Intl.NumberFormat(locale).format(seat.tokenLimit)} · ${periodLabel(seat.tokenPeriod)}`;
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
