/** Seller termination terms for a paid Share seat, as edited in the owner workspace. */
import type { ShareEarlyTermination, ShareMarketSeat } from "./types";

/** Highest seller rate the Router accepts, in percent. */
export const MAX_SEAT_TERMINATION_RATE_PERCENT = 50;

export type SeatTerminationDraft = {
  earlyTermination: ShareEarlyTermination;
  /** Percent as typed; empty follows the platform default. */
  terminationRate: string;
};

export function seatTerminationDraft(seat?: Pick<ShareMarketSeat, "terminationTerms">): SeatTerminationDraft {
  const terms = seat?.terminationTerms;
  return {
    earlyTermination: terms?.earlyTermination ?? "allowed",
    terminationRate: terms?.rateSource === "seat" ? String(terms.rateBps / 100) : "",
  };
}

/** Basis points for the typed percent, null for the platform default, or undefined when invalid. */
export function seatTerminationRateBps(value: string): number | null | undefined {
  const text = value.trim();
  if (!text) return null;
  if (!/^\d{1,2}(?:\.\d{1,2})?$/.test(text)) return undefined;
  const bps = Math.round(Number(text) * 100);
  return bps >= 0 && bps <= MAX_SEAT_TERMINATION_RATE_PERCENT * 100 ? bps : undefined;
}
