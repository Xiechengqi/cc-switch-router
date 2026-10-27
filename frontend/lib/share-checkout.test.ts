import assert from "node:assert/strict";
import test from "node:test";
import { ApiError } from "./api";
import { shareCheckoutState, isWalletQuoteConflict } from "./share-checkout";
import { validWalletQuoteFunding } from "./market-wallet-quote";
import type { ShareMarketRentQuote } from "./types";

const quote = {
  offer: { dailyRateMinor: 100, currency: "USD" },
  walletFunding: {
    assetCode: "USD", availableUnits: "8640100", heldUnits: "10",
    committedUnits: "100", requiredUnits: "8640000", shortfallUnits: "0",
  },
} as ShareMarketRentQuote;

test("paid Share uses platform balance, accounting for existing commitments", () => {
  assert.deepEqual(shareCheckoutState(quote), { paid: true, shortfall: false, unsupported: false });
  const funding = { ...quote.walletFunding!, committedUnits: "101", shortfallUnits: "1" };
  assert.deepEqual(shareCheckoutState({ ...quote, walletFunding: funding }),
    { paid: true, shortfall: true, unsupported: false });
  assert.equal(validWalletQuoteFunding({ ...funding, availableUnits: "9999999", shortfallUnits: "0" }), true);
});

test("Share trials cannot bypass the first-period balance requirement", () => {
  const trial = { ...quote, trialSecondsRemaining: 43200 };
  assert.deepEqual(shareCheckoutState({ ...trial, walletFunding: { ...quote.walletFunding!, availableUnits: "0", committedUnits: "0", shortfallUnits: "8640000" } }), { paid: true, shortfall: true, unsupported: false });
  assert.equal(shareCheckoutState({ ...trial, walletFunding: { ...quote.walletFunding!, committedUnits: "0", requiredUnits: "0" } }).unsupported, true);
});

test("invalid paid funding fails closed, never enables a Provider payment fallback", () => {
  assert.equal(shareCheckoutState({ ...quote, walletFunding: undefined }).unsupported, true);
  for (const field of ["availableUnits", "heldUnits", "committedUnits", "requiredUnits", "shortfallUnits"]) {
    for (const value of [undefined, "-1", "NaN", "1.5", "9223372036854775808"]) {
      const funding = { ...quote.walletFunding!, [field]: value };
      assert.equal(shareCheckoutState({ ...quote, walletFunding: funding }).unsupported, true, `${field}: ${value}`);
    }
  }
  assert.equal(shareCheckoutState({ ...quote, walletFunding: { ...quote.walletFunding!, shortfallUnits: "1" } }).unsupported, true);
  assert.equal(shareCheckoutState({ ...quote, offer: { ...quote.offer, currency: "CNY" as "USD" } }).unsupported, true);
});

test("free quotes require no wallet and wallet conflicts are specifically identified", () => {
  assert.deepEqual(shareCheckoutState(), { paid: false, shortfall: false, unsupported: false });
  assert.equal(shareCheckoutState({ ...quote, offer: { ...quote.offer, dailyRateMinor: undefined }, walletFunding: undefined }).unsupported, false);
  assert.equal(isWalletQuoteConflict(new Error("MARKET_WALLET_REQUIRED")), false);
  assert.equal(isWalletQuoteConflict(new ApiError(409, "reload quote", "MARKET_WALLET_REQUIRED")), true);
});
