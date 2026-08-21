#!/usr/bin/env node
/**
 * Build self-hosted, Apple-like country-flag strikes from pinned Twemoji faces.
 *
 * Apple Color Emoji paints each flag inside a square em tile. The visible art
 * occupies about 142 × 102 of a 160 × 160 tile. We keep that square canvas in
 * the PNG so browsers do not have to position and shrink a tight crop at
 * fractional CSS pixels. Compact and body variants are rasterized directly at
 * their final 1x/2x/3x canvas sizes.
 *
 * The waving silhouette and lighting below are project-owned approximations;
 * no Apple artwork or font data is distributed.
 */
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { initWasm, Resvg } from "@resvg/resvg-wasm";

const ROOT = dirname(fileURLToPath(import.meta.url));
const FRONTEND = dirname(ROOT);
const SLUG_FILE = join(ROOT, "flag-slugs.txt");
const CACHE_DIR = join(FRONTEND, ".cache", "flag-src");
const LEGACY_SRC = join(FRONTEND, "assets", "flag-src");
const OUT_DIR = join(FRONTEND, "public", "flags");
const BUILD_MANIFEST = join(OUT_DIR, ".build.json");

const TWEMOJI = "14.0.2";
const CDN = `https://cdn.jsdelivr.net/gh/twitter/twemoji@${TWEMOJI}/assets/svg`;
const DENSITIES = [1, 2, 3];
const GENERATOR_VERSION = "apple-like-em-strikes-v1";

export const FLAG_VARIANTS = Object.freeze({
  compact: 12,
  body: 14,
});

/** Measured from 160px Apple flag reference tiles; used as geometry, not art. */
export const APPLE_FLAG_GEOMETRY = Object.freeze({
  canvas: 160,
  x: 9,
  y: 29,
  width: 142,
  height: 102,
});

/**
 * A custom three-fold ribbon whose painted bounds follow the measured Apple-
 * like tile while retaining an independently drawn silhouette.
 */
export const FLAG_SILHOUETTE =
  "M9 33C27 24 46 38 66 32C85 26 105 38 124 32C135 28 144 28 151 32" +
  "V126C134 120 116 133 97 127C78 121 59 134 40 127C27 122 17 125 9 130Z";

const INNER_RE = /<svg[^>]*>(.*)<\/svg>\s*$/is;
const SPOT_CHECK = ["1f1e8-1f1f3", "1f1f9-1f1fc", "1f1fa-1f1f8", "1f1ef-1f1f5", "1f1e9-1f1ea"];
const FETCH_CONCURRENCY = 8;
const FORCE = process.argv.includes("--force") || process.env.FLAGS_RASTER_FORCE === "1";

function outputName(slug, density) {
  return `${slug}${density === 1 ? "" : `@${density}x`}.png`;
}

function outputPath(variant, slug, density) {
  return join(OUT_DIR, variant, outputName(slug, density));
}

export function pngSize(buf) {
  if (buf.length < 24 || buf[0] !== 0x89 || buf[1] !== 0x50) {
    throw new Error("not a PNG");
  }
  return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
}

export function alphaBounds(pixels, width, height, threshold = 8) {
  let left = width;
  let top = height;
  let right = -1;
  let bottom = -1;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      if (pixels[(y * width + x) * 4 + 3] <= threshold) continue;
      left = Math.min(left, x);
      top = Math.min(top, y);
      right = Math.max(right, x);
      bottom = Math.max(bottom, y);
    }
  }
  if (right < left || bottom < top) return undefined;
  return {
    left,
    top,
    right,
    bottom,
    width: right - left + 1,
    height: bottom - top + 1,
  };
}

