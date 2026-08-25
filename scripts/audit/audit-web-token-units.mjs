#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const frontendRoot = path.join(root, "frontend");
const errors = [];

function walk(relativeRoot) {
  const absoluteRoot = path.join(root, relativeRoot);
  if (!fs.existsSync(absoluteRoot)) return [];
  const files = [];
  for (const entry of fs.readdirSync(absoluteRoot, { withFileTypes: true })) {
    const relativePath = path.join(relativeRoot, entry.name);
    if (entry.isDirectory()) files.push(...walk(relativePath));
    else if (entry.name.endsWith(".tsx")) files.push(relativePath);
  }
  return files;
}

function lineNumber(source, index) {
  return source.slice(0, index).split("\n").length;
}

const tokenUnitSource = fs.readFileSync(path.join(frontendRoot, "lib/token-units.ts"), "utf8");
for (const marker of [
  "TOKENS_PER_MILLION = 1_000_000",
  "TOKENS_PER_WAN = 10_000",
  "TOKENS_PER_YI = 100_000_000",
  "MILLION_INPUT_PATTERN",
  "millionsInputToTokens",
  "tokensToMillionsInput",
  "Number.isSafeInteger(tokens)",
  '"万"',
  '"亿"',
]) {
  if (!tokenUnitSource.includes(marker)) errors.push(`frontend/lib/token-units.ts is missing ${marker}`);
}

const tokenUnitTests = fs.readFileSync(path.join(frontendRoot, "lib/token-units.test.ts"), "utf8");
for (const marker of [
  "0.000001",
  "12.000001",
  "9007199254.740992",
  "1.25 M",
  "9,007,199,254.740991 M",
  "125万",
  "1亿",
]) {
  if (!tokenUnitTests.includes(marker)) errors.push(`frontend/lib/token-units.test.ts is missing ${marker}`);
}

for (const relativePath of [
  ...walk("frontend/app"),
  ...walk("frontend/components"),
]) {
  const source = fs.readFileSync(path.join(root, relativePath), "utf8");
  const inputPattern = /<(?:input|Input)\b[\s\S]*?\/>/g;
  for (const match of source.matchAll(inputPattern)) {
    const tag = match[0];
    if (!/(?:tokenLimit|consumedTokens)/.test(tag)) continue;
    const location = `${relativePath}:${lineNumber(source, match.index)}`;
    if (!tag.includes('inputMode="decimal"')) {
      errors.push(`${location} configures a Token quantity without decimal inputMode`);
    }
    if (tag.includes('type="number"')) {
      errors.push(`${location} configures a Token quantity as a raw number input`);
    }
    if (!source.includes("millionsInputToTokens")) {
      errors.push(`${location} configures a Token quantity without M-to-Token conversion`);
    }
  }
}

const messages = fs.readFileSync(path.join(frontendRoot, "lib/i18n.ts"), "utf8");
for (const marker of [
  '"shareMarket.tokensMillions": "Token limit (M)"',
  '"shareMarket.tokensMillions": "Token 限额（百万）"',
  '"dashboard.field.tokenLimit": "Token limit (M)"',
  '"dashboard.field.tokenLimit": "Token 限制（M）"',
  '"dashboard.userLimit.consumedTokens": "Consumed tokens (M, current period)"',
  '"dashboard.userLimit.consumedTokens": "已消耗 Token（M，当前周期）"',
]) {
  if (!messages.includes(marker)) errors.push(`frontend/lib/i18n.ts is missing ${marker}`);
}

if (errors.length) {
  console.error(`web Token unit audit failed:\n${errors.map((error) => `- ${error}`).join("\n")}`);
  process.exit(1);
}

console.log("web Token unit audit ok: editable Token quantities use exact M conversion");
