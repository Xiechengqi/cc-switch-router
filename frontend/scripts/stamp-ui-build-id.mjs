import fs from "node:fs";
import path from "node:path";

const outDir = "out";
const payload = fs.readFileSync(path.join(outDir, "__next._full.txt"), "utf8");
const match = payload.match(/"b":"([^"\\]+)"/);
const id = match?.[1] || "";
if (!id) {
  throw new Error("frontend export has no Next build id to stamp");
}
const meta = `<meta name="ccr-ui-build" content="${id}"/>`;

function stamp(file) {
  const source = fs.readFileSync(file, "utf8");
  if (source.includes(meta) || !source.includes("<head>")) return;
  const stamped = source.replace("<head>", `<head>${meta}`);
  if (stamped === source) throw new Error(`${file} has no head to stamp`);
  fs.writeFileSync(file, stamped);
}

function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full);
    else if (entry.name.endsWith(".html")) stamp(full);
  }
}

walk(outDir);
