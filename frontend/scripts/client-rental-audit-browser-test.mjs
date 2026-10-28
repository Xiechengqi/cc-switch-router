import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Local production export and synthetic identities only. No real rental,
// payment gateway, email, Host or Router is contacted.
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
const base = `http://127.0.0.1:${server.address().port}`, at = "2026-10-28T00:00:00Z";
const money = { chargedUnits: "432000", returnedUnits: "345600", unfulfilledUnits: "43200", serviceUnits: "43200", providerNetUnits: "38880", platformFeeUnits: "4320", providerCreditedUnits: "0", compensationUnits: "0", compensationCreditedUnits: "0" };
function rental(n) {
  return { id: `job:order-${n}`, jobId: `order-${n}`, installationId: `original-client-${n}`, hostId: "removed-host", buyerEmail: "original-buyer@example.test", providerEmail: "original-provider@example.test", clientSubdomain: `historical-client-${n}`,
    hostIp: "192.0.2.10", hostPort: 22, countryCode: "SG", provenance: "order_snapshot", createdAt: at, activatedAt: at, endedAt: at, cleanupConfirmedAt: null,
    contractId: `contract-${n}`, status: "ended", cleanupStatus: "failed", attention: "cleanup_failed", terminationId: n === 1 ? "original-termination" : null, hostPresent: false, pricingModel: "prepaid_calendar_month", cyclePriceMinor: 5, dailyRateMinor: null, offerRevision: 1, trialSecondsRemaining: 0, renewalPolicy: "manual", money };
}
const records = Array.from({ length: 53 }, (_, n) => rental(n + 1));
let actor = "owner", auditFailure = false, auditForbidden = false, holdDetail = false, releaseDetail;
const requests = [], writes = [], errors = [];
const json = (route, value, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(value) });
let browser, page;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });
  page = await browser.newPage({ viewport: { width: 1366, height: 1000 } }); page.setDefaultTimeout(10000);
  page.on("pageerror", e => errors.push(e.message));
  await page.addInitScript(() => {
    localStorage.setItem("cc_switch_router_locale_v1", new URL(location.href).searchParams.get("testLocale") || "en");
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({ authDeviceId: "test", publicKey: "test", privateKey: "test", accessToken: "test", expiresAt: "2099-01-01T00:00:00Z" }));
  });
  await page.route("**/v1/**", async route => {
    const url = new URL(route.request().url()), path = url.pathname;
    // The shared dashboard reports viewer presence independently of this
    // read-only audit. Every other mutation remains forbidden in this test.
    if (route.request().method() !== "GET" && path !== "/v1/dashboard/presence") writes.push(path);
    if (path === "/v1/auth/session/me") return json(route, { authenticated: !!actor, isAdmin: actor === "admin", canAuditClientMarket: actor === "owner", user: actor ? { id: actor, email: `${actor}@example.test` } : null, password: { hasPassword: true, version: 1 } });
    if (path === "/v1/auth/home") return json(route, { hasActivity: true, entry: null, source: null });
    if (path === "/v1/client-market/hosts" || path === "/v1/client-market/my-rentals" || path === "/v1/regions") return json(route, []);
    if (path === "/v1/client-market/termination-workspace") return json(route, { hosts: [], actionRequiredCount: 0, canAdjudicate: actor === "owner", serverNow: at });
    if (path.startsWith("/v1/admin/client-market/rental-audit")) {
      requests.push({ actor, path, query: url.search });
      if (actor !== "owner") return json(route, { message: "Router owner privilege required" }, actor ? 403 : 401);
      if (auditForbidden) return json(route, { message: "Router owner privilege required" }, 403);
      if (auditFailure) return json(route, { message: "audit unavailable" }, 503);
      if (path === "/v1/admin/client-market/rental-audit") {
        const filtered = records.filter(r => (!url.searchParams.get("search") || r.clientSubdomain.startsWith(url.searchParams.get("search"))) && (!url.searchParams.get("buyerEmail") || r.buyerEmail === url.searchParams.get("buyerEmail")));
        const start = url.searchParams.get("cursor") ? filtered.findIndex(r => r.id === url.searchParams.get("cursor")) + 1 : 0;
        const items = filtered.slice(start, start + 50);
        return json(route, { items, nextCursor: filtered.length > start + 50 ? items.at(-1).id : null, total: filtered.length, serverNow: at });
      }
      const record = records.find(r => r.id === decodeURIComponent(path.split("/").at(-1)));
      if (!record) return json(route, {}, 404);
      const olderPeriods = url.searchParams.has("periodCursor"), olderEvents = url.searchParams.has("eventCursor");
      const periods = Array.from({ length: olderPeriods ? 5 : 50 }, (_, n) => ({ id: `period-${olderPeriods ? 5 - n : 55 - n}`, sequence: olderPeriods ? 5 - n : 55 - n, startedAt: at, endedAt: at, debitedAt: at, amountUnits: "432000", serviceUnits: "43200", returnedUnits: "345600", compensationUnits: "43200", journalId: `journal-${olderPeriods ? 5 - n : 55 - n}`, status: "closed", terminationRateBps: 1000, commissionRateBps: 1000 }));
      const events = Array.from({ length: olderEvents ? 5 : 50 }, (_, n) => ({ id: `event-${olderEvents ? 5 - n : 55 - n}`, action: "cleanup_started", actorEmail: null, createdAt: at, note: `Audit timeline ${olderEvents ? 5 - n : 55 - n}` }));
      const value = { record, periods, periodsNextCursor: olderPeriods ? null : 6, events, eventsNextCursor: olderEvents ? null : "event-6", trialConsumedSeconds: 3600,
        settlement: { heldUnits: "43200", returnedUnits: "345600", compensationUnits: "0", reason: "provider_issue", responsibility: "undetermined", cutoffAt: at, cleanupConfirmedAt: null }, serverNow: at };
      if (holdDetail) await new Promise(resolve => { releaseDetail = resolve; });
      return json(route, value);
    }
    if (path === "/v1/client-market/terminations/original-termination") return json(route, { message: "Synthetic termination detail unavailable" }, 503);
    if (path === "/v1/notifications") return json(route, { items: [], nextCursor: null, unread: 0, readThrough: null });
    if (path === "/v1/market-wallet") return json(route, { assetCode: "USD", postedUnits: "8640000", availableUnits: "8640000", heldUnits: "0", version: 1 });
    if (path.endsWith("/rooms")) return json(route, { rooms: [], totalUnread: 0 });
    if (path.includes("inbox-summary")) return json(route, { pendingRequests: 0 });
    if (path === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
    return json(route, {});
  });
  const panel = () => page.getByTestId("client-rental-audit");
  const modal = () => page.getByRole("dialog", { name: "Rental audit details", exact: true });
  const goto = locale => page.goto(`${base}/client-market/?testLocale=${locale || "en"}`);
  const switchActor = async next => { actor = next; await page.evaluate(() => window.dispatchEvent(new Event("router-auth-changed"))); };
  if (process.env.AUDIT_SCREENSHOTS === "1") await mkdir("/tmp/client-rental-audit-review", { recursive: true });
  await goto(); await panel().getByText("53 matching records · page 1", { exact: true }).waitFor();
  assert.equal(await panel().locator("tbody tr").count(), 50);
  await panel().getByRole("button", { name: "Next", exact: true }).click();
  await panel().getByText("53 matching records · page 2", { exact: true }).waitFor(); assert.equal(await panel().locator("tbody tr").count(), 3);
  await panel().getByRole("button", { name: "Previous", exact: true }).click(); await panel().getByText("53 matching records · page 1", { exact: true }).waitFor();
  await panel().getByLabel("Renter email", { exact: true }).fill("nobody@example.test"); await panel().getByRole("button", { name: "Search", exact: true }).click();
  await panel().getByText("No matching rental records.", { exact: true }).waitFor();
  await panel().getByRole("button", { name: "Reset", exact: true }).click(); await panel().getByText("53 matching records · page 1", { exact: true }).waitFor();
  await panel().getByRole("button", { name: "View rental", exact: true }).first().click();
  await modal().getByText("Identity captured at order creation", { exact: true }).waitFor();
  await modal().getByText("$0.05 / month", { exact: true }).waitFor();
  await modal().getByText("Manual / ends at period expiry", { exact: true }).waitFor();
  assert.ok(await modal().getByText("$0.005", { exact: true }).count() > 0);
  assert.equal(await modal().getByText("original-provider@example.test", { exact: true }).count(), 1);
  await modal().getByRole("button", { name: "Load earlier periods", exact: true }).click(); await modal().getByText("Debit journal: journal-1", { exact: true }).waitFor();
  await modal().getByRole("button", { name: "Load earlier events", exact: true }).click(); await modal().getByText("Audit timeline 1", { exact: true }).waitFor();
  assert.equal(await modal().getByText("Debit journal: journal-1", { exact: true }).count(), 1);
  await modal().getByRole("button", { name: "View termination", exact: true }).click();
  await modal().waitFor({ state: "hidden" });
  const termination = page.getByRole("dialog", { name: "Client termination", exact: true });
  await termination.getByRole("alert").waitFor(); await termination.getByRole("button", { name: "Close", exact: true }).click();

  auditFailure = true; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByRole("alert").waitFor(); assert.equal(await page.getByRole("button", { name: "Add host", exact: true }).count(), 1);
  auditFailure = false; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByRole("alert").waitFor({ state: "hidden" });

  auditForbidden = true; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByRole("alert").waitFor(); assert.equal(await panel().locator("tbody tr").count(), 0, "revoked access must clear previously visible audit records");
  auditForbidden = false; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByText("53 matching records · page 1", { exact: true }).waitFor();

  holdDetail = true; await panel().getByRole("button", { name: "View rental", exact: true }).first().click();
  await page.waitForFunction(() => document.querySelector('[role="dialog"]'));
  await assert.doesNotReject(async () => {
    const deadline = Date.now() + 10000;
    while (!releaseDetail && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(typeof releaseDetail, "function", "the delayed detail request must start before changing accounts");
  });
  await switchActor("buyer"); await panel().waitFor({ state: "hidden" }); await modal().waitFor({ state: "hidden" });
  releaseDetail?.(); holdDetail = false;
  await switchActor("admin"); await page.getByRole("button", { name: "Add host", exact: true }).waitFor();
  assert.equal(await panel().count(), 0);
  await switchActor(""); await goto(); assert.equal(await panel().count(), 0);
  await switchActor("owner"); await panel().getByText("53 matching records · page 1", { exact: true }).waitFor();
  assert.equal(requests.filter(r => r.actor !== "owner").length, 0, "non-owners must not issue audit reads");
  assert.equal(await page.evaluate(() => Object.values(localStorage).some(v => v.includes("original-buyer@example.test") || v.includes("original-provider@example.test"))), false);
  for (const locale of ["en", "zh-CN"]) for (const width of [320, 375, 768, 1366]) {
    await page.setViewportSize({ width, height: 1000 }); await goto(locale);
    await panel().locator("tbody tr").first().waitFor();
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `page overflow at ${locale}/${width}`);
    if (process.env.AUDIT_SCREENSHOTS === "1" && (width === 375 || width === 1366)) await page.screenshot({ path: `/tmp/client-rental-audit-review/${locale}-${width}-table.png`, animations: "disabled", fullPage: true });
    await panel().getByRole("button", { name: locale === "en" ? "View rental" : "查看租用", exact: true }).first().click();
    const dialog = page.getByRole("dialog", { name: locale === "en" ? "Rental audit details" : "租用审计详情", exact: true });
    await dialog.getByText("original-provider@example.test", { exact: true }).waitFor();
    const box = await dialog.boundingBox(); assert.ok(box.x >= 0 && box.x + box.width <= width + 1);
    if (process.env.AUDIT_SCREENSHOTS === "1" && (width === 375 || width === 1366)) await page.screenshot({ path: `/tmp/client-rental-audit-review/${locale}-${width}.png`, animations: "disabled" });
    await page.keyboard.press("Escape"); await dialog.waitFor({ state: "hidden" });
  }
  assert.equal(writes.length, 0, `audit must remain read-only: ${writes.join(",")}`);
  assert.deepEqual(errors, []);
  console.log("Client rental audit: Owner-only capability independent of admin, 53-row pagination, filters, exact sub-cent money, paged periods/events, termination entry, isolated failures, revoked-access clearing, account-change cancellation, no private browser storage or writes; EN/ZH at four widths passed");
} finally { releaseDetail?.(); await page?.close(); await browser?.close(); await new Promise(resolve => server.close(resolve)); }
