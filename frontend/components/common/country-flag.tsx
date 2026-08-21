"use client";

import * as React from "react";
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

const TW_FLAG = countryFlagEmoji("TW");
const REF_GLYPH = "\uFFFD";

function canvasHasAppleFlags(): boolean {
  if (typeof document === "undefined") return false;
  const canvas = document.createElement("canvas");
  canvas.width = 32;
  canvas.height = 32;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return false;
  ctx.textBaseline = "top";
  ctx.font = "24px \"Apple Color Emoji\", sans-serif";
  ctx.fillStyle = "#000";
  ctx.fillText(TW_FLAG, 0, 0);
  const flag = ctx.getImageData(0, 0, 32, 32).data;
  ctx.clearRect(0, 0, 32, 32);
  ctx.fillText(REF_GLYPH, 0, 0);
  const missing = ctx.getImageData(0, 0, 32, 32).data;
  let colored = 0;
  let same = 0;
  for (let i = 0; i < flag.length; i += 4) {
    if (flag[i + 3] > 16) {
      colored += 1;
      if (Math.abs(flag[i] - missing[i]) < 8
        && Math.abs(flag[i + 1] - missing[i + 1]) < 8
        && Math.abs(flag[i + 2] - missing[i + 2]) < 8
        && Math.abs(flag[i + 3] - missing[i + 3]) < 8) {
        same += 1;
      }
    }
  }
  // Apple Color Emoji paints TW as a multi-color glyph, not tofu / replacement.
  return colored > 24 && same / Math.max(colored, 1) < 0.85;
}

let appleFlagsMemo: boolean | undefined;

function prefersAppleFlagGlyphs() {
  if (appleFlagsMemo !== undefined) return appleFlagsMemo;
  try {
    appleFlagsMemo = canvasHasAppleFlags();
  } catch {
    appleFlagsMemo = false;
  }
  return appleFlagsMemo;
}

/**
 * Apple Color Emoji draws waving flags, including TW. Other platforms get a
 * pre-rasterized Twemoji PNG so Chrome/Linux cannot substitute Noto rectangles.
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
  const [useAppleGlyph, setUseAppleGlyph] = React.useState(false);

  React.useEffect(() => {
    setUseAppleGlyph(prefersAppleFlagGlyphs());
  }, []);

  if (!iso2 || !flag) return null;

  const slug = twemojiFlagSlug(iso2);
  const label = title || iso2;
  return (
    <span
      role="img"
      title={label}
      aria-label={label}
      className={cn("country-flag", useAppleGlyph && "country-flag-native", className)}
    >
      {useAppleGlyph ? (
        <span className="country-flag-glyph">{flag}</span>
      ) : (
        <>
          <img
            src={`/flags/${slug}.png`}
            srcSet={`/flags/${slug}.png 1x, /flags/${slug}@2x.png 2x`}
            width={16}
            height={16}
            alt=""
            draggable={false}
            decoding="async"
            aria-hidden="true"
          />
          <span className="country-flag-copy" aria-hidden="true">
            {flag}
          </span>
        </>
      )}
    </span>
  );
}
