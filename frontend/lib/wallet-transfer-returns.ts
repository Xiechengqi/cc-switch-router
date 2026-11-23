import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

/**
 * Legacy return requests. New return requests are retired; requests made
 * earlier still wait for the recipient's decision.
 */
export type AwaitingReturn = { id: string; transferId: string; reason: string; version: number; createdAt: string; senderEmail: string; amountUnits: string };

const UNITS = /^[1-9]\d{0,18}$/;
const ID = /^[a-f0-9-]{36}$/;

export async function getAwaitingReturns(): Promise<AwaitingReturn[]> {
  const page = await parseJson<{ items: AwaitingReturn[] }>(await authFetch("/v1/market-wallet/transfer-returns/awaiting", { cache: "no-store" }));
  if (!page || !Array.isArray(page.items) || page.items.some(r => !r || !ID.test(r.transferId) || !UNITS.test(r.amountUnits) || !Number.isSafeInteger(r.version))) throw new Error("Return requests unavailable; reload.");
  return page.items;
}

export async function decideTransferReturn(transferId: string, version: number, approve: boolean) {
  return parseJson<unknown>(await authFetch(`/v1/market-wallet/transfers/${encodeURIComponent(transferId)}/return/${approve ? "approve" : "decline"}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ expectedVersion: version }) }));
}
