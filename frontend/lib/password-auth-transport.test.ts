import assert from "node:assert/strict";
import test from "node:test";
import {
  AuthError,
  readAuthState,
  updateLoginPassword,
  type AuthState,
} from "./auth";

const key = "cc_switch_router_auth_v2";
const initial: AuthState = {
  loginEpoch: "original-login",
  email: "original@example.test",
  accessToken: "original-access",
  refreshToken: "original-refresh",
  expiresAt: "2099-01-01T00:00:00Z",
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
  const entries = new Map([[key, JSON.stringify(initial)]]);
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
    await run((state) => entries.set(key, JSON.stringify(state)));
  } finally {
    for (const [name, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
  }
}

for (const [name, replacement] of [
  [
    "another account",
    {
      ...initial,
      loginEpoch: "another-login",
      email: "another@example.test",
      accessToken: "another-access",
      refreshToken: "another-refresh",
    },
  ],
  [
    "a fresh login to the same account",
    { ...initial, loginEpoch: "fresh-login" },
  ],
  ["explicit logout", { email: null, loginEpoch: null }],
] as const) {
  test(`password update cannot retry a 401 under ${name}`, async () => {
    await withAuthEnvironment(async (setState) => {
      let requests = 0;
      globalThis.fetch = async () => {
        requests++;
        setState(replacement);
        return Response.json({}, { status: 401 });
      };
      await assert.rejects(
        updateLoginPassword("new secure password", 0),
        (error: unknown) =>
          error instanceof AuthError &&
          error.code === "password_session_changed",
      );
      assert.equal(
        requests,
        1,
        "never send the credential body with another session",
      );
      assert.deepEqual(readAuthState(), replacement);
    });
  });
}

test("password update stays bound to its account while refreshing an expired token", async () => {
  await withAuthEnvironment(async (setState) => {
    setState({ ...initial, expiresAt: "2000-01-01T00:00:00Z" });
    const requests: string[] = [];
    globalThis.fetch = async (url) => {
      requests.push(String(url));
      setState({
        ...initial,
        loginEpoch: "another-login",
        email: "another@example.test",
        accessToken: "another-access",
        refreshToken: "another-refresh",
      });
      return Response.json({
        accessToken: "refreshed-original-access",
        refreshToken: "refreshed-original-refresh",
        expiresAt: "2099-01-01T00:00:00Z",
        refreshExpiresAt: "2099-02-01T00:00:00Z",
      });
    };
    await assert.rejects(
      updateLoginPassword("new secure password", 0),
      (error: unknown) =>
        error instanceof AuthError && error.code === "password_session_changed",
    );
    assert.deepEqual(requests, ["/v1/auth/session/refresh"]);
    assert.equal(readAuthState().accessToken, "another-access");
  });
});

test("password update can retry normal token rotation within the same login", async () => {
  await withAuthEnvironment(async (setState) => {
    const authorizations: (string | null)[] = [];
    globalThis.fetch = async (_url, options) => {
      authorizations.push(new Headers(options?.headers).get("Authorization"));
      if (authorizations.length === 1) {
        setState({
          ...initial,
          accessToken: "rotated-access",
          refreshToken: "rotated-refresh",
        });
        return Response.json({}, { status: 401 });
      }
      return Response.json({
        user: { id: "original-user", email: initial.email },
        accessToken: "replacement-access",
        refreshToken: "replacement-refresh",
        expiresAt: "2099-01-01T00:00:00Z",
        refreshExpiresAt: "2099-02-01T00:00:00Z",
      });
    };
    await updateLoginPassword("new secure password", 0);
    assert.deepEqual(authorizations, [
      "Bearer original-access",
      "Bearer rotated-access",
    ]);
    assert.equal(readAuthState().accessToken, "replacement-access");
    assert.equal(readAuthState().email, initial.email);
  });
});

test("password updates send only two-field form data and its version, never an old password", async () => {
  await withAuthEnvironment(async () => {
    globalThis.fetch = async (_url, options) => {
      assert.deepEqual(JSON.parse(String(options?.body)), {
        newPassword: "valid6",
        expectedVersion: 3,
      });
      return Response.json({
        user: { id: "original-user", email: initial.email },
        accessToken: "replacement-access",
        refreshToken: "replacement-refresh",
        expiresAt: "2099-01-01T00:00:00Z",
        refreshExpiresAt: "2099-02-01T00:00:00Z",
      });
    };
    await updateLoginPassword("valid6", 3);
  });
});
