import type { ClientMarketBilling, ClientMarketHost } from "@/lib/types";

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
 * `ClientMarketHost` / `ClientMarketBilling` it MUST be added here too, otherwise the
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
  "priceCents",
  "rentalPeriodDays",
  "offerRevision",
  "countryCode",
  "hostname",
  "sshHostKeyFingerprint",
  "status",
  "clientSubdomain",
  "clientOwnerEmail",
  "installationId",
  "canWebTerminal",
  "isHostOwner",
  "isClientOwner",
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
  if (JSON.stringify(left.paymentMethods ?? []) !== JSON.stringify(right.paymentMethods ?? [])) return false;
  if (JSON.stringify(left.contacts ?? []) !== JSON.stringify(right.contacts ?? [])) return false;
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

const BILLING_SCALAR_KEYS = [
  "installationId",
  "hostId",
  "providerId",
  "hostOwnerEmail",
  "clientOwnerEmail",
  "status",
  "priceCents",
  "rentalPeriodDays",
  "offerRevision",
  "currentPeriodEnd",
  "paymentDeadline",
  "openInvoiceId",
  "paymentProfileUpdatedAt",
  "isClientOwner",
  "canDeclarePaid",
  "canRelease",
  "activeCleanupJobId",
  "updatedAt",
] as const satisfies readonly (keyof ClientMarketBilling)[];

function sameBilling(left: ClientMarketBilling, right: ClientMarketBilling): boolean {
  if (left === right) return true;
  for (const key of BILLING_SCALAR_KEYS) {
    if (left[key] !== right[key]) return false;
  }
  if (!sameStringList(left.paymentMethodKinds, right.paymentMethodKinds)) return false;
  if (JSON.stringify(left.contacts ?? []) !== JSON.stringify(right.contacts ?? [])) return false;
  return (
    JSON.stringify(left.paymentMethods ?? null) === JSON.stringify(right.paymentMethods ?? null)
  );
}

export function mergeBillingMap(
  prev: Map<string, ClientMarketBilling>,
  next: ClientMarketBilling[],
): Map<string, ClientMarketBilling> {
  let changed = prev.size !== next.length;
  const merged = new Map<string, ClientMarketBilling>();
  for (const record of next) {
    const existing = prev.get(record.installationId);
    if (existing && sameBilling(existing, record)) {
      merged.set(record.installationId, existing);
      continue;
    }
    changed = true;
    merged.set(record.installationId, record);
  }
  return changed ? merged : prev;
}
