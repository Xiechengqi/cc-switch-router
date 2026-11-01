import type { MessageKey } from "@/lib/i18n";
import { SETTINGS_GROUP_SLUG, settingsMessagesEn } from "@/lib/settings-messages";
import type { SettingsCategory, SettingsField, SettingsSection } from "@/lib/types";

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

const CATEGORY_SLUG: Record<string, string> = {
  site_access: "siteAccess",
  accounts_security: "accountsSecurity",
  marketplace: "marketplace",
  notifications: "notifications",
  monitoring: "monitoring",
  data_retention: "dataRetention",
  connectivity: "connectivity",
  deployment: "deployment",
};

export function settingsCategoryLabel(t: TranslateFn, category?: SettingsCategory) {
  if (!category) return "";
  const slug = CATEGORY_SLUG[category.id];
  return translateSettings(t, `settings.category.${slug}` as MessageKey, category.label);
}

export function settingsCategoryDescription(t: TranslateFn, category?: SettingsCategory) {
  if (!category) return "";
  const slug = CATEGORY_SLUG[category.id];
  return translateSettings(t, `settings.category.${slug}.description` as MessageKey, category.description);
}

export function settingsSectionLabel(t: TranslateFn, section: SettingsSection) {
  return translateSettings(t, `settings.section.${section.id}` as MessageKey, section.label);
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
  const normalized = source === "env_file" ? "envFile" : source;
  const key = `settings.source.${normalized}` as MessageKey;
  return translateSettings(t, key, source);
}
