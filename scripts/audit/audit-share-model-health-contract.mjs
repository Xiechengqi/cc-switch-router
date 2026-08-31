#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const errors = [];

function read(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!fs.existsSync(absolutePath)) {
    errors.push(`${relativePath} is missing`);
    return "";
  }
  return fs.readFileSync(absolutePath, "utf8");
}

function requireMarkers(relativePath, markers) {
  const source = read(relativePath);
  for (const marker of markers) {
    if (!source.includes(marker)) errors.push(`${relativePath} is missing ${marker}`);
  }
  return source;
}

const models = requireMarkers("src/models.rs", [
  "MIN_SHARE_MARKET_CONTRACT_VERSION: u16 = 5",
  "SHARE_CONTRACT_VERSION: u16 = 6",
  "pub struct ProviderModelProbe",
  "pub requested_model: String",
  "pub wire_model: String",
  "pub response_mode: String",
  "pub health_fingerprint: String",
  "pub struct ShareModelHealthCalendarResponse",
  "pub observed_checks: u16",
  "pub monitoring_gap_checks: u16",
  "pub struct ShareModelHealthProbeEpoch",
]);
const store = requireMarkers("src/store.rs", [
  "validate_share_model_probes(share)?",
  "probe.payload_revision != 2",
  "model_probe_body_contains_sensitive_field",
  "invalid {app} modelProbe wire contract",
  "descriptor_model_probe_epoch_input",
  '.find_map(|(app_type, api_type, enabled, runtime)|',
  "claim_share_model_health_slot",
  "epoch_is_current",
  "STALE_CLAIM_SECS: i64 = 10 * 60",
  "share_model_health_calendar",
  "SLOTS_PER_DAY: u16 = 48",
  "expected_epoch_by_slot",
  "outcome == \"success\"",
  "outcome = 'unobserved'",
  "share_model_probe_observations",
  "can_view_share_model_health_calendar",
]);
const scheduler = requireMarkers("src/share_model_health.rs", [
  "SLOT_SECONDS: i64 = 30 * 60",
  "SLOT_RETENTION_DAYS: i64 = 400",
  "MAX_CONCURRENT_INSTALLATION_BATCHES: usize = 16",
  "std::time::Duration::from_secs(7 * 60)",
  ".buffer_unordered(MAX_CONCURRENT_INSTALLATION_BATCHES)",
  'BATCH_V1_PATH: &str = "/_share-router/model-health/batch"',
  'BATCH_V2_PATH: &str = "/_share-router/model-health/batch-v2"',
  "MAX_SHARE_ROUTE_FALLBACKS: usize = 2",
  "PreparedTarget",
  "slot_accepts_new_claim",
  "remaining_probe_budget",
  "list_client_tunnel_route_targets",
  "response_targets_mismatch",
  "validate_batch_result",
  "model_health_observation_id",
  'evidence_scope: "share_legacy"',
  'failure_domain: Some(failure_domain.to_string())',
]);
const main = requireMarkers("src/main.rs", [
  "mod share_model_health;",
  '"Share model health"',
  "crate::share_model_health::run_cycle",
]);
requireMarkers("src/api.rs", [
  '"/v1/shares/:share_id/model-health-calendar"',
  "share_model_probe_for_app",
  "probe.response_mode",
  "read_probe_body(resp, response_mode)",
]);
requireMarkers("src/schema.rs", [
  'include_str!("../schema/0027_share_model_health_slots.sql")',
  'include_str!("../schema/0028_share_model_health_evidence.sql")',
]);
requireMarkers("schema/0027_share_model_health_slots.sql", [
  "CREATE TABLE share_model_health_slots",
  "PRIMARY KEY (share_id, slot_start)",
  "slot_start % 1800 = 0",
  "idx_share_model_health_slots_retention",
]);
requireMarkers("schema/0028_share_model_health_evidence.sql", [
  "CREATE TABLE share_model_probe_observations",
  "CREATE TABLE share_model_probe_epochs",
  "evidence_scope",
  "failure_domain",
  "idx_share_model_health_slots_observation",
]);

