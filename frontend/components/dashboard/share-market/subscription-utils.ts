import type { ShareMarketSubscription } from "@/lib/types";

function isTerminalSubscription(status: string) {
  return status === "released" || status === "grant_failed";
}

export function mergeShareMarketSubscriptionPage(
  current: ShareMarketSubscription[],
  incoming: ShareMarketSubscription[],
  appendHistory: boolean,
) {
  const retained = appendHistory
    ? current
    : current.filter((subscription) => isTerminalSubscription(subscription.status));
  const merged = new Map(retained.map((subscription) => [subscription.id, subscription]));
  for (const subscription of incoming) merged.set(subscription.id, subscription);
  return [...merged.values()];
}
