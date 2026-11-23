import assert from "node:assert/strict";
import test from "node:test";
import type { ShareMarketAppCapability, ShareMarketOwnedShare } from "@/lib/types";
import { ownedShareModelEntries } from "./market-share-identity";

function capability(app: string, overrides: Partial<ShareMarketAppCapability> = {}): ShareMarketAppCapability {
  return {
    app,
    providerFamily: "anthropic",
    modelMode: "unknown",
    healthState: "unknown",
    ...overrides,
  };
}

function share(overrides: Partial<ShareMarketOwnedShare> = {}): Pick<ShareMarketOwnedShare, "appCapabilities" | "supportedApps"> {
  return {
    supportedApps: ["claude"],
    appCapabilities: [capability("claude")],
    ...overrides,
  };
}

test("owned-share picker lists one model entry per supported core app in core order", () => {
  const entries = ownedShareModelEntries(
    share({
      supportedApps: ["gemini", "claude"],
      appCapabilities: [
        capability("gemini", { modelMode: "fixed", upstreamModel: "gemini-3-pro" }),
        capability("claude", { modelMode: "passthrough" }),
      ],
    }),
    "Passthrough",
    "Unknown",
  );
  assert.deepEqual(entries, [
    { app: "claude", model: "Passthrough" },
    { app: "gemini", model: "gemini-3-pro" },
  ]);
});

test("fixed capabilities fall back to the first advertised model, unknown ones to the unknown label", () => {
  const entries = ownedShareModelEntries(
    share({
      supportedApps: ["claude", "codex"],
      appCapabilities: [
        capability("claude", { modelMode: "fixed", models: ["claude-sonnet-5-5"] }),
        capability("codex", { modelMode: "unknown" }),
      ],
    }),
    "Passthrough",
    "Unknown",
  );
  assert.deepEqual(entries, [
    { app: "claude", model: "claude-sonnet-5-5" },
    { app: "codex", model: "Unknown" },
  ]);
});

test("unsupported apps and missing capabilities are skipped", () => {
  const entries = ownedShareModelEntries(
    share({
      supportedApps: ["claude", "gemini"],
      appCapabilities: [
        capability("codex", { modelMode: "passthrough" }),
        capability("gemini", { modelMode: "fixed", upstreamModel: "gemini-3-pro" }),
      ],
    }),
    "Passthrough",
    "Unknown",
  );
  assert.deepEqual(entries, [{ app: "gemini", model: "gemini-3-pro" }]);
});

test("a share without capability data lists no model entries", () => {
  assert.deepEqual(
    ownedShareModelEntries(
      { supportedApps: ["claude"], appCapabilities: undefined },
      "Passthrough",
      "Unknown",
    ),
    [],
  );
});
