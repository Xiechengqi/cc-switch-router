import { strict as assert } from "node:assert";
import test from "node:test";
import { loadZenCliCatalog, zenClaudeCodeCommand, zenCodexCliCommand, ZenCliCatalogError } from "./zen-cli";

test("Zen Claude Code settings use the authenticated Share binding and server model limits", async (context) => {
  const fetch = context.mock.method(globalThis, "fetch", async (url: URL, options: RequestInit) => {
    assert.equal(url.origin, "https://zen.router.example.test");
    assert.equal(url.pathname, "/v1/models");
    assert.equal(url.searchParams.get("app"), "claude");
    assert.equal(url.searchParams.get("expectedShareId"), "share-1");
    assert.equal(url.searchParams.get("expectedProviderId"), "provider-1");
    assert.equal(new Headers(options.headers).get("authorization"), "Bearer synthetic-test-token");
    assert.equal(options.credentials, "omit");
    assert.equal(options.redirect, "error");
    return Response.json({
      source: "opencode_zen_live_model_catalog",
      claudeCode: { fixed: true, models: [{ id: "small-free", displayName: "Small", maxOutputTokens: 12000, effort: "low" }] },
    });
  });
  const signal = new AbortController().signal;
  await assert.rejects(loadZenCliCatalog("claude", "https://other.example.test", "router.example.test", "synthetic-test-token", "share-1", "provider-1", signal));
  assert.equal(fetch.mock.callCount(), 0);
  const catalog = await loadZenCliCatalog("claude", "https://zen.router.example.test", "router.example.test", "synthetic-test-token", "share-1", "provider-1", signal);
  assert.equal(catalog.fixed, true);
  const command = zenClaudeCodeCommand("https://zen.router.example.test", catalog.models[0], true);
  assert.match(command, /CLAUDE_CODE_MAX_OUTPUT_TOKENS=12000/);
  assert.match(command, /claude --model='small-free' --effort='low' -p /);
  assert.match(command, /TOKENSWITCH_API_KEY/);
  assert.doesNotMatch(command, /synthetic-test-token/);
});

test("Zen Codex settings select the actual Share model and an HTTP Responses provider", async (context) => {
  let calls = 0;
  context.mock.method(globalThis, "fetch", async (url: URL, options: RequestInit) => {
    assert.equal(url.origin, "https://zen.router.example.test");
    assert.equal(url.pathname, "/v1/models");
    assert.deepEqual(Object.fromEntries(url.searchParams), {
      app: "codex", expectedShareId: "share-1", expectedProviderId: "provider-1",
    });
    assert.equal(new Headers(options.headers).get("authorization"), "Bearer synthetic-test-token");
    assert.equal(new Headers(options.headers).has("anthropic-version"), false);
    assert.equal(options.credentials, "omit");
    assert.equal(options.redirect, "error");
    calls += 1;
    if (calls === 2) return new Response(null, { status: 409 });
    return Response.json({
      source: "opencode_zen_live_model_catalog",
      stale: calls === 3,
      codexCli: { fixed: true, models: [{ id: "hy4-flash-free", displayName: "HY4", effort: "high" }] },
    });
  });
  const signal = new AbortController().signal;
  const load = () => loadZenCliCatalog("codex", "https://zen.router.example.test", "router.example.test", "synthetic-test-token", "share-1", "provider-1", signal);
  const catalog = await load();
  assert.equal(catalog.fixed, true);
  const command = zenCodexCliCommand("https://zen.router.example.test", catalog.models[0], true);
  assert.match(command, /^codex exec .*--model 'hy4-flash-free'/);
  assert.match(command, /base_url="https:\/\/zen\.router\.example\.test\/v1"/);
  assert.match(command, /wire_api="responses"/);
  assert.match(command, /supports_websockets=false/);
  assert.match(command, /model_reasoning_effort="high"/);
  assert.match(command, /model_reasoning_summary="auto"/);
  assert.match(command, /model_verbosity="medium"/);
  assert.match(command, /web_search="disabled"/);
  assert.match(command, /env_key="TOKENSWITCH_API_KEY"/);
  assert.doesNotMatch(command, /synthetic-test-token/);
  await assert.rejects(load(), (e: unknown) => e instanceof ZenCliCatalogError && e.reason === "bindingChanged");
  await assert.rejects(load(), (e: unknown) => e instanceof ZenCliCatalogError && e.reason === "stale");
});
