import { onlineRequest } from "./online-client-market";

/** Whether the signed-in user sells on the Client market. The server decides
 * `canSell`: owning an active Client, or still having listings, rentals or
 * past rentals to look back on. */
export type ClientSellerSummary = {
  ownedClients: number;
  listed: number;
  rented: number;
  history: number;
  canSell: boolean;
};

const count = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;

export function parseClientSellerSummary(value: unknown): ClientSellerSummary {
  const v = value as Record<string, unknown> | null;
  if (!v || typeof v !== "object" || !["ownedClients", "listed", "rented", "history"].every((key) => count(v[key])) || typeof v.canSell !== "boolean") {
    throw new Error("Invalid Client seller summary");
  }
  return v as unknown as ClientSellerSummary;
}

export async function getClientSellerSummary(signal?: AbortSignal) {
  return parseClientSellerSummary(await onlineRequest<unknown>("seller-summary", "GET", undefined, signal));
}
