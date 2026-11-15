import assert from "node:assert/strict";
import test from "node:test";
import { getDashboard, getShareMarketCatalog, getUserModelRouting, replaceUserModelRouting } from "./api";
import { getSharePageShare, writeShareApiCredentials, clearShareApiCredentials } from "./share-api";
import { loginContextKey } from "./password-auth";
import type { AuthState } from "./auth";

const authKey = "cc_switch_router_auth_v2";
const initial: AuthState = { email: "owner@example.test", loginEpoch: "original-login", accessToken: "original-access", refreshToken: "original-refresh", expiresAt: "2099-01-01T00:00:00Z" };

async function environment(run: (setAuth: (auth: AuthState) => void) => Promise<void>) {
  const names = ["window", "localStorage", "sessionStorage", "fetch"];
  const originals = names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  const values = new Map([[authKey, JSON.stringify({...initial, loginEpoch: crypto.randomUUID()})]]);
  const storage = {getItem: (name: string) => values.get(name) ?? null, setItem: (name: string, value: string) => values.set(name, value), removeItem: (name: string) => values.delete(name)};
  Object.defineProperty(globalThis, "window", {configurable: true, value: new EventTarget()});
  for (const name of ["localStorage", "sessionStorage"]) Object.defineProperty(globalThis, name, {configurable: true, value: storage});
  try { await run(auth => values.set(authKey, JSON.stringify(auth))); }
  finally {
    for (const [name, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => {resolve = done;});
  return {promise, resolve};
}

test("login cache scopes exclude secrets and survive access-token rotation", () => {
  const key = loginContextKey(initial);
  assert.equal(key, loginContextKey({...initial, accessToken: "rotated-access"}));
  assert.notEqual(key, loginContextKey({...initial, loginEpoch: "new-login"}));
  assert.notEqual(key, loginContextKey({...initial, email: "other@example.test"}));
  assert.doesNotMatch(key, /original-access|original-refresh/);
});

test("private Share readers reject late owner responses after changing login or clearing credentials", async () => {
  await environment(async setAuth => {
    for (const reader of [getDashboard, getUserModelRouting, () => replaceUserModelRouting({expectedRevision: 1, routes: []})]) {
      for (const next of [
        {...initial, email: "other@example.test"},
        {...initial, loginEpoch: "new-login"},
        {...initial, accessToken: null, refreshToken: null},
      ]) {
        setAuth(initial);
        const response = deferred<Response>();
        globalThis.fetch = async () => response.promise;
        const pending = reader();
        setAuth(next);
        response.resolve(Response.json({privateAccount: "owner-account"}));
        await assert.rejects(pending, {name: "AbortError"});
      }
    }
  });
});

test("private Share readers check login again after an asynchronously decoded body", async () => {
  await environment(async setAuth => {
    for (const reader of [getDashboard, getUserModelRouting, getShareMarketCatalog]) {
      setAuth({...initial, loginEpoch: crypto.randomUUID()});
      const body = deferred<unknown>();
      const reading = deferred<void>();
      const response = Response.json({});
      response.json = () => { reading.resolve(); return body.promise; };
      globalThis.fetch = async () => response;
      const pending = reader();
      await reading.promise;
      setAuth({...initial, email: "other@example.test"});
      body.resolve({privateAccount: "owner-account"});
      await assert.rejects(pending, {name: "AbortError"});
    }
  });
});

test("Share Market revalidation is scoped by login and rejects late cached owner bodies", async () => {
  await environment(async setAuth => {
    setAuth(initial);
    const ownerBody = {listings: [{accountHint: "owner-account@example.test"}]};
    globalThis.fetch = async () => Response.json(ownerBody, {headers: {etag: '"owner-tag"'}});
    assert.deepEqual(await getShareMarketCatalog(), ownerBody);
    globalThis.fetch = async (_url, options) => {
      assert.equal(new Headers(options?.headers).get("If-None-Match"), '"owner-tag"');
      return new Response(null, {status: 304});
    };
    assert.deepEqual(await getShareMarketCatalog(), ownerBody);
    const response = deferred<Response>();
    globalThis.fetch = async () => response.promise;
    const pending = getShareMarketCatalog();
    setAuth({...initial, loginEpoch: "another-login"});
    response.resolve(new Response(null, {status: 304}));
    await assert.rejects(pending, {name: "AbortError"});
    globalThis.fetch = async (_url, options) => {
      assert.equal(new Headers(options?.headers).get("If-None-Match"), null);
      return Response.json({listings: []});
    };
    assert.deepEqual(await getShareMarketCatalog(), {listings: []});
  });
});

test("standalone Share ignores a full owner response after its API token is cleared", async () => {
  await environment(async () => {
    writeShareApiCredentials("owner@example.test", "owner-api-token");
    const response = deferred<Response>();
    globalThis.fetch = async () => response.promise;
    const pending = getSharePageShare();
    clearShareApiCredentials();
    response.resolve(Response.json({share: {accountEmail: "owner-account@example.test"}}));
    await assert.rejects(pending, {name: "AbortError"});
  });
});
