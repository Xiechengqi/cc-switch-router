#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const iconDirectory = "frontend/public/provider-icons";
const iconSourceId = "lobehub-icons-static-svg";
const expectedPackage = "@lobehub/icons-static-svg@1.91.0";
const expectedPackageIntegrity =
  "sha512-ZDflEq0uUvAkH4WK4h3qNvvY09ts4OqUb5azD7A0xKfcuYhffGwB1Q/As2RguZYq4Gh4v925CJ8iodiClzc4zw==";

function sha256(content) {
  return crypto.createHash("sha256").update(content).digest("hex");
}

function sorted(values) {
  return [...values].sort((left, right) => left.localeCompare(right));
}

export function svgAssetViolations(pathName, content, expectedSha256) {
  const violations = [];
  if (!/^[a-f0-9]{64}$/.test(expectedSha256 ?? "")) {
    violations.push(`${pathName}: invalid provenance sha256`);
  } else if (sha256(content) !== expectedSha256) {
    violations.push(`${pathName}: content hash differs from SOURCE_PROVENANCE.json`);
  }
  const source = content.toString("utf8");
  if (!source.startsWith("<svg") || !source.endsWith("</svg>")) {
    violations.push(`${pathName}: asset must be a complete SVG document`);
  }
  if (/<script\b|<image\b|(?:href|src)=["'](?:https?:|\/\/|data:)/i.test(source)) {
    violations.push(`${pathName}: SVG contains executable or external content`);
  }
  return violations;
}

export function auditProviderIconProvenance(root = repoRoot) {
  const violations = [];
  const provenancePath = path.join(root, "SOURCE_PROVENANCE.json");
  if (!fs.existsSync(provenancePath)) {
    return ["SOURCE_PROVENANCE.json: required compliance file missing"];
  }
  const provenance = JSON.parse(fs.readFileSync(provenancePath, "utf8"));
  const iconSource = (provenance.vendoredSources ?? []).find(
    (entry) => entry.id === iconSourceId,
  );
  if (!iconSource) {
    return [`SOURCE_PROVENANCE.json: ${iconSourceId} source missing`];
  }
  if (iconSource.package !== expectedPackage) {
    violations.push(
      `SOURCE_PROVENANCE.json: icon package must be ${expectedPackage}`,
    );
  }
  if (iconSource.packageIntegrity !== expectedPackageIntegrity) {
    violations.push("SOURCE_PROVENANCE.json: icon package integrity mismatch");
  }
  if (iconSource.license !== "MIT") {
    violations.push("SOURCE_PROVENANCE.json: icon source license must be MIT");
  }

  const absoluteIconDirectory = path.join(root, iconDirectory);
  const actualFiles = sorted(
    fs
      .readdirSync(absoluteIconDirectory, { withFileTypes: true })
      .filter((entry) => entry.isFile())
      .map((entry) => `${iconDirectory}/${entry.name}`),
  );
  const declaredFiles = new Map();
  for (const file of iconSource.files ?? []) {
    if (declaredFiles.has(file.path)) {
      violations.push(`SOURCE_PROVENANCE.json: duplicate icon path ${file.path}`);
      continue;
    }
    declaredFiles.set(file.path, file);
    const baseName = path.posix.basename(file.path ?? "");
    if (
      file.path !== `${iconDirectory}/${baseName}` ||
      !baseName.endsWith(".svg")
    ) {
      violations.push(`${file.path ?? "unknown icon"}: invalid repository icon path`);
    }
    if (!/^icons\/[a-z0-9-]+[.]svg$/.test(file.packagePath ?? "")) {
      violations.push(`${file.path ?? "unknown icon"}: invalid package source path`);
    }
  }

  const declaredPaths = sorted(declaredFiles.keys());
  for (const undeclared of actualFiles.filter(
    (pathName) => !declaredFiles.has(pathName),
  )) {
    violations.push(`${undeclared}: icon is not declared in SOURCE_PROVENANCE.json`);
  }
  for (const missing of declaredPaths.filter(
    (pathName) => !actualFiles.includes(pathName),
  )) {
    violations.push(`${missing}: declared icon is missing`);
  }
  for (const pathName of actualFiles) {
    if (!pathName.endsWith(".svg")) {
      violations.push(`${pathName}: raster provider icons are not allowed`);
      continue;
    }
    const declaration = declaredFiles.get(pathName);
    if (!declaration) continue;
    violations.push(
      ...svgAssetViolations(
        pathName,
        fs.readFileSync(path.join(root, pathName)),
        declaration.sha256,
      ),
    );
  }

  const logoComponentPath = path.join(
    root,
    "frontend/components/dashboard/share-provider-logo.tsx",
  );
  const logoComponent = fs.readFileSync(logoComponentPath, "utf8");
  const referencedIcons = new Set(
    [...logoComponent.matchAll(/\/provider-icons\/([a-z0-9.-]+)/g)].map(
      (match) => `${iconDirectory}/${match[1]}`,
    ),
  );
  for (const referencedIcon of sorted(referencedIcons)) {
    if (!declaredFiles.has(referencedIcon)) {
      violations.push(
        `frontend/components/dashboard/share-provider-logo.tsx: undeclared icon ${referencedIcon}`,
      );
    }
  }
  for (const requiredSvg of ["cursor.svg", "kiro.svg"]) {
    if (!referencedIcons.has(`${iconDirectory}/${requiredSvg}`)) {
      violations.push(
        `frontend/components/dashboard/share-provider-logo.tsx: ${requiredSvg} reference missing`,
      );
    }
  }
  return violations;
}

function main() {
  const violations = auditProviderIconProvenance();
  if (violations.length > 0) {
    throw new Error(`Provider icon provenance violations:\n${violations.join("\n")}`);
  }
  console.log("provider icon provenance ok: 12 vendored SVG assets verified");
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
