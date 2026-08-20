import localFont from "next/font/local";

/**
 * Twemoji waving flags, including TW.
 * Bundled via next/font so static export emits a same-origin preload under /_next/static.
 * `display: "block"` keeps Windows from painting a missing TW glyph before the font arrives.
 */
export const countryFlagFont = localFont({
  src: "./fonts/TwemojiCountryFlags.woff2",
  display: "block",
  preload: true,
  adjustFontFallback: false,
  fallback: [
    "Apple Color Emoji",
    "Segoe UI Emoji",
    "Noto Color Emoji",
    "Segoe UI Symbol",
    "sans-serif",
  ],
  declarations: [{ prop: "unicode-range", value: "U+1F1E6-1F1FF" }],
  variable: "--font-country-flags",
});
