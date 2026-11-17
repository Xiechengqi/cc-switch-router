import assert from "node:assert/strict";
import { generateKeyPairSync, createHash, verify } from "node:crypto";
import test from "node:test";
import { isFinancialWrite, signedFinancialFetch } from "./financial-auth";

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
