import type { RouterUpgradeLog, RouterUpgradeStatus, RouterUpgradeTask } from "@/lib/types";

export const ROUTER_UPGRADE_DISMISSED_PREFIX = "cc_switch_router_upgrade_dismissed_v1:";
export const ROUTER_UPGRADE_LOG_LIMIT = 1000;

export function isRouterUpgradeTerminal(status: RouterUpgradeStatus) {
  return ["success", "failed", "cancelled", "interrupted"].includes(status);
}

export function routerUpgradeDismissedKey(accountId: string) {
  return `${ROUTER_UPGRADE_DISMISSED_PREFIX}${encodeURIComponent(accountId)}`;
}

export function mergeRouterUpgradeLogs(
  current: RouterUpgradeLog[], incoming: RouterUpgradeLog[], taskId: string,
) {
  const lines = new Map<number, RouterUpgradeLog>();
  for (const entry of [...current, ...incoming]) {
    if (entry.taskId === taskId && Number.isSafeInteger(entry.seq) && entry.seq > 0) lines.set(entry.seq, entry);
  }
  return [...lines.values()].sort((left, right) => left.seq - right.seq).slice(-ROUTER_UPGRADE_LOG_LIMIT);
}

/** A poll can finish after a newer SSE state. It must never regress a task. */
export function mergeRouterUpgradeTask(current: RouterUpgradeTask | null, incoming: RouterUpgradeTask) {
  if (!current) return incoming;
  if (current.taskId !== incoming.taskId) {
    return Date.parse(incoming.startedAt) < Date.parse(current.startedAt) ? current : incoming;
  }
  const newest = incoming.revision >= current.revision ? incoming : current;
  return { ...newest, logs: mergeRouterUpgradeLogs(current.logs, incoming.logs, newest.taskId) };
}
