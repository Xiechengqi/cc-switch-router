import assert from "node:assert/strict";
import test from "node:test";
import { buildShareLocalTestUrl, openShareLocalTest, SHARE_LOCAL_TEST_ORIGIN, shareLocalTestUnavailable, type ShareLocalTestInput } from "./share-local-test";
import { messages } from "./i18n";

const input: ShareLocalTestInput = {
  app: "codex", operation: "text", baseUrl: "https://share.router.example:8443/",
  apiToken: "ccrt_fixture-only-key",
  probe: {
    apiType: "openai", requestedModel: "vendor/test-model@high", wireModel: "different-upstream-model",
    method: "POST", path: "/v1/responses", body: { model: "different-upstream-model", input: "hello" },
    stream: true, responseMode: "responses_sse", payloadRevision: 2,
  },
  authenticated: true, canExecute: true, isBound: true,
};

test("local test targets the exact Share's Chat endpoint and displayed model, never the unified API or upstream model", () => {
  const url = new URL(buildShareLocalTestUrl(input)!);
  assert.equal(url.origin, SHARE_LOCAL_TEST_ORIGIN);
  assert.equal(url.pathname, "/");
  assert.deepEqual([...url.searchParams.keys()], ["api", "key", "model"]);
  assert.equal(url.searchParams.get("api"), "https://share.router.example:8443/v1/chat/completions");
  assert.equal(url.searchParams.get("key"), input.apiToken);
  assert.equal(url.searchParams.get("model"), input.probe!.requestedModel);
  assert.ok(!url.href.includes(input.probe!.wireModel));
  assert.ok(!url.href.includes("/v1/responses"));
});

test("URLSearchParams encodes nested URLs, Keys and model identifiers exactly once", () => {
  const apiToken = "ccrt_fixture+&=?/#%";
  const model = "vendor/model+name@high?reasoning=on&x=%23#模型";
  const url = new URL(buildShareLocalTestUrl({ ...input, apiToken, probe: { ...input.probe!, requestedModel: model } })!);
  assert.equal(url.searchParams.size, 3);
  assert.equal(url.searchParams.get("key"), apiToken);
  assert.equal(url.searchParams.get("model"), model);
  assert.equal(url.searchParams.get("api"), "https://share.router.example:8443/v1/chat/completions");
});

for (const [name, overrides, reason] of [
  ["anonymous", { authenticated: false }, "test.needAuth"],
  ["unauthorized", { canExecute: false }, "test.needPermission"],
  ["unbound", { isBound: false }, "test.notBound"],
  ["Claude", { app: "claude" }, "localTest.unsupported"],
  ["Gemini", { app: "gemini" }, "localTest.unsupported"],
  ["image", { operation: "image_generation" }, "localTest.unsupported"],
  ["video", { operation: "video_generation" }, "localTest.unsupported"],
  ["missing probe", { probe: undefined }, "test.probeUnavailable"],
  ["missing model", { probe: { ...input.probe!, requestedModel: " " } }, "test.probeUnavailable"],
  ["wrong wire protocol", { probe: { ...input.probe!, apiType: "anthropic" } }, "localTest.unsupported"],
  ["loading Key", { tokenLoading: true }, "localTest.keyLoading"],
  ["failed Key read", { tokenError: true }, "localTest.keyUnavailable"],
  ["missing Key", { apiToken: "" }, "localTest.keyUnavailable"],
  ["masked Key", { apiToken: "ccrt_prefix••••••••" }, "localTest.keyUnavailable"],
  ["invalid Key", { apiToken: "ccrt_fixture\nkey" }, "localTest.keyUnavailable"],
] satisfies [string, Partial<ShareLocalTestInput>, string][]) {
  test(`local test refuses ${name} without generating a sensitive URL`, () => {
    const next = { ...input, ...overrides };
    assert.equal(shareLocalTestUnavailable(next), `dashboard.connectDialog.${reason}`);
    assert.equal(buildShareLocalTestUrl(next), null);
  });
}

for (const baseUrl of ["", "https://", "http://share.router.example", "javascript:alert(1)", "//share.router.example",
  "https://user:password@share.router.example", "https://share.router.example/v1", "https://share.router.example/?key=secret", "https://share.router.example/#fragment"]) {
  test(`local test rejects unsafe or non-root Share address: ${baseUrl}`, () => {
    assert.equal(buildShareLocalTestUrl({ ...input, baseUrl }), null);
  });
}

test("explicit launch opens one isolated new tab without fetching, storage, or navigating the Router tab", () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "window");
  const opened: unknown[][] = [];
  Object.defineProperty(globalThis, "window", { configurable: true, value: { open: (...args: unknown[]) => { opened.push(args); return null; } } });
  try {
    assert.equal(openShareLocalTest(input, () => false), false);
    assert.equal(openShareLocalTest({ ...input, apiToken: "" }, () => true), false);
    assert.equal(opened.length, 0);
    assert.equal(openShareLocalTest(input, () => true), true);
    assert.equal(opened.length, 1);
    assert.deepEqual(opened[0].slice(1), ["_blank", "noopener,noreferrer"]);
    assert.equal(new URL(String(opened[0][0])).searchParams.get("key"), input.apiToken);
  } finally {
    if (original) Object.defineProperty(globalThis, "window", original);
    else Reflect.deleteProperty(globalThis, "window");
  }
});

test("local test labels, warnings and recovery copy exist in both locales without example credentials", () => {
  const keys = Object.keys(messages.en).filter(key => key.startsWith("dashboard.connectDialog.localTest."));
  assert.equal(keys.length, 10);
  for (const key of keys) {
    assert.equal(typeof messages["zh-CN"][key as keyof typeof messages.en], "string");
    for (const locale of ["en", "zh-CN"] as const) {
      const copy = messages[locale][key as keyof typeof messages.en];
      assert.ok(copy.length > 0);
      assert.ok(!copy.includes("ccrt_"));
    }
  }
});

test("a browser launch exception cannot surface a sensitive URL as an error", () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    open: (url: string) => { throw new Error(`Blocked navigation to ${url}`); },
  } });
  try {
    assert.equal(openShareLocalTest(input, () => true), false);
  } finally {
    if (original) Object.defineProperty(globalThis, "window", original);
    else Reflect.deleteProperty(globalThis, "window");
  }
});
