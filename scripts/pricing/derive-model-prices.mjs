#!/usr/bin/env node
// Derives pricing/model-prices.json from an upstream LiteLLM
// model_prices_and_context_window.json.
//
// The upstream file is NOT vendored: SOURCE_PROVENANCE.json enforces
// `runtimeAndBuildInputsMustBeRepositoryOwned: true`, and
// scripts/audit/audit-source-provenance.mjs hard-requires `technicalInput: false`
// on every vendored source. A catalog that drives computation is a technical
// input by definition, so it must be derived into a repository-owned artifact
// and reviewed as a diff. See docs/design-share-user-model-usage-and-pricing.md §5.3.
//
// Usage:
//   node scripts/pricing/derive-model-prices.mjs --source <litellm.json> [--out pricing/model-prices.json]

import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

const SERVICE_TIERS = [
  { tier: "standard", suffix: "" },
  { tier: "priority", suffix: "_priority" },
  { tier: "flex", suffix: "_flex" },
];

const CATEGORY_STEMS = {
  input: "input_cost_per_token",
  output: "output_cost_per_token",
  cacheRead: "cache_read_input_token_cost",
  cacheWrite5m: "cache_creation_input_token_cost",
};

// micro-USD per 1M tokens = usd_per_token * 1e6 (per 1M) * 1e6 (micro) = * 1e12
const SCALE_EXPONENT = 12;

function parseArgs(argv) {
  const args = { out: path.join(root, "pricing/model-prices.json") };
  for (let i = 2; i < argv.length; i += 1) {
    const flag = argv[i];
    if (flag === "--source") args.source = argv[++i];
    else if (flag === "--out") args.out = path.resolve(argv[++i]);
    else if (flag === "--source-url") args.sourceUrl = argv[++i];
    else throw new Error(`unknown flag: ${flag}`);
  }
  if (!args.source) throw new Error("--source <litellm model_prices_and_context_window.json> is required");
  return args;
}

// Exact decimal -> integer micro-USD/1M, no float arithmetic.
// The input is the shortest round-tripping decimal string for the parsed double,
// which for these short source literals reproduces the literal exactly.
export function decimalStringToMicrosPer1M(literal) {
  const match = /^([+-]?)(\d*)(?:\.(\d*))?(?:[eE]([+-]?\d+))?$/.exec(String(literal).trim());
  if (!match) throw new Error(`not a decimal literal: ${literal}`);
  const [, sign, intPart = "", fracPart = "", expPart] = match;
  if (sign === "-") throw new Error(`negative price is not representable: ${literal}`);
  const digits = `${intPart}${fracPart}`.replace(/^0+(?=\d)/, "");
  if (digits === "") return 0n;
  const exponent = (expPart ? Number.parseInt(expPart, 10) : 0) - fracPart.length;
  const shift = exponent + SCALE_EXPONENT;
  const value = BigInt(digits);
  if (shift >= 0) return value * 10n ** BigInt(shift);
  const divisor = 10n ** BigInt(-shift);
  const quotient = value / divisor;
  const remainder = value % divisor;
  // round half up
  return remainder * 2n >= divisor ? quotient + 1n : quotient;
}

// Inverse, used by the audit to prove the conversion is lossless within tolerance.
export function microsPer1MToDecimalString(micros) {
  const negative = micros < 0n;
  const digits = (negative ? -micros : micros).toString().padStart(SCALE_EXPONENT + 1, "0");
  const intPart = digits.slice(0, digits.length - SCALE_EXPONENT);
  const fracPart = digits.slice(digits.length - SCALE_EXPONENT).replace(/0+$/, "");
  return `${negative ? "-" : ""}${intPart}${fracPart ? `.${fracPart}` : ""}`;
}

function readNumber(entry, key) {
  const value = entry[key];
  if (value === undefined || value === null) return null;
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return null;
  // String(number) yields the shortest decimal that round-trips.
  return decimalStringToMicrosPer1M(String(value));
}

function detectThresholds(entry) {
  const found = new Set();
  for (const key of Object.keys(entry)) {
    const match = /_above_(\d+)k_tokens(?:_priority|_flex)?$/.exec(key);
    if (match) found.add(Number.parseInt(match[1], 10) * 1000);
  }
  if (found.size === 0) return null;
  // A model declaring several thresholds is not representable by a single
  // two-tier catalog; take the lowest and record it, rather than guessing.
  return [...found].sort((a, b) => a - b)[0];
}

