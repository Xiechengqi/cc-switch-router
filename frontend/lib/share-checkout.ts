import { ApiError } from "./api";
import type { ShareMarketRentQuote } from "./types";
import { validPaidWalletQuoteFunding, validWalletQuoteFunding } from "./market-wallet-quote";

/** A paid quote may never fall back to a retired Provider funding path. */
export function shareCheckoutState(quote?: ShareMarketRentQuote) {
  const paid = !!quote && (quote.offer.dailyRateMinor != null || quote.offer.cyclePriceMinor != null);
  const funding = quote?.walletFunding;
  const valid = paid ? validPaidWalletQuoteFunding(funding, quote!.offer.cyclePriceMinor ?? quote!.offer.dailyRateMinor ?? 0) : validWalletQuoteFunding(funding);
  const shortfall = valid && BigInt(funding!.shortfallUnits) > 0n;
  return { paid, shortfall, unsupported: paid && (!valid || quote?.offer.currency !== "USD") };
}

export function isWalletQuoteConflict(error: unknown) {
  return error instanceof ApiError && error.code === "MARKET_WALLET_REQUIRED";
}