/** Compose a flat Twemoji face into the custom ribbon and add restrained folds. */
export function appleLikeFlagSvg(slug, face) {
  const match = INNER_RE.exec(face);
  if (!match) throw new Error(`no inner svg markup in ${slug}`);
  const inner = match[1].trim();
  const id = slug.replaceAll("-", "");
  const clipId = `flag${id}`;
  const sheenId = `sheen${id}`;
  const foldId = `fold${id}`;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 160">` +
    `<defs>` +
    `<clipPath id="${clipId}"><path d="${FLAG_SILHOUETTE}"/></clipPath>` +
    `<linearGradient id="${sheenId}" x1="0" y1="29" x2="0" y2="128" gradientUnits="userSpaceOnUse">` +
    `<stop offset="0" stop-color="#fff" stop-opacity=".16"/>` +
    `<stop offset=".46" stop-color="#fff" stop-opacity="0"/>` +
    `<stop offset="1" stop-color="#020617" stop-opacity=".17"/>` +
    `</linearGradient>` +
    `<linearGradient id="${foldId}" x1="9" y1="0" x2="151" y2="0" gradientUnits="userSpaceOnUse">` +
    `<stop offset="0" stop-color="#fff" stop-opacity=".03"/>` +
    `<stop offset=".22" stop-color="#fff" stop-opacity=".13"/>` +
    `<stop offset=".40" stop-color="#020617" stop-opacity=".09"/>` +
    `<stop offset=".58" stop-color="#fff" stop-opacity=".12"/>` +
    `<stop offset=".78" stop-color="#020617" stop-opacity=".10"/>` +
    `<stop offset="1" stop-color="#fff" stop-opacity=".05"/>` +
    `</linearGradient>` +
    `</defs>` +
    `<path d="${FLAG_SILHOUETTE}" transform="translate(0 2)" fill="#020617" opacity=".28"/>` +
    `<g clip-path="url(#${clipId})">` +
    `<svg x="9" y="27" width="142" height="104" viewBox="0 5 36 26" preserveAspectRatio="none">` +
    `${inner}</svg>` +
    `<rect x="9" y="27" width="142" height="104" fill="url(#${sheenId})"/>` +
    `<rect x="9" y="27" width="142" height="104" fill="url(#${foldId})"/>` +
    `<path d="M10 42C29 34 47 47 67 41C86 35 105 47 125 41C136 37 144 37 151 40" ` +
    `fill="none" stroke="#fff" stroke-opacity=".12" stroke-width="3"/>` +
    `<path d="M9 120C27 114 46 126 66 120C85 114 105 126 124 120C136 116 144 116 151 119" ` +
    `fill="none" stroke="#020617" stroke-opacity=".18" stroke-width="4"/>` +
    `</g></svg>`
  );
}

async function readSlugs() {
  const text = await readFile(SLUG_FILE, "utf8");
  const slugs = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => /^1f1[0-9a-f]{2}-1f1[0-9a-f]{2}$/.test(line));
  if (slugs.length < 200) throw new Error(`flag slug list looks short: ${slugs.length}`);
  return slugs;
}

function outputsComplete(slugs) {
  if (FORCE) return false;
  try {
    const manifest = JSON.parse(readFileSync(BUILD_MANIFEST, "utf8"));
    if (manifest.generatorVersion !== GENERATOR_VERSION || manifest.twemoji !== TWEMOJI) return false;
    if (JSON.stringify(manifest.variants) !== JSON.stringify(FLAG_VARIANTS)) return false;
    return Object.entries(FLAG_VARIANTS).every(([variant, cssPixels]) =>
      slugs.every((slug) =>
        DENSITIES.every((density) => {
          const path = outputPath(variant, slug, density);
          if (!existsSync(path)) return false;
          const size = pngSize(readFileSync(path));
          const expected = cssPixels * density;
          return size.width === expected && size.height === expected;
        }),
      ),
    );
  } catch {
    return false;
  }
}

async function fetchFace(slug) {
  const cachePath = join(CACHE_DIR, `${slug}.svg`);
  if (existsSync(cachePath)) return readFile(cachePath, "utf8");
  const legacyPath = join(LEGACY_SRC, `${slug}.svg`);
  if (existsSync(legacyPath)) {
    const text = await readFile(legacyPath, "utf8");
    await writeFile(cachePath, text);
    return text;
  }
  const url = `${CDN}/${slug}.svg`;
  let lastErr;
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`${url} -> ${response.status}`);
      const text = await response.text();
      if (!text.includes("<svg")) throw new Error(`${url} is not SVG`);
      await writeFile(cachePath, text);
      return text;
    } catch (err) {
      lastErr = err;
      await new Promise((done) => setTimeout(done, 250 * attempt));
    }
  }
  throw lastErr;
}

