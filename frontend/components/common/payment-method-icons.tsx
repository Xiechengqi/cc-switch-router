"use client";

import * as React from "react";

const META: Record<string, { label: string; src: string }> = {
  alipay: { label: "Alipay", src: "/payment-icons/alipay.svg" },
  wechat: { label: "WeChat Pay", src: "/payment-icons/wechat.svg" },
  binance: { label: "Binance Pay", src: "/payment-icons/binance.svg" },
  usdt: { label: "USDT", src: "/payment-icons/usdt.svg" },
  usdc: { label: "USDC", src: "/payment-icons/usdc.svg" },
  crypto: { label: "USDT / USDC", src: "/payment-icons/usdc.svg" },
  custom: { label: "Custom payment", src: "/payment-icons/custom.svg" },
};

/** Expand generic crypto kind into concrete token logos. */
function resolveKinds(kinds: string[]): string[] {
  const resolved: string[] = [];
  for (const raw of kinds) {
    const kind = raw.toLowerCase();
    if (kind === "crypto") {
      resolved.push("usdt", "usdc");
      continue;
    }
    resolved.push(kind);
  }
  return Array.from(new Set(resolved)).filter((kind) => META[kind]);
}

export function PaymentMethodIcons({
  kinds,
  className = "",
}: {
  kinds: string[];
  className?: string;
}) {
  const unique = resolveKinds(kinds);
  if (!unique.length) return null;
  return (
    <span className={`inline-flex items-center gap-1 ${className}`}>
      {unique.map((kind) => (
        <img
          key={kind}
          src={META[kind].src}
          alt={META[kind].label}
          title={META[kind].label}
          className="h-5 w-5 shrink-0 object-contain"
        />
      ))}
    </span>
  );
}