function contextSuffix(threshold) {
  if (threshold === null) return "";
  return `_above_${threshold / 1000}k_tokens`;
}

function prettifyName(priceKey) {
  const UPPER = new Set(["gpt", "ai", "llm", "ui", "us", "eu", "hd", "tts", "v2", "v3", "xl", "sd"]);
  return priceKey
    .split("/")
    .pop()
    .split(/[-_.]/)
    .filter(Boolean)
    .map((token) => {
      if (UPPER.has(token.toLowerCase())) return token.toUpperCase();
      if (/^\d/.test(token)) return token;
      return token.charAt(0).toUpperCase() + token.slice(1);
    })
    .join(" ");
}

function buildRateRow(entry, tierSuffix, threshold) {
  const ctx = contextSuffix(threshold);
  const row = {};
  // Tracked per dimension: a long-context key proves nothing about the service
  // tier. Conflating them emits a `priority` row holding standard prices, which
  // silently suppresses the `serviceTierFellBack` note the reader needs.
  let tierExplicit = false;
  let contextExplicit = false;
  for (const [category, stem] of Object.entries(CATEGORY_STEMS)) {
    // Most specific first. `specific` marks a key that actually encodes the
    // dimension we are building; anything else is a fallback and must NOT be
    // mistaken for upstream having a distinct price for this combination.
    // (Candidates are labelled rather than positional: an earlier version
    // filtered nulls out of this list, which reindexed it and made every model
    // look like it had a priority price.)
    const candidates = [
      { key: `${stem}${ctx}${tierSuffix}`, tier: Boolean(tierSuffix), context: Boolean(ctx) },
      { key: `${stem}${ctx}`, tier: false, context: Boolean(ctx) },
      { key: `${stem}${tierSuffix}`, tier: Boolean(tierSuffix), context: false },
      { key: stem, tier: false, context: false },
    ];
    let micros = null;
    const seen = new Set();
    for (const candidate of candidates) {
      if (seen.has(candidate.key)) continue;
      seen.add(candidate.key);
      const value = readNumber(entry, candidate.key);
      if (value === null) continue;
      micros = value;
      if (candidate.tier) tierExplicit = true;
      if (candidate.context) contextExplicit = true;
      break;
    }
    row[category] = micros;
  }
  if (row.input === null || row.output === null) return null;
  // cache_read / cache_write are optional; absent means no cache pricing upstream.
  row.cacheRead = row.cacheRead ?? 0n;
  row.cacheWrite5m = row.cacheWrite5m ?? 0n;
  // The 1h cache-write price has no context or tier variants upstream, so the
  // base value is all there is. It is only ever used as the §7.5 upper bound,
  // which requires 1h >= 5m for THIS row — on a long-context row the 5m price
  // is scaled up while the 1h price is not, and can exceed it. Emitting it
  // anyway would produce an "upper" bound below the point estimate.
  const cacheWrite1h = readNumber(entry, "cache_creation_input_token_cost_above_1hr");
  row.cacheWrite1h = cacheWrite1h !== null && cacheWrite1h > row.cacheWrite5m ? cacheWrite1h : null;
  return { row, tierExplicit, contextExplicit };
}

