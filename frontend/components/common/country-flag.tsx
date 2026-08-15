"use client";

import { cn } from "@/lib/utils";

/** Normalize common non-ISO aliases to ISO 3166-1 alpha-2 for flag assets. */
function normalizeIso2(code: string) {
  if (code === "UK") return "GB";
  return code;
}

export function countryFlagIso2(code?: string | null) {
  const cc = String(code || "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(cc)) return undefined;
  return normalizeIso2(cc);
}

/** Twemoji regional-indicator filename, e.g. TW → 1f1f9-1f1fc. */
function twemojiFlagSlug(iso2: string) {
  return [...iso2]
    .map((ch) => (127397 + ch.charCodeAt(0)).toString(16))
    .join("-");
}

/**
 * Renders a country/region flag as Twemoji's wavy glyph.
 * System emoji fonts often omit flags (Taiwan 🇹🇼 is a frequent miss);
 * Twemoji ships the same waving shape as an image, including TW.
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
  if (!iso2) return null;

  const slug = twemojiFlagSlug(iso2);
  return (
    <img
      src={`https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/svg/${slug}.svg`}
      width={20}
      height={20}
      alt=""
      title={title || iso2}
      aria-label={title || iso2}
      loading="lazy"
      decoding="async"
      className={cn(
        "inline-block h-[1.15em] w-[1.15em] shrink-0 align-[-0.2em] object-contain",
        className,
      )}
    />
  );
}
