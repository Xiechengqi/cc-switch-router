import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, verify } from "node:crypto";
import { createServer } from "node:http";
import { build } from "esbuild";
import { chromium } from "playwright";
import { fileURLToPath } from "node:url";

// Exercise real WebCrypto request signatures and the password confirmation
// round trip. No production API, payment or persistent credential is used.
const lib = (name) => fileURLToPath(new URL(`../lib/${name}`, import.meta.url));
const bundle = await build({
  stdin: {
    contents: `export * from ${JSON.stringify(lib("financial-auth.ts"))}; export { registerFinancialConfirmHost } from ${JSON.stringify(lib("financial-confirm.ts"))};`,
    resolveDir: fileURLToPath(new URL("..", import.meta.url)),
    loader: "ts",
  },
  bundle: true, platform: "browser", format: "iife", globalName: "financialAuth", write: false,
  alias: { "@": fileURLToPath(new URL("..", import.meta.url)) },
});
const pair = generateKeyPairSync("ed25519");
const device = { authDeviceId: "financial-browser-device", privateKey: pair.privateKey.export({ type: "pkcs8", format: "der" }).toString("base64") };
const token = "local-financial-browser-session";
const hash = value => createHash("sha256").update(value).digest();
const originalBody = '{"quoteId":"frozen-quote","note":"额度"}\n';
const nonces = new Set();
const attempts = [];
const errors = [];
let base, confirmed = false;
const passwords = [];
const json = (res, value, status = 200) => {
  res.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" });
  res.end(JSON.stringify(value));
};
const server = createServer(async (req, res) => {
  try {
    if (req.url === "/") {
      res.writeHead(200, { "Content-Type": "text/html" });
      return res.end('<!doctype html><title>Local financial authorization test</title><script src="/financial.js"></script>');
    }
    if (req.url === "/financial.js") {
      res.writeHead(200, { "Content-Type": "text/javascript" });
      return res.end(bundle.outputFiles[0].text);
    }
    if (req.method !== "POST") return res.writeHead(404).end();
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const body = Buffer.concat(chunks);
    const h = req.headers;
    assert.equal(h.authorization, `Bearer ${token}`);
    assert.equal(h["x-router-device"], device.authDeviceId);
    assert.equal(h["x-router-verified-financial-proof"], undefined);
    assert.ok(Math.abs(Date.now() - Number(h["x-router-request-time"])) < 300_000);
    assert.ok(!nonces.has(h["x-router-request-nonce"]), "each HTTP execution has a fresh nonce");
    nonces.add(h["x-router-request-nonce"]);
    const canonical = `market-finance-v1\n${base}\n${device.authDeviceId}\n${hash(token).toString("hex")}\nPOST\n${req.url}\n${hash(body).toString("hex")}\n${h["idempotency-key"] || ""}\n${h["x-router-request-time"]}\n${h["x-router-request-nonce"]}`;
    assert.ok(verify(null, Buffer.from(canonical), pair.publicKey, Buffer.from(h["x-router-request-signature"], "base64")));
    if (req.url === "/v1/market-wallet/security/confirm") {
      const { challengeId, password } = JSON.parse(body);
      assert.equal(challengeId, "local-operation-challenge");
      passwords.push(password);
      if (password !== "correct horse") return json(res, { code: "FINANCIAL_PASSWORD_INVALID" }, 403);
      confirmed = true;
      return json(res, { stepupId: "local-operation-challenge" });
    }
    assert.equal(req.url, "/v1/market-wallet/transfers?check=1");
    assert.equal(body.toString(), originalBody);
    assert.equal(h["idempotency-key"], "original-business-key");
    attempts.push(h["x-router-stepup"] || null);
    if (!h["x-router-stepup"]) return json(res, {
      code: "FINANCIAL_STEPUP_REQUIRED",
      details: { challengeId: "local-operation-challenge", method: "password", risk: "confirm" },
    }, 428);
    assert.equal(h["x-router-stepup"], "local-operation-challenge");
    assert.equal(confirmed, true);
    json(res, { id: "one-local-transfer" });
  } catch (error) {
    errors.push(error);
    json(res, { message: "local authorization assertion failed" }, 500);
  }
});
await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
base = `http://localhost:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });
  const page = await browser.newPage();
  page.on("pageerror", error => errors.push(error));
  await page.goto(base);
  // A stand-in for the dialog: first a mistyped password, then the right one.
  await page.evaluate(() => {
    const answers = ["typo", "correct horse"];
    financialAuth.registerFinancialConfirmHost({ askPassword: async () => answers.shift() ?? null, requirePassword: () => {} });
  });
  const result = await page.evaluate(async ({ device, token, body }) => {
    const response = await financialAuth.signedFinancialFetch("/v1/market-wallet/transfers?check=1", {
      method: "POST", headers: { "Content-Type": "application/json", "Idempotency-Key": "original-business-key", "x-router-verified-financial-proof": "forged" }, body,
    }, token, device, () => {});
    return { status: response.status, body: await response.json() };
  }, { device, token, body: originalBody });
  assert.deepEqual(errors, []);
  assert.deepEqual(result, { status: 200, body: { id: "one-local-transfer" } });
  assert.deepEqual(attempts, [null, "local-operation-challenge"]);
  assert.deepEqual(passwords, ["typo", "correct horse"]);
  assert.equal(nonces.size, 4);
  console.log("financial browser: real Ed25519 request signatures, password confirmation with one retry, exact request resend and fresh nonces passed");
} finally {
  if (browser) await browser.close();
  await new Promise(resolve => server.close(resolve));
}
