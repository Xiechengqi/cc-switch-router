"use client";

import * as React from "react";

const META: Record<string, { label: string; src: string }> = {
  alipay: { label: "Alipay", src: "/payment-icons/alipay.svg" },
  wechat: { label: "WeChat Pay", src: "/payment-icons/wechat.svg" },
  binance: { label: "Binance Pay", src: "/payment-icons/binance.svg" },
  crypto: { label: "USDT / USDC", src: "/payment-icons/crypto.svg" },
  custom: { label: "Custom payment", src: "/payment-icons/custom.svg" },
};

export function PaymentMethodIcons({
  kinds,
  className = "",
}: {
  kinds: string[];
  className?: string;
}) {
  const unique = Array.from(new Set(kinds.map((kind) => kind.toLowerCase()))).filter(
    (kind) => META[kind],
  );
  if (!unique.length) return null;
  return (
    <span className={`inline-flex items-center gap-1 ${className}`}>
      {unique.map((kind) => (
        <img
          key={kind}
          src={META[kind].src}
          alt={META[kind].label}
          title={META[kind].label}
          className="h-5 w-5 shrink-0 rounded"
        />
      ))}
    </span>
  );
}
