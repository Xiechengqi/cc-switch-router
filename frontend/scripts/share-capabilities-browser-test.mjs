import assert from "node:assert/strict";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright";

// Real component interaction, with local protocol fixtures; never calls an upstream.
const root = fileURLToPath(new URL("../", import.meta.url));
const source = `
import React from "react";
import {createRoot} from "react-dom/client";
import {LocaleProvider} from "@/components/i18n/locale-provider";
import {ShareCapabilitiesEditor} from "@/components/dashboard/share-edit/share-capabilities-editor";
import {emptyCapabilityPolicy,capabilityQuotaContext,SHARE_OPERATIONS} from "@/lib/share-capabilities";
const op="tool.web_search";
const fingerprint="a".repeat(64);
const initial={"a@example.com":{email:"a@example.com",role:"shareto",active:true,policy:{tokenPeriod:"day",tokenLimit:100000000}},"market@example.com":{email:"market@example.com",role:"shareto",active:true,manager:"routerShareMarket",policy:{tokenPeriod:"day",allowedOperations:[]}}};
function preview(shareId,input){
  window.lastPreview=input;
  const selected=input.policy.allowedOperations.includes(op);
  const bindingSelected=selected&&(!input.policy.operationBindings[op]||input.policy.operationBindings[op].some(b=>b.fingerprint===fingerprint));
  const policy={...input.policy,operationBindings:selected?{[op]:bindingSelected?[{app:"codex",fingerprint}]:[]}: {}};
  const scopes=Object.entries(input.policies).filter(([,p])=>!p.allowedOperations||p.allowedOperations.includes(op)).map(([scope,p])=>({scope,quotaUnit:p.valueQuota?"usdEquivalent":"tokens",coverage:p.valueQuota?"unknown":"partial",reason:p.valueQuota?"priced_requests_only":"resource_not_in_token_ledger",condition:p.valueQuota?undefined:"gap:"+scope}));
  const required=selected?scopes.filter(s=>s.condition).map(s=>({condition:s.condition,scope:s.scope,operation:op,quotaUnit:s.quotaUnit,bindingFingerprint:fingerprint,actor:"",acceptedAtMs:0})):[];
  return Promise.resolve({shareId,configRevision:1,ready:true,legacy:false,policy,expiresAt:Date.now()+900000,previewCredential:"ticket",requiredAcknowledgements:required,blockers:[],rows:SHARE_OPERATIONS.map(operation=>({app:"codex",operation,bindingFingerprint:fingerprint,controllable:operation===op,selected:operation===op&&bindingSelected,execution:"unknown",measurement:"estimated",nativeUnit:operation,pricing:"missing",admission:"estimated_reservation",scopes:operation===op?scopes:[]}))});
}
function Harness(){
  const [policy,setPolicy]=React.useState(emptyCapabilityPolicy);const [grants,setGrants]=React.useState(initial);const [consent,setConsent]=React.useState();const [scope,setScope]=React.useState();
  window.latest={policy,grants,consent};
  const valid=!!consent&&scope===capabilityQuotaContext(policy,grants,-1);
  return <><ShareCapabilitiesEditor shareId="share" revision={1} policy={policy} savedPolicy={emptyCapabilityPolicy()} grants={grants} tokenLimit={-1} previewCapabilities={preview} onChange={(p,c)=>{setPolicy(p);setConsent(c);setScope(c?capabilityQuotaContext(p,grants,-1):undefined);}} onGrantOperationsChange={(email,allowedOperations)=>{setGrants(current=>({...current,[email]:{...current[email],policy:{...current[email].policy,allowedOperations}}}));setConsent(undefined);}}/><button id="unit" onClick={()=>setGrants(current=>({...current,"a@example.com":{...current["a@example.com"],policy:{tokenPeriod:"day",valueQuota:{limitUsdMicros:"100000000",tariffPolicyId:"api-equivalent-v2",unpricedUsage:"reject"}}}}))}>Switch to USD</button><output id="valid">{String(valid)}</output></>;
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
const server = createServer((req, res) => {
  res.setHeader(
    "Content-Type",
    req.url === "/app.js" ? "text/javascript" : "text/html",
  );
  res.end(
    req.url === "/app.js"
      ? bundle.outputFiles[0].text
      : '<html><body><div id="root"></div><script src="/app.js"></script></body></html>',
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
  const search = page.getByLabel("Web search", { exact: true });
  await page.waitForFunction(() =>
    document.querySelector('section input[type="checkbox"]:not(:disabled)'),
  );
  assert.equal(await search.isChecked(), false, "new policy starts disabled");
  await search.check();
  const confirm = page.getByRole("button", {
    name: "Confirm capability configuration",
    exact: true,
  });
  const accept = page
    .locator("label")
    .filter({ hasText: "a@example.com · Web search" })
    .locator("input");
  await accept.waitFor();
  assert.equal(await confirm.isDisabled(), true);
  await accept.check();
  await confirm.click();
  await page.waitForFunction(
    () => document.querySelector("#valid").textContent === "true",
  );
  assert.deepEqual(
    await page.evaluate(() => window.latest.consent.acceptUncovered),
    ["gap:a@example.com"],
  );
  await page.locator("#unit").click();
  await page.waitForFunction(
    () => document.querySelector("#valid").textContent === "false",
  );
  await page.waitForFunction(
    () =>
      window.lastPreview.policies["a@example.com"].valueQuota?.unpricedUsage ===
      "reject",
  );
  await confirm.click();
  await page.waitForFunction(
    () => document.querySelector("#valid").textContent === "true",
  );
  assert.deepEqual(
    await page.evaluate(() => window.latest.consent.acceptUncovered),
    [],
    "strict USD does not waive missing prices",
  );
  await page.getByText("User capability scopes", { exact: true }).click();
  assert.equal(
    await page.getByLabel("market@example.com capabilities").count(),
    0,
    "market contracts stay read-only",
  );
  await page
    .getByLabel("a@example.com capabilities", { exact: true })
    .selectOption("explicit");
  await page.waitForFunction(() =>
    Array.isArray(
      window.latest.grants["a@example.com"].policy.allowedOperations,
    ),
  );
  assert.deepEqual(
    await page.evaluate(
      () => window.latest.grants["a@example.com"].policy.allowedOperations,
    ),
    [],
  );
  assert.equal(await page.locator("#valid").textContent(), "false");
  assert.deepEqual(errors, []);
  console.log(
    "Share capabilities browser: defaults, scoped acceptance, unit change, explicit empty grant and Market protection passed",
  );
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
