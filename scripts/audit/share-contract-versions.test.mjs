import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { auditShareContractVersions } from "./share-contract-versions.mjs";

const models = `
pub const MIN_SHARE_CONTRACT_VERSION: u16 = 2;
pub const MIN_SHARE_MARKET_CONTRACT_VERSION: u16 = 5;
pub const SHARE_CONTRACT_VERSION: u16 = 8;
`;
const protocol = "Reader: Share Contract v2..v8; ordinary: Share Contract v7; market: `MIN_SHARE_MARKET_CONTRACT_VERSION=5`.";

test("reader v8, ordinary v7 and market v5 are separate compatible contracts", () => {
  const result = auditShareContractVersions(models, protocol);
  assert.deepEqual(result.errors, []);
  assert.deepEqual(result.versions, { minimumVersion: 2, marketMinimumVersion: 5, currentVersion: 8 });
});

test("future reader upgrades do not require hardcoding a new audit ceiling", () => {
  const upgraded = models.replace("SHARE_CONTRACT_VERSION: u16 = 8", "SHARE_CONTRACT_VERSION: u16 = 9");
  assert.deepEqual(auditShareContractVersions(upgraded, protocol.replace("v2..v8", "v2..v9")).errors, []);
  assert.match(auditShareContractVersions(upgraded, protocol).errors.join("\n"), /reader range.*v2\.\.v9/);
});

test("reader and market floors cannot silently drop rolling-deployment compatibility", () => {
  const reader = models.replace("MIN_SHARE_CONTRACT_VERSION: u16 = 2", "MIN_SHARE_CONTRACT_VERSION: u16 = 7");
  assert.match(auditShareContractVersions(reader, protocol).errors.join("\n"), /reader floor at v2/);
  const market = models.replace("MIN_SHARE_MARKET_CONTRACT_VERSION: u16 = 5", "MIN_SHARE_MARKET_CONTRACT_VERSION: u16 = 8");
  assert.match(auditShareContractVersions(market, protocol).errors.join("\n"), /MARKET_CONTRACT_VERSION at v5/);
  const oldReader = models.replace("SHARE_CONTRACT_VERSION: u16 = 8", "SHARE_CONTRACT_VERSION: u16 = 7");
  assert.match(auditShareContractVersions(oldReader, protocol).errors.join("\n"), /support accountReset Contract v8/);
});

test("stale or missing protocol versions fail the audit", () => {
  for (const stale of ["v2..v7", "v2..v80", "v3..v8", "no reader range"]) {
    assert.match(auditShareContractVersions(models, protocol.replace("v2..v8", stale)).errors.join("\n"), /reader range/);
  }
  assert.match(auditShareContractVersions(models, protocol.replace("Share Contract v7", "Share Contract v8")).errors.join("\n"), /ordinary Share Contract v7 compatibility/);
  for (const stale of ["MIN_SHARE_MARKET_CONTRACT_VERSION=8", "no market floor"]) {
    assert.match(auditShareContractVersions(models, protocol.replace("MIN_SHARE_MARKET_CONTRACT_VERSION=5", stale)).errors.join("\n"), /document MIN_SHARE_MARKET_CONTRACT_VERSION=5/);
  }
});

test("missing or invalid Rust constants cannot match a suffix or an old comment", () => {
  for (const invalid of [
    models.replace("pub const SHARE_CONTRACT_VERSION: u16 = 8;", ""),
    models.replace("pub const SHARE_CONTRACT_VERSION: u16 = 8;", "// pub const SHARE_CONTRACT_VERSION: u16 = 8;"),
    models.replace("SHARE_CONTRACT_VERSION: u16 = 8", "SHARE_CONTRACT_VERSION: u16 = 65536"),
    models.replace("SHARE_CONTRACT_VERSION: u16 = 8", "SHARE_CONTRACT_VERSION: u16 = 0"),
  ]) {
    assert.match(auditShareContractVersions(invalid, protocol).errors.join("\n"), /declare SHARE_CONTRACT_VERSION/);
  }
});

test("checked-in reader versions and protocol documentation stay aligned", () => {
  const source = readFileSync(new URL("../../src/models.rs", import.meta.url), "utf8");
  const document = readFileSync(new URL("../../PROTOCOL.md", import.meta.url), "utf8");
  assert.deepEqual(auditShareContractVersions(source, document).errors, []);
});
