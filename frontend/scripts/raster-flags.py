#!/usr/bin/env python3
"""Compose Twemoji flag faces onto Twemoji's waving white-flag silhouette and rasterize."""

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

FABRIC = (
    "M32.415 3.09c-1.752-.799-3.615-1.187-5.698-1.187-2.518 0-5.02.57-7.438 1.122"
    "-2.418.551-4.702 1.072-6.995 1.072-1.79 0-3.382-.329-4.868-1.006-.309-.142"
    "-.67-.115-.956.068C6.173 3.343 6 3.66 6 4v19c0 .392.229.747.585.91 1.752.799"
    " 3.616 1.187 5.698 1.187 2.518 0 5.02-.57 7.438-1.122 2.418-.551 4.702-1.071"
    " 6.995-1.071 1.79 0 3.383.329 4.868 1.007.311.14.67.115.956-.069.287-.185"
    ".46-.502.46-.842V4c0-.392-.229-.748-.585-.91z"
)
POLE = (
    '<path fill="#8899A6" d="M5 36c-1.104 0-2-.896-2-2V3c0-1.104.896-2 2-2s2 .896 2 2v31c0 1.104-.896 2-2 2z"/>'
    '<path fill="#AAB8C2" d="M5 1c-1.105 0-2 .895-2 2v31c0 .276.224.5.5.5s.5-.224.5-.5V4.414C4 3.633 4.633 3 5.414 3H7c0-1.105-.895-2-2-2z"/>'
)
INNER_RE = re.compile(r"<svg[^>]*>(.*)</svg>\s*$", re.I | re.S)
SPOT_CHECK = (
    "1f1e8-1f1f3",
    "1f1f9-1f1fc",
    "1f1fa-1f1f8",
    "1f1ef-1f1f5",
    "1f1e9-1f1ea",
)


def waving_svg(name: str, face: str) -> str:
    inner_match = INNER_RE.search(face)
    if not inner_match:
        raise ValueError(f"no inner svg markup in {name}")
    inner = inner_match.group(1).strip()
    clip_id = f"w{name.replace('.svg', '').replace('-', '')}"
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 36 36">'
        f'<defs><clipPath id="{clip_id}"><path d="{FABRIC}"/></clipPath></defs>'
        f'<g clip-path="url(#{clip_id})">'
        '<svg x="6" y="1.9" width="27" height="23.2" viewBox="0 5 36 26" preserveAspectRatio="xMidYMid slice">'
        f"{inner}"
        "</svg></g>"
        f"{POLE}"
        "</svg>"
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
