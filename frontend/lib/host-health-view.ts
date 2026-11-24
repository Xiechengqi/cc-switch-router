import type { ClientMarketHost, HostHealth } from "./types";

export function canBootstrapHost(host: ClientMarketHost) {
  return !!host.isHostOwner && host.status === "allocated" && !!host.installationId
    && !!host.hostHealth?.deployment?.canBootstrap && Number.isSafeInteger(host.hostHealth.machineEpoch) && host.hostHealth.machineEpoch! > 0;
}

export function hostHealthDisplayState(health?: HostHealth | null) {
  if (!health) return "not_enabled";
  if (health.lastReceivedAt || health.state === "revoked") return health.state;
  if (health.deployment?.state === "queued") return "queued";
  if (health.deployment?.state === "installing") return "installing";
  if (health.deployment?.state === "failed") return "deployment_failed";
  if (health.state === "awaiting_first" && !health.nextDueAt) return "not_enabled";
  return health.state;
}

export function hostHealthRecord(health?: HostHealth | null) {
  return health?.observable24h ? `${health.received24h}/${health.observable24h}` : null;
}

export function hostHealthRecordRatio(health?: HostHealth | null) {
  return health?.observable24h ? health.received24h / health.observable24h : -1;
}

export function formatHostHealthTime(value: string, locale: string, compact = false) {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "—";
  return new Intl.DateTimeFormat(locale, compact
    ? { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }
    : { dateStyle: "medium", timeStyle: "short" }).format(date);
}
