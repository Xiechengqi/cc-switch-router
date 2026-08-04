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

/** Scalar fields of `ClientMarketHost`; payment collections and `ipIntel` are
 *  compared separately because they are not primitives. */
const HOST_SCALAR_KEYS = [
  "id",
  "providerId",
  "ip",
  "port",
  "hostOwnerEmail",
  "dailyRateMinor",
  "currency",
  "freeDurationDays",
  "offerRevision",
  "countryCode",
  "hostname",
  "sshHostKeyFingerprint",
  "status",
  "clientSubdomain",
  "clientOwnerEmail",
  "installationId",
  "sellerApprovalRequired",
  "canWebTerminal",
  "isHostOwner",
  "isClientOwner",
  "canControlRecovery",
  "lastVerifiedAt",
  "lastError",
  "note",
  "createdAt",
  "updatedAt",
] as const satisfies readonly (keyof ClientMarketHost)[];

function sameStringList(left?: readonly string[], right?: readonly string[]): boolean {
  if (left === right) return true;
  if (!left || !right) return !left && !right;
  if (left.length !== right.length) return false;
  return left.every((value, index) => value === right[index]);
}

function sameHost(left: ClientMarketHost, right: ClientMarketHost): boolean {
  if (left === right) return true;
  for (const key of HOST_SCALAR_KEYS) {
    if (left[key] !== right[key]) return false;
  }
  if (!sameStringList(left.paymentMethodKinds, right.paymentMethodKinds)) return false;
  if (JSON.stringify(left.contacts ?? []) !== JSON.stringify(right.contacts ?? [])) return false;
  if (JSON.stringify(left.eligibility) !== JSON.stringify(right.eligibility)) return false;
  if (JSON.stringify(left.recovery ?? null) !== JSON.stringify(right.recovery ?? null)) return false;
  // ipIntel is a nested object refreshed rarely; only pay for it once the cheap
  // scalar pass has found no difference.
  return JSON.stringify(left.ipIntel ?? null) === JSON.stringify(right.ipIntel ?? null);
}

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

const RENTAL_SCALAR_KEYS = [
  "installationId",
  "hostId",
  "providerId",
  "hostOwnerEmail",
  "clientOwnerEmail",
  "status",
  "dailyRateMinor",
  "currency",
  "offerRevision",
  "isClientOwner",
  "canRelease",
  "activeCleanupJobId",
  "updatedAt",
] as const satisfies readonly (keyof ClientMarketRental)[];

function sameRental(left: ClientMarketRental, right: ClientMarketRental): boolean {
  if (left === right) return true;
  for (const key of RENTAL_SCALAR_KEYS) {
    if (left[key] !== right[key]) return false;
  }
  if (!sameStringList(left.paymentMethodKinds, right.paymentMethodKinds)) return false;
  if (JSON.stringify(left.contacts ?? []) !== JSON.stringify(right.contacts ?? [])) return false;
  return true;
}

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
