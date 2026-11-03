import { authFetch, readAuthState } from "./auth";
import { parseJson } from "./api";
import type { ShareMarketListing, ShareMarketSeat, ShareMarketSubscription } from "./types";

export type ClientShareEntry = {
  shareId: string;
  entryKind: "none" | "rent" | "rental" | "settlement";
  vacantSeats: number;
  subdomain: string | null;
  resourceState: "listed" | "unlisted" | "inactive" | "missing" | "unavailable";
};
export type ClientShareEntryPage = { items: ClientShareEntry[]; nextCursor: string | null };
export type ClientShareSeatContext = {
  shareId: string;
  resourceState: ClientShareEntry["resourceState"];
  listing: ShareMarketListing | null;
  subscriptions: ShareMarketSubscription[];
  nextCursor: string | null;
};
const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const id = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 256 && !/[\u0000-\u001f\u007f,]/.test(v);
const cursor = (v: unknown): v is string | null => v === null || typeof v === "string" && v.length > 0 && v.length <= 2048;
const resource = (v: unknown) => ["listed", "unlisted", "inactive", "missing", "unavailable"].includes(v as string);

export function parseClientShareEntries(value: unknown, requested?: readonly string[]): ClientShareEntryPage {
  if (!object(value) || !Array.isArray(value.items) || !cursor(value.nextCursor)) throw new Error("Invalid Share entries");
  const seen = new Set<string>();
  for (const v of value.items) {
    if (!object(v) || !id(v.shareId) || seen.has(v.shareId) || requested && !requested.includes(v.shareId)
      || !["none", "rent", "rental", "settlement"].includes(v.entryKind as string)
      || !Number.isSafeInteger(v.vacantSeats) || (v.vacantSeats as number) < 0 || !resource(v.resourceState)
      || !(v.subdomain === null || typeof v.subdomain === "string")) throw new Error("Invalid Share entry");
    seen.add(v.shareId);
  }
  if (requested && new Set(requested).size !== seen.size) throw new Error("Incomplete Share entries");
  return value as ClientShareEntryPage;
}

export function parseClientSeatContext(value: unknown, shareId: string): ClientShareSeatContext {
  if (!object(value) || value.shareId !== shareId || !resource(value.resourceState) || !cursor(value.nextCursor)
    || !Array.isArray(value.subscriptions)) throw new Error("Invalid Share seat context");
  const seen = new Set<string>();
  for (const sub of value.subscriptions) {
    if (!object(sub) || !id(sub.id) || seen.has(sub.id) || sub.shareId !== shareId || !id(sub.seatId)
      || typeof sub.status !== "string" || typeof sub.canRelease !== "boolean"
      || typeof sub.canTerminateImmediately !== "boolean") throw new Error("Invalid private rental context");
    seen.add(sub.id);
  }
  if (value.listing !== null) {
    const listing = value.listing;
    if (!object(listing) || !id(listing.id) || listing.shareId !== shareId || !Array.isArray(listing.seats)) throw new Error("Invalid public seats");
    const seatIds = new Set<string>();
    for (const seat of listing.seats) {
      // Occupancy is public, another renter's subscription never is.
      if (!object(seat) || !id(seat.id) || seatIds.has(seat.id) || seat.subscription != null
        || typeof seat.status !== "string" || typeof seat.canRent !== "boolean" || !Number.isSafeInteger(seat.position)) throw new Error("Invalid public seat");
      seatIds.add(seat.id);
    }
  }
  return value as ClientShareSeatContext;
}

async function get(path: string, signal?: AbortSignal) {
  const timeout = AbortSignal.timeout(12_000);
  return parseJson<unknown>(await authFetch(path, { cache: "no-store", signal: signal ? AbortSignal.any([signal, timeout]) : timeout }, readAuthState()));
}
export async function getClientShareEntries(ids: readonly string[], signal?: AbortSignal) {
  const unique = [...new Set(ids)].sort();
  if (unique.length > 100 || !unique.every(id)) throw new Error("Invalid Share IDs");
  if (!unique.length) return { items: [], nextCursor: null } as ClientShareEntryPage;
  const q = new URLSearchParams({ shareIds: unique.join(",") });
  return parseClientShareEntries(await get(`/v1/share-market/client-entries?${q}`, signal), unique);
}
export async function getClientRentalContexts(after?: string | null, signal?: AbortSignal) {
  const q = new URLSearchParams();
  if (after) q.set("cursor", after);
  return parseClientShareEntries(await get(`/v1/share-market/me/client-rental-contexts?${q}`, signal));
}
export async function getClientSeatContext(shareId: string, options: { cursor?: string | null; subscriptionId?: string } = {}, signal?: AbortSignal) {
  if (!id(shareId)) throw new Error("Invalid Share ID");
  const q = new URLSearchParams();
  if (options.cursor) q.set("cursor", options.cursor);
  if (options.subscriptionId) q.set("subscriptionId", options.subscriptionId);
  return parseClientSeatContext(await get(`/v1/share-market/shares/${encodeURIComponent(shareId)}/seat-context?${q}`, signal), shareId);
}

export function clientShareHref(shareId: string, seatId?: string, subscriptionId?: string) {
  const q = new URLSearchParams({ marketShare: shareId });
  if (seatId) q.set("seatId", seatId);
  if (subscriptionId) q.set("subscriptionId", subscriptionId);
  return `/?${q}`;
}

/** Match the market's approval gate: access approval cannot repair an
 * offline, exhausted, or otherwise non-rentable service. */
export function clientSeatAction(seat: ShareMarketSeat, authenticated: boolean) {
  if (seat.readOnly || seat.status !== "available") return seat.status === "occupied" ? "occupied" : "unavailable";
  if (!authenticated) return "login";
  if (seat.canRent) return "rent";
  if (seat.rentPrerequisitesMet && seat.eligibility?.status === "access_required") return "approval";
  return "unavailable";
}

export function clientSeatGroups(context: ClientShareSeatContext) {
  const live = context.subscriptions.filter(s => !["released", "grant_failed"].includes(s.status));
  const ownIds = new Set(live.map(s => s.seatId));
  const remaining = (context.listing?.seats ?? []).filter(s => !ownIds.has(s.id));
  const sort = (a: ShareMarketSeat, b: ShareMarketSeat) => a.position - b.position || a.id.localeCompare(b.id);
  return {
    mine: live.sort((a, b) => a.seatPosition - b.seatPosition || a.id.localeCompare(b.id)),
    available: remaining.filter(s => s.status === "available" && !s.readOnly).sort(sort),
    occupied: remaining.filter(s => s.status !== "available" || s.readOnly).sort(sort),
    history: context.subscriptions.filter(s => ["released", "grant_failed"].includes(s.status)),
  };
}
