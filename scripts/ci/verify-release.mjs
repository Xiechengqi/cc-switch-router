import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export function verifyBuildInfo(info, sha) {
  assert.match(sha, /^[0-9a-f]{40}$/);
  assert.ok(info.commit === sha || info.commit === sha.slice(0, 7), "binary commit does not match this workflow");
  assert.match(info.version, /^\d+\.\d+\.\d+/);
  assert.ok(Number.isFinite(Date.parse(info.buildTime)), "binary has no valid build timestamp");
}

export function verifyEmbeddedAssets(binary, exportRoot) {
  assert.equal(binary.subarray(0, 4).toString("hex"), "7f454c46", "release is not an ELF executable");
  const files = ["index.html", "account/wallet/index.html", "settings/index.html", "providers/index.html", "router-logo.svg"];
  const walk = (directory, prefix = "") => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const name = join(prefix, entry.name);
      if (entry.isDirectory()) walk(join(directory, entry.name), name);
      else if (/\.(js|css)$/.test(entry.name)) files.push(join("_next/static", name));
    }
  };
  walk(join(exportRoot, "_next/static"));
  assert.ok(files.some(file => file.endsWith(".js")), "export has no JavaScript");
  assert.ok(files.some(file => file.endsWith(".css")), "export has no stylesheet");
  for (const file of files) {
    const bytes = readFileSync(join(exportRoot, file));
    assert.ok(bytes.length > 0, `empty exported asset: ${file}`);
    assert.ok(binary.includes(bytes), `binary does not embed this run's asset: ${file}`);
  }
  return files.length;
}

export function verifyRelease(binaryPath, exportRoot, sha) {
  const info = JSON.parse(execFileSync(resolve(binaryPath), ["build-info"], { encoding: "utf8", timeout: 10_000 }));
  verifyBuildInfo(info, sha);
  const count = verifyEmbeddedAssets(readFileSync(binaryPath), exportRoot);
  console.log(`Verified ${info.commit}: build identity and ${count} embedded HTML/JS/CSS assets match this run.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [, , binaryPath, exportRoot, sha] = process.argv;
  assert.ok(binaryPath && exportRoot && sha, "usage: verify-release.mjs BINARY EXPORT_DIR COMMIT_SHA");
  verifyRelease(binaryPath, exportRoot, sha);
}