async function mapPool(items, limit, worker) {
  const pending = [...items];
  const runners = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (pending.length) await worker(pending.shift());
  });
  await Promise.all(runners);
}

async function clearGeneratedPngs() {
  if (!existsSync(OUT_DIR)) return;
  for (const entry of await readdir(OUT_DIR, { withFileTypes: true })) {
    const path = join(OUT_DIR, entry.name);
    if (entry.isFile() && entry.name.endsWith(".png")) await rm(path);
    if (entry.isDirectory() && Object.hasOwn(FLAG_VARIANTS, entry.name)) {
      await rm(path, { recursive: true, force: true });
    }
  }
}

function assertRenderedGeometry(slug, variant, density, rendered) {
  const bounds = alphaBounds(rendered.pixels, rendered.width, rendered.height, 64);
  if (!bounds) throw new Error(`${slug}/${variant}@${density}x has no painted pixels`);
  const widthRatio = bounds.width / rendered.width;
  const heightRatio = bounds.height / rendered.height;
  // Tiny 12px strikes quantize heavily; these bounds catch padding regressions
  // without pretending a single device pixel can match the 160px master.
  if (widthRatio < 0.79 || widthRatio > 1 || heightRatio < 0.56 || heightRatio > 0.75) {
    throw new Error(
      `${slug}/${variant}@${density}x ink ${widthRatio.toFixed(3)} × ${heightRatio.toFixed(3)} is outside the Apple-like tile`,
    );
  }
}

export async function main() {
  const slugs = await readSlugs();
  await mkdir(CACHE_DIR, { recursive: true });
  await mkdir(OUT_DIR, { recursive: true });

  if (outputsComplete(slugs)) {
    console.log(`flags already rasterized (${slugs.length} × ${Object.keys(FLAG_VARIANTS).length} variants × ${DENSITIES.length} densities)`);
    return 0;
  }

  const require = createRequire(import.meta.url);
  await initWasm(await readFile(require.resolve("@resvg/resvg-wasm/index_bg.wasm")));

  const faces = new Map();
  await mapPool(slugs, FETCH_CONCURRENCY, async (slug) => {
    faces.set(slug, await fetchFace(slug));
  });

  await clearGeneratedPngs();
  for (const variant of Object.keys(FLAG_VARIANTS)) {
    await mkdir(join(OUT_DIR, variant), { recursive: true });
  }

  for (const slug of slugs) {
    const svg = appleLikeFlagSvg(slug, faces.get(slug));
    for (const [variant, cssPixels] of Object.entries(FLAG_VARIANTS)) {
      for (const density of DENSITIES) {
        const pixels = cssPixels * density;
        const rendered = new Resvg(svg, {
          fitTo: { mode: "width", value: pixels },
          font: { loadSystemFonts: false },
        }).render();
        assertRenderedGeometry(slug, variant, density, rendered);
        await writeFile(outputPath(variant, slug, density), Buffer.from(rendered.asPng()));
      }
    }
  }

  for (const slug of SPOT_CHECK) {
    for (const [variant, cssPixels] of Object.entries(FLAG_VARIANTS)) {
      for (const density of DENSITIES) {
        const path = outputPath(variant, slug, density);
        const buf = await readFile(path);
        if (buf.length < 64) throw new Error(`missing or empty ${path}`);
        const size = pngSize(buf);
        const expected = cssPixels * density;
        if (size.width !== expected || size.height !== expected) {
          throw new Error(`${path} is ${size.width} × ${size.height}, expected ${expected} × ${expected}`);
        }
      }
    }
  }

  await writeFile(
    BUILD_MANIFEST,
    `${JSON.stringify({ generatorVersion: GENERATOR_VERSION, twemoji: TWEMOJI, variants: FLAG_VARIANTS }, null, 2)}\n`,
  );

  console.log(
    `rasterized ${slugs.length} flags × ${Object.keys(FLAG_VARIANTS).length} variants × ${DENSITIES.length} densities -> ${OUT_DIR}`,
  );
  return 0;
}

const isEntryPoint = process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1]);
if (isEntryPoint) {
  main().then(
    (code) => process.exit(code),
    (err) => {
      console.error(err instanceof Error ? err.stack || err.message : err);
      process.exit(1);
    },
  );
}
