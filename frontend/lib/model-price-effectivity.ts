import type { AdminModelPriceHistoryItem, AdminModelPriceListItem } from "@/lib/types";

type ModelPriceSource = AdminModelPriceListItem["source"];

// Backend-derived prices use Unix second 0 as a retroactive fallback, not a calendar date.
export function isRetroactiveModelPrice(
  source: ModelPriceSource,
  effectiveFrom: number | null | undefined,
): boolean {
  return source === "derived" && effectiveFrom === 0;
}

export function formatModelPriceEffectiveFrom(
  source: ModelPriceSource,
  effectiveFrom: number | null | undefined,
  locale: string,
  derivedLabel: string,
): string {
  if (isRetroactiveModelPrice(source, effectiveFrom)) return derivedLabel;
  if (effectiveFrom == null) return "—";
  return new Date(effectiveFrom * 1000).toLocaleString(locale);
}

export function formatModelPriceHistoryRange(
  item: Pick<AdminModelPriceHistoryItem, "source" | "effectiveFrom" | "effectiveTo">,
  locale: string,
  derivedLabel: string,
): string {
  if (isRetroactiveModelPrice(item.source, item.effectiveFrom)) return derivedLabel;
  const from = formatModelPriceEffectiveFrom(item.source, item.effectiveFrom, locale, derivedLabel);
  const to = item.effectiveTo == null ? "—" : new Date(item.effectiveTo * 1000).toLocaleString(locale);
  return `${from} → ${to}`;
}
