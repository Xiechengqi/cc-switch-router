const UNITS_PER_DOLLAR = 8_640_000n;

export function formatWalletUnitsAsCny(raw: string, rateMicros: number, locale = "en-US") {
  const units = BigInt(raw);
  const scaled = units * BigInt(rateMicros);
  const converted = scaled >= 0n
    ? (scaled + 500_000n) / 1_000_000n
    : (scaled - 500_000n) / 1_000_000n;
  return `≈${formatWalletUnits(converted.toString(), "¥", locale)}`;
}

export function formatWalletUnits(raw: string, currencySymbol = "$", locale = "en-US") {
  const units = BigInt(raw);
  const negative = units < 0n;
  const absolute = negative ? -units : units;
  const whole = absolute / UNITS_PER_DOLLAR;
  const remainder = absolute % UNITS_PER_DOLLAR;
  const millionths = remainder * 1_000_000n / UNITS_PER_DOLLAR;
  const fraction = millionths.toString().padStart(6, "0").replace(/0+$/, "");
  const decimals = fraction.length < 2 ? fraction.padEnd(2, "0") : fraction;
  if (whole === 0n && remainder > 0n && millionths === 0n) {
    return `${negative ? "−" : ""}<${currencySymbol}0.000001`;
  }
  return `${negative ? "−" : ""}${currencySymbol}${whole.toLocaleString(locale)}.${decimals}`;
}

export function parseUsdMinor(raw: string): number | null {
  const value = raw.trim();
  if (!/^(?:0|[1-9]\d{0,3})(?:\.\d{1,2})?$/.test(value)) return null;
  const [dollars, fraction = ""] = value.split(".");
  const minor = Number(dollars) * 100 + Number(fraction.padEnd(2, "0"));
  return minor >= 100 && minor <= 100_000 ? minor : null;
}

export function parseWithdrawalMinor(raw: string): number | null {
  const value = raw.trim();
  if (!/^(?:0|[1-9]\d{0,12})(?:\.\d{1,2})?$/.test(value)) return null;
  const [dollars, fraction = ""] = value.split(".");
  const minor = BigInt(dollars) * 100n + BigInt(fraction.padEnd(2, "0") || "0");
  return minor >= 100n && minor <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(minor) : null;
}

export function parseNonNegativeUsdMinor(raw: string): number | null {
  const value = raw.trim();
  if (!/^(?:0|[1-9]\d{0,12})(?:\.\d{1,2})?$/.test(value)) return null;
  const [dollars, fraction = ""] = value.split(".");
  const minor = BigInt(dollars) * 100n + BigInt(fraction.padEnd(2, "0") || "0");
  return minor <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(minor) : null;
}

export function safeCheckoutUrl(raw: string | null) {
  if (!raw) return null;
  try {
    const url = new URL(raw);
    if (raw.includes("\\") || /\s/.test(raw)
      || url.origin !== "https://www.dujiaopay.com"
      || !/^\/checkout\/[A-Za-z0-9_-]+$/.test(url.pathname)
      || url.username || url.password || raw.includes("?") || raw.includes("#")) return null;
    return url.href;
  } catch {
    return null;
  }
}

// Omitted status is only for a freshly created order; null means a history
// lookup could not confirm that the checkout is still payable.
export function payableCheckoutUrl(status: string, raw: string | null, gatewayStatus?: string | null, abandoned = false) {
  if (abandoned || status !== "pending" || (gatewayStatus !== undefined && !["awaiting_payment", "pending"].includes(gatewayStatus ?? ""))) return null;
  return safeCheckoutUrl(raw);
}

export function topupDisplayStatus(status: string, gatewayStatus?: string | null, abandoned = false) {
  if (abandoned && status === "pending" && (!gatewayStatus || ["awaiting_payment", "pending"].includes(gatewayStatus))) return "buyer_abandoned";
  if (status !== "pending" || !gatewayStatus) return status;
  if (gatewayStatus === "awaiting_payment") return "awaiting_payment";
  if (gatewayStatus === "pending") return "gateway_pending";
  if (gatewayStatus === "confirming") return "gateway_confirming";
  if (gatewayStatus === "paid") return "gateway_paid";
  if (["canceled", "cancelled"].includes(gatewayStatus)) return "gateway_canceled";
  if (gatewayStatus === "expired") return "gateway_expired";
  return "gateway_unknown";
}

export function topupCancelAction(
  status: string,
  gateway: { canCancel: boolean; canAbandon: boolean } | null,
  abandoned = false,
): "gateway_cancel" | "abandon" | "unavailable" {
  if (status !== "pending" || !gateway) return "unavailable";
  if (gateway.canCancel) return "gateway_cancel";
  if (gateway.canAbandon && !abandoned) return "abandon";
  return "unavailable";
}

export function topupAutoCheckCountdown(autoCloseAt: string | null, nowMs: number) {
  if (!autoCloseAt) return null;
  const deadlineMs = Date.parse(autoCloseAt);
  if (!Number.isFinite(deadlineMs) || !Number.isFinite(nowMs)) return null;
  const remainingSeconds = Math.max(0, Math.ceil((deadlineMs - nowMs) / 1000));
  return {
    remainingSeconds,
    phase: remainingSeconds === 0 ? "checking" as const
      : remainingSeconds <= 300 ? "soon" as const : "active" as const,
  };
}

export function formatTopupCountdown(seconds: number) {
  const whole = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(whole / 60);
  return `${String(minutes).padStart(2, "0")}:${String(whole % 60).padStart(2, "0")}`;
}

export function safeMarketReturnPath(raw: string | null): string | null {
  if (!raw || !raw.startsWith("/") || raw.startsWith("//") || raw.includes("\\")) return null;
  try {
    const url = new URL(raw, "https://router.invalid");
    if (url.origin !== "https://router.invalid") return null;
    if (url.pathname !== "/share-market/" && url.pathname !== "/client-market/") return null;
    return `${url.pathname}${url.search}`;
  } catch {
    return null;
  }
}

export function walletTopupHref(shortfallUnits: string, returnTo: string): string {
  const params = new URLSearchParams();
  const shortfall = BigInt(shortfallUnits);
  if (shortfall > 0n) {
    const minor = (shortfall + 86_399n) / 86_400n;
    const capped = minor < 100n ? 100n : minor > 100_000n ? 100_000n : minor;
    params.set("amount", `${capped / 100n}.${(capped % 100n).toString().padStart(2, "0")}`);
  }
  const path = safeMarketReturnPath(returnTo);
  if (path) params.set("returnTo", path);
  const query = params.toString();
  return `/account/wallet/${query ? `?${query}` : ""}`;
}
