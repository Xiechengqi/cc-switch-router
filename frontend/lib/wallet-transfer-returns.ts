import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import type { WalletTransfer } from "@/lib/wallet-policy-api";

/**
 * Pending transfers and legacy return requests.
 *
 * A transfer to a new recipient waits before it credits and only its sender
 * sees it until then. New return requests are retired; requests made earlier
 * still wait for the recipient's decision.
 */
export type AwaitingReturn = { id: string; transferId: string; reason: string; version: number; createdAt: string; senderEmail: string; amountUnits: string };

const UNITS = /^[1-9]\d{0,18}$/;
const ID = /^[a-f0-9-]{36}$/;

export async function getPendingTransfers(): Promise<WalletTransfer[]> {
  const page = await parseJson<{ items: WalletTransfer[] }>(await authFetch("/v1/market-wallet/transfers/pending", { cache: "no-store" }));
  if (!page || !Array.isArray(page.items) || page.items.some(t => !t || !ID.test(t.id) || !UNITS.test(t.amountUnits) || typeof t.recipientEmail !== "string" || typeof t.releaseAt !== "string")) throw new Error("Transfers unavailable; reload.");
  return page.items;
}

export async function withdrawPendingTransfer(id: string) {
  return parseJson<WalletTransfer>(await authFetch(`/v1/market-wallet/transfers/pending/${encodeURIComponent(id)}/cancel`, { method: "POST" }));
}

export async function getAwaitingReturns(): Promise<AwaitingReturn[]> {
  const page = await parseJson<{ items: AwaitingReturn[] }>(await authFetch("/v1/market-wallet/transfer-returns/awaiting", { cache: "no-store" }));
  if (!page || !Array.isArray(page.items) || page.items.some(r => !r || !ID.test(r.transferId) || !UNITS.test(r.amountUnits) || !Number.isSafeInteger(r.version))) throw new Error("Return requests unavailable; reload.");
  return page.items;
}

export async function decideTransferReturn(transferId: string, version: number, approve: boolean) {
  return parseJson<unknown>(await authFetch(`/v1/market-wallet/transfers/${encodeURIComponent(transferId)}/return/${approve ? "approve" : "decline"}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ expectedVersion: version }) }));
}
