#!/usr/bin/env node
// Asserts the derived model-price catalog, the Rust PricingNote enum, the
// TypeScript ShareUsagePricingNote union, and the i18n note keys stay in lockstep.
//
// Usage: node scripts/audit/audit-model-prices.mjs --check

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { decimalStringToMicrosPer1M, microsPer1MToDecimalString } from "../pricing/derive-model-prices.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const errors = [];

function fail(message) {
  errors.push(message);
}

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function unique(values) {
  return [...new Set(values)];
}

function difference(left, right) {
  const rightSet = new Set(right);
  return left.filter((value) => !rightSet.has(value));
}

function parseArgs(argv) {
  const args = { check: false };
  for (let i = 2; i < argv.length; i += 1) {
    if (argv[i] === "--check") args.check = true;
    else throw new Error(`unknown flag: ${argv[i]}`);
  }
  if (!args.check) throw new Error("--check is required");
  return args;
}

const INTEGER_MICROS = /^-?\d+$/;
const CATALOG_MODEL_FIELDS = [
  "priceKey",
  "displayName",
  "longContextThreshold",
  "longContextInclusive",
  "supportsCacheBreakdown",
  "sourceNote",
  "rates",
];
const CATALOG_RATE_FIELDS = [
  "serviceTier",
  "contextTier",
  "input",
  "output",
  "cacheRead",
  "cacheWrite5m",
  "cacheWrite1h",
];
const CATALOG_ALIAS_FIELDS = ["pattern", "matchKind", "priceKey", "priority"];
const CATALOG_SOURCE_FIELDS = ["name", "url", "sha256", "license"];
const DOCUMENT_FIELDS = [
  "schemaVersion",
  "revision",
  "derivedAt",
  "source",
  "models",
  "aliases",
];
const RUST_MODEL_PRICE_FIELDS = [
  "price_key",
  "display_name",
  "long_context_threshold",
  "long_context_inclusive",
  "supports_cache_breakdown",
  "rates",
];
const RUST_RATE_SET_FIELDS = [
  "input_micros_per_1m",
  "output_micros_per_1m",
  "cache_read_micros_per_1m",
  "cache_write_5m_micros_per_1m",
  "cache_write_1h_micros_per_1m",
];

parseArgs(process.argv);

const catalogPath = path.join(root, "pricing/model-prices.json");
if (!fs.existsSync(catalogPath)) {
  fail("pricing/model-prices.json is missing");
  reportAndExit();
}

const catalog = JSON.parse(fs.readFileSync(catalogPath, "utf8"));
if (catalog.schemaVersion !== 1) {
  fail(`pricing/model-prices.json schemaVersion must be 1, got ${catalog.schemaVersion}`);
}

const extraDocumentFields = difference(Object.keys(catalog), DOCUMENT_FIELDS);
const missingDocumentFields = difference(DOCUMENT_FIELDS, Object.keys(catalog));
for (const field of extraDocumentFields) fail(`catalog document has unknown field ${field}`);
for (const field of missingDocumentFields) fail(`catalog document is missing field ${field}`);

if (!catalog.source || typeof catalog.source !== "object") {
  fail("catalog source block is missing");
} else {
  for (const field of difference(Object.keys(catalog.source), CATALOG_SOURCE_FIELDS)) {
    fail(`catalog source has unknown field ${field}`);
  }
  for (const field of difference(CATALOG_SOURCE_FIELDS, Object.keys(catalog.source))) {
    fail(`catalog source is missing field ${field}`);
  }
}

if (!Array.isArray(catalog.models) || catalog.models.length === 0) {
  fail("catalog contains no models");
}

