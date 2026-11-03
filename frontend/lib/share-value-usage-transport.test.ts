import assert from "node:assert/strict";
import test from "node:test";
import { AuthError, readAuthState, type AuthState } from "./auth";
import { editValueUsage, type ValueUsageEdit } from "./share-value-quota";

const authKey = "cc_switch_router_auth_v2";
const initial: AuthState = {
  loginEpoch: "usage-editor-login",
  email: "owner@example.test",
  accessToken: "original-access",
  refreshToken: "original-refresh",
  expiresAt: "2099-01-01T00:00:00Z",
};
const edits: Record<string, ValueUsageEdit> = {
  "user@example.test": {
    action: "set",
    targetPicos: "1",
    expectedGrantRevision: 3,
    windowId: "Lifetime:0:0",
    expectedWatermark: 4,
    operationId: "usage-correction",
    reason: "Reviewed usage",
  },
};

async function withAuthEnvironment(
  run: (setState: (state: AuthState) => void) => Promise<void>,
) {
  const originals = new Map(
    ["window", "localStorage", "fetch"].map((name) => [
      name,
      Object.getOwnPropertyDescriptor(globalThis, name),
    ]),
  );
  const entries = new Map([[authKey, JSON.stringify(initial)]]);
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: new EventTarget(),
  });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (name: string) => entries.get(name) ?? null,
      setItem: (name: string, value: string) => entries.set(name, value),
    },
  });
  try {
    await run((state) => entries.set(authKey, JSON.stringify(state)));
  } finally {
    for (const [name, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
  }
}

test("usage correction carries the exact target and keeps an unchanged login's result", async () => {
  await withAuthEnvironment(async () => {
    globalThis.fetch = async (url, options) => {
      assert.equal(url, "/v1/shares/share%2Ftest/value-usage-edits");
      assert.equal(new Headers(options?.headers).get("Authorization"), "Bearer original-access");
      assert.deepEqual(JSON.parse(String(options?.body)), {expectedConfigRevision: 7, edits});
      return Response.json({items: []});
    };
    assert.deepEqual(await editValueUsage("share/test", 7, edits), {items: [], errors: []});
  });
});

for (const [name, replacement] of [
  ["another account", {...initial, loginEpoch: "another-login", email: "other@example.test", accessToken: "other-access", refreshToken: "other-refresh"}],
  ["a fresh login to the same account", {...initial, loginEpoch: "fresh-login"}],
  ["logout", {email: null, loginEpoch: null}],
] as const) {
  test(`usage correction discards a late success after ${name}`, async () => {
    await withAuthEnvironment(async (setState) => {
      globalThis.fetch = async () => {
        setState(replacement);
        return Response.json({items: []});
      };
      await assert.rejects(editValueUsage("share", 7, edits), {name: "AbortError"});
      assert.deepEqual(readAuthState(), replacement);
    });
  });

  test(`usage correction cannot retry under ${name}`, async () => {
    await withAuthEnvironment(async (setState) => {
      let requests = 0;
      globalThis.fetch = async () => {
        requests++;
        setState(replacement);
        return Response.json({}, {status: 401});
      };
      await assert.rejects(editValueUsage("share", 7, edits), (error: unknown) =>
        error instanceof AuthError && error.code === "password_session_changed");
      assert.equal(requests, 1, "never send another identity's correction");
      assert.deepEqual(readAuthState(), replacement);
    });
  });
}

test("usage correction permits token rotation within the same login", async () => {
  await withAuthEnvironment(async (setState) => {
    const authorizations: (string | null)[] = [];
    globalThis.fetch = async (_url, options) => {
      authorizations.push(new Headers(options?.headers).get("Authorization"));
      if (authorizations.length === 1) {
        setState({...initial, accessToken: "rotated-access", refreshToken: "rotated-refresh"});
        return Response.json({}, {status: 401});
      }
      return Response.json({items: []});
    };
    assert.deepEqual(await editValueUsage("share", 7, edits), {items: [], errors: []});
    assert.deepEqual(authorizations, ["Bearer original-access", "Bearer rotated-access"]);
  });
});
