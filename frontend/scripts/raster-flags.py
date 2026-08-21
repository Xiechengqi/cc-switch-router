#!/usr/bin/env python3
"""Clip Twemoji flag faces to Twemoji's waving fabric (no pole) and rasterize.

Apple Color Emoji paints a waving flag inside ~88% of the em square. The PNG
path uses the same 1em CSS box, so the fabric is inset here instead of
tight-cropped; otherwise non-Apple (and TW on Apple) flags look larger.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

try:
    import cairosvg
except ImportError as exc:
    raise SystemExit("cairosvg is required: pip install cairosvg") from exc

ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "assets" / "flag-src"
OUT_DIR = ROOT / "public" / "flags"
SIZES = (("", 20), ("@2x", 40))

# Twemoji U+1F3F3 fabric, original 36em canvas, pole omitted.
CANVAS = 36.0
FABRIC_X = 5.5
FABRIC_Y = 1.5
FABRIC_W = 28.5
FABRIC_H = 24.5
APPLE_OPTICAL = 0.88

FABRIC = (
    "M32.415 3.09c-1.752-.799-3.615-1.187-5.698-1.187-2.518 0-5.02.57-7.438 1.122"
    "-2.418.551-4.702 1.072-6.995 1.072-1.79 0-3.382-.329-4.868-1.006-.309-.142"
    "-.67-.115-.956.068C6.173 3.343 6 3.66 6 4v19c0 .392.229.747.585.91 1.752.799"
    " 3.616 1.187 5.698 1.187 2.518 0 5.02-.57 7.438-1.122 2.418-.551 4.702-1.071"
    " 6.995-1.071 1.79 0 3.383.329 4.868 1.007.311.14.67.115.956-.069.287-.185"
    ".46-.502.46-.842V4c0-.392-.229-.748-.585-.91z"
)
INNER_RE = re.compile(r"<svg[^>]*>(.*)</svg>\s*$", re.I | re.S)
SPOT_CHECK = (
    "1f1e8-1f1f3",
    "1f1f9-1f1fc",
    "1f1fa-1f1f8",
    "1f1ef-1f1f5",
    "1f1e9-1f1ea",
)


def fabric_frame() -> tuple[float, float, float, float]:
    box = CANVAS * APPLE_OPTICAL
    scale = min(box / FABRIC_W, box / FABRIC_H)
    width = FABRIC_W * scale
    height = FABRIC_H * scale
    return ((CANVAS - width) / 2, (CANVAS - height) / 2, width, height)


def waving_svg(name: str, face: str) -> str:
    inner_match = INNER_RE.search(face)
    if not inner_match:
        raise ValueError(f"no inner svg markup in {name}")
    inner = inner_match.group(1).strip()
    clip_id = f"w{name.replace('.svg', '').replace('-', '')}"
    x, y, width, height = fabric_frame()
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {CANVAS:g} {CANVAS:g}">'
        f'<svg x="{x:.4f}" y="{y:.4f}" width="{width:.4f}" height="{height:.4f}" '
        f'viewBox="{FABRIC_X:g} {FABRIC_Y:g} {FABRIC_W:g} {FABRIC_H:g}">'
        f'<defs><clipPath id="{clip_id}"><path d="{FABRIC}"/></clipPath></defs>'
        f'<g clip-path="url(#{clip_id})">'
        '<svg x="6" y="1.9" width="27" height="23.2" viewBox="0 5 36 26" preserveAspectRatio="xMidYMid slice">'
        f"{inner}"
        "</svg></g></svg></svg>"
    )


def main() -> int:
    sources = sorted(SRC_DIR.glob("1f1*.svg"))
    if not sources:
        print(f"no flag sources in {SRC_DIR}", file=sys.stderr)
        return 1
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for stale in OUT_DIR.glob("1f1*"):
        stale.unlink()
    for src in sources:
        composed = waving_svg(src.name, src.read_text())
        stem = src.stem
        for suffix, px in SIZES:
            dest = OUT_DIR / f"{stem}{suffix}.png"
            cairosvg.svg2png(
                bytestring=composed.encode("utf-8"),
                write_to=str(dest),
                output_width=px,
                output_height=px,
            )
    print(f"rasterized {len(sources)} flags × {len(SIZES)} sizes -> {OUT_DIR}")
    for stem in SPOT_CHECK:
        for name in (f"{stem}.png", f"{stem}@2x.png"):
            path = OUT_DIR / name
            if not path.is_file() or path.stat().st_size < 64:
                print(f"missing or empty {path}", file=sys.stderr)
                return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
