import type { ShareMarketSubscription } from "@/lib/types";
import { formatTokenMillions } from "@/lib/token-units";
import { formatUsdLimit } from "@/lib/share-value-quota";

export function shareTrialSummary(subscription: ShareMarketSubscription, locale: string, t: (key: "marketTrial.converting" | "marketTrial.releasing" | "marketTrial.timeOnly" | "marketTrial.remaining" | "marketTrial.valueRemaining", params?: Record<string, string | number>) => string): string {
  const trial = subscription.trial;
  if (!trial || trial.phase === "paid") return "";
  if (trial.phase === "activating") return t("marketTrial.converting");
  if (trial.phase === "revoking" || subscription.walletContract?.status === "ending") return t("marketTrial.releasing");
  if (trial.valueAllowanceUsdMicros != null) return trial.valueRemainingUsdMicros == null
    ? t("marketTrial.timeOnly")
    : t("marketTrial.valueRemaining", { amount: formatUsdLimit(trial.valueRemainingUsdMicros) });
  return trial.tokensRemaining == null ? t("marketTrial.timeOnly") : t("marketTrial.remaining", { tokens: formatTokenMillions(trial.tokensRemaining, locale) });
}
