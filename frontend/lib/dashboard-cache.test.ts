import assert from "node:assert/strict";
import test from "node:test";
import { bindVisibleInterval } from "./visible-interval";
import { clearDashboardGetCache, getDashboard } from "./api";
import type { DashboardResponse } from "./types";

const authKey = "cc_switch_router_auth_v2";

test("a hidden dashboard tab does not poll until it is visible again", () => {
  let visibility = "visible";
  const listeners = new Set<() => void>();
  let intervals = 0;
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  Object.assign(globalThis, {
    window: {
      setInterval() {
        intervals += 1;
        return intervals;
      },
      clearInterval() {},
    },
    document: {
      get visibilityState() {
        return visibility;
      },
      addEventListener(_name: string, listener: () => void) {
        listeners.add(listener);
      },
      removeEventListener(_name: string, listener: () => void) {
        listeners.delete(listener);
      },
    },
  });
  try {
    let ticks = 0;
    const stop = bindVisibleInterval(() => {
      ticks += 1;
    }, 5000);
    assert.equal(ticks, 1);
    assert.equal(intervals, 1);
    visibility = "hidden";
    listeners.forEach((listener) => listener());
    assert.equal(ticks, 1);
    visibility = "visible";
    listeners.forEach((listener) => listener());
    assert.equal(ticks, 2);
    stop();
    visibility = "hidden";
    listeners.forEach((listener) => listener());
    visibility = "visible";
    listeners.forEach((listener) => listener());
    assert.equal(ticks, 2);
  } finally {
    if (previousWindow === undefined) delete (globalThis as { window?: unknown }).window;
    else Object.assign(globalThis, { window: previousWindow });
    if (previousDocument === undefined) delete (globalThis as { document?: unknown }).document;
    else Object.assign(globalThis, { document: previousDocument });
  }
});

test("dashboard 304 reuses the body and refreshes generatedAt", async () => {
  clearDashboardGetCache();
  const previousWindow = globalThis.window;
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const previousFetch = globalThis.fetch;
  const values = new Map([[authKey, JSON.stringify({
    email: "owner@example.test",
    loginEpoch: "epoch-1",
    accessToken: "access",
    refreshToken: "refresh",
    expiresAt: "2099-01-01T00:00:00Z",
  })]]);
  Object.defineProperty(globalThis, "window", { configurable: true, value: new EventTarget() });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: (name: string) => values.get(name) ?? null },
  });
  const body = {
    generatedAt: "2026-11-22T00:00:00Z",
    stats: { clients: 1, activeShares: 0, totalActiveRequests: 0 },
  } as DashboardResponse;
  try {
    globalThis.fetch = async () => Response.json(body, { headers: { etag: "\"dash-1\"" } });
    assert.equal((await getDashboard()).generatedAt, "2026-11-22T00:00:00Z");
    globalThis.fetch = async (_url, options) => {
      assert.equal(new Headers(options?.headers).get("If-None-Match"), "\"dash-1\"");
      return new Response(null, { status: 304, headers: { etag: "\"dash-1\"" } });
    };
    const refreshed = await getDashboard();
    assert.notEqual(refreshed.generatedAt, "2026-11-22T00:00:00Z");
    assert.equal(Date.now() - Date.parse(refreshed.generatedAt) < 5_000, true);
    assert.equal(refreshed.stats.clients, 1);
  } finally {
    clearDashboardGetCache();
    globalThis.fetch = previousFetch;
    if (previousStorage) Object.defineProperty(globalThis, "localStorage", previousStorage);
    else Reflect.deleteProperty(globalThis, "localStorage");
    if (previousWindow === undefined) delete (globalThis as { window?: unknown }).window;
    else Object.assign(globalThis, { window: previousWindow });
  }
});
