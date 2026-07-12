import type { MessageKey } from "@/lib/i18n";
import { SETTINGS_GROUP_SLUG, settingsMessagesEn } from "@/lib/settings-messages";
import type { SettingsField } from "@/lib/types";

type TranslateFn = (key: MessageKey, values?: Record<string, string | number>) => string;

function translateSettings(t: TranslateFn, key: MessageKey, fallback: string) {
  if (!(key in settingsMessagesEn)) return fallback;
  return t(key);
}

export function settingsGroupLabel(t: TranslateFn, group: string) {
  const slug = SETTINGS_GROUP_SLUG[group];
  if (!slug) return group;
  return translateSettings(t, `settings.group.${slug}` as MessageKey, group);
}

export function settingsFieldLabel(t: TranslateFn, field: SettingsField) {
  return translateSettings(t, `settings.field.${field.key}.label` as MessageKey, field.label);
}

export function settingsFieldDescription(t: TranslateFn, field: SettingsField) {
  return translateSettings(t, `settings.field.${field.key}.description` as MessageKey, field.description);
}

export function settingsFieldPlaceholder(t: TranslateFn, field: SettingsField) {
  const fallback = field.placeholder || field.default || "";
  if (!fallback) return "";
  return translateSettings(t, `settings.field.${field.key}.placeholder` as MessageKey, fallback);
}

export function settingsValueSource(t: TranslateFn, source?: string) {
  if (!source) return t("common.unset");
  const key = `settings.source.${source}` as MessageKey;
  return translateSettings(t, key, source);
}
