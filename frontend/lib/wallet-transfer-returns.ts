import { authFetch, readAuthState } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import type { WalletTransfer } from "@/lib/wallet-policy-api";
export type TransferReturn = { id: string; transferId: string; requestedBy: string; reason: string; status: "requested" | "approved" | "declined"; version: number; journalId: string | null; createdAt: string; resolvedAt: string | null };
function validReturn(r: TransferReturn | null, transfer: string) {
  if (r !== null && (!r || r.transferId !== transfer || !["requested", "approved", "declined"].includes(r.status) || !Number.isSafeInteger(r.version) || r.version < 1 || typeof r.reason !== "string" || typeof r.id !== "string" || !Number.isFinite(Date.parse(r.createdAt)))) throw new Error("Transfer return unavailable; reload.");
  return r;
}
export async function getTransferReturns(offset = 0) {
  const context = readAuthState();
  const page = await parseJson<{ items: WalletTransfer[]; total: number }>(await authFetch(`/v1/market-wallet/transfers?offset=${offset}&limit=20`, { cache: "no-store" }, context));
  if (!page || !Array.isArray(page.items) || !Number.isSafeInteger(page.total) || page.total < 0 || page.items.some(t => !t || !/^[a-f0-9-]{36}$/.test(t.id) || typeof t.senderEmail !== "string" || typeof t.recipientEmail !== "string" || !/^[1-9]\d{0,18}$/.test(t.amountUnits) || BigInt(t.amountUnits) > 9_223_372_036_854_775_807n)) throw new Error("Transfers unavailable; reload.");
  const items = await Promise.all(page.items.map(async transfer => ({ transfer, request: validReturn(await parseJson<TransferReturn | null>(await authFetch(`/v1/market-wallet/transfers/${transfer.id}/return`, { cache: "no-store" }, context)), transfer.id) })));
  return { items, total: page.total };
}
export async function requestTransferReturn(id: string, reason: string) { return validReturn(await parseJson<TransferReturn>(await authFetch(`/v1/market-wallet/transfers/${encodeURIComponent(id)}/return`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ reason }) })), id)!; }
export async function decideTransferReturn(id: string, version: number, approve: boolean) { return validReturn(await parseJson<TransferReturn>(await authFetch(`/v1/market-wallet/transfers/${encodeURIComponent(id)}/return/${approve ? "approve" : "decline"}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ expectedVersion: version }) })), id)!; }
