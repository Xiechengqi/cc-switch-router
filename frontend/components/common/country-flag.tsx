"use client";

import { cn } from "@/lib/utils";

/** Normalize common non-ISO aliases to ISO 3166-1 alpha-2 for flag glyphs. */
function normalizeIso2(code: string) {
  if (code === "UK") return "GB";
  return code;
}

export function countryFlagIso2(code?: string | null) {
  const cc = String(code || "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(cc)) return undefined;
  return normalizeIso2(cc);
}

/** Regional-indicator pair, e.g. TW → 🇹🇼. */
function countryFlagEmoji(code?: string | null) {
  const iso2 = countryFlagIso2(code);
  if (!iso2) return "";
  return String.fromCodePoint(...[...iso2].map((ch) => 127397 + ch.charCodeAt(0)));
}

/**
 * Renders a country/region flag as Twemoji's waving glyph.
 * Self-hosted TwemojiCountryFlags covers Windows/Linux gaps (Taiwan 🇹🇼
 * is the frequent miss). Apple/Segoe/Noto remain local fallbacks.
 */
export function CountryFlag({
  code,
  className,
  title,
}: {
  code?: string | null;
  className?: string;
  title?: string;
}) {
  const iso2 = countryFlagIso2(code);
  const flag = countryFlagEmoji(iso2);
  if (!iso2 || !flag) return null;

  return (
    <span
      role="img"
      title={title || iso2}
      aria-label={title || iso2}
      className={cn("country-flag inline-block shrink-0 font-normal leading-none", className)}
    >
      {flag}
    </span>
  );
}
