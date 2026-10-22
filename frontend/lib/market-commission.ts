// Same integer round-half-up as the server. A preview is an estimate: actual
// fees accrue on fulfilled ledger units, not on the unfulfilled monthly price.
export function commissionPreview(price: string, rateBps: number) {
  if (!/^\d+(\.\d{1,2})?$/.test(price.trim()) || !Number.isInteger(rateBps) || rateBps < 0 || rateBps > 10000) return null;
  const [whole, fraction = ""] = price.trim().split(".");
  const cents = BigInt(whole) * 100n + BigInt(fraction.padEnd(2, "0"));
  if (cents <= 0n || cents > BigInt(Number.MAX_SAFE_INTEGER)) return null;
  const grossUnits = cents * 86400n;
  const feeUnits = (grossUnits * BigInt(rateBps) + 5000n) / 10000n;
  return { grossUnits: grossUnits.toString(), feeUnits: feeUnits.toString(), netUnits: (grossUnits - feeUnits).toString() };
}
