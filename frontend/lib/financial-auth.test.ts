import assert from "node:assert/strict";
import { generateKeyPairSync, createHash, verify } from "node:crypto";
import test from "node:test";
import { isFinancialWrite, signedFinancialFetch } from "./financial-auth";
import { registerFinancialConfirmHost } from "./financial-confirm";

const pair = generateKeyPairSync("ed25519");
const device = { authDeviceId: "browser-test", privateKey: pair.privateKey.export({ type: "pkcs8", format: "der" }).toString("base64") };
const hash = (value: string) => createHash("sha256").update(value).digest("hex");

test("financial writes include webhook-secret rotation and preserve read-only/public paths", () => {
  assert.equal(isFinancialWrite("https://router.invalid/v1/admin/market-payment-channels/c/webhook-secret", { method: "POST" }), true);
  assert.equal(isFinancialWrite("https://router.invalid/v1/market-wallet/transfers", { method: "POST" }), true);
  assert.equal(isFinancialWrite("https://router.invalid/v1/market-wallet/transfers"), false);
  assert.equal(isFinancialWrite("https://router.invalid/v1/payment-webhooks/dujiaopay", { method: "POST" }), false);
});

test("financial fetch signs the original Request bytes, route, session and idempotency key", async () => {
  const original = globalThis.fetch;
  const seen: Request[] = [];
  globalThis.fetch = async (input, init) => { seen.push(new Request(input, init)); return new Response("{}", { status: 200 }); };
  try {
    const body = '{"quoteId":"q","unicode":"额度"}\n';
    const request = new Request("https://router.invalid/v1/market-wallet/transfers?check=1", { method: "POST", headers: { "Content-Type": "application/json", "Idempotency-Key": "original-key", "x-router-verified-financial-proof": "forged" }, body });
    await signedFinancialFetch(request, {}, "session-token", device, () => {});
    await signedFinancialFetch(request, {}, "session-token", device, () => {});
    for (const sent of seen) {
      const h = sent.headers;
      assert.equal(await sent.text(), body);
      assert.equal(h.get("authorization"), "Bearer session-token");
      assert.equal(h.has("x-router-verified-financial-proof"), false);
      assert.equal(h.get("idempotency-key"), "original-key");
      const canonical = `market-finance-v1\nhttps://router.invalid\nbrowser-test\n${hash("session-token")}\nPOST\n/v1/market-wallet/transfers?check=1\n${hash(body)}\noriginal-key\n${h.get("x-router-request-time")}\n${h.get("x-router-request-nonce")}`;
      assert.equal(verify(null, Buffer.from(canonical), pair.publicKey, Buffer.from(h.get("x-router-request-signature")!, "base64")), true);
      assert.equal(verify(null, Buffer.from(canonical.replace("original-key", "changed-key")), pair.publicKey, Buffer.from(h.get("x-router-request-signature")!, "base64")), false);
    }
    assert.notEqual(seen[0].headers.get("x-router-request-nonce"), seen[1].headers.get("x-router-request-nonce"));
  } finally { globalThis.fetch = original; }
});

test("an account change during signing prevents the financial request", async () => {
  const original = globalThis.fetch;
  let calls = 0; let guards = 0;
  globalThis.fetch = async () => { calls++; return new Response(); };
  try {
    await assert.rejects(signedFinancialFetch("https://router.invalid/v1/market-wallet/transfers", { method: "POST", body: "{}" }, "token", device, () => { if (++guards === 2) throw new Error("account changed"); }), /account changed/);
    assert.equal(calls, 0);
  } finally { globalThis.fetch = original; }
});

test("a 428 asks for the login password, confirms the challenge and resends with the confirmation", async () => {
  const original = globalThis.fetch;
  const seen: { url: string; stepup: string | null; body: string }[] = [];
  let confirmAttempts = 0;
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init);
    const body = await request.text();
    seen.push({ url: new URL(request.url).pathname, stepup: request.headers.get("x-router-stepup"), body });
    if (request.url.endsWith("/v1/market-wallet/security/confirm")) {
      confirmAttempts++;
      return confirmAttempts === 1
        ? new Response(JSON.stringify({ code: "FINANCIAL_PASSWORD_INVALID" }), { status: 403 })
        : new Response(JSON.stringify({ stepupId: "challenge-1" }), { status: 200 });
    }
    return request.headers.get("x-router-stepup")
      ? new Response("{}", { status: 200 })
      : new Response(JSON.stringify({ code: "FINANCIAL_STEPUP_REQUIRED", details: { challengeId: "challenge-1", method: "password", risk: "immediate" } }), { status: 428 });
  };
  const asked: { risk: string; error?: string }[] = [];
  const unregister = registerFinancialConfirmHost({
    askPassword: async (request) => { asked.push(request); return asked.length === 1 ? "wrong" : "right"; },
    requirePassword: () => {},
  });
  try {
    const response = await signedFinancialFetch("https://router.invalid/v1/market-wallet/transfers", { method: "POST", body: "{\"quoteId\":\"q\"}" }, "token", device, () => {});
    assert.equal(response.status, 200);
    assert.deepEqual(asked, [{ risk: "immediate", error: undefined }, { risk: "immediate", error: "FINANCIAL_PASSWORD_INVALID" }]);
    assert.deepEqual(seen.map((item) => [item.url, item.stepup]), [
      ["/v1/market-wallet/transfers", null],
      ["/v1/market-wallet/security/confirm", null],
      ["/v1/market-wallet/security/confirm", null],
      ["/v1/market-wallet/transfers", "challenge-1"],
    ]);
    assert.match(seen[1].body, /"password":"wrong"/);
    assert.equal(seen[3].body, "{\"quoteId\":\"q\"}", "the original bytes are resent unchanged");
  } finally { unregister(); globalThis.fetch = original; }
});

test("cancelling the password prompt fails the operation without resending", async () => {
  const original = globalThis.fetch;
  let calls = 0;
  globalThis.fetch = async () => { calls++; return new Response(JSON.stringify({ code: "FINANCIAL_STEPUP_REQUIRED", details: { challengeId: "c", method: "password", risk: "confirm" } }), { status: 428 }); };
  const unregister = registerFinancialConfirmHost({ askPassword: async () => null, requirePassword: () => {} });
  try {
    await assert.rejects(signedFinancialFetch("https://router.invalid/v1/market-wallet/transfers", { method: "POST", body: "{}" }, "token", device, () => {}), /cancelled/i);
    assert.equal(calls, 1);
  } finally { unregister(); globalThis.fetch = original; }
});

test("an account without a password is sent to set one", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async () => new Response(JSON.stringify({ code: "FINANCIAL_PASSWORD_REQUIRED" }), { status: 403 });
  let opened = 0;
  const unregister = registerFinancialConfirmHost({ askPassword: async () => null, requirePassword: () => { opened++; } });
  try {
    const response = await signedFinancialFetch("https://router.invalid/v1/market-wallet/transfers", { method: "POST", body: "{}" }, "token", device, () => {});
    assert.equal(response.status, 403);
    assert.equal(opened, 1);
  } finally { unregister(); globalThis.fetch = original; }
});
