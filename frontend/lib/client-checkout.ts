import type { ClientMarketAllocationQuote, WalletLifecycle } from "./types";
import type { MessageKey } from "./i18n";
import { validPaidWalletQuoteFunding, validWalletQuoteFunding } from "./market-wallet-quote";

export type ClientDraft = {
  subdomain: string;
  password: string;
  autoRenew: boolean;
};
export type ClientSubdomainCheck = {
  value: string;
  status: "checking" | "available" | "unavailable" | "error";
  message?: string;
};

/** Paid checkout fails closed when wallet data is absent; free quotes need no wallet. */
export function clientCheckoutState(quote: ClientMarketAllocationQuote) {
  const paid = quote.items.filter(
    (item) => item.dailyRateMinor != null || item.cyclePriceMinor != null,
  );
  const unsupported =
    paid.length > 0 &&
    (quote.fundingRail !== "platform_wallet" ||
      !validPaidWalletQuoteFunding(quote.walletFunding, paid.reduce((sum, item) => sum + (item.cyclePriceMinor ?? item.dailyRateMinor ?? 0), 0)) ||
      paid.some(
        (item) =>
          ![item.trialSecondsRemaining, item.trialAllowanceSeconds, item.trialSecondsConsumed]
            .every((seconds) => Number.isSafeInteger(seconds) && seconds >= 0) ||
          item.trialAllowanceSeconds > 30 * 86400 ||
          item.trialSecondsRemaining !== Math.max(0, item.trialAllowanceSeconds - item.trialSecondsConsumed),
      ));
  return {
    paid: paid.length > 0,
    unsupported,
    trial:
      paid.length > 0 && paid.every((item) => item.trialSecondsRemaining > 0),
    shortfall: validWalletQuoteFunding(quote.walletFunding)
      ? BigInt(quote.walletFunding!.shortfallUnits) > 0n
      : false,
  };
}

export function preserveClientDrafts(
  quote: ClientMarketAllocationQuote,
  drafts: Record<string, ClientDraft>,
) {
  return Object.fromEntries(
    quote.items.flatMap((item) =>
      drafts[item.id] ? [[item.hostId, drafts[item.id]]] : [],
    ),
  );
}

export function walletLifecycleKey(contract: WalletLifecycle): MessageKey {
  switch (contract.status) {
    case "trialing":
      return "clientCheckout.lifecycle.trial";
    case "suspended":
      return "clientCheckout.lifecycle.suspended";
    case "review_required":
      return "clientCheckout.lifecycle.review";
    case "ending":
      return "clientCheckout.lifecycle.ending";
    case "ended":
    case "terminated":
    case "activation_failed":
      return "clientCheckout.lifecycle.ended";
    case "pending_activation":
      return "clientCheckout.lifecycle.pending";
    case "active":
      return contract.periodEnd
        ? contract.renewalPolicy === "automatic"
          ? "clientCheckout.lifecycle.renews"
          : "clientCheckout.lifecycle.expires"
        : "clientCheckout.lifecycle.active";
    default:
      return "clientCheckout.lifecycle.unknown";
  }
}

export function trialHours(seconds: number, locale: string) {
  // Never round a positive remaining allowance down to zero.
  return (Math.ceil(Math.max(0, seconds) / 36) / 100).toLocaleString(locale, {
    maximumFractionDigits: 2,
  });
}

/** Exact whole-second display; a positive allowance never rounds to zero. */
export function trialDuration(seconds: number, locale: string) {
  if (!Number.isSafeInteger(seconds) || seconds < 0) return "—";
  const parts: string[] = [];
  for (const [unit, size] of [["hour", 3600], ["minute", 60], ["second", 1]] as const) {
    const value = Math.floor(seconds / size);
    seconds %= size;
    if (value || (unit === "second" && !parts.length)) {
      parts.push(new Intl.NumberFormat(locale, { style: "unit", unit, unitDisplay: "short" }).format(value));
    }
  }
  return parts.join(" ");
}

/** Disabling renewal keeps an already-paid monthly service until its period end. */
export function walletReleaseScheduled(contract: WalletLifecycle | undefined, pricingModel: string) {
  return pricingModel === "prepaid_calendar_month" && contract?.status === "active" && contract.renewalPolicy === "manual";
}
