import assert from "node:assert/strict";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright";

// Exercises current source, not a potentially stale static export. No screenshots,
// external calls, or real quota mutations.
const root = fileURLToPath(new URL("../", import.meta.url));
const source = `
import React from "react";
import { createRoot } from "react-dom/client";
import { LocaleProvider, useLocaleText } from "@/components/i18n/locale-provider";
import { useShareEditForm } from "@/components/dashboard/share-edit/share-edit-form";
import { ShareUserGrantsEditor } from "@/components/dashboard/share-edit/share-user-grants-editor";
import { ShareEditDialog } from "@/components/dashboard/share-edit-dialog";
const grant = (email, tokens, revision) => ({
  email, role: email.startsWith("owner") ? "owner" : "shareto", active: true, revision,
  policy: { tokenPeriod: "lifetime", parallelLimit: 2, tokenLimit: 10000000 },
  usageQuota: { period: "lifetime", effectiveTokensUsed: tokens, observedTokensUsed: tokens,
    manualOffsetTokens: 0, observedRequestsCount: 1, rebaseApplies: false },
});
const initial = {
  shareId: "share-1", configRevision: 5, capacityPoolId: "pool-1", shareName: "test",
  ownerEmail: "owner@example.com", freeAccess: false, subdomain: "test--client",
  appType: "codex", bindings: { codex: "provider" }, support: { claude: false, codex: true, gemini: false },
  tokenLimit: -1, parallelLimit: -1, expiresAt: "2099-12-31T23:59:59Z",
  supportedUserTokenPeriods: ["lifetime"], canEditSettings: true, canManage: true,
  userGrants: { "owner@example.com": grant("owner@example.com", 0, 1),
    "a@example.com": grant("a@example.com", 1000000, 3), "b@example.com": grant("b@example.com", 2000000, 4) },
};
window.closedCount = 0; window.savedCount = 0;
function Harness() {
  const { t } = useLocaleText();
  const [share, setShare] = React.useState(initial);
  const [full, setFull] = React.useState(false);
  const onClose = React.useCallback(() => { window.closedCount++; setShare(null); }, []);
  const onSaved = React.useCallback(async result => { if (result.appliedSynchronously) window.savedCount++; }, []);
  const form = useShareEditForm({ share, t, onClose, onSaved });
  window.form = form;
  window.reopen = () => setShare(structuredClone(initial));
  window.remote = () => setShare(current => ({ ...current, configRevision: 6, userGrants: {
    ...current.userGrants, "b@example.com": { ...current.userGrants["b@example.com"], revision: 5, policy: { tokenPeriod: "lifetime", parallelLimit: 9 } },
  }}));
  window.traffic = () => setShare(current => ({ ...current, userGrants: { ...current.userGrants,
    "b@example.com": { ...current.userGrants["b@example.com"], usageQuota: { ...current.userGrants["b@example.com"].usageQuota, observedTokensUsed: 3000000 } },
  }}));
  window.replaceSession = () => setShare(current => ({ ...initial, shareId: "share-2" }));
  window.fullDialog = () => { setShare(structuredClone(initial)); setFull(true); };
  if (full) return <ShareEditDialog share={share} onClose={onClose} onSaved={onSaved} />;
  if (!share || !form) return <div>Closed</div>;
  return <div data-editor>
    <ShareUserGrantsEditor value={form.draft.userGrants} ownerEmail={share.ownerEmail}
      defaultPolicy={{ tokenPeriod: "lifetime" }} t={t} disabled={form.busy}
      usageEdits={form.draft.userUsageEdits}
      onUsageEditsChange={userUsageEdits => form.onDraftChange(current => ({ ...current, userUsageEdits }))}
      onChange={userGrants => form.onDraftChange(current => ({ ...current, userGrants }))} />
    <button onClick={form.save} disabled={form.busy || form.locked || !form.isDirty}>Persist draft</button>
    <p data-error>{form.error}</p><p data-notice>{form.notice}</p>
  </div>;
}
createRoot(document.getElementById("root")).render(<React.StrictMode><LocaleProvider><Harness /></LocaleProvider></React.StrictMode>);
`;
const bundle = await build({ stdin: { contents: source, loader: "tsx", resolveDir: root },
  absWorkingDir: root, bundle: true, write: false, format: "iife", platform: "browser",
  define: { "process.env.NODE_ENV": '"production"' }, tsconfig: root + "tsconfig.json" });
