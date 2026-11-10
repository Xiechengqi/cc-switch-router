/** Exact USD/native-unit arithmetic; monetary values never cross Number. */
export const RESOURCE_UNITS = {
  "image.output": 1n, "video.output_millisecond": 1000n,
  "audio.input_millisecond": 1000n, "audio.output_millisecond": 1000n,
  "image.input_token": 1000000n, "image.output_token": 1000000n,
  "audio.input_token": 1000000n, "audio.output_token": 1000000n,
  "tool.web_search": 1n, "tool.web_fetch": 1n, "tool.file_search": 1n,
  "tool.code_execution": 1n, "tool.computer": 1n, "tool.mcp": 1n,
  "context.cached_token": 1000000n,
} as const;
export type ResourceDimension = keyof typeof RESOURCE_UNITS;
export type ResourcePriceSettings = {
  models: Record<string, Record<string, string>>;
  exactAliases: Record<string, string>;
};
export type ResourcePriceState = {
  revision: number; settings: ResourcePriceSettings;
  pricingRevision?: string | null; effectiveFromMs?: number | null;
};
const SCALE = 1000000000000n;
export function resourceRateInput(picos: string, unit: bigint): string {
  if (!/^(0|[1-9]\d*)$/.test(picos)) throw new Error("invalid stored resource price");
  const amount = BigInt(picos) * unit;
  const fraction = (amount % SCALE).toString().padStart(12, "0").replace(/0+$/, "");
  return (amount / SCALE).toString() + (fraction ? "." + fraction : "");
}
export function parseResourceRate(usd: string, unit: bigint): string | undefined {
  const match = /^(\d+)(?:\.(\d{0,12}))?$/.exec(usd.trim());
  if (!match || unit <= 0n) return undefined;
  const amount = BigInt(match[1]) * SCALE + BigInt((match[2] ?? "").padEnd(12, "0"));
  if (amount % unit !== 0n || amount / unit > 9223372036854775807n) return undefined;
  return (amount / unit).toString();
}
