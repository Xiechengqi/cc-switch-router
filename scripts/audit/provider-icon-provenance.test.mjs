import assert from "node:assert/strict";
import test from "node:test";

import {
  auditProviderIconProvenance,
  svgAssetViolations,
} from "./audit-provider-icon-provenance.mjs";

test("provider SVG validation detects content drift and active content", () => {
  const cleanSvg = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"></svg>');
  assert.match(
    svgAssetViolations("icon.svg", cleanSvg, "0".repeat(64)).join("\n"),
    /content hash differs/,
  );
  const activeSvg = Buffer.from(
    '<svg xmlns="http://www.w3.org/2000/svg"><script></script></svg>',
  );
  assert.match(
    svgAssetViolations("icon.svg", activeSvg, "0".repeat(64)).join("\n"),
    /executable or external content/,
  );
});

test("checked-in provider icons match the pinned package inventory", () => {
  assert.deepEqual(auditProviderIconProvenance(), []);
});