function main() {
  const args = parseArgs(process.argv);
  const raw = fs.readFileSync(args.source);
  const sourceSha256 = crypto.createHash("sha256").update(raw).digest("hex");
  const upstream = JSON.parse(raw.toString("utf8"));
  const derivedAt = new Date().toISOString().slice(0, 10);

  const models = [];
  const aliases = [];
  const report = { skippedNoPrice: [], skippedMultiplierOnlyLongContext: [], multiThreshold: [] };

  for (const [rawKey, entry] of Object.entries(upstream)) {
    if (!entry || typeof entry !== "object") continue;
    if (rawKey === "sample_spec") continue;
    if (entry.mode && entry.mode !== "chat" && entry.mode !== "responses") continue;

    const priceKey = rawKey.trim().toLowerCase();
    if (!priceKey) continue;

    let threshold = detectThresholds(entry);
    const hasExplicitLongPrices =
      threshold !== null &&
      Object.keys(entry).some((key) => key.includes(`_above_${threshold / 1000}k_tokens`));
    const multiplierOnly =
      threshold === null &&
      (entry.long_context_input_cost_multiplier !== undefined ||
        entry.long_context_output_cost_multiplier !== undefined);
    if (multiplierOnly) {
      // Two multipliers cover input/output only — they say nothing about
      // cache_read, which dominates Claude Code traffic. Rather than fabricate
      // the missing two categories, emit no long tier for this model.
      report.skippedMultiplierOnlyLongContext.push(priceKey);
    }
    if (threshold !== null && !hasExplicitLongPrices) threshold = null;

    const rates = [];
    for (const { tier, suffix } of SERVICE_TIERS) {
      for (const contextTier of threshold === null ? ["base"] : ["base", "long"]) {
        const built = buildRateRow(entry, suffix, contextTier === "long" ? threshold : null);
        if (!built) continue;
        // Only emit non-standard tiers when upstream actually carries a distinct
        // price; otherwise the runtime fallback chain (§5.2) handles it and
        // emits a `serviceTierFellBack` note instead of a silent duplicate.
        if (tier !== "standard" && !built.tierExplicit) continue;
        if (contextTier === "long" && !built.contextExplicit) continue;
        rates.push({
          serviceTier: tier,
          contextTier,
          input: built.row.input.toString(),
          output: built.row.output.toString(),
          cacheRead: built.row.cacheRead.toString(),
          cacheWrite5m: built.row.cacheWrite5m.toString(),
          cacheWrite1h: built.row.cacheWrite1h === null ? null : built.row.cacheWrite1h.toString(),
        });
      }
    }
    if (!rates.some((rate) => rate.serviceTier === "standard" && rate.contextTier === "base")) {
      report.skippedNoPrice.push(priceKey);
      continue;
    }
    if (threshold !== null && !rates.some((rate) => rate.contextTier === "long")) threshold = null;

    models.push({
      priceKey,
      displayName: prettifyName(priceKey),
      longContextThreshold: threshold,
      // Upstream key names read "above N tokens"; the vendors document a strict
      // greater-than. Admin override exists for vendors that document otherwise.
      longContextInclusive: false,
      supportsCacheBreakdown: entry.cache_creation_input_token_cost_above_1hr !== undefined,
      sourceNote: `derived from LiteLLM model_prices_and_context_window.json entry \`${rawKey}\` (sha256 ${sourceSha256.slice(0, 12)}) on ${derivedAt}`,
      rates,
    });

    const bare = priceKey.includes("/") ? priceKey.slice(priceKey.lastIndexOf("/") + 1) : null;
    if (bare && bare !== priceKey) {
      aliases.push({ pattern: bare, matchKind: "exact", priceKey, priority: 50 });
    }
  }

  models.sort((a, b) => (a.priceKey < b.priceKey ? -1 : a.priceKey > b.priceKey ? 1 : 0));

  // An alias must not shadow a real model key, and must be unique.
  const modelKeys = new Set(models.map((model) => model.priceKey));
  const seenAlias = new Set();
  const cleanAliases = aliases
    .filter((alias) => !modelKeys.has(alias.pattern))
    .filter((alias) => {
      if (seenAlias.has(alias.pattern)) return false;
      seenAlias.add(alias.pattern);
      return true;
    })
    .sort((a, b) => (a.pattern < b.pattern ? -1 : a.pattern > b.pattern ? 1 : 0));

  const body = { schemaVersion: 1, models, aliases: cleanAliases };
  const revision = crypto
    .createHash("sha256")
    .update(JSON.stringify(body))
    .digest("hex")
    .slice(0, 12);

  const output = {
    schemaVersion: 1,
    revision,
    derivedAt,
    source: {
      name: "LiteLLM model_prices_and_context_window.json",
      url: args.sourceUrl ?? "https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json",
      sha256: sourceSha256,
      license: "MIT",
    },
    models,
    aliases: cleanAliases,
  };

  fs.mkdirSync(path.dirname(args.out), { recursive: true });
  fs.writeFileSync(args.out, `${JSON.stringify(output, null, 2)}\n`, "utf8");

  process.stderr.write(
    `derived ${models.length} models, ${cleanAliases.length} aliases -> ${path.relative(root, args.out)}\n` +
      `  revision ${revision}\n` +
      `  skipped (no usable price): ${report.skippedNoPrice.length}\n` +
      `  long context dropped (multiplier-only upstream): ${report.skippedMultiplierOnlyLongContext.length}` +
      `${report.skippedMultiplierOnlyLongContext.length ? ` [${report.skippedMultiplierOnlyLongContext.join(", ")}]` : ""}\n`,
  );
}

if (import.meta.url === `file://${process.argv[1]}`) main();
