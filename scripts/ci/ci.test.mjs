import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { verifyBuildInfo, verifyEmbeddedAssets } from "./verify-release.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const sha = "1234567890abcdef1234567890abcdef12345678";

function temporary(t) {
  const directory = mkdtempSync(join(tmpdir(), "router-ci-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

test("parallel CI retains every existing financial gate and separates profile caches", () => {
  const source = readFileSync(join(root, ".github/workflows/build-release.yml"), "utf8");
  const job = name => {
    const start = source.indexOf(`\n  ${name}:\n`);
    assert.notEqual(start, -1, `missing job ${name}`);
    const rest = source.slice(start + 1);
    const next = rest.slice(1).search(/^  [a-z][\w-]*:\s*$/m);
    return next === -1 ? rest : rest.slice(0, next + 1);
  };
  assert.doesNotMatch(job("frontend"), /^    needs:/m);
  assert.doesNotMatch(job("rust-tests"), /^    needs:/m);
  assert.match(job("build"), /needs: frontend/);
  assert.match(job("release"), /needs: \[frontend, rust-tests, build\]/);
  assert.match(job("release"), /if: github.ref == 'refs\/heads\/master'/);
  for (const name of ["frontend", "rust-tests", "build"]) {
    assert.match(job(name), /cancel-in-progress: true/);
    assert.doesNotMatch(job(name), /contents: write/);
  }
  assert.match(job("release"), /cancel-in-progress: false/);
  assert.match(job("rust-tests"), /shared-key: router-tests/);
  assert.match(job("rust-tests"), /mkdir -p frontend\/out\s+bash [^\n]*cargo test --locked --no-run/);
  assert.match(job("build"), /shared-key: router-release-x86_64-unknown-linux-gnu/);
  assert.match(job("frontend"), /path: frontend\/\.next\/cache/);
  assert.match(job("frontend"), /node --test [^\n]*scripts\/audit\/share-contract-versions\.test\.mjs/);
  assert.doesNotMatch(source, /path: frontend\/\.next\s*$/m);
  assert.match(job("frontend"), /name: frontend-\$\{\{ github.sha \}\}/);
  assert.match(job("build"), /name: frontend-\$\{\{ github.sha \}\}/);
  for (const command of ["test:flags", "test:token-units", "test:usd-micros", "test:market-wallet", "test:market-notifications", "test:market-notifications-browser", "test:share-model-probe", "test:share-model-health-heatmap", "audit:settings-i18n", "audit:market-billing-contract", "audit:web-token-units", "audit:share-model-health-contract", "audit:model-prices", "test:wallet-checkout-browser", "test:c2c-browser", "test:navbar-wallet-browser", "test:auth-password", "test:auth-password-browser"]) {
    assert.ok(job("frontend").includes(`npm run ${command}`), `lost frontend gate: ${command}`);
  }
  assert.match(job("rust-tests"), /cargo test --locked market_ -- --skip schema::tests/);
  assert.ok(job("frontend").includes("npm run test:client-checkout-browser"), "lost Client trial checkout browser gate");
  assert.ok(job("frontend").includes("npm run test:client-termination-browser"), "lost Client termination workbench browser gate");
  assert.ok(job("frontend").includes("npm run test:client-rental-audit-browser"), "lost Owner Client rental audit browser gate");
  assert.match(job("rust-tests"), /cargo test --locked share_market::tests/);
  assert.match(job("rust-tests"), /cargo test --locked client_market::tests/);
  assert.match(job("rust-tests"), /cargo test --locked client_termination_chat_/);
  assert.match(job("rust-tests"), /cargo test --locked user_notifications::tests/);
  assert.match(job("rust-tests"), /cargo test --locked alerting::store::tests/);
  for (const suite of ["test:share-market", "test:client-market", "test:providers"]) {
    assert.ok(job("frontend").includes(`npm run ${suite}`), `lost checkout gate: ${suite}`);
  }
  assert.match(job("rust-tests"), /cargo test --locked schema::tests/);
  assert.match(job("rust-tests"), /cargo test --locked test_support::tests/);
  assert.match(job("rust-tests"), /cargo test --locked db::tests/);
  for (const suite of ["password_auth::tests", "password_", "user_cli::tests", "session_", "auth_code_verification_"]) {
    assert.ok(job("rust-tests").includes(`cargo test --locked ${suite}`), `lost authentication gate: ${suite}`);
  }
  assert.match(job("build"), /verify-release\.mjs/);
  assert.doesNotMatch(source, /git push|release delete|continue-on-error/);
});

test("timing wrapper preserves failures, argument boundaries and summary output", t => {
  const directory = temporary(t);
  const summary = join(directory, "summary");
  const run = (...args) => spawnSync("bash", [join(root, "scripts/ci/timed.sh"), ...args], { encoding: "utf8", env: { ...process.env, GITHUB_STEP_SUMMARY: summary } });
  assert.equal(run("A check", "bash", "-c", "exit 23").status, 23);
  const success = run("Arguments", process.execPath, "-e", "process.stdout.write(process.argv[1])", "a b;$literal");
  assert.equal(success.status, 0, success.stderr);
  assert.match(success.stdout, /a b;\$literal/);
  assert.match(readFileSync(summary, "utf8"), /A check: \d+s \(exit 23\)/);
  assert.equal(run("missing command").status, 2);
});

test("artifact verification rejects stale build identity and missing or stale embedded assets", t => {
  const directory = temporary(t);
  const files = ["index.html", "account/wallet/index.html", "settings/index.html", "providers/index.html", "router-logo.svg", "_next/static/chunks/a.js", "_next/static/chunks/a.css"];
  for (const [index, name] of files.entries()) {
    const path = join(directory, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, `unique export asset ${index}`);
  }
  const binary = Buffer.concat([Buffer.from("7f454c46", "hex"), ...files.map(name => readFileSync(join(directory, name)))]);
  const info = { commit: sha.slice(0, 7), version: "0.1.0", buildTime: "2026-10-24T00:00:00Z" };
  assert.doesNotThrow(() => verifyBuildInfo(info, sha));
  assert.throws(() => verifyBuildInfo({ ...info, commit: "old-sha" }, sha), /commit/);
  assert.throws(() => verifyBuildInfo({ ...info, buildTime: "unknown" }, sha), /timestamp/);
  assert.equal(verifyEmbeddedAssets(binary, directory), files.length);
  assert.throws(() => verifyEmbeddedAssets(Buffer.from("old"), directory), /ELF/);
  writeFileSync(join(directory, "_next/static/chunks/a.js"), "a new chunk not in the old binary");
  assert.throws(() => verifyEmbeddedAssets(binary, directory), /does not embed/);
});

function publication(t, options = {}) {
  const directory = temporary(t);
  const bin = join(directory, "bin");
  mkdirSync(bin);
  mkdirSync(join(directory, "release-files"));
  writeFileSync(join(directory, "release-files/cc-switch-router-linux-amd64"), "fixture, not executable");
  const log = join(directory, "calls");
  writeFileSync(join(bin, "git"), `#!${process.execPath}\nconst fs=require('node:fs'); if(process.env.TEST_GIT_FAIL)process.exit(3); console.log(process.env.TEST_REMOTE_SHA+'\\trefs/heads/master');\n`, { mode: 0o755 });
  writeFileSync(join(bin, "gh"), `#!${process.execPath}\nconst fs=require('node:fs'); const a=process.argv.slice(2); fs.appendFileSync(process.env.TEST_CALLS,JSON.stringify(a)+'\\n'); if(process.env.TEST_UPLOAD_FAIL&&a[0]==='release'&&a[1]==='upload')process.exit(9); if(process.env.TEST_NEW&&((a[0]==='api'&&a[1].endsWith('/git/ref/tags/latest'))||(a[0]==='release'&&a[1]==='view')))process.exit(1);\n`, { mode: 0o755 });
  const result = spawnSync("bash", [join(root, "scripts/ci/publish-release.sh")], {
    cwd: directory, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GITHUB_REPOSITORY: "owner/router", GITHUB_SHA: sha, ROUTER_RELEASE_VERSION: "0.1.0", TEST_REMOTE_SHA: options.stale ? "a".repeat(40) : sha, TEST_CALLS: log, TEST_NEW: options.new ? "1" : "", TEST_GIT_FAIL: options.gitFail ? "1" : "", TEST_UPLOAD_FAIL: options.uploadFail ? "1" : "" },
  });
  let calls = [];
  try { calls = readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch (error) { if (error.code !== "ENOENT") throw error; }
  return { result, calls };
}

test("superseded or unverified commits never mutate tags/releases", t => {
  const stale = publication(t, { stale: true });
  assert.equal(stale.result.status, 0, stale.result.stderr);
  assert.deepEqual(stale.calls, []);
  const unavailable = publication(t, { gitFail: true });
  assert.notEqual(unavailable.result.status, 0);
  assert.deepEqual(unavailable.calls, []);
});

test("publication updates latest in place and fails if the asset upload fails", t => {
  const existing = publication(t);
  assert.equal(existing.result.status, 0, existing.result.stderr);
  assert.ok(existing.calls.some(args => args.includes("PATCH")));
  assert.ok(existing.calls.some(args => args[0] === "release" && args[1] === "edit"));
  assert.ok(existing.calls.some(args => args[0] === "release" && args[1] === "upload" && args.includes("--clobber")));
  assert.ok(existing.calls.every(args => !args.includes("delete")));
  const uploaded = existing.calls.findIndex(args => args[0] === "release" && args[1] === "upload");
  assert.ok(existing.calls.findIndex(args => args.includes("PATCH")) > uploaded);
  assert.ok(existing.calls.findIndex(args => args[0] === "release" && args[1] === "edit") > uploaded);
  const initial = publication(t, { new: true });
  assert.equal(initial.result.status, 0, initial.result.stderr);
  assert.ok(initial.calls.some(args => args.includes("POST")));
  assert.ok(initial.calls.some(args => args[0] === "release" && args[1] === "create" && args.includes("--draft")));
  for (const fresh of [false, true]) {
    const failed = publication(t, { new: fresh, uploadFail: true });
    assert.equal(failed.result.status, 9);
    assert.ok(failed.calls.every(args => !args.includes("PATCH") && !(args[0] === "release" && args[1] === "edit")));
  }
});
