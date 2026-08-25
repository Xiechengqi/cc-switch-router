import type { ShareMarketSubscription } from "@/lib/types";

const ATTENTION_STATUSES = new Set([
  "billing_control_failed",
  "revoke_failed",
  "grant_failed",
  "billing_suspended",
]);

const ANOMALY_RANK = [
  "billing_control_failed",
  "revoke_failed",
  "grant_failed",
  "billing_suspended",
  "billing_suspend_pending",
  "billing_resume_pending",
  "revoke_pending",
  "grant_pending",
];

function isTerminalSubscription(status: string) {
  return status === "released" || status === "grant_failed";
}

export function isHistorySubscription(status: string) {
  return status === "released";
}

export function needsRentalAttention(subscription: Pick<
  ShareMarketSubscription,
  "status" | "integrityState" | "priceChange"
>) {
  return ATTENTION_STATUSES.has(subscription.status)
    || (subscription.integrityState != null && subscription.integrityState !== "compatible")
    || subscription.priceChange?.status === "pending";
}

function anomalyRank(status: string) {
  const rank = ANOMALY_RANK.indexOf(status);
  return rank < 0 ? 99 : rank;
}

export function sortShareMarketSubscriptions(
  left: ShareMarketSubscription,
  right: ShareMarketSubscription,
) {
  return (
    anomalyRank(left.status) - anomalyRank(right.status)
    || Date.parse(right.updatedAt) - Date.parse(left.updatedAt)
    || left.shareName.localeCompare(right.shareName)
  );
}

export function partitionShareMarketSubscriptions(subscriptions: ShareMarketSubscription[]) {
  const attention: ShareMarketSubscription[] = [];
  const active: ShareMarketSubscription[] = [];
  const history: ShareMarketSubscription[] = [];
  for (const subscription of [...subscriptions].sort(sortShareMarketSubscriptions)) {
    if (isHistorySubscription(subscription.status)) history.push(subscription);
    else if (needsRentalAttention(subscription)) attention.push(subscription);
    else active.push(subscription);
  }
  return { attention, active, history };
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
