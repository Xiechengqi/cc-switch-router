/** Buyers choose how to pay, not which gateway account is behind it. */
export type TopupMethod = "crypto";

export function parseTopupMethod(category: unknown): TopupMethod | null {
  return category === "crypto" ? category : null;
}

/**
 * One option per payment method. The Router allows a single active channel per
 * gateway kind; if an older deployment still lists several, the first listed
 * (Owner sort order) represents the method so buyers never see duplicates.
 */
export function topupMethodOptions<T extends { id: string; methodCategory?: unknown }>(
  channels: readonly T[],
): { method: TopupMethod; channel: T }[] {
  const seen = new Set<TopupMethod>();
  const options: { method: TopupMethod; channel: T }[] = [];
  for (const channel of channels) {
    const method = parseTopupMethod(channel.methodCategory);
    if (!method || seen.has(method)) continue;
    seen.add(method);
    options.push({ method, channel });
  }
  return options;
}
