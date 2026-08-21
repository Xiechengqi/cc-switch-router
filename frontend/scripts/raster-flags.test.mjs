import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { initWasm, Resvg } from "@resvg/resvg-wasm";

import {
  alphaBounds,
  APPLE_FLAG_GEOMETRY,
  appleLikeFlagSvg,
  FLAG_VARIANTS,
  pngSize,
} from "./raster-flags.mjs";

const ROOT = dirname(fileURLToPath(import.meta.url));
const FRONTEND = dirname(ROOT);
const SAMPLE_SLUGS = ["1f1e8-1f1f3", "1f1e9-1f1ea", "1f1ef-1f1f5", "1f1fa-1f1f8"];

let wasmReady;
function initResvg() {
  if (!wasmReady) {
    const require = createRequire(import.meta.url);
    wasmReady = readFile(require.resolve("@resvg/resvg-wasm/index_bg.wasm")).then(initWasm);
  }
  return wasmReady;
}

test("Apple-like geometry retains the measured square-tile proportions", () => {
  const geometry = APPLE_FLAG_GEOMETRY;
  assert.equal(geometry.width / geometry.canvas, 0.8875);
  assert.equal(geometry.height / geometry.canvas, 0.6375);
  assert.equal(geometry.x / geometry.canvas, 0.05625);
  assert.equal(geometry.y / geometry.canvas, 0.18125);
});

test("all generated strikes have exact CSS-density dimensions", async () => {
  for (const slug of SAMPLE_SLUGS) {
    for (const [variant, cssPixels] of Object.entries(FLAG_VARIANTS)) {
      for (const density of [1, 2, 3]) {
        const suffix = density === 1 ? "" : `@${density}x`;
        const path = join(FRONTEND, "public", "flags", variant, `${slug}${suffix}.png`);
        const size = pngSize(await readFile(path));
        assert.deepEqual(size, { width: cssPixels * density, height: cssPixels * density });
      }
    }
  }
});

test("master silhouette paints an Apple-like optical box before quantization", async () => {
  await initResvg();
  const slug = "1f1fa-1f1f8";
  const face = await readFile(join(FRONTEND, ".cache", "flag-src", `${slug}.svg`), "utf8");
  const rendered = new Resvg(appleLikeFlagSvg(slug, face), {
    fitTo: { mode: "width", value: 160 },
    font: { loadSystemFonts: false },
  }).render();
  const bounds = alphaBounds(rendered.pixels, rendered.width, rendered.height, 64);

  assert.ok(bounds);
  assert.ok(bounds.left >= 8 && bounds.left <= 10, `left=${bounds.left}`);
  assert.ok(bounds.top >= 28 && bounds.top <= 30, `top=${bounds.top}`);
  assert.ok(bounds.right >= 149 && bounds.right <= 151, `right=${bounds.right}`);
  assert.ok(bounds.bottom >= 129 && bounds.bottom <= 131, `bottom=${bounds.bottom}`);
});

test("composed flags use square em tiles without Apple-owned embedded assets", async () => {
  const slug = "1f1ef-1f1f5";
  const face = await readFile(join(FRONTEND, ".cache", "flag-src", `${slug}.svg`), "utf8");
  const svg = appleLikeFlagSvg(slug, face);

  assert.match(svg, /viewBox="0 0 160 160"/);
  assert.match(svg, /clipPath/);
  assert.doesNotMatch(svg, /<image|data:image|Apple Color Emoji/);
});

test("CountryFlag uses variant-specific square assets without fractional image positioning", async () => {
  const component = await readFile(join(FRONTEND, "components", "common", "country-flag.tsx"), "utf8");
  const css = await readFile(join(FRONTEND, "app", "globals.css"), "utf8");

  assert.match(component, /size = "body"/);
  assert.match(component, /`\/flags\/\$\{size\}\/\$\{slug\}\.png`/);
  assert.match(component, /width=\{canvasPx\}/);
  assert.match(component, /height=\{canvasPx\}/);
  assert.match(css, /\.country-flag\.country-flag--compact\s*\{\s*font-size: 12px;/);
  assert.match(css, /\.country-flag\.country-flag--body\s*\{\s*font-size: 14px;/);
  assert.match(css, /\.country-flag img\s*\{[^}]*inset: 0;[^}]*width: 1em;[^}]*height: 1em;/s);
  assert.doesNotMatch(css, /\.country-flag img\s*\{[^}]*(?:left|top):\s*0\.[0-9]+em/s);
});
