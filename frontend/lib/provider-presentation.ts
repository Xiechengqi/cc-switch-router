import catalog from "./generated/provider-presentation.json";
import type { AppLocale } from "./i18n";
import type { PublicProviderIdentity, ShareProviderCategory } from "./types";

export const PROVIDER_CATEGORY_ORDER = Object.keys(catalog.categories) as ShareProviderCategory[];

export function providerCategory(identity?: PublicProviderIdentity): ShareProviderCategory {
  return identity?.schemaVersion === 1 && Object.hasOwn(catalog.categories, identity.category)
    ? identity.category as ShareProviderCategory : "unknown";
}

export function providerCategoryLabel(category: ShareProviderCategory, locale: AppLocale): string {
  const labels = catalog.categories[category];
  return locale === "zh-CN" ? labels.zh : labels.en;
}

export function providerLogo(identity?: PublicProviderIdentity) {
  if (!identity || identity.schemaVersion !== 1 || !identity.icon) return null;
  const key = identity.icon.trim().toLowerCase().replace(/[\s_]+/g, "-");
  if (!Object.hasOwn(catalog.icons, key)) return null;
  const asset = (catalog.icons as Record<string, { mode: string; content: string; defaultColor?: string }>)[key];
  return {
    key,
    label: identity.displayName,
    ...asset,
    color: identity.iconColor?.trim() || (asset.defaultColor !== "currentColor" ? asset.defaultColor : undefined),
  };
}

export function providerInitials(name: string) {
  return name.split(" ").map((word) => word[0]).join("").toUpperCase().slice(0, 2);
}
