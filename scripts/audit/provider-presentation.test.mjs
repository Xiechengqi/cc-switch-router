import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { auditProviderPresentation, presentationViolations } from "./audit-provider-presentation.mjs";

test("frozen provider presentation is independently auditable", () => {
  assert.deepEqual(auditProviderPresentation(), []);
});
test("catalog drift, missing icons and active SVG cannot pass", () => {
  const catalog = JSON.parse(fs.readFileSync(new URL("../../assets/provider-presentation.json", import.meta.url)));
  catalog.icons.openai.content = '<svg onload="alert(1)"></svg>';
  catalog.families["family.custom_http"].icon = "missing";
  const errors = presentationViolations(catalog).join("\n");
  assert.match(errors, /content hash mismatch/);
  assert.match(errors, /asset hash\/mode mismatch/);
  assert.match(errors, /active\/external SVG/);
  assert.match(errors, /missing icon/);
});
