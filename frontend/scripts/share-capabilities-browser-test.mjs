import assert from "node:assert/strict";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { readFile } from "node:fs/promises";
import postcss from "postcss";
import tailwind from "@tailwindcss/postcss";
import { build } from "esbuild";
import { chromium } from "playwright";

// Real component interaction, with local protocol fixtures; never calls an upstream.
const root = fileURLToPath(new URL("../", import.meta.url));
const source = `
import React from "react";
import { createRoot } from "react-dom/client";
import { LocaleProvider } from "@/components/i18n/locale-provider";
import { GrantCapabilityFields } from "@/components/dashboard/share-edit/grant-capability-fields";
import { ShareRequestMetering } from "@/components/dashboard/share-request-metering";
import { changedShareSettings } from "@/lib/share-settings";
import { reviewGrantCapabilities } from "@/lib/share-grant-capabilities";

const email = "a@example.com";
const op = "tool.web_search";
const saved = {
  [email]: { email, role: "shareto", active: true, revision: 2, policy: { tokenLimit: 1000000, allowedOperations: [] } },
  "b@example.com": { email: "b@example.com", role: "shareto", active: true, revision: 6, policy: { tokenLimit: 1000000, allowedOperations: [] } },
};
let situation = "normal";
async function preview(shareId, input) {
  if (situation === "offline" || situation === "offlineSaved") throw new Error("Unreachable");
  return {
    displayVersion: 3, shareId, subject: input.subject, configRevision: input.expectedConfigRevision, ready: true, unavailable: false,
    rows: situation === "empty" ? [] : [{operation: op, available: situation !== "gone", selected: situation === "changed" || situation === "gone", needsReconfirm: situation === "changed"}],
    requiredAcknowledgements: Object.keys(input.grantCapabilityEdits).map(scope => ({scope, operation: op, condition: "gap:" + scope})),
    previewCredential: "test-ticket", expiresAt: Date.now() + 60000,
  };
}
const meter = async (_shareId, requestId) => [
  {requestId, attempt: 1, chargeMode:"estimate",settledPicos:"0",meteringStatus:"observed",unpricedReason:"model_unpriced",operations:[],dimensions:[],pricingRevision:"f".repeat(64)},
  {requestId, attempt: 2, chargeMode:"debited",settledPicos:"1",meteringStatus:"observed",operations:[op],dimensions:[]},
];
function Harness() {
  const [revision,setRevision] = React.useState(1);
  const [selected,setSelected] = React.useState([]);
  const [reconfirm,setReconfirm] = React.useState([]);
  window.loadScenario = name => {
    situation = name;
    setSelected(name === "changed" || name === "gone" || name === "offlineSaved" ? [op] : []);
    setReconfirm([]);
    setRevision(current => current + 1);
  };
  return <main className="mx-auto grid max-w-lg gap-4 p-4">
    <GrantCapabilityFields shareId="share" revision={revision} email={email} request={preview}
      selected={selected} reconfirm={reconfirm} quota="tokens" zh={false} onChange={setSelected}
      onReconfirm={operation => setReconfirm([operation])} />
    <button type="button" onClick={async () => {
      const draft = {...saved, [email]: {...saved[email], reconfirmOperations: reconfirm,policy:{...saved[email].policy,allowedOperations:selected}}};
      const patch = changedShareSettings({userGrants:draft},{userGrants:saved},{},{},true);
      const proof = await reviewGrantCapabilities(preview,"share",revision,patch,false);
      if (proof) window.saved = {...patch,capabilityConsent:proof};
    }}>Save</button>
    <ShareRequestMetering requestId="one" shareId="share" load={meter} zh={false} />
  </main>;
}
createRoot(document.getElementById("root")).render(<LocaleProvider><Harness /></LocaleProvider>);
`;
const bundle = await build({
  stdin: { contents: source, loader: "tsx", resolveDir: root },
  absWorkingDir: root,
  bundle: true,
  write: false,
  format: "iife",
  platform: "browser",
  define: { "process.env.NODE_ENV": '"production"' },
  tsconfig: root + "tsconfig.json",
});
const css = await postcss([tailwind({ base: root })]).process(
  await readFile(root + "app/globals.css", "utf8"),
  { from: root + "app/globals.css" },
);
const server = createServer((req, res) => {
  if (req.url === "/app.css") {
    res.setHeader("Content-Type", "text/css");
    res.end(css.css);
    return;
  }
  res.setHeader(
    "Content-Type",
    req.url === "/app.js" ? "text/javascript" : "text/html",
  );
  res.end(
    req.url === "/app.js"
      ? bundle.outputFiles[0].text
      : '<!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="/app.css"></head><body><div id="root"></div><script src="/app.js"></script></body></html>',
  );
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome",
    headless: true,
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() =>
    localStorage.setItem("cc_switch_router_locale_v1", "en"),
  );
  await page.route("**/v1/**", (route) =>
    route.fulfill({ json: { mode: "router" } }),
  );
  await page.goto(`http://127.0.0.1:${server.address().port}`);

  const search = page.getByRole("switch", { name: "Web search" });
  await search.waitFor();
  await search.focus();
  await page.keyboard.press("Space");
  assert.equal(await search.isChecked(), true);
  page.once("dialog", dialog => dialog.dismiss());
  await page.getByRole("button", { name: "Save", exact: true }).click();
  assert.equal(await page.evaluate(() => window.saved), undefined);
  page.once("dialog", dialog => dialog.accept());
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page.waitForFunction(() => window.saved);
  assert.deepEqual(await page.evaluate(() => window.saved.grantCapabilityEdits), {
    "a@example.com": {expectedRevision:2,operations:["tool.web_search"]},
  });
  assert.equal(await page.evaluate(() => window.saved.userGrantEdits), undefined);
  assert.deepEqual(await page.evaluate(() => window.saved.capabilityConsent.acceptUncovered), ["gap:a@example.com"]);

  await page.evaluate(() => window.loadScenario("empty"));
  await page.getByText("Loading available operations…").waitFor({ state: "hidden" });
  assert.equal(await page.getByText("Media generation and tools", { exact: true }).count(), 0);
  await page.evaluate(() => window.loadScenario("offline"));
  await page.getByRole("button", { name: "Retry" }).waitFor();
  await page.evaluate(() => window.loadScenario("changed"));
  const reconfirm = page.getByRole("button", { name: "Entry points changed. Confirm to allow." });
  await reconfirm.waitFor();
  await reconfirm.click();
  assert.equal(await reconfirm.count(), 0);
  page.once("dialog", dialog => dialog.accept());
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page.waitForFunction(() => window.saved.grantCapabilityEdits["a@example.com"].reconfirm?.length);
  await page.evaluate(() => window.loadScenario("gone"));
  await page.getByText("Unavailable. You can remove permission.").waitFor();
  await search.focus();
  await search.press("Space");
  assert.equal(await search.isDisabled(), true);
  await page.evaluate(() => window.loadScenario("offlineSaved"));
  await page.getByRole("button", { name: "Retry" }).waitFor();
  assert.equal(await search.isEnabled(), true);
  await search.focus();
  await search.press("Space");
  await search.waitFor({ state: "detached" });

  await page.getByText("Usage and USD quota", {exact:true}).click();
  await page.getByText("Estimate only. No USD quota debit.", {exact:false}).waitFor();
  await page.getByText("Debited from USD quota $0.000000000001", {exact:false}).waitFor();
  assert.equal(await page.getByText("Debited from USD quota $0.00", {exact:false}).count(), 1);
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({width,height:750});
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  }
  assert.deepEqual(errors, []);
  console.log("PASS individual media and tool grants, retry, new bindings, stale removal, and request metering");
} finally {
  await browser?.close();
  await new Promise(resolve => server.close(resolve));
}
