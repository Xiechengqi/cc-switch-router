import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";

export type CommissionTerms = { policyVersion: number; rateBps: number; effectiveAt: string };
export type CommissionPolicy = { version: number; current: CommissionTerms; scheduled: CommissionTerms | null; increaseNoticeDays: number; canEdit: boolean };
export async function getCommissionPolicy(admin = false) {
  return validCommissionPolicy(await parseJson<CommissionPolicy>(await authFetch(admin ? "/v1/admin/market-finance/commission-policy" : "/v1/market-wallet/commission-policy", { cache: "no-store" })));
}
export async function saveCommissionPolicy(version: number, rateBps: number) {
  return validCommissionPolicy(await parseJson<CommissionPolicy>(await authFetch("/v1/admin/market-finance/commission-policy", {
    method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ expectedVersion: version, rateBps }),
  })));
}
function validCommissionPolicy(value: CommissionPolicy) {
  const valid = (terms: CommissionTerms | null | undefined) => !!terms && Number.isSafeInteger(terms.policyVersion) && terms.policyVersion > 0 && Number.isInteger(terms.rateBps) && terms.rateBps >= 0 && terms.rateBps <= 10000 && typeof terms.effectiveAt === "string" && Number.isFinite(Date.parse(terms.effectiveAt));
  if (!value || !Number.isSafeInteger(value.version) || value.version < 1 || typeof value.canEdit !== "boolean" || value.increaseNoticeDays !== 7 || !valid(value.current) || (value.scheduled !== null && !valid(value.scheduled))) throw new Error("Service fee policy unavailable; reload to retry.");
  return value;
}

export type WalletPolicy = {
  version: number;
  maintenanceFeeBps: number;
  transfersEnabled: boolean;
  transfersPaused: boolean;
  transferMaxMinor: number;
  transferDailyMaxMinor: number;
  balanceRefundable: boolean;
  canEdit?: boolean;
};
export type TopupQuote = {
  id: string;
  creditMinor: number;
  feeBps: number;
  feeMinor: number;
  payMinor: number;
  policyVersion: number;
  channelRevision: number;
  channelSettingsVersion: number;
  expiresAt: string;
};
export type TransferCapacity = {
  transferableUnits: string;
  committedUnits: string;
  dailyRemainingUnits: string;
  enabled: boolean;
  verified: boolean;
  blockers: ("disabled" | "paused" | "identity" | "finance_hold")[];
};
export type TransferQuote = {
  id: string;
  senderEmail: string;
  recipientEmail: string;
  amountUnits: string;
  expiresAt: string;
};
export type WalletTransfer = {
  id: string;
  senderEmail: string;
  recipientEmail: string;
  amountUnits: string;
  journalId: string;
  createdAt: string;
};

export async function getWalletPolicy(admin = false) {
  return parseJson<WalletPolicy>(
    await authFetch(
      admin
        ? "/v1/admin/market-finance/wallet-policy"
        : "/v1/market-wallet/policy",
      { cache: "no-store" },
    ),
  );
}
export async function saveWalletPolicy(policy: WalletPolicy) {
  const {
    version,
    balanceRefundable: _unused,
    canEdit: _canEdit,
    ...fields
  } = policy;
  return parseJson<WalletPolicy>(
    await authFetch("/v1/admin/market-finance/wallet-policy", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...fields, expectedVersion: version }),
    }),
  );
}
export async function quoteWalletTopup(
  channelId: string,
  amountMinor: number,
  tokenId?: string,
) {
  return parseJson<TopupQuote>(
    await authFetch("/v1/market-wallet/topup-quotes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        channelId,
        amountMinor,
        ...(tokenId ? { tokenId } : {}),
      }),
    }),
  );
}
export async function getTransferCapacity() {
  return parseJson<TransferCapacity>(
    await authFetch("/v1/market-wallet/transfer-capacity", {
      cache: "no-store",
    }),
  );
}
export async function quoteWalletTransfer(
  recipientEmail: string,
  amountMinor: number | null,
  all: boolean,
) {
  return parseJson<TransferQuote>(
    await authFetch("/v1/market-wallet/transfer-quotes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ recipientEmail, amountMinor, all }),
    }),
  );
}
export async function confirmWalletTransfer(
  quoteId: string,
  idempotencyKey: string,
) {
  return parseJson<WalletTransfer>(
    await authFetch("/v1/market-wallet/transfers", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "Idempotency-Key": idempotencyKey,
      },
      body: JSON.stringify({ quoteId }),
    }),
  );
}
