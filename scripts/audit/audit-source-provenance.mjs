#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const ignoredDirectories = new Set([
  ".git",
  ".next",
  "node_modules",
  "release-files",
  "target",
]);
const historicalOwner = ["farion", "1231"].join("");
const historicalRepository = [historicalOwner, "cc-switch"].join("/");
const forbiddenTechnicalMarkers = Object.freeze([
  ["external CC Switch checkout root", ["CC_SWITCH", "PROVIDER_AUDIT_ROOT"].join("_")],
  ["external CC Switch baseline", ["upstream-provider", "source-baseline.json"].join("-")],
  ["external CC Switch audit", ["audit-upstream", "provider-baseline"].join("-")],
  ["retired CC Switch import ledger", ["UPSTREAM", "IMPORT.md"].join("_")],
]);

function walkFiles(root) {
  const files = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      if (entry.isDirectory() && ignoredDirectories.has(entry.name)) continue;
      const absolutePath = path.join(current, entry.name);
      if (entry.isDirectory()) stack.push(absolutePath);
      else if (entry.isFile()) files.push(absolutePath);
    }
  }
  return files.sort();
}

function relativePath(root, absolutePath) {
  return path.relative(root, absolutePath).replaceAll(path.sep, "/");
}

function readText(absolutePath) {
  const content = fs.readFileSync(absolutePath);
  return content.includes(0) ? null : content.toString("utf8");
}

export function repositoryBoundaryViolations(pathName, source) {
  const violations = [];
  const normalized = source.toLowerCase();
  if (
    normalized.includes(historicalRepository) ||
    normalized.includes(["github.com", historicalOwner].join("/"))
  ) {
    violations.push(`${pathName}: historical CC Switch repository reference`);
  }
  for (const [label, marker] of forbiddenTechnicalMarkers) {
    if (source.includes(marker)) violations.push(`${pathName}: ${label}`);
  }
  return violations;
}

export function auditSourceProvenance(root = repoRoot) {
  const violations = [];
  for (const absolutePath of walkFiles(root)) {
    const source = readText(absolutePath);
    if (source === null) continue;
    violations.push(
      ...repositoryBoundaryViolations(relativePath(root, absolutePath), source),
    );
  }

  const requiredFiles = [
    "LICENSE",
    "SOURCE_PROVENANCE.json",
    "THIRD_PARTY_NOTICES.md",
  ];
  for (const requiredFile of requiredFiles) {
    if (!fs.existsSync(path.join(root, requiredFile))) {
      violations.push(`${requiredFile}: required compliance file missing`);
    }
  }

  const provenancePath = path.join(root, "SOURCE_PROVENANCE.json");
  if (!fs.existsSync(provenancePath)) return violations;
  const provenance = JSON.parse(fs.readFileSync(provenancePath, "utf8"));
  if (provenance.project !== "cc-switch-router") {
    violations.push("SOURCE_PROVENANCE.json: project must be cc-switch-router");
  }
  if (provenance.projectLicense !== "MIT") {
    violations.push("SOURCE_PROVENANCE.json: projectLicense must be MIT");
  }
  if (
    provenance.policy?.runtimeAndBuildInputsMustBeRepositoryOwned !== true ||
    provenance.policy?.historicalAttributionIsNotATechnicalDependency !== true
  ) {
    violations.push("SOURCE_PROVENANCE.json: repository-owned input policy missing");
  }
  if (!Array.isArray(provenance.vendoredSources)) {
    violations.push("SOURCE_PROVENANCE.json: vendoredSources must be an array");
    return violations;
  }
  for (const entry of provenance.vendoredSources) {
    if (entry.technicalInput !== false) {
      violations.push(
        `SOURCE_PROVENANCE.json: ${entry.id ?? "unknown source"} must declare technicalInput=false`,
      );
    }
    if (entry.notice !== "THIRD_PARTY_NOTICES.md") {
      violations.push(
        `SOURCE_PROVENANCE.json: ${entry.id ?? "unknown source"} notice must be repository-local`,
      );
    }
  }
  return violations;
}

function main() {
  const violations = auditSourceProvenance();
  if (violations.length > 0) {
    throw new Error(`Source provenance violations:\n${violations.join("\n")}`);
  }
  console.log("source provenance ok: no external CC Switch technical dependency");
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
