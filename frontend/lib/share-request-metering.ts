import type { ShareOperation } from "./share-capabilities";

export type RequestMeteringRow = {
  attempt: number;
  requestId: string;
  chargeMode: "debited" | "estimate" | "none" | "unknown";
  model?: string | null;
  meteringStatus: string;
  operations: ShareOperation[];
  settledPicos?: string | null;
  pendingRiskPicos?: string | null;
  pricingRevision?: string | null;
  unpricedReason?: string | null;
  estimated?: boolean | null;
  dimensions: { dimension: string; quantity: number; amountPicos?: string | null; pricing: string }[];
};
export type RequestMeteringLoader = (shareId: string, requestId: string, signal?: AbortSignal) => Promise<RequestMeteringRow[]>;

export function parseRequestMetering(value: unknown, shareId: string, requestId: string): RequestMeteringRow[] {
  const result = value as { shareId?: string; requestId?: string; rows?: RequestMeteringRow[] } | null;
  if (result?.shareId !== shareId || result.requestId !== requestId || !Array.isArray(result.rows)
    || result.rows.length > 100 || result.rows.some(row => !row || row.requestId !== requestId
      || !Number.isInteger(row.attempt) || row.attempt < 1
      || !["debited", "estimate", "none", "unknown"].includes(row.chargeMode)
      || !Array.isArray(row.operations) || !Array.isArray(row.dimensions))) {
    throw new Error("Invalid request metering response");
  }
  return result.rows;
}

export function preciseRequestAmount(value: string | null | undefined): string | undefined {
  if (typeof value !== "string" || !/^\d{1,40}$/.test(value)) return;
  const amount = BigInt(value);
  const scale = 1_000_000_000_000n;
  const fraction = (amount % scale).toString().padStart(12, "0").replace(/0+$/, "").padEnd(2, "0");
  return "$" + (amount / scale) + "." + fraction;
}