const priceKeys = [];
const modelsByKey = new Map();
for (const [index, model] of (catalog.models || []).entries()) {
  const label = `models[${index}]`;
  for (const field of difference(Object.keys(model), CATALOG_MODEL_FIELDS)) {
    fail(`${label} has unknown field ${field}`);
  }
  for (const field of difference(CATALOG_MODEL_FIELDS, Object.keys(model))) {
    fail(`${label} is missing field ${field}`);
  }
  if (typeof model.priceKey !== "string" || !model.priceKey.trim()) {
    fail(`${label} priceKey is empty`);
    continue;
  }
  if (model.priceKey !== model.priceKey.trim().toLowerCase()) {
    fail(`${label} priceKey must already be lower+trim`);
  }
  if (priceKeys.includes(model.priceKey)) fail(`duplicate priceKey ${model.priceKey}`);
  priceKeys.push(model.priceKey);
  modelsByKey.set(model.priceKey, model);

  if (typeof model.sourceNote !== "string" || !model.sourceNote.trim()) {
    fail(`${model.priceKey}: sourceNote is empty`);
  }
  if (model.longContextInclusive !== false) {
    fail(
      `${model.priceKey}: longContextInclusive must be false in the derived catalog (phase 1 underestimates rather than guessing vendor inclusivity)`,
    );
  }
  if (!Array.isArray(model.rates) || model.rates.length === 0) {
    fail(`${model.priceKey}: rates is empty`);
    continue;
  }

  const seenTiers = new Set();
  for (const [rateIndex, rate] of model.rates.entries()) {
    const rateLabel = `${model.priceKey}.rates[${rateIndex}]`;
    for (const field of difference(Object.keys(rate), CATALOG_RATE_FIELDS)) {
      fail(`${rateLabel} has unknown field ${field}`);
    }
    for (const field of difference(CATALOG_RATE_FIELDS, Object.keys(rate))) {
      fail(`${rateLabel} is missing field ${field}`);
    }
    const combo = `${rate.serviceTier}/${rate.contextTier}`;
    if (seenTiers.has(combo)) fail(`${model.priceKey}: duplicate rate row ${combo}`);
    seenTiers.add(combo);
    for (const field of ["input", "output", "cacheRead", "cacheWrite5m", "cacheWrite1h"]) {
      const raw = rate[field];
      if (field === "cacheWrite1h" && raw == null) continue;
      if (typeof raw !== "string" || !INTEGER_MICROS.test(raw)) {
        fail(`${rateLabel}.${field} is not an integer micro-USD string`);
        continue;
      }
      if (raw.includes(".")) fail(`${rateLabel}.${field} contains a float literal`);
      const value = BigInt(raw);
      if (value < 0n) fail(`${rateLabel}.${field} is negative`);
      const roundTripped = decimalStringToMicrosPer1M(microsPer1MToDecimalString(value));
      if (roundTripped !== value) {
        fail(`${rateLabel}.${field}: micros ${raw} is not reversible through usd-per-token`);
      }
    }
  }
  if (!seenTiers.has("standard/base")) {
    fail(`${model.priceKey}: missing fallback rate (standard, base)`);
  }
  if (model.longContextThreshold != null) {
    if (!Number.isInteger(model.longContextThreshold) || model.longContextThreshold <= 0) {
      fail(`${model.priceKey}: longContextThreshold must be a positive integer`);
    }
    const hasLong = model.rates.some(
      (rate) => rate.serviceTier === "standard" && rate.contextTier === "long",
    );
    if (!hasLong) {
      fail(`${model.priceKey}: declared longContextThreshold without a (standard, long) rate`);
    }
  }
  const has1h = model.rates.some((rate) => rate.cacheWrite1h != null);
  if (has1h && model.supportsCacheBreakdown !== true) {
    fail(`${model.priceKey}: cacheWrite1h is set but supportsCacheBreakdown is not true`);
  }
}

const aliasPatterns = [];
for (const [index, alias] of (catalog.aliases || []).entries()) {
  const label = `aliases[${index}]`;
  for (const field of difference(Object.keys(alias), CATALOG_ALIAS_FIELDS)) {
    fail(`${label} has unknown field ${field}`);
  }
  for (const field of difference(CATALOG_ALIAS_FIELDS, Object.keys(alias))) {
    fail(`${label} is missing field ${field}`);
  }
  if (typeof alias.pattern !== "string" || !alias.pattern.trim()) {
    fail(`${label} pattern is empty`);
    continue;
  }
  if (alias.pattern !== alias.pattern.trim().toLowerCase()) {
    fail(`${label} pattern must already be lower+trim`);
  }
  const identity = `${alias.pattern}\0${alias.matchKind}`;
  if (aliasPatterns.includes(identity)) {
    fail(`duplicate alias ${alias.pattern} (${alias.matchKind})`);
  }
  aliasPatterns.push(identity);
  if (!modelsByKey.has(alias.priceKey)) {
    fail(`${label} points at unknown priceKey ${alias.priceKey}`);
  }
}

