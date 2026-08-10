import type { AccountUsagePeriod } from "@/lib/types";

export type EmbedTheme = "dark" | "light";
export type EmbedNumberFormat = "compact" | "full";

export type UsageEmbedOptions = {
  period: AccountUsagePeriod;
  theme: EmbedTheme;
  models: number;
  showBreakdown: boolean;
  showModels: boolean;
  compact: boolean;
  format: EmbedNumberFormat;
};

export const DEFAULT_EMBED_OPTIONS: UsageEmbedOptions = {
  period: "24h",
  theme: "light",
  models: 8,
  showBreakdown: true,
  showModels: true,
  compact: false,
  format: "compact",
};

const STORAGE_KEY_PREFIX = "cc_switch_router_usage_card_v1:";
const PERIODS = new Set<AccountUsagePeriod>(["24h", "7d", "30d"]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function normalizeUsageEmbedOptions(value: unknown): UsageEmbedOptions {
  if (!isRecord(value)) return { ...DEFAULT_EMBED_OPTIONS };

  const period = PERIODS.has(value.period as AccountUsagePeriod)
    ? (value.period as AccountUsagePeriod)
    : DEFAULT_EMBED_OPTIONS.period;
  const theme = value.theme === "dark" || value.theme === "light"
    ? value.theme
    : DEFAULT_EMBED_OPTIONS.theme;
  const models = typeof value.models === "number" && Number.isInteger(value.models)
    ? Math.min(16, Math.max(1, value.models))
    : DEFAULT_EMBED_OPTIONS.models;
  const format = value.format === "compact" || value.format === "full"
    ? value.format
    : DEFAULT_EMBED_OPTIONS.format;

  return {
    period,
    theme,
    models,
    showBreakdown: typeof value.showBreakdown === "boolean"
      ? value.showBreakdown
      : DEFAULT_EMBED_OPTIONS.showBreakdown,
    showModels: typeof value.showModels === "boolean"
      ? value.showModels
      : DEFAULT_EMBED_OPTIONS.showModels,
    compact: typeof value.compact === "boolean"
      ? value.compact
      : DEFAULT_EMBED_OPTIONS.compact,
    format,
  };
}

export function usageCardPreferencesKey(userId: string) {
  return `${STORAGE_KEY_PREFIX}${userId.trim()}`;
}

export function loadUsageCardPreferences(storage: Storage, userId: string): UsageEmbedOptions {
  if (!userId.trim()) return { ...DEFAULT_EMBED_OPTIONS };
  try {
    const raw = storage.getItem(usageCardPreferencesKey(userId));
    return raw ? normalizeUsageEmbedOptions(JSON.parse(raw)) : { ...DEFAULT_EMBED_OPTIONS };
  } catch {
    return { ...DEFAULT_EMBED_OPTIONS };
  }
}

export function saveUsageCardPreferences(
  storage: Storage,
  userId: string,
  options: UsageEmbedOptions,
) {
  if (!userId.trim()) return;
  try {
    storage.setItem(
      usageCardPreferencesKey(userId),
      JSON.stringify(normalizeUsageEmbedOptions(options)),
    );
  } catch {
    // Browsers may deny storage in private or restricted contexts; the live UI still works.
  }
}
