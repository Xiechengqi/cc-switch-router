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

/**
 * Renders a country/region flag as an image.
 * Prefer images over regional-indicator emoji: Windows Chrome and many Linux
 * fonts omit flag glyphs (Taiwan 🇹🇼 is a frequent miss).
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

  return (
    <img
      src={`https://flagcdn.com/w40/${iso2.toLowerCase()}.png`}
      srcSet={`https://flagcdn.com/w40/${iso2.toLowerCase()}.png 1x, https://flagcdn.com/w80/${iso2.toLowerCase()}.png 2x`}
      width={20}
      height={15}
      alt=""
      title={title || iso2}
      aria-label={title || iso2}
      loading="lazy"
      decoding="async"
      className={cn(
        "inline-block h-[0.95em] w-auto shrink-0 rounded-[1px] align-[-0.12em] object-cover",
        className,
      )}
    />
  );
}
