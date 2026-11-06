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
if (schemaKeys.length !== 137) errors.push(`expected 137 Settings fields, found ${schemaKeys.length}`);
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
  "SiteAccess",
  "AccountsSecurity",
  "Marketplace",
  "Notifications",
  "Monitoring",
  "DataRetention",
  "Connectivity",
  "Deployment",
]) {
  if (!settingsSource.includes(`Self::${category}`)) errors.push(`missing Settings category ${category}`);
}

// The leftover-key cleanup offers every Router-prefixed env-file entry that
// is neither a Settings field nor in UNLISTED_ENV_KEYS. A key read anywhere
// else must be registered, or cleanup could delete a live setting.
const unlistedBlock = settingsSource.match(/pub const UNLISTED_ENV_KEYS: &\[&str\] = &\[([\s\S]*?)\n\];/);
if (!unlistedBlock) throw new Error("UNLISTED_ENV_KEYS block not found");
const unlistedKeys = new Set([...unlistedBlock[1].matchAll(/"(CC_SWITCH_ROUTER_[A-Z0-9_]+)"/g)].map((match) => match[1]));
// Retired keys the code mentions only to prove they stay ignored, and
// test-only scratch keys. Both are fine to clean up.
const ignoredKeys = new Set([
  "CC_SWITCH_ROUTER_CLIENT_ALERT_EMAILS",
  "CC_SWITCH_ROUTER_CLIENT_OFFLINE_NOTIFY_OWNER",
  "CC_SWITCH_ROUTER_MANAGED_ENV_CLEAR_TEST",
  "CC_SWITCH_ROUTER_MANAGED_ENV_PRECEDENCE_TEST",
  "CC_SWITCH_ROUTER_TEST_BOOL_GARBAGE",
  "CC_SWITCH_ROUTER_RETIRED_GATEWAY_SECRET",
  "CC_SWITCH_ROUTER_UX_TELEMETRY_ENABLED",
  "CC_SWITCH_ROUTER_NOT_IN_FILE",
]);
function rustFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) return rustFiles(full);
    return entry.name.endsWith(".rs") ? [full] : [];
  });
}
const schemaKeySet = new Set(schemaKeys);
for (const file of rustFiles(path.join(root, "src"))) {
  const source = fs.readFileSync(file, "utf8");
  for (const [, key] of source.matchAll(/"(CC_SWITCH_ROUTER_[A-Z0-9_]*[A-Z0-9])"/g)) {
    if (schemaKeySet.has(key) || unlistedKeys.has(key) || ignoredKeys.has(key)) continue;
    errors.push(`${path.relative(root, file)} reads ${key}, which is neither a Settings field nor in UNLISTED_ENV_KEYS`);
  }
}
for (const key of unlistedKeys) {
  if (schemaKeySet.has(key)) errors.push(`UNLISTED_ENV_KEYS repeats Settings field ${key}`);
}

for (const route of [
  '"/v1/admin/settings"',
  '"/v1/admin/settings/validate"',
  '"/v1/admin/settings/unrecognized-keys/remove"',
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