const codexPriority = scheduler.indexOf("if target.support.codex");
const claudePriority = scheduler.indexOf("if target.support.claude");
const geminiPriority = scheduler.indexOf("if target.support.gemini");
if (!(codexPriority >= 0 && codexPriority < claudePriority && claudePriority < geminiPriority)) {
  errors.push("src/share_model_health.rs must select Codex/OpenAI before Claude/Anthropic before Gemini");
}

const connection = [
  "frontend/components/dashboard/share-connect-dialog.tsx",
  "frontend/components/dashboard/share-connection-test.tsx",
  "frontend/lib/share-model-probe.ts",
].map((relativePath) => [relativePath, read(relativePath)]);
requireMarkers("frontend/components/dashboard/share-connection-test.tsx", [
  "runtime?.modelProbe",
  "buildShareProbeCurl(baseUrl, probe, apiToken)",
  "probe.requestedModel",
  "runtime.modelPolicy.upstreamModel",
]);
requireMarkers("frontend/lib/share-model-probe.ts", [
  "probe.path",
  "JSON.stringify(probe.body)",
  "probe.responseMode",
]);
requireMarkers("frontend/components/dashboard/share-model-health-heatmap.tsx", [
  "getShareModelHealthCalendar(shareId, 365",
  "buildHeatmapCells",
  "dashboard.healthCalendar.inactiveDay",
  "dashboard.healthCalendar.currentProbe",
  "dashboard.healthCalendar.sharedProbe",
  "day.observedChecks",
  "day.monitoringGapChecks",
]);
requireMarkers("frontend/components/dashboard/client-board.tsx", [
  "<ShareModelHealthHeatmap shareId={selectedShare.shareId} />",
]);
requireMarkers("frontend/components/dashboard/share-market/buyer-catalog.tsx", [
  "<ShareModelHealthHeatmap shareId={selected.listing.shareId} />",
]);

for (const [relativePath, source] of connection) {
  for (const forbidden of [
    /\bAPP_PROBE\b/,
    /\bapp_probe_for_kind\b/,
    /\bkind\s*[:=]\s*["'](?:chat|image|tools)["']/i,
    /["'](?:gpt|claude|gemini)-[a-z0-9]/i,
  ]) {
    if (forbidden.test(source)) {
      errors.push(`${relativePath} contains forbidden hardcoded probe surface ${forbidden}`);
    }
  }
}

if (/\bAPP_PROBE\b/.test(models + store + scheduler + main)) {
  errors.push("Router Rust contract contains the retired APP_PROBE table");
}

requireMarkers("PROTOCOL.md", [
  "Share Contract v6",
  "MIN_SHARE_MARKET_CONTRACT_VERSION",
  "modelProbe",
  "每个完整监测日固定 48 槽",
  "Codex → `appType=openai`",
  "连通性与模型健康是两套信号",
  "model-health-calendar?days=N",
  "batch-v2",
  "probe epoch",
  "monitoring gap",
  "pending claim 超过 10 分钟",
  "共享 7 分钟总预算",
  "每批最多回退一次",
]);
requireMarkers("frontend/package.json", [
  '"test:share-model-probe"',
  '"test:share-model-health-heatmap"',
  '"audit:share-model-health-contract"',
]);
requireMarkers(".github/workflows/build-release.yml", [
  "npm run test:share-model-probe",
  "npm run test:share-model-health-heatmap",
  "npm run audit:share-model-health-contract",
]);

if (errors.length) {
  console.error(`Share model health contract audit failed:\n${errors.map((error) => `- ${error}`).join("\n")}`);
  process.exit(1);
}

console.log("Share model health contract audit ok: Contract v6, market floor v5, evidence v2, probe epochs, UTC slots, and heatmap aligned");
