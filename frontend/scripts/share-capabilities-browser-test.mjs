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
import {createRoot} from "react-dom/client";
import {LocaleProvider} from "@/components/i18n/locale-provider";
import {ShareCapabilitiesEditor} from "@/components/dashboard/share-edit/share-capabilities-editor";
import {ShareCapabilityPermissionFields} from "@/components/dashboard/share-edit/share-capability-permission-fields";
import {emptyCapabilityPolicy,capabilityQuotaContext,SHARE_OPERATIONS,reconcileCapabilityGrants} from "@/lib/share-capabilities";
const op="tool.web_search",fingerprint="a".repeat(64),savedPolicy=emptyCapabilityPolicy();
const initial=Object.fromEntries(["a@example.com","b@example.com","market@example.com"].map(email=>[email,{email,role:"shareto",active:true,...(email.startsWith("market")?{manager:"routerShareMarket"}:{}),policy:{tokenPeriod:"day",tokenLimit:100000000,...(email.startsWith("market")?{allowedOperations:[]}: {})}}]));
function preview(shareId,input){
  window.lastPreview=input;window.previewCalls=(window.previewCalls||0)+1;
  if(window.scenario==="offline")return Promise.reject(new Error("fixture-offline"));
  const selected=input.policy.allowedOperations.includes(op);
  const bindingSelected=selected&&(!input.policy.operationBindings[op]||input.policy.operationBindings[op].some(b=>b.fingerprint===fingerprint));
  const policy={...input.policy,operationBindings:Object.fromEntries(input.policy.allowedOperations.map(operation=>[operation,operation===op&&bindingSelected?[{app:"codex",fingerprint}]:[]]))};
  const scopes=Object.entries(input.policies).filter(([,p])=>!p.allowedOperations||p.allowedOperations.includes(op)).map(([scope,p])=>({scope,quotaUnit:p.valueQuota?"usdEquivalent":"tokens",coverage:p.valueQuota?"unknown":"partial",reason:p.valueQuota?"priced_requests_only":"resource_not_in_token_ledger",condition:p.valueQuota?undefined:"gap:"+scope}));
  const required=bindingSelected?scopes.filter(s=>s.condition).map(s=>({condition:s.condition,scope:s.scope,operation:op,quotaUnit:s.quotaUnit,bindingFingerprint:fingerprint,actor:"",acceptedAtMs:0})):[];
  const result={shareId,configRevision:input.expectedConfigRevision,displayVersion:2,ready:true,legacy:false,policy,expiresAt:Date.now()+900000,previewCredential:"ticket",requiredAcknowledgements:required,blockers:[],rows:SHARE_OPERATIONS.map(operation=>({app:"codex",operation,bindingFingerprint:fingerprint,controllable:operation===op,implemented:operation===op||operation==="image.generate",providerEnabled:operation!=="image.generate",accountSupport:"unknown",unavailableReason:operation===op?null:operation==="image.generate"?"provider_disabled":"driver_not_implemented",modelScope:"request",pricingCoverage:"request_dependent",selected:operation===op&&bindingSelected,execution:"unknown",measurement:"estimated",nativeUnit:operation,pricing:"missing",admission:"estimated_reservation",scopes:operation===op?scopes:[]}))};
  if(["empty","binding-failure","blocked"].includes(window.scenario)){result.rows=[];result.requiredAcknowledgements=[];}
  if(window.scenario==="binding-failure")result.unavailableBindings=[{app:"codex",reason:"runtime_unavailable"}];
  if(window.scenario==="blocked")result.blockers=[{scope:"owner",blocker:"MARKET_CAPABILITY_CONFLICT"}];
  if(window.scenario==="setup")result.rows=result.rows.filter(row=>row.operation==="image.generate");
  return Promise.resolve(result);
}
function Harness(){
  const ref=React.useRef();const [policy,setPolicy]=React.useState(emptyCapabilityPolicy);const [grants,setGrants]=React.useState(initial);const [consent,setConsent]=React.useState();const [scope,setScope]=React.useState();
  const [revision,setRevision]=React.useState(1),[baseline,setBaseline]=React.useState(savedPolicy);
  window.configureScenario=(name,p=emptyCapabilityPolicy(),saved=emptyCapabilityPolicy())=>{window.scenario=name;setPolicy(p);setBaseline(saved);setRevision(v=>v+1);};
  window.latest={policy,grants,consent};
  const valid=!!consent&&scope===capabilityQuotaContext(policy,grants,-1);
  return <main className="mx-auto grid max-w-xl gap-4 p-4"><ShareCapabilitiesEditor shareId="share" revision={revision} policy={policy} savedPolicy={baseline} grants={grants} tokenLimit={-1} previewCapabilities={preview} reviewRef={ref} onChange={p=>{setPolicy(p);setGrants(g=>reconcileCapabilityGrants(p,g));}}/>
    <section className="rounded-xl border p-4"><h3 className="mb-3">a@example.com</h3><ShareCapabilityPermissionFields available={policy.allowedOperations} value={grants["a@example.com"].policy.allowedOperations} onChange={allowedOperations=>setGrants(g=>({...g,"a@example.com":{...g["a@example.com"],policy:{...g["a@example.com"].policy,allowedOperations}}}))}/></section>
    <button id="save" onClick={async()=>{const approval=await ref.current.review();if(approval){setPolicy(approval.policy);setConsent(approval.consent);setScope(capabilityQuotaContext(approval.policy,grants,-1));window.saved=(window.saved||0)+1;}}}>Save settings</button>
    <button id="unit" onClick={()=>setGrants(current=>Object.fromEntries(Object.entries(current).map(([email,g])=>[email,g.manager?g:{...g,policy:{tokenPeriod:"day",valueQuota:{limitUsdMicros:"100000000",tariffPolicyId:"api-equivalent-v2",unpricedUsage:"reject"}}}]))) }>Switch to USD</button>
    <button id="retired" onClick={()=>setPolicy(p=>({...p,allowedOperations:[...p.allowedOperations,"video.generate"],operationBindings:{...p.operationBindings,"video.generate":[{app:"codex",fingerprint:"old"}]}}))}>Restore unavailable selection</button>
    <output id="valid">{String(valid)}</output></main>;
}
createRoot(document.getElementById("root")).render(<LocaleProvider><Harness/></LocaleProvider>);
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
  const search = page.getByRole("checkbox", {
    name: /Web search.*Verification pending/,
  });
  await search.waitFor();
  await page.waitForFunction(() =>
    document.querySelector('section input[type="checkbox"]:not(:disabled)'),
  );
  const info = page.getByRole("button", {
    name: "About extensions",
    exact: true,
  });
  const tooltip = page.getByRole("tooltip");
  await info.hover();
  await tooltip.waitFor({ state: "visible" });
  await page.mouse.move(0, 0);
  await tooltip.waitFor({ state: "hidden" });
  await info.hover();
  await tooltip.waitFor({ state: "visible" });
  await info.click();
  await page.mouse.move(0, 0);
  assert.equal(await tooltip.isVisible(), true, "click pins the hover preview");
  await page.keyboard.press("Escape");
  await tooltip.waitFor({ state: "hidden" });
  await page.keyboard.press("Tab");
  await page.keyboard.press("Shift+Tab");
  await tooltip.waitFor({ state: "visible" });
  await page.keyboard.press("Tab");
  await tooltip.waitFor({ state: "hidden" });
  await page.keyboard.press("Shift+Tab");
  await tooltip.waitFor({ state: "visible" });
  await page.keyboard.press("Escape");
  await info.click();
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForFunction(() => {
      const hint = document.querySelector('[role="tooltip"]');
      const box = hint.getBoundingClientRect();
      return (
        box.left >= 0 &&
        box.right <= innerWidth &&
        hint.scrollWidth <= hint.clientWidth
      );
    });
  }
  await page.mouse.click(4, 4);
  assert.equal(await search.isChecked(), false);
  assert.equal(
    await page.getByRole("checkbox").count(),
    1,
    "unsupported operations are hidden",
  );
  assert.equal(
    await page.getByText("Setup instructions", { exact: true }).count(),
    1,
  );
  await search.check();
  await page
    .getByText("Resources do not debit Token quotas", { exact: true })
    .waitFor();
  assert.equal(
    await page.getByRole("dialog").count(),
    0,
    "editing alone does not prompt",
  );
  await page.locator("#save").click();
  const dialog = page.getByRole("dialog");
  await dialog.waitFor();
  const confirm = page.getByRole("button", {
    name: "Confirm and save",
    exact: true,
  });
  assert.equal(await confirm.isDisabled(), true);
  assert.equal(
    await dialog.getByRole("checkbox").count(),
    1,
    "same risks are grouped across users",
  );
  await dialog.getByText("Affected users and access", { exact: true }).click();
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForFunction(() => {
      const dialog = document.querySelector('[role="dialog"]');
      if (!dialog) return false;
      const bounds = dialog.getBoundingClientRect();
      return (
        bounds.left >= 0 &&
        bounds.right <= innerWidth &&
        bounds.top >= 0 &&
        bounds.bottom <= innerHeight &&
        dialog.scrollWidth <= dialog.clientWidth
      );
    });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      true,
      `dialog fits ${width}px`,
    );
  }
  await page.setViewportSize({ width: 375, height: 900 });
  await page.screenshot({
    path: "/tmp/cc-capability-router-review-mobile.png",
    fullPage: true,
  });
  await dialog.getByRole("checkbox").check();
  await confirm.click();
  await page.waitForFunction(() => window.saved === 1);
  assert.deepEqual(
    await page.evaluate(() => window.latest.consent.acceptUncovered),
    ["gap:a@example.com", "gap:b@example.com"],
  );
  await page.locator("#unit").click();
  await page.waitForFunction(
    () => document.querySelector("#valid").textContent === "false",
  );
  await page.locator("#save").click();
  await page.waitForFunction(() => window.saved === 2);
  assert.deepEqual(
    await page.evaluate(() => window.latest.consent.acceptUncovered),
    [],
    "strict USD cannot waive missing prices",
  );
  await page.getByLabel("Extension permission").selectOption("custom");
  assert.deepEqual(
    await page.evaluate(
      () => window.latest.grants["a@example.com"].policy.allowedOperations,
    ),
    ["tool.web_search"],
    "custom mode retains current Share choices",
  );
  await page.getByLabel("Extension permission").selectOption("disabled");
  assert.deepEqual(
    await page.evaluate(
      () => window.latest.grants["a@example.com"].policy.allowedOperations,
    ),
    [],
  );
  assert.deepEqual(
    await page.evaluate(
      () => window.latest.grants["market@example.com"].policy.allowedOperations,
    ),
    [],
    "Market contracts remain immutable",
  );
  await page.locator("#retired").click();
  const retired = page.getByRole("checkbox", {
    name: /Video generation.*Unavailable/,
  });
  await retired.waitFor();
  await retired.click();
  assert.equal(
    await retired.count(),
    0,
    "unavailable saved operations can be removed",
  );
  await page.waitForFunction(() =>
    document.querySelector('section input[type="checkbox"]:not(:disabled)'),
  );
  for (const width of [320, 375, 414, 768, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      true,
      `editor fits ${width}px`,
    );
  }
  await page.screenshot({
    path: "/tmp/cc-capability-router-desktop.png",
    fullPage: true,
  });
  await page.evaluate(() => window.configureScenario("empty"));
  await page.waitForFunction(
    () => !document.querySelector("[data-capability-view]"),
  );
  const calls = await page.evaluate(() => window.previewCalls);
  const saved = await page.evaluate(() => window.saved);
  await page.locator("#save").click();
  await page.waitForFunction((count) => window.saved === count + 1, saved);
  assert.equal(
    await page.evaluate(() => window.previewCalls),
    calls + 1,
    "hidden card still performs a fresh review",
  );
  assert.deepEqual(
    await page.evaluate(() => window.latest.policy.allowedOperations),
    [],
  );
  await page.evaluate(() => window.configureScenario("offline"));
  await page.getByText("Extensions unavailable", { exact: true }).waitFor();
  assert.equal(
    await page.locator('[data-capability-view="compact"]').count(),
    1,
  );
  assert.equal(
    await page
      .getByText("The current bindings have no configurable extensions.", {
        exact: true,
      })
      .count(),
    0,
  );
  await info.click();
  await page
    .getByRole("tooltip")
    .getByText("fixture-offline", { exact: true })
    .waitFor();
  await page.mouse.click(4, 4);
  await page.evaluate(() => {
    window.scenario = "empty";
  });
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("[data-capability-view]"),
  );
  await page.evaluate(() => window.configureScenario("binding-failure"));
  await page.getByText("Extensions unavailable", { exact: true }).waitFor();
  await page.evaluate(() => window.configureScenario("blocked"));
  await page
    .getByText("Extension changes cannot be saved yet.", { exact: true })
    .waitFor();
  await info.click();
  await tooltip
    .getByText(/An existing purchased plan prevents these changes/)
    .waitFor();
  await page.mouse.click(4, 4);
  await page.evaluate(() => window.configureScenario("setup"));
  await page.getByText("Extensions need setup", { exact: true }).waitFor();
  await page.getByText("Setup instructions", { exact: true }).click();
  await page
    .getByText(/Image generation.*Enable in Provider settings first/)
    .waitFor();
  const retiredPolicy = {
    version: 1,
    allowedOperations: ["video.generate"],
    operationBindings: {},
  };
  await page.evaluate(
    (policy) => window.configureScenario("empty", policy, policy),
    retiredPolicy,
  );
  await retired.waitFor();
  await retired.click();
  await page
    .getByText("Extension changes await saving.", { exact: true })
    .waitFor();
  assert.equal(
    await page.locator('[data-capability-view="full"]').count(),
    1,
    "last removal is retained until save",
  );
  const beforeRemovalSave = await page.evaluate(() => window.saved);
  await page.locator("#save").click();
  await page.waitForFunction(
    (count) => window.saved === count + 1,
    beforeRemovalSave,
  );
  assert.deepEqual(
    await page.evaluate(() => window.latest.policy.allowedOperations),
    [],
  );
  await page.evaluate(() => window.configureScenario("available"));
  await search.waitFor();
  await page.waitForFunction(() =>
    document.querySelector('section input[type="checkbox"]:not(:disabled)'),
  );
  assert.equal(
    await search.isEnabled(),
    true,
    "capabilities return when bindings change",
  );

  const touchPage = await browser.newPage({
    hasTouch: true,
    isMobile: true,
    viewport: { width: 375, height: 900 },
  });
  await touchPage.addInitScript(() =>
    localStorage.setItem("cc_switch_router_locale_v1", "en"),
  );
  await touchPage.goto(`http://127.0.0.1:${server.address().port}`);
  await touchPage
    .getByRole("button", { name: "About extensions", exact: true })
    .tap();
  await touchPage.getByRole("tooltip").waitFor({ state: "visible" });
  await touchPage.touchscreen.tap(4, 4);
  await touchPage.getByRole("tooltip").waitFor({ state: "hidden" });
  await touchPage.close();
  assert.deepEqual(errors, []);
  console.log(
    "Share capabilities browser: empty/failed/setup states, retained hidden review and removals, hover/focus/tap hints, risk visibility, grouped review, USD enforcement, user modes and responsive layouts passed",
  );
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
