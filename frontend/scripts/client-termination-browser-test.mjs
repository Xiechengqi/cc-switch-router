import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Production export and local mocks only: never contacts a Host or moves money.
const output = fileURLToPath(new URL("../out/", import.meta.url));
const server = createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const path = resolve(output, `.${pathname.endsWith("/") ? `${pathname}index.html` : pathname}`);
    if (!path.startsWith(resolve(output) + sep)) return response.writeHead(403).end();
    const body = await readFile(path);
    response.writeHead(200, { "Content-Type": ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".txt": "text/plain", ".svg": "image/svg+xml", ".woff2": "font/woff2" })[extname(path)] || "application/octet-stream" });
    response.end(body);
  } catch { response.writeHead(404).end(); }
});
await new Promise((ready, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", ready); });
const base = `http://127.0.0.1:${server.address().port}`;
const iso = () => new Date().toISOString();
const initial = (patch = {}) => ({
  id: "old-termination", installationId: "old-installation", hostId: "host-one", contractId: "old-contract", periodId: "old-period",
  buyerUserId: "buyer", providerUserId: "provider", buyerEmail: "buyer@example.test", providerEmail: "provider@example.test",
  policyVersion: 1, rateBps: 1000, periodAmountUnits: "432000", remainingUnits: "388800", compensationCapUnits: "43200",
  returnedUnits: "345600", compensationUnits: "0", heldUnits: "43200", creditedUnits: "0", reason: "provider_issue",
  claim: "Original rental claim, not the new tenant's issue.", responsibility: "undetermined", status: "disputed",
  cutoffAt: iso(), cleanupConfirmedAt: iso(), cleanupJobId: "cleanup", disputeUsed: true, disputeOpenedAt: iso(),
  disputeDeadlineAt: new Date(Date.now() + 72 * 3600000).toISOString(), providerRespondedAt: null,
  resolution: null, chatAvailable: false, version: 1, updatedAt: iso(), unlockAt: null, clientSubdomain: "original-client",
  events: [], eventsNextCursor: null, ...patch,
});
let actor = "provider", record = initial(), hostPresent = true, conflict = false, workspaceFailure = false;
let anonymousPrivateReads = 0, holdDetail = false, releaseDetail;
let dualRole = false, historyPaging = false, jobLookupAttempts = 0;
const writes = [], reads = [], errors = [];
const json = (route, data, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(data) });
function view() {
  const provider = actor === "provider", owner = actor === "owner", buyer = actor === "buyer" || dualRole && provider;
  const due = record.responsibility === "undetermined" && !record.providerRespondedAt && Date.parse(record.disputeDeadlineAt) <= Date.now();
  const unsettled = record.responsibility === "undetermined" && record.disputeUsed;
  const remaining = BigInt(record.compensationCapUnits) > 0n;
  const decide = remaining && !due && record.status !== "finance_pending" && record.creditedUnits === "0";
  const respond = provider && unsettled && !record.providerRespondedAt && !due && remaining;
  const cleanup = provider && record.status === "cleanup_failed";
  const attention = respond ? "provider_response" : owner && record.status === "finance_pending" ? "finance_review"
    : owner && unsettled && record.providerRespondedAt && decide ? "owner_decision" : cleanup ? "cleanup_failed"
    : due ? "timeout_pending" : unsettled ? record.providerRespondedAt ? "waiting_owner" : "waiting_provider"
    : record.status === "completed" ? "completed" : "processing";
  return { ...record, serverNow: iso(), attention, actionRequired: respond || cleanup || owner && (attention === "owner_decision" || attention === "finance_review"),
    permissions: { buyer, provider, owner, dispute: buyer && !record.disputeUsed && decide,
      withdraw: buyer && unsettled && decide, waive: provider && decide, respond, resolve: owner && decide, retryCleanup: cleanup } };
}
let browser, page;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });
  page = await browser.newPage({ viewport: { width: 768, height: 1000 } });
  page.setDefaultTimeout(10000);
  page.on("pageerror", e => errors.push(e.message));
  await page.addInitScript(() => {
    localStorage.setItem("cc_switch_router_locale_v1", new URL(location.href).searchParams.get("testLocale") || "en");
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({ authDeviceId: "test", publicKey: "test", privateKey: "test", accessToken: "test", expiresAt: "2099-01-01T00:00:00Z" }));
  });
  await page.route("**/v1/**", async route => {
    const url = new URL(route.request().url()), path = url.pathname, method = route.request().method();
    if (path === "/v1/auth/home") return json(route, { hasActivity: true, entry: null, source: null });
    // Finance authority comes from server capabilities, not the general admin flag.
    if (path === "/v1/auth/session/me") return json(route, { authenticated: !!actor, isAdmin: false, user: actor ? { id: actor, email: `${actor}@example.test` } : null, password: { hasPassword: true, version: 1 } });
    if (path === "/v1/client-market/hosts") return json(route, hostPresent ? [{
      id: "host-one", providerId: "provider", hostOwnerEmail: "provider@example.test", ip: "192.0.2.10", status: "allocated",
      clientSubdomain: "new-tenant-client", installationId: actor === "provider" ? "current-installation" : undefined,
      clientOwnerEmail: "new-tenant@example.test", isHostOwner: actor === "provider", isClientOwner: false,
      pricingModel: "prepaid_calendar_month", cyclePriceMinor: 5, offerRevision: 1, eligibility: { allowed: true }, contacts: [],
    }] : []);
    if (path === "/v1/client-market/my-rentals" || path.endsWith("/usage-history")) return json(route, []);
    if (path.startsWith("/v1/client-market/termination-workspace")) {
      reads.push({ actor, path, query: url.search });
      if (!actor) anonymousPrivateReads++;
      if (workspaceFailure) return json(route, { message: "workspace unavailable" }, 503);
      const allowed = actor === "owner" || actor === "provider";
      const value = view();
      if (path.endsWith("/records")) return json(route, { items: allowed && (!url.searchParams.has("pendingOnly") || value.actionRequired) ? [value] : [], nextCursor: null, serverNow: iso() });
      return json(route, { hosts: allowed ? [{ hostId: record.hostId, terminationId: record.id, attention: value.attention,
        actionRequiredCount: Number(value.actionRequired), responseDeadlineAt: record.disputeDeadlineAt }] : [],
        actionRequiredCount: allowed ? Number(value.actionRequired) : 0, canAdjudicate: actor === "owner", serverNow: iso() });
    }
    if (path === `/v1/client-market/terminations/${record.id}` && method === "GET") {
      reads.push({ actor, path }); if (!actor) anonymousPrivateReads++;
      const response = view();
      if (historyPaging) {
        const older = url.searchParams.has("eventCursor");
        response.events = Array.from({ length: older ? 10 : 50 }, (_, n) => ({ id: `history-${older ? n + 50 : n}`, action: "provider_response", actorRole: "provider", note: `Private history ${older ? n + 50 : n}`, awardUnits: null, createdAt: iso() }));
        response.eventsNextCursor = older ? null : "history-49";
      }
      const allowed = ["owner", "provider", "buyer"].includes(actor);
      if (holdDetail) { holdDetail = false; await new Promise(ready => { releaseDetail = ready; }); }
      return json(route, allowed ? response : { message: "not allowed to view this termination" }, allowed ? 200 : 403);
    }
    if (method === "POST" && /\/(?:waive|respond|resolve|dispute|withdraw)$/.test(path)) {
      const body = route.request().postDataJSON(); writes.push({ path, actor, body });
      if (body.expectedVersion !== record.version) return json(route, { message: "termination changed; reload before deciding" }, 409);
      if (conflict) {
        conflict = false; record = initial({ status: "completed", responsibility: "none", heldUnits: "0", compensationCapUnits: "0", version: record.version + 1 });
        return json(route, { message: "termination changed; reload before deciding" }, 409);
      }
      if (path.endsWith("/respond")) record.providerRespondedAt = iso();
      else {
        const amount = body.awardUnits || "0";
        record.returnedUnits = (BigInt(record.returnedUnits) + BigInt(record.heldUnits) - BigInt(amount)).toString();
        record.heldUnits = amount; record.compensationCapUnits = amount; record.providerRespondedAt = iso();
        if (actor === "owner" || amount === "0") { record.responsibility = body.responsibility; record.status = "completed"; record.heldUnits = "0"; }
      }
      record.version++;
      record.events.unshift({ id: `event-${record.version}`, action: path.endsWith("/respond") ? "provider_response" : actor === "owner" ? "owner_decision" : "provider_waiver", note: body.note, createdAt: iso(), actorRole: actor, awardUnits: body.awardUnits || null });
      return json(route, record);
    }
    if (path.endsWith("/provider-cleanup")) {
      writes.push({ path, actor, body: route.request().postDataJSON() });
      assert.ok(path.includes("old-installation"));
      record = { ...record, status: "completed", cleanupConfirmedAt: iso(), compensationCapUnits: "0", heldUnits: "0" };
      return json(route, { jobId: "cleanup-retry" });
    }
    if (path === "/v1/client-market/jobs/cleanup-retry") {
      if (++jobLookupAttempts === 1) return json(route, { message: "temporary job lookup failure" }, 503);
      return json(route, { id: "cleanup-retry", status: "succeeded", log: "Verified original rental cleanup", phase: "complete", jobType: "cleanup" });
    }
    if (path === "/v1/notifications") return json(route, { items: [], nextCursor: null, unread: 0, readThrough: null });
    if (path === "/v1/market-wallet") return json(route, { assetCode: "USD", postedUnits: "8640000", availableUnits: "8640000", heldUnits: "0", version: 1 });
    if (path === "/v1/regions") return json(route, []);
    if (path === "/v1/market/config") return json(route, {});
    if (path.endsWith("/rooms")) return json(route, { rooms: [], totalUnread: 0 });
    if (path.includes("inbox-summary")) return json(route, { pendingRequests: 0 });
    if (path === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
    return json(route, {});
  });
  const dialog = () => page.getByRole("dialog", { name: "Client termination", exact: true });
  const loaded = modal => modal.getByTestId("termination-original-client").filter({ hasText: "original-client" }).waitFor();
  const open = async () => { await page.getByRole("button", { name: "Handle termination · 1", exact: true }).click(); await loaded(dialog()); };
  const close = async () => { await dialog().getByRole("button", { name: "Close", exact: true }).click(); await dialog().waitFor({ state: "hidden" }); };
  const goto = () => page.goto(`${base}/client-market/`);
  const switchActor = async next => { actor = next; await page.evaluate(() => window.dispatchEvent(new Event("router-auth-changed"))); };
  if (process.env.TERMINATION_SCREENSHOTS === "1") await mkdir("/tmp/client-termination-review", { recursive: true });

  await goto(); await open();
  assert.equal(await dialog().getByText("new-tenant@example.test", { exact: true }).count(), 0);
  assert.equal(await dialog().getByRole("button", { name: "Decide responsibility", exact: true }).count(), 0);
  await dialog().getByRole("button", { name: "Reduce / waive compensation", exact: true }).click();
  await dialog().getByLabel("Provider award", { exact: true }).selectOption("custom");
  await dialog().getByLabel("Custom amount (USD)", { exact: true }).fill("0.0025");
  await dialog().getByLabel("Decision explanation (required)", { exact: true }).fill("Partial waiver; please decide the rest.");
  assert.match(await dialog().getByTestId("termination-award-preview").innerText(), /0\.0025/);
  await dialog().getByRole("button", { name: "Save", exact: true }).click();
  await dialog().getByText("Termination settlement updated.", { exact: true }).waitFor();
  assert.equal(writes.at(-1).body.awardUnits, "21600");
  assert.ok(writes.at(-1).path.endsWith("/waive"));
  await close();
  await switchActor("owner");
  await page.locator('[data-segment-id="all"]').click(); await open();
  await dialog().getByRole("button", { name: "Decide responsibility", exact: true }).click();
  await dialog().getByLabel("Decision explanation (required)", { exact: true }).fill("Evidence reviewed; all compensation waived.");
  if (process.env.TERMINATION_SCREENSHOTS === "1") await page.screenshot({ path: "/tmp/client-termination-review/owner-decision.png" });
  await dialog().getByRole("button", { name: "Save", exact: true }).click();
  await dialog().getByText("Termination settlement updated.", { exact: true }).waitFor();
  assert.ok(writes.at(-1).path.startsWith("/v1/admin/"));
  await close();

  // Deleted Host: independent queue and deep links still reach the original case.
  record = initial({ providerRespondedAt: iso() }); hostPresent = false;
  await goto();
  await page.getByRole("button", { name: "Termination tasks · 1", exact: true }).click();
  await page.getByRole("region", { name: "Terminations needing attention", exact: true }).getByRole("button").filter({ hasText: "original-client" }).click();
  await loaded(dialog()); await close();
  await page.goto(`${base}/client-market/?terminationId=old-termination`);
  await loaded(dialog()); await close();
  await page.waitForURL(url => !url.searchParams.has("terminationId"));
  assert.equal(new URL(page.url()).searchParams.has("terminationId"), false);
  await page.evaluate(() => window.history.pushState(null, "", "/client-market/?terminationId=old-termination"));
  await loaded(dialog()); await close();
  await page.waitForURL(url => !url.searchParams.has("terminationId"));

  // Role-checked reply, stale version, deadline, finance hold and cleanup paths.
  actor = "provider"; hostPresent = true; record = initial(); await goto(); await open();
  await dialog().getByRole("button", { name: "Disagree / request Owner decision", exact: true }).click();
  await dialog().getByLabel("Decision explanation (required)", { exact: true }).fill("Please review the evidence independently.");
  await dialog().getByRole("button", { name: "Save", exact: true }).click();
  await dialog().getByText("Termination settlement updated.", { exact: true }).waitFor();
  assert.ok(writes.at(-1).path.endsWith("/respond")); await close();
  record = initial(); conflict = true; await goto(); await open();
  await dialog().getByRole("button", { name: "Reduce / waive compensation", exact: true }).click();
  await dialog().getByLabel("Decision explanation (required)", { exact: true }).fill("Waive all disputed compensation.");
  await dialog().getByRole("button", { name: "Save", exact: true }).click();
  await dialog().getByRole("alert").filter({ hasText: "termination changed" }).waitFor();
  assert.equal(await dialog().getByRole("button", { name: "Save", exact: true }).isDisabled(), true); await close();
  record = initial({ disputeDeadlineAt: new Date(Date.now() - 1000).toISOString() }); await goto();
  await page.getByRole("button", { name: "View termination", exact: true }).click();
  await dialog().getByText("Response deadline passed · automatic waiver pending", { exact: true }).waitFor();
  assert.equal(await dialog().getByRole("button", { name: "Disagree / request Owner decision", exact: true }).count(), 0); await close();
  record = initial({ status: "finance_pending" }); await goto(); await open();
  assert.equal(await dialog().getByRole("button", { name: "Reduce / waive compensation", exact: true }).count(), 0);
  assert.equal(await dialog().getByRole("button", { name: "Disagree / request Owner decision", exact: true }).count(), 1); await close();
  record = initial({ status: "cleanup_failed", responsibility: "buyer", disputeUsed: false, compensationCapUnits: "0", heldUnits: "0", cleanupConfirmedAt: null });
  await goto(); await open(); await dialog().getByRole("button", { name: "Retry cleanup", exact: true }).click();
  await dialog().getByRole("alert").filter({ hasText: "temporary job lookup failure" }).waitFor();
  await dialog().getByText("Verified original rental cleanup", { exact: true }).waitFor();
  assert.equal(jobLookupAttempts, 2);
  assert.equal(writes.filter(write => write.path.endsWith("/provider-cleanup")).length, 1); await close();

  dualRole = true; record = initial({ buyerUserId: "provider" }); await goto(); await open();
  await dialog().getByLabel("Acting as", { exact: true }).selectOption("buyer");
  await dialog().getByRole("button", { name: "End compensation claim", exact: true }).waitFor();
  assert.equal(await dialog().getByRole("button", { name: "Disagree / request Owner decision", exact: true }).count(), 0);
  dualRole = false; await close();

  historyPaging = true; record = initial(); await goto(); await open();
  await dialog().getByRole("button", { name: "Reduce / waive compensation", exact: true }).click();
  const explanation = "Review evidence; preserve this draft while reading history.";
  await dialog().getByLabel("Decision explanation (required)", { exact: true }).fill(explanation);
  const editedVersion = record.version;
  record.version++; // Another tab changed the record before history paging.
  await dialog().locator("summary").filter({ hasText: "Processing history" }).click();
  await dialog().getByRole("button", { name: "Load more", exact: true }).click();
  await dialog().getByText("Private history 59", { exact: true }).waitFor();
  assert.equal(await dialog().getByLabel("Decision explanation (required)", { exact: true }).inputValue(), explanation);
  await dialog().getByRole("button", { name: "Save", exact: true }).click();
  await dialog().getByRole("alert").filter({ hasText: "termination changed" }).waitFor();
  assert.equal(writes.at(-1).body.expectedVersion, editedVersion, "history reads must not silently advance an edited decision version");
  assert.equal(await dialog().getByRole("button", { name: "Save", exact: true }).isDisabled(), true, "an ambiguous write needs an explicit review even if the action remains available");
  const historyRead = page.waitForResponse(response => new URL(response.url()).pathname === `/v1/client-market/terminations/${record.id}`);
  await dialog().getByRole("button", { name: "Reload", exact: true }).click();
  await historyRead;
  await page.waitForFunction(() => [...document.querySelectorAll('[role="dialog"] button')].some(button => button.textContent === "Reload" && !button.disabled));
  assert.equal(await dialog().getByText("Private history 59", { exact: true }).count(), 1);
  assert.equal(await dialog().getByRole("button", { name: "Save", exact: true }).isDisabled(), false);
  historyPaging = false; await close();

  // Ordinary trial exits are informational, never a new manual approval step.
  record = initial({ status: "completed", reason: "voluntary", responsibility: "none", disputeUsed: false, compensationCapUnits: "0", heldUnits: "0" });
  await goto(); await page.getByRole("button", { name: "View termination", exact: true }).click();
  assert.equal(await dialog().getByRole("button", { name: "Reduce / waive compensation", exact: true }).count(), 0); await close();
  workspaceFailure = true; await goto();
  await page.getByRole("alert").filter({ hasText: "Termination tasks could not be loaded" }).waitFor();
  workspaceFailure = false; await page.getByRole("button", { name: "Reload", exact: true }).click();
  await page.getByRole("button", { name: "View termination", exact: true }).waitFor();

  // Localised dialog geometry, keyboard dismissal, no horizontal page overflow.
  record = initial();
  for (const locale of ["en", "zh-CN"]) for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.goto(`${base}/client-market/?terminationId=old-termination&testLocale=${locale}`);
    const modal = page.getByRole("dialog", { name: locale === "en" ? "Client termination" : "Client 退租处理", exact: true });
    await loaded(modal);
    const box = await modal.boundingBox(); assert.ok(box.x >= 0 && box.x + box.width <= width + 1);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `page overflows at ${width}`);
    const body = modal.locator('[data-slot="modal-body"]');
    assert.ok(await body.evaluate(element => element.scrollWidth <= element.clientWidth + 1), `dialog body overflows at ${locale}/${width}`);
    await modal.getByRole("button", { name: locale === "en" ? "Reduce / waive compensation" : "减免退租补偿", exact: true }).click();
    await modal.getByLabel(locale === "en" ? "Decision explanation (required)" : "处理说明（必填）", { exact: true }).fill("Evidence reviewed; waive all compensation.");
    assert.ok(await body.evaluate(element => element.scrollWidth <= element.clientWidth + 1), `decision form overflows at ${locale}/${width}`);
    if (process.env.TERMINATION_SCREENSHOTS === "1") await page.screenshot({ path: `/tmp/client-termination-review/${locale}-${width}.png` });
    await page.keyboard.press("Escape"); await modal.waitFor({ state: "hidden" });
  }

  // A late private read must not re-open a case after account replacement.
  record = initial(); actor = "provider"; await page.goto(`${base}/client-market/?testLocale=en`);
  holdDetail = true; await page.getByRole("button", { name: "Handle termination · 1", exact: true }).click();
  await page.waitForFunction(() => !!document.querySelector('[role="dialog"]'));
  await switchActor("stranger");
  await page.getByRole("button", { name: "Termination tasks · 0", exact: true }).waitFor();
  assert.equal(typeof releaseDetail, "function"); releaseDetail();
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  assert.equal(await page.getByText("Original rental claim, not the new tenant's issue.", { exact: true }).count(), 0);
  actor = null; await goto(); assert.equal(anonymousPrivateReads, 0);
  assert.deepEqual(errors, []);
  console.log("Client termination browser: Provider waiver/reply, non-admin finance Owner adjudication, deleted-Host queue and repeat deep links, explicit conflict review, stable edited versions across history paging, deadlines, finance holds, original-installation cleanup with transient recovery, dual roles, trial-only history, account-switch race, keyboard and four widths in both locales passed; no real operations.");
} catch (cause) {
  console.error("Termination browser failure:", (await page?.getByRole("dialog").allTextContents())?.map(text => text.slice(0, 1800)), { writes: writes.slice(-3), reads: reads.slice(-5) });
  if (page && process.env.TERMINATION_SCREENSHOTS === "1") await page.screenshot({ path: "/tmp/client-termination-review/failure.png" });
  throw cause;
} finally { await browser?.close(); await new Promise(ready => server.close(ready)); }
