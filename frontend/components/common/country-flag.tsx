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

/** Regional-indicator pair, e.g. TW → 🇹🇼. */
function countryFlagEmoji(code?: string | null) {
  const iso2 = countryFlagIso2(code);
  if (!iso2) return "";
  return String.fromCodePoint(...[...iso2].map((ch) => 127397 + ch.charCodeAt(0)));
}

/** Twemoji regional-indicator filename, e.g. TW → 1f1f9-1f1fc. */
function twemojiFlagSlug(iso2: string) {
  return [...iso2]
    .map((ch) => (127397 + ch.charCodeAt(0)).toString(16))
    .join("-");
}

/**
 * Paints a pre-rasterized waving Twemoji PNG (fabric only, no pole) so
 * desktop Chrome does not blur a live-scaled SVG. A transparent emoji
 * sits on top so copy still yields 🇹🇼, including TW.
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

  const slug = twemojiFlagSlug(iso2);
  const label = title || iso2;
  return (
    <span
      role="img"
      title={label}
      aria-label={label}
      className={cn("country-flag", className)}
    >
      <img
        src={`/flags/${slug}.png`}
        srcSet={`/flags/${slug}.png 1x, /flags/${slug}@2x.png 2x`}
        width={20}
        height={20}
        alt=""
        draggable={false}
        decoding="async"
        aria-hidden="true"
      />
      <span className="country-flag-copy" aria-hidden="true">
        {flag}
      </span>
    </span>
  );
}
