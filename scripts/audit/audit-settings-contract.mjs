#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const settingsSource = fs.readFileSync(path.join(root, "src/admin/settings.rs"), "utf8");
const configSource = fs.readFileSync(path.join(root, "src/config.rs"), "utf8");
const frontendSource = fs.readFileSync(path.join(root, "frontend/lib/settings-messages.ts"), "utf8");
const apiSource = fs.readFileSync(path.join(root, "src/api.rs"), "utf8");

function difference(left, right) {
  const rightSet = new Set(right);
  return left.filter((value) => !rightSet.has(value));
}

const settingsBlock = settingsSource.match(/pub const SETTINGS_FIELDS: &\[SettingsField\] = &\[([\s\S]*?)\n\];\n\npub fn schema_response/);
if (!settingsBlock) throw new Error("SETTINGS_FIELDS block not found");
const schemaKeyOccurrences = [...settingsBlock[1].matchAll(/key: "(CC_SWITCH_ROUTER_[A-Z0-9_]+)"/g)].map((match) => match[1]);
const schemaKeys = [...new Set(schemaKeyOccurrences)].sort();
const envFunction = configSource.match(/fn default_env_contents\(\) -> String \{([\s\S]*?)\n\}\n\nfn env_var/);
if (!envFunction) throw new Error("default_env_contents function not found");
const defaultEnvOccurrences = [...envFunction[1].matchAll(/^(CC_SWITCH_ROUTER_[A-Z0-9_]+)=/gm)].map((match) => match[1]);
const defaultEnvKeys = [...new Set(defaultEnvOccurrences)].sort();
const frontendBlock = frontendSource.match(/export const SETTINGS_FIELD_KEYS = \[([\s\S]*?)\] as const;/);
if (!frontendBlock) throw new Error("SETTINGS_FIELD_KEYS block not found");
const frontendOccurrences = [...frontendBlock[1].matchAll(/"(CC_SWITCH_ROUTER_[A-Z0-9_]+)"/g)].map((match) => match[1]);
const frontendKeys = [...new Set(frontendOccurrences)].sort();

const errors = [];
if (schemaKeys.length !== 119) errors.push(`expected 119 Settings fields, found ${schemaKeys.length}`);
for (const [label, occurrences, unique] of [
  ["Settings schema", schemaKeyOccurrences, schemaKeys],
  ["default env", defaultEnvOccurrences, defaultEnvKeys],
  ["frontend catalog", frontendOccurrences, frontendKeys],
]) {
  if (occurrences.length !== unique.length) errors.push(`${label} contains duplicate fields`);
}
for (const [label, keys] of [["default env", defaultEnvKeys], ["frontend catalog", frontendKeys]]) {
  for (const key of difference(schemaKeys, keys)) errors.push(`${label} is missing ${key}`);
  for (const key of difference(keys, schemaKeys)) errors.push(`${label} has stale ${key}`);
}

for (const category of [
  "GeneralDisplay",
  "Connectivity",
  "DataLifecycle",
  "IdentitySecurity",
  "Notifications",
  "Observability",
  "Marketplace",
]) {
  if (!settingsSource.includes(`Self::${category}`)) errors.push(`missing Settings category ${category}`);
}

for (const route of [
  '"/v1/admin/settings"',
  '"/v1/admin/settings/validate"',
  '"/v1/admin/client-server-release/validate"',
]) {
  if (!apiSource.includes(route)) errors.push(`missing Settings API route ${route}`);
}
for (const staleRoute of [
  '"/v1/admin/settings/schema"',
  '"/v1/admin/settings/values"',
]) {
  if (apiSource.includes(staleRoute)) errors.push(`stale Settings API route ${staleRoute}`);
}

if (errors.length) {
  console.error(`settings contract audit failed:\n${errors.map((error) => `- ${error}`).join("\n")}`);
  process.exit(1);
}

console.log(`settings contract audit ok: ${schemaKeys.length} fields, default env and frontend catalog aligned`);
