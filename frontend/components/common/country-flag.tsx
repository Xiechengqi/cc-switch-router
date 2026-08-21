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

const CN_FLAG = countryFlagEmoji("CN");
const US_FLAG = countryFlagEmoji("US");

function canvasPaintsAppleFlag(flag: string): boolean {
  if (typeof document === "undefined") return false;
  const canvas = document.createElement("canvas");
  canvas.width = 32;
  canvas.height = 32;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return false;
  ctx.textBaseline = "top";
  ctx.font = '24px "Apple Color Emoji"';
  ctx.fillText(flag, 0, 0);
  const data = ctx.getImageData(0, 0, 32, 32).data;
  let colored = 0;
  const hues = new Set<string>();
  for (let i = 0; i < data.length; i += 4) {
    if (data[i + 3] < 64) continue;
    colored += 1;
    hues.add(`${data[i] >> 5}-${data[i + 1] >> 5}-${data[i + 2] >> 5}`);
  }
  // A real Apple waving flag is a filled, multi-hue glyph — not tofu / X.
  return colored > 40 && hues.size >= 3;
}

let appleFlagsMemo: boolean | undefined;

function prefersAppleFlagGlyphs() {
  if (appleFlagsMemo !== undefined) return appleFlagsMemo;
  try {
    appleFlagsMemo = canvasPaintsAppleFlag(CN_FLAG) || canvasPaintsAppleFlag(US_FLAG);
  } catch {
    appleFlagsMemo = false;
  }
  return appleFlagsMemo;
}

/**
 * Apple platforms use Apple Color Emoji except TW (often a missing-glyph X).
 * Other platforms, and TW everywhere, use the pre-rasterized Twemoji PNG.
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
  const nativeExceptTw = iso2 !== "TW";
  const [useAppleGlyph, setUseAppleGlyph] = React.useState(false);

  React.useEffect(() => {
    setUseAppleGlyph(nativeExceptTw && prefersAppleFlagGlyphs());
  }, [nativeExceptTw]);

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
