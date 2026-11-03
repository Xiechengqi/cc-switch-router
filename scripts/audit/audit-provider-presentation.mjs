import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function presentationViolations(catalog) {
  const errors = [];
  const { catalogVersion, ...payload } = catalog;
  if (catalogVersion !== hash(JSON.stringify(payload))) errors.push("catalog content hash mismatch");
  if (catalog.schemaVersion !== 1) errors.push("unsupported catalog schema");
  for (const [id, icon] of Object.entries(catalog.icons ?? {})) {
    const image = icon.mode === "image";
    const bytes = image ? Buffer.from(icon.content.split(",")[1] ?? "", "base64") : Buffer.from(icon.content);
    if (!["inline", "image"].includes(icon.mode) || hash(bytes) !== icon.sha256) errors.push(`${id}: asset hash/mode mismatch`);
    if (image && !/^data:image\/(png|svg\+xml);base64,/.test(icon.content)) errors.push(`${id}: invalid image data URL`);
    const svg = !image || icon.content.startsWith("data:image/svg+xml;");
    if (svg && (/<(?:script|foreignObject)\b|\bon\w+\s*=|(?:href|src)\s*=\s*["'](?:https?:|\/\/|javascript:)/i.test(bytes.toString()))) errors.push(`${id}: active/external SVG`);
  }
  for (const [id, family] of Object.entries(catalog.families ?? {})) {
    if (!Object.hasOwn(catalog.categories ?? {}, family.category)) errors.push(`${id}: missing category`);
    if (!Object.hasOwn(catalog.icons ?? {}, family.icon)) errors.push(`${id}: missing icon`);
  }
  return errors;
}

export function auditProviderPresentation(repoRoot = root) {
  const read = (file) => fs.readFileSync(path.join(repoRoot, file), "utf8");
  const backend = read("assets/provider-presentation.json");
  const frontend = read("frontend/lib/generated/provider-presentation.json");
  const errors = presentationViolations(JSON.parse(backend));
  if (backend !== frontend) errors.push("frontend/backend catalog mismatch");
  const fixtures = JSON.parse(read("assets/provider-identity-fixtures.json"));
  const catalog = JSON.parse(backend);
  if (fixtures.length !== Object.keys(catalog.families).length) errors.push("family fixtures incomplete");
  for (const fixture of fixtures) {
    if (fixture.expectedCategory !== catalog.families[fixture.name]?.category) errors.push(`${fixture.name}: fixture category mismatch`);
  }
  return errors;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const errors = auditProviderPresentation();
  if (errors.length) throw new Error(errors.join("\n"));
  console.log("provider presentation: independent frozen catalogs, asset hashes and family fixtures verified");
}
