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