const server = createServer((request, response) => {
  response.setHeader("Content-Type", request.url === "/app.js" ? "text/javascript" : "text/html");
  response.end(request.url === "/app.js" ? bundle.outputFiles[0].text
    : '<html><body><div id="root"></div><script src="/app.js"></script></body></html>');
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", error => { pageErrors.push(error.message); console.error(error.message); });
  await page.clock.install();
  await page.addInitScript(() => {
    localStorage.setItem("cc_switch_router_locale_v1", "en");
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({ accessToken: "test", expiresAt: "2099-01-01T00:00:00Z" }));
  });
  const writes = [];
  let mode = "applied";
  let status = "pending";
  let release;
  await page.route("**/v1/**", async route => {
    const request = route.request();
    if (request.method() === "PATCH") {
      writes.push(request.postDataJSON());
      if (mode === "held") await new Promise(resolve => { release = resolve; });
      return route.fulfill({ status: mode === "conflict" ? 409 : 200, json: mode === "conflict"
        ? { message: "share changed; reload the editor" }
        : { ok: true, appliedSynchronously: mode !== "queued", edit: { id: "edit-1", status: mode === "queued" ? "pending" : "applied" } } });
    }
    if (request.url().includes("/v1/auth/home")) return route.fulfill({ json: { hasActivity: true, entry: null, source: null } });
    if (request.url().includes("/settings-edits/")) return route.fulfill({ json: { id: "edit-1", status, errorMessage: "revision conflict" } });
    if (request.url().endsWith("/requested-model-blocks")) return route.fulfill({ json: { shareId: "share-1", revision: 1, blockedModelsByApp: {} } });
    return route.fulfill({ json: { rows: [], usdCnyRateMicros: 7000000, fxRateFresh: true } });
  });
  await page.goto("http://127.0.0.1:" + server.address().port);
  await page.getByRole("button", { name: "Batch edit", exact: true }).click();
  for (const email of ["a@example.com", "b@example.com"]) {
    await page.getByRole("checkbox", { name: "Select " + email, exact: true }).evaluate(element => element.click());
  }
  await page.getByRole("button", { name: "Edit selected (2)", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const assertBatchDefaults = async (batch) => {
    const checkboxes = batch.getByRole("checkbox");
    assert.equal(await checkboxes.count(), 4);
    for (const checkbox of await checkboxes.all()) {
      assert.equal(await checkbox.isChecked(), false);
    }
    for (const input of await batch.locator('input:not([type="checkbox"]):not([type="hidden"])').all()) {
      assert.equal(await input.isDisabled(), true);
    }
    assert.equal(await batch.getByRole("button", { name: "Apply to 2 users", exact: true }).isDisabled(), true);
  };
  await assertBatchDefaults(dialog);
  for (const checkbox of await dialog.getByRole("checkbox").all()) {
    await checkbox.evaluate(element => element.click());
    assert.equal(await checkbox.isChecked(), true);
  }
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal(await page.evaluate(() => window.form.isDirty), false);
  await page.getByRole("button", { name: "Edit selected (2)", exact: true }).click();
  await assertBatchDefaults(dialog);
  await dialog.getByRole("checkbox", { name: "Parallel limit", exact: true }).evaluate(element => element.click());
  await dialog.locator('input[type="number"]').fill("3");
  await dialog.getByRole("button", { name: "Apply to 2 users", exact: true }).click();
  assert.equal(await dialog.count(), 0);
  assert.deepEqual(await page.evaluate(() => window.form.draft.userUsageEdits), {});
  const appliedDraft = await page.evaluate(() => window.form.draft);
  await page.getByRole("button", { name: "Batch edit", exact: true }).click();
  for (const email of ["a@example.com", "b@example.com"]) {
    await page.getByRole("checkbox", { name: "Select " + email, exact: true }).evaluate(element => element.click());
  }
  await page.getByRole("button", { name: "Edit selected (2)", exact: true }).click();
  await assertBatchDefaults(dialog);
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.form.draft), appliedDraft);
  console.log("PASS batch options default unchecked on first open and after cancel/apply; empty selection cannot apply");
  await page.evaluate(() => window.traffic());
  assert.equal(await page.evaluate(() => window.form.remoteRefreshPending), false);
  await page.getByRole("button", { name: "Persist draft" }).click();
  await page.waitForFunction(() => window.closedCount === 1);
  assert.equal(writes.length, 1);
  assert.equal(writes[0].baseConfigRevision, 5);
  assert.deepEqual(Object.keys(writes[0].patch), ["userGrantEdits"]);
  assert.deepEqual(Object.keys(writes[0].patch.userGrantEdits), ["a@example.com", "b@example.com"]);
  console.log("PASS batch opt-in, unequal usage, sparse edits, traffic does not create config conflict");

  const reopenAndEdit = async () => {
    await page.evaluate(() => window.reopen());
    await page.waitForFunction(() => !!window.form && !window.form.isDirty);
    await page.evaluate(() => window.form.onDescriptionChange("Changed"));
    await page.waitForFunction(() => window.form.isDirty);
  };
  await reopenAndEdit();
  await page.evaluate(() => window.remote());
  await page.waitForFunction(() => window.form.remoteRefreshPending);
  mode = "conflict";
  await page.getByRole("button", { name: "Persist draft" }).click();
  await page.waitForFunction(() => !!window.form.error);
  assert.equal(writes.at(-1).baseConfigRevision, 5);
  assert.deepEqual(writes.at(-1).patch, { description: "Changed" });
  assert.equal(await page.evaluate(() => window.form.draft.description), "Changed");
  assert.equal(await page.evaluate(() => window.closedCount), 1);
  console.log("PASS frozen baseline and preserved draft on conflict");

  await page.evaluate(() => window.form.resetDraft());
  mode = "queued"; status = "pending";
  await page.evaluate(() => window.form.onDescriptionChange("Queued"));
  await page.evaluate(() => { void window.form.save(); void window.form.save(); });
  await page.waitForFunction(() => !window.form.busy && !!window.form.notice);
  const count = writes.length;
  await page.evaluate(() => window.form.save());
  assert.equal(writes.length, count);
  status = "applied";
  await page.clock.runFor(2500);
  await page.waitForFunction(() => window.closedCount === 2);
  await page.clock.runFor(5000);
  assert.equal(await page.evaluate(() => window.closedCount), 2);
  assert.equal(await page.evaluate(() => window.savedCount), 2);
  console.log("PASS duplicate-submit guard, exact queued receipt, single completion/close");

  await reopenAndEdit();
  mode = "held";
  await page.evaluate(() => { void window.form.save(); });
  await page.waitForFunction(() => window.form.busy);
  await page.waitForTimeout(100);
  assert.equal(typeof release, "function");
  await page.evaluate(() => window.replaceSession());
  await page.waitForFunction(() => window.form?.liveShare.shareId === "share-2");
  release();
  await page.clock.runFor(1000);
  assert.equal(await page.evaluate(() => window.closedCount), 2);
  assert.equal(await page.evaluate(() => window.form.liveShare.shareId), "share-2");
  console.log("PASS late old-session response never closes or overwrites a new editor");

  await reopenAndEdit();
  mode = "queued"; status = "pending";
  await page.getByRole("button", { name: "Persist draft" }).click();
  await page.waitForFunction(() => !window.form.busy && !!window.form.notice);
  status = "rejected";
  await page.clock.runFor(2500);
  await page.waitForFunction(() => window.form.error === "revision conflict");
  assert.equal(await page.evaluate(() => window.form.draft.description), "Changed");
  assert.equal(await page.evaluate(() => window.form.isDirty), true);
  assert.equal(await page.evaluate(() => window.closedCount), 2);
  console.log("PASS queued rejection preserves the editable draft and never closes the dialog");

  mode = "applied";
  await page.evaluate(() => window.fullDialog());
  await page.getByRole("dialog").waitFor();
  await page.clock.runFor(1000);
  await page.getByRole("button", { name: "Batch edit", exact: true }).click({ timeout: 5000 }).catch(async error => {
    console.error(await page.locator("body").innerText());
    throw error;
  });
  for (const email of ["a@example.com", "b@example.com"]) {
    await page.getByRole("checkbox", { name: "Select " + email, exact: true }).evaluate(element => element.click());
  }
  await page.getByRole("button", { name: "Edit selected (2)", exact: true }).click();
  const batchDialog = page.getByRole("dialog", { name: "Batch edit user limits" });
  await assertBatchDefaults(batchDialog);
  await batchDialog.getByRole("checkbox", { name: "Parallel limit", exact: true }).evaluate(element => element.click());
  await batchDialog.locator('input[type="number"]').fill("4");
  await batchDialog.getByRole("button", { name: "Apply to 2 users", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll('[role="dialog"]').length === 1);
  const beforeSave = writes.length;
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page.waitForFunction(() => window.closedCount === 3);
  await page.clock.runFor(5000);
  assert.equal(writes.length, beforeSave + 1);
  assert.equal(await page.getByRole("dialog").count(), 0);
  assert.equal(await page.evaluate(() => window.closedCount), 3);
  console.log("PASS actual nested Share dialogs apply locally and close once on persistence");
  assert.deepEqual(pageErrors, []);
} finally {
  await browser?.close();
  await new Promise(resolve => server.close(resolve));
}
