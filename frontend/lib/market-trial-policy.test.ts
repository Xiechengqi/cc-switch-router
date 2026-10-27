import assert from "node:assert/strict";
import test from "node:test";
import { validateTrialPolicy } from "./market-trial-policy";
import { shareTrialSummary } from "./share-trial";
import type { ShareMarketSubscription } from "./types";

test("market defaults are explicit, bounded and do not guess on failed lookups", () => {
  const policy = { version: 1, clientHours: 12, shareHours: 12, shareTokens: 1_000_000 };
  assert.deepEqual(validateTrialPolicy(policy), policy);
  assert.equal(validateTrialPolicy({ ...policy, clientHours: 0, shareHours: 720, shareTokens: 0 }).shareTokens, 0);
  for (const patch of [{ version: 0 }, { clientHours: -1 }, { shareHours: 721 }, { shareHours: 1.5 }, { shareTokens: -1 }, { shareTokens: Number.MAX_SAFE_INTEGER + 1 }, { shareTokens: 1.5 }]) {
    assert.throws(() => validateTrialPolicy({ ...policy, ...patch }));
  }
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
