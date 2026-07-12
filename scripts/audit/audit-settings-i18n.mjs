#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "../..");
const messagesPath = path.join(root, "frontend/lib/settings-messages.ts");
const rustSettingsPath = path.join(root, "src/admin/settings.rs");
const checkOnly = process.argv.includes("--check");

function read(filePath) {
  return fs.readFileSync(filePath, "utf8");
}

function extractFieldKeysFromRust(source) {
  return [...source.matchAll(/key: "(CC_SWITCH_ROUTER_[^"]+)"/g)].map((match) => match[1]);
}

function extractFieldKeysFromMessages(source) {
  const block = source.match(/export const SETTINGS_FIELD_KEYS = \[([\s\S]*?)\] as const;/);
  if (!block) throw new Error("SETTINGS_FIELD_KEYS block not found");
  return [...block[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

function extractMessageKeys(source, exportName) {
  const block = source.match(new RegExp(`export const ${exportName} = \\{([\\s\\S]*?)\\} as const;`));
  if (!block) throw new Error(`${exportName} block not found`);
  return new Set([...block[1].matchAll(/"([^"]+)":/g)].map((match) => match[1]));
}

function extractGroupSlugs(source) {
  const block = source.match(/export const SETTINGS_GROUP_SLUG: Record<string, string> = \{([\s\S]*?)\};/);
  if (!block) throw new Error("SETTINGS_GROUP_SLUG block not found");
  return [...block[1].matchAll(/:\s*"([^"]+)"/g)].map((match) => match[1]);
}

function main() {
  const messagesSource = read(messagesPath);
  const rustSource = read(rustSettingsPath);
  const rustKeys = extractFieldKeysFromRust(rustSource);
  const catalogKeys = extractFieldKeysFromMessages(messagesSource);
  const enKeys = extractMessageKeys(messagesSource, "settingsMessagesEn");
  const zhKeys = extractMessageKeys(messagesSource, "settingsMessagesZh");
  const groupSlugs = extractGroupSlugs(messagesSource);
  const errors = [];

  const rustSet = new Set(rustKeys);
  const catalogSet = new Set(catalogKeys);
  for (const key of rustSet) {
    if (!catalogSet.has(key)) errors.push(`missing catalog entry for ${key}`);
  }
  for (const key of catalogSet) {
    if (!rustSet.has(key)) errors.push(`stale catalog entry ${key} (not in SETTINGS_FIELDS)`);
  }

  for (const key of catalogKeys) {
    for (const suffix of ["label", "description"]) {
      const messageKey = `settings.field.${key}.${suffix}`;
      if (!enKeys.has(messageKey)) errors.push(`missing en ${messageKey}`);
      if (!zhKeys.has(messageKey)) errors.push(`missing zh-CN ${messageKey}`);
    }
  }

  for (const slug of groupSlugs) {
    const messageKey = `settings.group.${slug}`;
    if (!enKeys.has(messageKey)) errors.push(`missing en ${messageKey}`);
    if (!zhKeys.has(messageKey)) errors.push(`missing zh-CN ${messageKey}`);
  }

  for (const source of ["envFile", "default", "unset"]) {
    const messageKey = `settings.source.${source}`;
    if (!enKeys.has(messageKey)) errors.push(`missing en ${messageKey}`);
    if (!zhKeys.has(messageKey)) errors.push(`missing zh-CN ${messageKey}`);
  }

  if (errors.length) {
    console.error("settings i18n audit failed:\n" + errors.map((line) => `- ${line}`).join("\n"));
    process.exit(1);
  }

  console.log(
    `settings i18n audit ok: ${catalogKeys.length} fields, ${groupSlugs.length} groups, ${enKeys.size} en keys, ${zhKeys.size} zh-CN keys`,
  );
  if (!checkOnly) return;
}

main();