const rustPricing = read("src/model_pricing.rs");
const rustCatalog = read("src/model_price_catalog.rs");
for (const field of RUST_MODEL_PRICE_FIELDS) {
  if (!rustPricing.includes(`pub ${field}:`)) {
    fail(`src/model_pricing.rs ModelPrice is missing field ${field}`);
  }
}
for (const field of RUST_RATE_SET_FIELDS) {
  if (!rustPricing.includes(`pub ${field}:`)) {
    fail(`src/model_pricing.rs RateSet is missing field ${field}`);
  }
}
for (const field of [
  "price_key",
  "display_name",
  "long_context_threshold",
  "long_context_inclusive",
  "supports_cache_breakdown",
  "source_note",
  "rates",
]) {
  if (!rustCatalog.includes(`${field}:`)) {
    fail(`src/model_price_catalog.rs CatalogModel is missing ${field}`);
  }
}

const allBlock = rustPricing.match(
  /pub const ALL: \[PricingNote; (\d+)\] = \[([\s\S]*?)\];/,
);
if (!allBlock) {
  fail("PricingNote::ALL is missing");
}
const allVariants = unique(
  [...(allBlock?.[2] || "").matchAll(/Self::(\w+)/g)].map((match) => match[1]),
);
if (allBlock && Number(allBlock[1]) !== allVariants.length) {
  fail(
    `PricingNote::ALL length ${allBlock[1]} does not match ${allVariants.length} listed variants`,
  );
}
const rustNotes = [];
for (const variant of allVariants) {
  const mapped = rustPricing.match(
    new RegExp(`Self::${variant} => "([A-Za-z0-9]+)"`),
  );
  if (!mapped) {
    fail(`PricingNote::${variant} has no as_str mapping`);
    continue;
  }
  rustNotes.push(mapped[1]);
}
rustNotes.sort();
if (unique(rustNotes).length !== rustNotes.length) {
  fail("PricingNote::as_str mappings are not unique");
}

const typesSource = read("frontend/lib/types.ts");
const typeUnion = typesSource.match(
  /export type ShareUsagePricingNote =\s*([\s\S]*?);/,
);
if (!typeUnion) {
  fail("ShareUsagePricingNote union is missing");
} else {
  const tsNotes = unique(
    [...typeUnion[1].matchAll(/"([A-Za-z0-9]+)"/g)].map((match) => match[1]),
  ).sort();
  for (const note of difference(rustNotes, tsNotes)) {
    fail(`ShareUsagePricingNote is missing ${note}`);
  }
  for (const note of difference(tsNotes, rustNotes)) {
    fail(`ShareUsagePricingNote has stale ${note}`);
  }
}

const i18nSource = read("frontend/lib/i18n.ts");
const i18nNotes = unique(
  [...i18nSource.matchAll(/"dashboard\.userLimit\.note\.([A-Za-z0-9]+)":/g)].map(
    (match) => match[1],
  ),
).sort();
for (const note of difference(rustNotes, i18nNotes)) {
  fail(`i18n is missing dashboard.userLimit.note.${note}`);
}
for (const note of difference(i18nNotes, rustNotes)) {
  fail(`i18n has stale dashboard.userLimit.note.${note}`);
}
for (const locale of ["en", "zh-CN"]) {
  for (const note of rustNotes) {
    const occurrences = [
      ...i18nSource.matchAll(
        new RegExp(`"dashboard\\.userLimit\\.note\\.${note}":`, "g"),
      ),
    ];
    if (occurrences.length < 2) {
      fail(`dashboard.userLimit.note.${note} is not present in both locales (${locale} check)`);
    }
  }
}

const catalogKeys = new Set(priceKeys);
const requiredServed = [
  "gpt-5",
  "claude-4-sonnet-20250514",
  "claude-sonnet-4-5",
  "gpt-4o",
];
for (const key of requiredServed) {
  const aliased = (catalog.aliases || []).some(
    (alias) => alias.pattern === key || alias.priceKey === key,
  );
  if (!catalogKeys.has(key) && !aliased) {
    fail(`derived catalog is missing served model ${key}`);
  }
}

if (priceKeys.length < 100) {
  fail(`derived catalog only has ${priceKeys.length} models; expected the full chat/responses set`);
}

function reportAndExit() {
  if (errors.length) {
    console.error(`model price audit failed:\n${errors.map((error) => `- ${error}`).join("\n")}`);
    process.exit(1);
  }
  console.log(
    `model price audit ok: ${priceKeys.length} priceKeys, ${rustNotes.length} notes, longContextInclusive hardcoded false`,
  );
}

reportAndExit();
