"use client";

import * as React from "react";
import { encodeQrText, type QrErrorCorrection } from "@/lib/qr-code";
import { cn } from "@/lib/utils";

const QUIET_ZONE = 4;

/** Crisp, self-contained QR Code rendered as one SVG path with a 4-module quiet zone. */
export function QrCodeSvg({ value, label, size = 224, ecc = "M", className }: {
  value: string;
  /** Accessible name, e.g. "QR code for the payment address". */
  label: string;
  /** Rendered width/height in CSS pixels. */
  size?: number;
  ecc?: QrErrorCorrection;
  className?: string;
}) {
  const symbol = React.useMemo(() => {
    try {
      const qr = encodeQrText(value, { ecc });
      let path = "";
      qr.modules.forEach((row, y) => {
        // Merge horizontal runs so the path stays small.
        for (let x = 0; x < qr.size; x++) {
          if (!row[x]) continue;
          let end = x;
          while (end + 1 < qr.size && row[end + 1]) end++;
          path += `M${x + QUIET_ZONE} ${y + QUIET_ZONE}h${end - x + 1}v1h-${end - x + 1}z`;
          x = end;
        }
      });
      return { path, dimension: qr.size + QUIET_ZONE * 2 };
    } catch {
      return null;
    }
  }, [value, ecc]);

  if (!symbol) return null;
  return (
    <svg
      role="img"
      aria-label={label}
      width={size}
      height={size}
      viewBox={`0 0 ${symbol.dimension} ${symbol.dimension}`}
      shapeRendering="crispEdges"
      className={cn("block h-auto max-w-full", className)}
      xmlns="http://www.w3.org/2000/svg"
    >
      <title>{label}</title>
      <rect width={symbol.dimension} height={symbol.dimension} fill="#ffffff" />
      <path d={symbol.path} fill="#0F172A" />
    </svg>
  );
}
