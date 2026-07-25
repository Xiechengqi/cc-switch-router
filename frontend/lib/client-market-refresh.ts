import type { ClientMarketBilling, ClientMarketHost } from "@/lib/types";

export function fingerprintHost(host: ClientMarketHost): string {
  return JSON.stringify(host);
}

export function fingerprintHosts(hosts: ClientMarketHost[]): string {
  return JSON.stringify(
    [...hosts]
      .sort((left, right) => left.id.localeCompare(right.id))
      .map((host) => fingerprintHost(host)),
  );
}

export function mergeHosts(prev: ClientMarketHost[], next: ClientMarketHost[]): ClientMarketHost[] {
  if (fingerprintHosts(prev) === fingerprintHosts(next)) return prev;
  const prevById = new Map(prev.map((host) => [host.id, host]));
  return next.map((host) => {
    const existing = prevById.get(host.id);
    if (existing && fingerprintHost(existing) === fingerprintHost(host)) return existing;
    return host;
  });
}

export function fingerprintBillingMap(map: Map<string, ClientMarketBilling>): string {
  return JSON.stringify(
    Array.from(map.entries())
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([, record]) => record),
  );
}

export function mergeBillingMap(
  prev: Map<string, ClientMarketBilling>,
  next: ClientMarketBilling[],
): Map<string, ClientMarketBilling> {
  const nextMap = new Map(next.map((record) => [record.installationId, record]));
  if (fingerprintBillingMap(prev) === fingerprintBillingMap(nextMap)) return prev;
  const merged = new Map<string, ClientMarketBilling>();
  for (const [installationId, record] of nextMap) {
    const existing = prev.get(installationId);
    if (existing && JSON.stringify(existing) === JSON.stringify(record)) {
      merged.set(installationId, existing);
    } else {
      merged.set(installationId, record);
    }
  }
  return merged;
}
