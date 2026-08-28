import type { ShareMarketSeat } from "@/lib/types";

type SelectableSeat = Pick<ShareMarketSeat, "id" | "status" | "readOnly">;

function isIdle(seat: SelectableSeat) {
  return seat.status === "available" && !seat.readOnly;
}

export function initialCatalogSeat<T extends SelectableSeat>(seats: T[], explicit?: T) {
  if (explicit) return explicit;
  const idle = seats.filter(isIdle);
  return idle.length === 1 ? idle[0] : undefined;
}

export function preserveCatalogSeat<T extends Pick<ShareMarketSeat, "id">>(
  seats: T[],
  selectedId: string,
) {
  return seats.find((seat) => seat.id === selectedId);
}

export function catalogSeatPreview<T extends SelectableSeat>(seats: T[]) {
  return seats.filter(isIdle).slice(0, 2);
}

export const MARKET_SHARE_CARD_SEAT_PREVIEW_LIMIT = 2;

export function marketShareCardSeatPreview<T extends SelectableSeat>(
  seats: T[],
  preferredIds: string[] = [],
) {
  const preferred = preferredIds
    .map((id) => seats.find((seat) => seat.id === id))
    .filter((seat): seat is T => !!seat);
  const idle = seats.filter(isIdle);
  const seen = new Set<string>();
  const preview: T[] = [];
  for (const seat of [...preferred, ...idle, ...seats]) {
    if (seen.has(seat.id)) continue;
    seen.add(seat.id);
    preview.push(seat);
    if (preview.length === MARKET_SHARE_CARD_SEAT_PREVIEW_LIMIT) break;
  }
  return {
    preview,
    hiddenCount: Math.max(0, seats.length - preview.length),
    idleHiddenCount: Math.max(0, idle.length - preview.filter(isIdle).length),
    idleCount: idle.length,
  };
}
