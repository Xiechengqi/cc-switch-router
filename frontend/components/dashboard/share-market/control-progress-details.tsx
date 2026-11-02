"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { controlProgressMessageKey, controlRetryTime } from "@/lib/share-control-progress";
import type { ShareMarketSubscription } from "@/lib/types";

/** Private diagnostics inside the existing rental details, never a top queue. */
export function ShareControlProgressDetails({ progress }: {
  progress: ShareMarketSubscription["controlProgress"];
}) {
  const { locale, t } = useLocaleText();
  if (!progress) return null;
  const message = controlProgressMessageKey(progress.stage);
  if (!message) return null;
  const retryTime = progress.stage === "retry_scheduled"
    ? controlRetryTime(progress.nextAttemptAt, locale) : null;
  return (
    <div role="status" className="grid gap-0.5">
      <p className="font-medium">{t(message)}</p>
      {retryTime ? <p>{t("shareMarket.controlProgress.nextRetry", { time: retryTime })}</p> : null}
      <p>{t("shareMarket.authorizationFailure.attempts", { count: progress.attempts })}</p>
    </div>
  );
}
