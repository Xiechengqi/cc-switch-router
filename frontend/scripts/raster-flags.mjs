#!/usr/bin/env node
/**
 * Fetch Twemoji 14 flag faces from jsDelivr, clip them onto Twemoji's waving
 * fabric (no pole), crop uniformly to Apple Color Emoji ribbon aspect, and
 * rasterize 32 / 64 / 96 PNGs.
 *
 * Transparent padding lives in CSS, not in the pixels. Do not non-uniformly
 * stretch the fabric — stars and stripes stay geometric.
 */
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { initWasm, Resvg } from "@resvg/resvg-wasm";

const ROOT = dirname(fileURLToPath(import.meta.url));
const FRONTEND = dirname(ROOT);
const SLUG_FILE = join(ROOT, "flag-slugs.txt");
const CACHE_DIR = join(FRONTEND, ".cache", "flag-src");
const LEGACY_SRC = join(FRONTEND, "assets", "flag-src");
const OUT_DIR = join(FRONTEND, "public", "flags");

const TWEMOJI = "14.0.2";
const CDN = `https://cdn.jsdelivr.net/gh/twitter/twemoji@${TWEMOJI}/assets/svg`;

const FABRIC_X = 5.5;
const FABRIC_Y = 1.5;
const FABRIC_W = 28.5;
const FABRIC_H = 24.5;
/** Apple ribbon ~0.88em × 0.62em. */
const APPLE_ASPECT = 0.88 / 0.62;
const CROP_H = FABRIC_W / APPLE_ASPECT;
const CROP_Y = FABRIC_Y + (FABRIC_H - CROP_H) / 2;
const CROP_X = FABRIC_X;

const FABRIC =
  "M32.415 3.09c-1.752-.799-3.615-1.187-5.698-1.187-2.518 0-5.02.57-7.438 1.122" +
  "-2.418.551-4.702 1.072-6.995 1.072-1.79 0-3.382-.329-4.868-1.006-.309-.142" +
  "-.67-.115-.956.068C6.173 3.343 6 3.66 6 4v19c0 .392.229.747.585.91 1.752.799" +
  " 3.616 1.187 5.698 1.187 2.518 0 5.02-.57 7.438-1.122 2.418-.551 4.702-1.071" +
  " 6.995-1.071 1.79 0 3.383.329 4.868 1.007.311.14.67.115.956-.069.287-.185" +
  ".46-.502.46-.842V4c0-.392-.229-.748-.585-.91z";

const SIZES = [
  ["", 32],
  ["@2x", 64],
  ["@3x", 96],
];
const INNER_RE = /<svg[^>]*>(.*)<\/svg>\s*$/is;
const SPOT_CHECK = ["1f1e8-1f1f3", "1f1f9-1f1fc", "1f1fa-1f1f8", "1f1ef-1f1f5", "1f1e9-1f1ea"];
const FETCH_CONCURRENCY = 8;
const FORCE = process.argv.includes("--force") || process.env.FLAGS_RASTER_FORCE === "1";

function pngSize(buf) {
  if (buf.length < 24 || buf[0] !== 0x89 || buf[1] !== 0x50) {
    throw new Error("not a PNG");
  }
  return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
}

function wavingSvg(slug, face) {
  const match = INNER_RE.exec(face);
  if (!match) throw new Error(`no inner svg markup in ${slug}`);
  const inner = match[1].trim();
  const clipId = `w${slug.replaceAll("-", "")}`;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${CROP_X} ${CROP_Y} ${FABRIC_W} ${CROP_H}">` +
    `<defs><clipPath id="${clipId}"><path d="${FABRIC}"/></clipPath></defs>` +
    `<g clip-path="url(#${clipId})">` +
    `<svg x="6" y="1.9" width="27" height="23.2" viewBox="0 5 36 26" preserveAspectRatio="xMidYMid slice">` +
    `${inner}` +
    `</svg></g></svg>`
  );
}

async function readSlugs() {
  const text = await readFile(SLUG_FILE, "utf8");
  const slugs = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => /^1f1[0-9a-f]{2}-1f1[0-9a-f]{2}$/.test(line));
  if (slugs.length < 200) {
    throw new Error(`flag slug list looks short: ${slugs.length}`);
  }
  return slugs;
}

function outputsComplete(slugs) {
  if (FORCE) return false;
  const probe = join(OUT_DIR, `${SPOT_CHECK[0]}.png`);
  if (!existsSync(probe)) return false;
  try {
    if (pngSize(readFileSync(probe)).width !== SIZES[0][1]) return false;
  } catch {
    return false;
  }
  return slugs.every((slug) =>
    SIZES.every(([suffix]) => existsSync(join(OUT_DIR, `${slug}${suffix}.png`))),
  );
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
      await new Promise((resolve) => setTimeout(resolve, 250 * attempt));
    }
  }
  throw lastErr;
}

async function mapPool(items, limit, worker) {
  const pending = [...items];
  const runners = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (pending.length) {
      const item = pending.shift();
      await worker(item);
    }
  });
  await Promise.all(runners);
}

async function main() {
  const slugs = await readSlugs();
  await mkdir(CACHE_DIR, { recursive: true });
  await mkdir(OUT_DIR, { recursive: true });

  if (outputsComplete(slugs)) {
    console.log(`flags already rasterized (${slugs.length} × ${SIZES.length}); pass --force to rebuild`);
    return 0;
  }

  const require = createRequire(import.meta.url);
  const wasmPath = require.resolve("@resvg/resvg-wasm/index_bg.wasm");
  await initWasm(await readFile(wasmPath));

  const faces = new Map();
  await mapPool(slugs, FETCH_CONCURRENCY, async (slug) => {
    faces.set(slug, await fetchFace(slug));
  });

  for (const stale of await readdir(OUT_DIR)) {
    if (stale.endsWith(".png")) await rm(join(OUT_DIR, stale));
  }

  for (const slug of slugs) {
    const svg = wavingSvg(slug, faces.get(slug));
    for (const [suffix, width] of SIZES) {
      const resvg = new Resvg(svg, {
        fitTo: { mode: "width", value: width },
        font: { loadSystemFonts: false },
      });
      const png = Buffer.from(resvg.render().asPng());
      await writeFile(join(OUT_DIR, `${slug}${suffix}.png`), png);
    }
  }

  console.log(`rasterized ${slugs.length} flags × ${SIZES.length} sizes -> ${OUT_DIR}`);

  for (const slug of SPOT_CHECK) {
    for (const [suffix, width] of SIZES) {
      const path = join(OUT_DIR, `${slug}${suffix}.png`);
      const buf = await readFile(path);
      if (buf.length < 64) throw new Error(`missing or empty ${path}`);
      const { width: pxW, height: pxH } = pngSize(buf);
      if (pxW !== width) throw new Error(`${path} width ${pxW} != ${width}`);
      const aspect = pxW / pxH;
      if (aspect < 1.35 || aspect > 1.5) {
        throw new Error(`${path} aspect ${aspect.toFixed(3)} not ~1.42`);
      }
    }
  }
  return 0;
}

main().then(
  (code) => process.exit(code),
  (err) => {
    console.error(err instanceof Error ? err.stack || err.message : err);
    process.exit(1);
  },
);
