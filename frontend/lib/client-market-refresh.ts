import type { ClientMarketHost, ClientMarketRental } from "@/lib/types";

/**
 * Identity-preserving merges for the 20s Client Market poll.
 *
 * The previous implementation compared whole collections by `JSON.stringify`, which
 * cost two full serializations on *every* tick even when nothing had changed — the
 * common case — and then two more per row during the merge. That is ~4N serializations
 * per poll, per open tab. These helpers keep the same "return `prev` when nothing
 * changed" contract so React can bail out of re-renders, but compare field by field
 * and stop at the first difference.
 *
 * Both comparators enumerate every field of their type. If a field is added to
 * `ClientMarketHost` / `ClientMarketRental` it MUST be added here too, otherwise the
 * table will silently render stale values.
 */

/** Requiring every key here makes a newly added API field a typecheck failure
 *  until its comparison strategy is chosen, including optional fields. */
function recordComparator<T extends object>(fields: Record<keyof T, "scalar" | "nested" | "list">) {
  const keys = Object.keys(fields) as (keyof T)[];
  const scalarKeys = keys.filter(key => fields[key] === "scalar");
  const nestedKeys = keys.filter(key => fields[key] !== "scalar");
  return (left: T, right: T): boolean => {
    if (left === right) return true;
    for (const key of scalarKeys) {
      if (left[key] !== right[key]) return false;
    }
    // Only serialize nested values after all cheap scalar checks pass. Missing
    // contacts still mean an empty list; missing nullable objects mean null.
    for (const key of nestedKeys) {
      if (left[key] === right[key]) continue;
      const fallback = fields[key] === "list" ? [] : null;
      if (JSON.stringify(left[key] ?? fallback) !== JSON.stringify(right[key] ?? fallback)) return false;
    }
    return true;
  };
}

const sameHost = recordComparator<ClientMarketHost>({
  id: "scalar",
  providerId: "scalar",
  marketProviderId: "scalar",
  ip: "scalar",
  port: "scalar",
  hostOwnerEmail: "scalar",
  dailyRateMinor: "scalar",
  pricingModel: "scalar",
  cyclePriceMinor: "scalar",
  billingInterval: "scalar",
  currency: "scalar",
  freeDurationDays: "scalar",
  trialHours: "scalar",
  defaultTrialHours: "scalar",
  offerRevision: "scalar",
  contacts: "list",
  sellerApprovalRequired: "scalar",
  eligibility: "nested",
  countryCode: "scalar",
  hostname: "scalar",
  sshHostKeyFingerprint: "scalar",
  status: "scalar",
  clientSubdomain: "scalar",
  clientOwnerEmail: "scalar",
  installationId: "scalar",
  canWebTerminal: "scalar",
  isHostOwner: "scalar",
  isClientOwner: "scalar",
  canRetireUnreachable: "scalar",
  clientConnection: "nested",
  lastVerifiedAt: "scalar",
  lastError: "scalar",
  note: "scalar",
  ipIntel: "nested",
  createdAt: "scalar",
  updatedAt: "scalar",
});

export function mergeHosts(prev: ClientMarketHost[], next: ClientMarketHost[]): ClientMarketHost[] {
  if (prev.length !== next.length) return next;
  const prevById = new Map(prev.map((host) => [host.id, host]));
  let changed = false;
  const merged = next.map((host, index) => {
    // Order is part of the rendered output, so a re-sorted list is a real change
    // even when every row is individually identical.
    if (prev[index]?.id !== host.id) changed = true;
    const existing = prevById.get(host.id);
    if (existing && sameHost(existing, host)) return existing;
    changed = true;
    return host;
  });
  return changed ? merged : prev;
}

const sameRental = recordComparator<ClientMarketRental>({
  installationId: "scalar",
  hostId: "scalar",
  providerId: "scalar",
  hostOwnerEmail: "scalar",
  clientOwnerEmail: "scalar",
  status: "scalar",
  dailyRateMinor: "scalar",
  pricingModel: "scalar",
  cyclePriceMinor: "scalar",
  billingInterval: "scalar",
  walletContractId: "scalar",
  walletContract: "nested",
  openWalletDispute: "scalar",
  currency: "scalar",
  freeDurationDays: "scalar",
  offerRevision: "scalar",
  activatedAt: "scalar",
  expiresAt: "scalar",
  contacts: "list",
  isClientOwner: "scalar",
  canRelease: "scalar",
  canFinalizeRelease: "scalar",
  hostCleanupPending: "scalar",
  cleanupFailureKind: "scalar",
  nextCleanupProbeAt: "scalar",
  cleanupProbeAttempts: "scalar",
  activeCleanupJobId: "scalar",
  providerTerminalAuthorizedUntil: "scalar",
  providerTerminalAccessActive: "scalar",
  canManageProviderTerminal: "scalar",
  updatedAt: "scalar",
});

export function mergeRentalMap(
  prev: Map<string, ClientMarketRental>,
  next: ClientMarketRental[],
): Map<string, ClientMarketRental> {
  let changed = prev.size !== next.length;
  const merged = new Map<string, ClientMarketRental>();
  for (const record of next) {
    const existing = prev.get(record.installationId);
    if (existing && sameRental(existing, record)) {
      merged.set(record.installationId, existing);
      continue;
    }
    changed = true;
    merged.set(record.installationId, record);
  }
  return changed ? merged : prev;
}
