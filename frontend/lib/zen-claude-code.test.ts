import { strict as assert } from "node:assert";
import test from "node:test";
import { loadZenClaudeCodeCatalog, zenClaudeCodeCommand } from "./zen-claude-code";

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
  await assert.rejects(loadZenClaudeCodeCatalog("https://other.example.test", "router.example.test", "synthetic-test-token", "share-1", "provider-1", signal));
  assert.equal(fetch.mock.callCount(), 0);
  const catalog = await loadZenClaudeCodeCatalog("https://zen.router.example.test", "router.example.test", "synthetic-test-token", "share-1", "provider-1", signal);
  assert.equal(catalog.fixed, true);
  const command = zenClaudeCodeCommand("https://zen.router.example.test", catalog.models[0], true);
  assert.match(command, /CLAUDE_CODE_MAX_OUTPUT_TOKENS=12000/);
  assert.match(command, /claude --model='small-free' --effort='low' -p /);
  assert.match(command, /TOKENSWITCH_API_KEY/);
  assert.doesNotMatch(command, /synthetic-test-token/);
});
