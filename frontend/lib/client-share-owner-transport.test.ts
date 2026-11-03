import assert from "node:assert/strict";
import test from "node:test";
import { getClientOwnerSeatContext } from "./client-share-market";
import { AuthError, type AuthState } from "./auth";

const initial: AuthState = { email: "owner@example.test", loginEpoch: "owner-login", accessToken: "fixture-access", expiresAt: "2099-01-01T00:00:00Z" };
const context = { shareId: "share/one", listings: [{ id: "listing", shareId: "share/one", isOwner: true, ownerEmail: initial.email, seats: [] }] };
async function withEnvironment(run: (setAuth: (next: AuthState) => void) => Promise<void>) {
  const originals = new Map(["window", "localStorage", "fetch"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  let auth = initial;
  Object.defineProperty(globalThis, "window", { configurable: true, value: new EventTarget() });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => JSON.stringify(auth) } });
  try { await run(next => { auth = next; }); }
  finally {
    for (const [key, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

test("management reads only the selected Share through the authenticated no-store endpoint", async () => {
  await withEnvironment(async () => {
    let calls = 0;
    globalThis.fetch = async (url, init) => {
      calls++;
      assert.equal(url, "/v1/share-market/shares/share%2Fone/owner-seat-context");
      assert.equal(init?.method ?? "GET", "GET");
      assert.equal(init?.cache, "no-store");
      assert.equal(init?.body, undefined);
      assert.equal(new Headers(init?.headers).get("Authorization"), "Bearer fixture-access");
      return Response.json(context);
    };
    assert.deepEqual(await getClientOwnerSeatContext("share/one"), context);
    assert.equal(calls, 1);
  });
});

test("closing management aborts its pending read", async () => {
  await withEnvironment(async () => {
    globalThis.fetch = async (_url, init) => new Promise((_resolve, reject) => {
      init?.signal?.throwIfAborted();
      init?.signal?.addEventListener("abort", () => reject(init.signal!.reason), { once: true });
    });
    const controller = new AbortController();
    const pending = getClientOwnerSeatContext("share/one", controller.signal);
    controller.abort();
    await assert.rejects(pending, { name: "AbortError" });
  });
});

test("ownership rejection and malformed private data never fall back to a public or whole-market request", async () => {
  await withEnvironment(async () => {
    let calls = 0;
    for (const response of [Response.json({ message: "not found" }, { status: 404 }), Response.json({ ...context, listings: [{ ...context.listings[0], isOwner: false }] })]) {
      globalThis.fetch = async () => { calls++; return response; };
      await assert.rejects(getClientOwnerSeatContext("share/one"));
    }
    assert.equal(calls, 2);
  });
});

for (const [name, replacement] of [
  ["account switch", { ...initial, email: "other@example.test", loginEpoch: "other-login", accessToken: "other-access" }],
  ["same-account new login", { ...initial, loginEpoch: "fresh-login" }],
  ["logout", {}],
] as const) {
  test(`management discards a late private response after ${name}`, async () => {
    await withEnvironment(async setAuth => {
      globalThis.fetch = async () => { setAuth(replacement); return Response.json(context); };
      await assert.rejects(getClientOwnerSeatContext("share/one"), { name: "AbortError" });
    });
  });
  test(`management cannot retry under a different identity after ${name}`, async () => {
    await withEnvironment(async setAuth => {
      let calls = 0;
      globalThis.fetch = async () => { calls++; setAuth(replacement); return Response.json({}, { status: 401 }); };
      await assert.rejects(getClientOwnerSeatContext("share/one"), (error: unknown) => error instanceof AuthError && error.code === "password_session_changed");
      assert.equal(calls, 1);
    });
  });
}

test("access token rotation is not a new management login", async () => {
  await withEnvironment(async setAuth => {
    globalThis.fetch = async () => { setAuth({ ...initial, accessToken: "rotated-access" }); return Response.json(context); };
    assert.deepEqual(await getClientOwnerSeatContext("share/one"), context);
  });
});
