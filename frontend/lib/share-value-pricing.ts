type RecordValue = Record<string, unknown>;
const object = (value: unknown): value is RecordValue => !!value && typeof value === "object" && !Array.isArray(value);
export type ExecutionPriceSummary = {model: string; rows: string[]; problems: string[]};
/** A convenience preview, never an authority: Rust validates again on publish. */
export function inspectExecutionPricing(value: unknown): ExecutionPriceSummary[] {
  if (!object(value) || !object(value.models)) return [];
  return Object.entries(value.models).map(([model, price]) => {
    const problems: string[] = [];
    if (!object(price) || !object(price.rates)) return {model, rows: [], problems: ["rates"]};
    if (!["rawInput", "freshInput"].includes(String(price.contextInputBasis))) problems.push("contextInputBasis");
    if (price.longContextThreshold != null && (!Number.isSafeInteger(price.longContextThreshold) || Number(price.longContextThreshold) <= 0)) problems.push("longContextThreshold");
    const rows = Object.keys(price.rates).sort();
    if (!rows.length) problems.push("rates");
    for (const tier of ["standard", "priority", "flex"]) {
      if (!rows.some(row => row.startsWith(tier + "/"))) continue;
      for (const context of price.longContextThreshold == null ? ["base"] : ["base", "long"]) {
        if (!rows.includes(tier + "/" + context)) problems.push(tier + "/" + context);
      }
    }
    for (const [name, rates] of Object.entries(price.rates)) {
      if (!/^(standard|priority|flex)\/(base|long)$/.test(name) || (!price.longContextThreshold && name.endsWith("/long"))) problems.push(name);
      if (!object(rates)) { problems.push(name); continue; }
      for (const field of ["input", "output", "cacheRead", "cacheWrite5m", "cacheWrite1h"]) {
        const rate = rates[field];
        if (rate == null) { if (field === "input" || field === "output") problems.push(name + "." + field); continue; }
        if (typeof rate !== "string" || !/^(0|[1-9]\d*)$/.test(rate) || BigInt(rate) > 9223372036854775807n) problems.push(name + "." + field);
      }
      const five = rates.cacheWrite5m != null, hour = rates.cacheWrite1h != null;
      const ttl = rates.cacheWritePolicy;
      if (!((ttl === "unavailable" && !five && !hour) || (ttl === "fiveMinutesOnly" && five && !hour)
        || (ttl === "oneHourOnly" && !five && hour) || (ttl === "both" && five && hour))) problems.push(name + ".cacheWritePolicy");
    }
    return {model, rows, problems};
  });
}
export function newExecutionPriceModel() {
  return {contextInputBasis: "rawInput", longContextThreshold: null, longContextInclusive: false,
    rates: {"standard/base": {input: null, output: null, cacheRead: null, cacheWrite5m: null, cacheWrite1h: null, cacheWritePolicy: "unavailable"}}};
}
