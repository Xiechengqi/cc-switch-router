import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
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
        const filtered = records.filter(r => (!url.searchParams.get("search") || r.clientSubdomain.startsWith(url.searchParams.get("search")))
          && (!url.searchParams.get("buyerEmail") || r.buyerEmail === url.searchParams.get("buyerEmail"))
          && (!url.searchParams.get("status") || r.status === url.searchParams.get("status"))
          && (!url.searchParams.get("attention") || r.attention === url.searchParams.get("attention")));
        const totalPages = Math.max(1, Math.ceil(filtered.length / 5));
        const page = Math.min(Number(url.searchParams.get("page") || 1), totalPages);
        const items = filtered.slice((page - 1) * 5, page * 5);
        return json(route, { items, page, pageSize: 5, totalPages, total: filtered.length, serverNow: at });
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
  const rows = () => panel().getByTestId("rental-audit-row");
  const detail = () => page.getByTestId("rental-audit-detail");
  const detailDialog = () => page.getByRole("dialog", { name: "Rental audit details", exact: true });
  const toggle = () => rows().first().getByRole("button");
  const goto = locale => page.goto(`${base}/client-market/?testLocale=${locale || "en"}`);
  const switchActor = async next => { actor = next; await page.evaluate(() => window.dispatchEvent(new Event("router-auth-changed"))); };
  await goto(); await panel().getByText("53 matching records · page 1 / 11", { exact: true }).waitFor();
  assert.equal(await rows().count(), 5);
  assert.equal(await detail().count(), 0, "rental details are loaded only on demand");
  assert.equal(requests.filter(r => r.path !== "/v1/admin/client-market/rental-audit").length, 0, "summary rows must not fetch private detail individually");
  assert.equal(await panel().getByRole("button", { name: "View termination", exact: true }).count(), 0);
  assert.equal(await panel().getByLabel("Provider email", { exact: true }).isVisible(), false);
  await panel().getByRole("button", { name: "Next", exact: true }).click();
  await panel().getByText("53 matching records · page 2 / 11", { exact: true }).waitFor(); assert.equal(await rows().count(), 5);
  await panel().getByLabel("Go to page", { exact: true }).fill("11");
  await panel().getByRole("button", { name: "Go", exact: true }).click();
  await panel().getByText("53 matching records · page 11 / 11", { exact: true }).waitFor(); assert.equal(await rows().count(), 3);
  await panel().getByLabel("Go to page", { exact: true }).fill("1");
  await panel().getByLabel("Go to page", { exact: true }).press("Enter");
  await panel().getByText("53 matching records · page 1 / 11", { exact: true }).waitFor();
  const selectFilter = async (label, option, within = panel()) => {
    await within.getByRole("button", { name: new RegExp(label) }).click();
    await page.getByRole("option", { name: option, exact: true }).click();
  };
  await selectFilter("Rental status", "Paid service");
  assert.equal(await rows().count(), 5, "selecting a draft filter must not submit the form");
  await panel().getByRole("button", { name: "Search", exact: true }).click();
  await panel().getByText("No matching rental records.", { exact: true }).waitFor();
  assert.equal(new URLSearchParams(requests.at(-1).query).get("status"), "paid");
  await selectFilter("Rental status", "All");
  await panel().getByRole("button", { name: "Search", exact: true }).click();
  await panel().getByText("53 matching records · page 1 / 11", { exact: true }).waitFor();
  assert.equal(new URLSearchParams(requests.at(-1).query).has("status"), false, "All restores the empty filter value");
  await panel().getByRole("button", { name: "More filters", exact: true }).click();
  const filterDialog = page.getByRole("dialog", { name: "More filters", exact: true });
  await selectFilter("Follow-up", "Cleanup failed", filterDialog);
  await filterDialog.getByLabel("Renter email", { exact: true }).fill("nobody@example.test"); await filterDialog.getByRole("button", { name: "Search", exact: true }).click();
  await panel().getByText("No matching rental records.", { exact: true }).waitFor();
  assert.equal(new URLSearchParams(requests.at(-1).query).get("attention"), "cleanup_failed");
  await panel().getByRole("button", { name: "Reset", exact: true }).click(); await panel().getByText("53 matching records · page 1 / 11", { exact: true }).waitFor();
  await rows().first().getByText("historical-client-1", { exact: true }).click();
  await detail().getByText("Identity captured at order creation", { exact: false }).waitFor();
  assert.equal(await toggle().getAttribute("aria-haspopup"), "dialog");
  assert.equal(await detailDialog().count(), 1, "details use a dialog without growing the five-row list");
  await detail().getByText("$0.05 / month", { exact: true }).waitFor();
  await detail().getByText("Manual / ends at period expiry", { exact: true }).waitFor();
  assert.ok(await detail().getByText("$0.005", { exact: true }).count() > 0);
  assert.equal(await detail().getByText("original-provider@example.test", { exact: true }).count(), 1);
  await detail().locator("summary").filter({ hasText: "Paid periods" }).click();
  await detail().getByRole("button", { name: "Load earlier periods", exact: true }).click(); await detail().getByText("Debit journal: journal-1", { exact: true }).waitFor();
  await detail().locator("summary").filter({ hasText: "Rental timeline" }).click();
  await detail().getByRole("button", { name: "Load earlier events", exact: true }).click(); await detail().getByText("Audit timeline 1", { exact: true }).waitFor();
  assert.equal(await detail().getByText("Debit journal: journal-1", { exact: true }).count(), 1);
  await detail().getByRole("button", { name: "View termination", exact: true }).click();
  const termination = page.getByRole("dialog", { name: "Client termination", exact: true });
  await termination.getByRole("alert").waitFor(); await termination.getByRole("button", { name: "Close", exact: true }).click();
  assert.equal(await detail().count(), 1, "closing termination returns to the audit details");
  await page.keyboard.press("Escape"); await detail().waitFor({ state: "hidden" });
  await toggle().focus(); await page.keyboard.press("Enter"); await detail().getByText("$0.05 / month", { exact: true }).waitFor();
  await page.keyboard.press("Escape"); await detail().waitFor({ state: "hidden" });
  await toggle().focus(); await page.keyboard.press("Space"); await detail().getByText("$0.05 / month", { exact: true }).waitFor();
  await page.keyboard.press("Escape"); await detail().waitFor({ state: "hidden" });
  await rows().nth(1).getByRole("button").click();
  await detail().getByText("job:order-2", { exact: true }).waitFor();
  assert.equal(await detail().count(), 1, "only the selected rental has a detail dialog");
  await page.keyboard.press("Escape"); await detail().waitFor({ state: "hidden" });
  await panel().getByRole("button", { name: "Next", exact: true }).click();
  await panel().getByText("53 matching records · page 2 / 11", { exact: true }).waitFor();
  assert.equal(await detail().count(), 0, "pagination never restores a previous detail");
  await panel().getByRole("button", { name: "Previous", exact: true }).click();
  await panel().getByText("53 matching records · page 1 / 11", { exact: true }).waitFor();

  auditFailure = true; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByRole("alert").waitFor(); assert.equal(await page.getByRole("button", { name: "Add host", exact: true }).count(), 1);
  auditFailure = false; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByRole("alert").waitFor({ state: "hidden" });

  auditForbidden = true; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByRole("alert").waitFor(); assert.equal(await rows().count(), 0, "revoked access must clear previously visible audit records");
  auditForbidden = false; await panel().getByRole("button", { name: "Refresh audit", exact: true }).click();
  await panel().getByText("53 matching records · page 1 / 11", { exact: true }).waitFor();

  holdDetail = true; await toggle().click(); await detail().waitFor();
  await assert.doesNotReject(async () => {
    const deadline = Date.now() + 10000;
    while (!releaseDetail && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(typeof releaseDetail, "function", "the delayed detail request must start before changing accounts");
  });
  await switchActor("buyer"); await panel().waitFor({ state: "hidden" }); await detail().waitFor({ state: "hidden" });
  releaseDetail?.(); holdDetail = false;
  await switchActor("admin"); await page.getByRole("button", { name: "Add host", exact: true }).waitFor();
  assert.equal(await panel().count(), 0);
  await switchActor(""); await goto(); assert.equal(await panel().count(), 0);
  await switchActor("owner"); await panel().getByText("53 matching records · page 1 / 11", { exact: true }).waitFor();
  assert.equal(requests.filter(r => r.actor !== "owner").length, 0, "non-owners must not issue audit reads");
  assert.equal(await page.evaluate(() => Object.values(localStorage).some(v => v.includes("original-buyer@example.test") || v.includes("original-provider@example.test"))), false);
  for (const locale of ["en", "zh-CN"]) for (const width of [320, 375, 414, 640, 768, 1366]) {
    await page.setViewportSize({ width, height: 1000 }); await goto(locale);
    await rows().first().waitFor();
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `page overflow at ${locale}/${width}`);
    const before = await panel().boundingBox();
    await toggle().focus(); await page.keyboard.press("Enter");
    await detail().getByText("original-provider@example.test", { exact: true }).waitFor();
    const box = await detail().boundingBox(); assert.ok(box.x >= 0 && box.x + box.width <= width + 1, `detail dialog overflows at ${locale}/${width}`);
    const after = await panel().boundingBox();
    assert.equal(before.height, after.height, `details must not grow the fixed audit card at ${locale}/${width}`);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `detail page overflow at ${locale}/${width}`);
    await page.keyboard.press("Escape"); await detail().waitFor({ state: "hidden" });
  }
  assert.equal(writes.length, 0, `audit must remain read-only: ${writes.join(",")}`);
  assert.deepEqual(errors, []);
  console.log("Client rental audit: fixed-height five-row list, filter and lazy detail dialogs, click/keyboard access, Owner-only capability, numbered pages and direct jumps, exact sub-cent money, independent paged periods/events, termination entry, isolated failures, revoked-access clearing, account-change cancellation, no screenshots, private browser storage or writes; EN/ZH at six widths passed");
} finally { releaseDetail?.(); await page?.close(); await browser?.close(); await new Promise(resolve => server.close(resolve)); }
