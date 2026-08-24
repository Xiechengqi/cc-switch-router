import assert from "node:assert/strict";
import test from "node:test";
import { buildShareProbeCurl, shellQuote } from "@/lib/share-model-probe";
import type { ProviderModelProbe } from "@/lib/types";

function probe(overrides: Partial<ProviderModelProbe> = {}): ProviderModelProbe {
  return {
    apiType: "openai",
    requestedModel: "vendor/model@low",
    wireModel: "vendor/model",
    method: "POST",
    path: "/v1/provider-test",
    body: { model: "vendor/model", input: "owner's probe" },
    stream: true,
    responseMode: "responses_sse",
    payloadRevision: 2,
    ...overrides,
  };
}

test("shellQuote protects apostrophes in structured probe values", () => {
  assert.equal(shellQuote("owner's token"), "'owner'\"'\"'s token'");
});

test("curl uses the authoritative probe path, body, model, and stream mode", () => {
  const command = buildShareProbeCurl(
    "https://share.router.example",
    probe(),
    "token-with-'quote",
  );

  assert.match(command, /https:\/\/share\.router\.example\/v1\/provider-test/);
  assert.match(command, /vendor\/model/);
  assert.match(command, /owner/);
  assert.match(command, /Accept: text\/event-stream/);
  assert.match(command, /curl -N -sS/);
  assert.ok(!command.includes("undefined"));
});

test("JSON probes omit streaming curl flags", () => {
  const command = buildShareProbeCurl(
    "https://share.router.example",
    probe({ stream: false, responseMode: "json" }),
    "",
  );

  assert.ok(!command.includes("curl -N"));
  assert.ok(!command.includes("Accept: text/event-stream"));
  assert.match(command, /Bearer <your-api-token>/);
});
