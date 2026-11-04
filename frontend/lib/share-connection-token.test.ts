import assert from "node:assert/strict";
import test from "node:test";
import { getUserApiToken } from "./api";
import { AuthError, type AuthState } from "./auth";
import { shareConnectionActor } from "./use-share-connection-token";

const initial: AuthState = { email: "user@example.test", loginEpoch: "fixture-login", accessToken: "fixture-access", refreshToken: "fixture-refresh", expiresAt: "2099-01-01T00:00:00Z" };
const result = { apiToken: "ccrt_fixture-only-key", token: { prefix: "ccrt_fixture", scopes: ["share:invoke"], createdAt: "2026-01-01T00:00:00Z" } };

async function withEnvironment(run: (setAuth: (state: AuthState) => void) => Promise<void>) {
  const originals = new Map(["window", "localStorage", "fetch"].map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  let auth = initial;
  Object.defineProperty(globalThis, "window", { configurable: true, value: new EventTarget() });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => JSON.stringify(auth) } });
  try { await run(next => { auth = next; }); }
  finally {
    for (const [name, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
  }
}

test("connection actor separates accounts, logout, and fresh logins without embedding credentials", () => {
  const actor = shareConnectionActor(initial);
  assert.notEqual(actor, shareConnectionActor({ ...initial, email: "another@example.test" }));
  assert.notEqual(actor, shareConnectionActor({ ...initial, loginEpoch: "fresh-login" }));
  assert.equal(shareConnectionActor({ ...initial, accessToken: null }), "");
  assert.equal(shareConnectionActor({}), "");
  assert.equal(actor, shareConnectionActor({ ...initial, accessToken: "rotated", refreshToken: "rotated-refresh" }));
  assert.ok(!actor.includes(initial.accessToken!));
  assert.ok(!actor.includes(initial.refreshToken!));
});

test("legacy sessions without a login epoch fail closed on credential changes without embedding tokens", () => {
  const legacy = { ...initial, loginEpoch: undefined };
  const actor = shareConnectionActor(legacy);
  assert.equal(actor, shareConnectionActor(legacy));
  assert.ok(!actor.includes(legacy.accessToken!));
  assert.notEqual(actor, shareConnectionActor({ ...legacy, accessToken: "different-legacy-login" }));
  assert.notEqual(actor, shareConnectionActor({ ...legacy, refreshToken: "different-refresh" }));
  shareConnectionActor({});
  assert.notEqual(actor, shareConnectionActor(legacy));
});

test("Key read is cancellable, read-only, no-store and bound to the current login", async () => {
  await withEnvironment(async () => {
    globalThis.fetch = async (url, init) => {
      assert.equal(url, "/v1/me/api-token");
      assert.equal(init?.method ?? "GET", "GET");
      assert.equal(init?.cache, "no-store");
      assert.ok(init?.signal);
      assert.equal(new Headers(init?.headers).get("Authorization"), "Bearer fixture-access");
      return Response.json(result);
    };
    assert.deepEqual(await getUserApiToken(), result);
  });
});

for (const [name, replacement] of [
  ["account switch", { ...initial, email: "another@example.test", loginEpoch: "another-login", accessToken: "another-access" }],
  ["fresh login", { ...initial, loginEpoch: "new-login" }],
  ["logout", {}],
] as const) {
  test(`late Key response is discarded after ${name}`, async () => {
    await withEnvironment(async setAuth => {
      globalThis.fetch = async () => { setAuth(replacement); return Response.json(result); };
      await assert.rejects(getUserApiToken(), { name: "AbortError" });
    });
  });
  test(`401 cannot retry the Key read under another identity after ${name}`, async () => {
    await withEnvironment(async setAuth => {
      let calls = 0;
      globalThis.fetch = async () => { calls++; setAuth(replacement); return Response.json({}, { status: 401 }); };
      await assert.rejects(getUserApiToken(), (error: unknown) => error instanceof AuthError && error.code === "password_session_changed");
      assert.equal(calls, 1);
    });
  });
}

test("Key read allows access-token rotation within the same login", async () => {
  await withEnvironment(async setAuth => {
    const headers: (string | null)[] = [];
    globalThis.fetch = async (_url, init) => {
      headers.push(new Headers(init?.headers).get("Authorization"));
      if (headers.length === 1) {
        setAuth({ ...initial, accessToken: "rotated-access" });
        return Response.json({}, { status: 401 });
      }
      return Response.json(result);
    };
    assert.deepEqual(await getUserApiToken(), result);
    assert.deepEqual(headers, ["Bearer fixture-access", "Bearer rotated-access"]);
  });
});

test("malformed successful replies are rejected without exposing response data", async () => {
  await withEnvironment(async () => {
    for (const malformed of [null, {}, { apiToken: result.apiToken }, { ...result, apiToken: {} },
      { ...result, token: { ...result.token, scopes: [null] } }]) {
      globalThis.fetch = async () => Response.json(malformed);
      await assert.rejects(getUserApiToken(), { message: "Invalid API Key response" });
    }
    globalThis.fetch = async () => Response.json({ token: result.token });
    assert.deepEqual(await getUserApiToken(), { token: result.token }, "legacy prefix-only replies stay displayable, not executable");
  });
});

test("closing aborts an in-flight Key read and even a late successful body is rejected", async () => {
  await withEnvironment(async () => {
    const controller = new AbortController();
    globalThis.fetch = async () => { controller.abort(); return Response.json(result); };
    await assert.rejects(getUserApiToken(controller.signal), { name: "AbortError" });
    let calls = 0;
    globalThis.fetch = async () => { calls++; return Response.json(result); };
    await assert.rejects(getUserApiToken(controller.signal), { name: "AbortError" });
    assert.equal(calls, 0);
  });
});
