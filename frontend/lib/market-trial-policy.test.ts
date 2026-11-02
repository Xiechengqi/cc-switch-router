import assert from "node:assert/strict";
import test from "node:test";
import { validateTrialPolicy } from "./market-trial-policy";
import { shareTrialSummary } from "./share-trial";
import type { ShareMarketSubscription } from "./types";

test("market defaults are explicit, bounded and do not guess on failed lookups", () => {
  const policy = { version: 1, clientHours: 12, shareHours: 12, shareTokens: 1_000_000, shareValueUsdMicros: "1000000" };
  assert.deepEqual(validateTrialPolicy(policy), policy);
  assert.equal(validateTrialPolicy({ ...policy, clientHours: 0, shareHours: 720, shareTokens: 0 }).shareTokens, 0);
  for (const patch of [{ version: 0 }, { clientHours: -1 }, { shareHours: 721 }, { shareHours: 1.5 }, { shareTokens: -1 }, { shareTokens: Number.MAX_SAFE_INTEGER + 1 }, { shareTokens: 1.5 }]) {
    assert.throws(() => validateTrialPolicy({ ...policy, ...patch }));
  }
  for (const shareValueUsdMicros of ["", "01", "1.5", "-1", "1000000000000001"]) assert.throws(() => validateTrialPolicy({ ...policy, shareValueUsdMicros }));
  assert.equal(validateTrialPolicy({ ...policy, shareValueUsdMicros: "0" }).shareValueUsdMicros, "0");
});

test("USD trial summaries preserve micro-dollar precision and show time-only explicitly", () => {
  const t = (key: string, params?: Record<string, string | number>) => `${key}:${params?.amount ?? ""}`;
  const rental = { trial: { phase: "trial", tokenAllowance: 0, tokensUsed: 0, tokensRemaining: null, valueAllowanceUsdMicros: "1000000", valueUsedUsdMicros: "999999", valueRemainingUsdMicros: "1" } } as ShareMarketSubscription;
  assert.equal(shareTrialSummary(rental, "en", t), "marketTrial.valueRemaining:US$0.000001");
  assert.equal(shareTrialSummary({ ...rental, trial: { ...rental.trial!, valueAllowanceUsdMicros: "0", valueRemainingUsdMicros: undefined } }, "en", t), "marketTrial.timeOnly:");
});

test("trial summary keeps a one-time budget separate from periodic quota", () => {
  const t = (key: string, params?: Record<string, string | number>) => `${key}:${params?.tokens ?? ""}`;
  const rental = { trial: { phase: "trial", tokenAllowance: 1_000_000, tokensUsed: 250_000, tokensRemaining: 750_000 } } as ShareMarketSubscription;
  assert.match(shareTrialSummary(rental, "en", t), /^marketTrial.remaining:/);
  assert.equal(shareTrialSummary({ ...rental, trial: { ...rental.trial!, tokensRemaining: null } }, "en", t), "marketTrial.timeOnly:");
  for (const [phase, key] of [["activating", "converting"], ["revoking", "releasing"]] as const) {
    assert.equal(shareTrialSummary({ ...rental, trial: { ...rental.trial!, phase } }, "en", t), `marketTrial.${key}:`);
  }
  assert.equal(shareTrialSummary({ ...rental, trial: { ...rental.trial!, phase: "paid" } }, "en", t), "");
});
