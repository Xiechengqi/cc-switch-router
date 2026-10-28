import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Exported UI + local API mocks only. No screenshots, real emails or writes.
const output = fileURLToPath(new URL("../out/", import.meta.url));
const server = createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const path = resolve(output, `.${pathname.endsWith("/") ? `${pathname}index.html` : pathname}`);
    if (!path.startsWith(`${resolve(output)}${sep}`)) return response.writeHead(403).end();
    const body = await readFile(path);
    const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".txt": "text/plain", ".svg": "image/svg+xml", ".ttf": "font/ttf", ".woff2": "font/woff2" };
    response.writeHead(200, { "Content-Type": types[extname(path)] || "application/octet-stream", "Cache-Control": "no-store" });
    response.end(body);
  } catch { response.writeHead(404).end(); }
});
await new Promise((ready, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", ready); });
const base = `http://127.0.0.1:${server.address().port}`;
const at = "2026-10-27T00:00:00Z";
const row = (id, providerStatus) => ({ id, eventId: `event-${id}`, recipientId: "buyer", channel: "email", purpose: "service_paused",
  submissionStatus: "sent", providerStatus, providerMessageId: `provider-${id}`, attempts: 1, createdAt: at,
  firstAttemptAt: at, acceptedAt: at, providerStatusAt: at, providerCheckedAt: at, suppressionReason: null,
  submissionError: null, providerError: null, objectId: `contract-${id}`, eventKind: "contract_suspended", targetMasked: "bu***@example.test" });
const result = (items, nextCursor = null) => ({ items, nextCursor, webhookConfigured: true,
  limits: { pauseCooldownSecs: 1800, pauseDailyLimit: 3, userHourlyLimit: 10, emailDailyLimit: 50, maxAgeSecs: 3600 },
  overview: { events: { sent: 410, suppressed: 1227 }, submissions: { sent: 410 }, provider: { delivered: 385, delayed: 19, failed: 6 },
    suppressions: { service_pause_cooldown: 20 }, pause: { eventCreatedAt: at, acceptedAt: at, pending: 0 },
    tracking: { lastPollAt: at, lastWebhookAt: at, pollErrorCode: null, pollBackoffUntil: null } } });

let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });
  const page = await browser.newPage();
  page.setDefaultTimeout(10_000);
  const errors = [];
  const writes = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    localStorage.setItem("cc_switch_router_locale_v1", "en");
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({ authDeviceId: "test", publicKey: "test", privateKey: "test",
      accessToken: "test-diagnostics", expiresAt: "2099-01-01T00:00:00Z" }));
  });
  let admin = true;
  let fail = false;
  const requests = [];
  const json = (route, body, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    if (route.request().method() !== "GET") { writes.push(path); return json(route, {}, 405); }
    if (path === "/v1/auth/home") return json(route, { hasActivity: true, entry: null, source: null });
    if (path === "/v1/auth/session/me") return json(route, { authenticated: true, isAdmin: admin,
      user: { id: "admin", email: "admin@example.test" }, password: { hasPassword: true, version: 1 } });
    if (path === "/v1/admin/market-notifications") {
      requests.push(url.search);
      assert.equal(route.request().headers().authorization, "Bearer test-diagnostics");
      if (!admin) return json(route, { message: "Forbidden" }, 403);
      if (fail) return json(route, { message: "Diagnostics unavailable" }, 503);
      if (url.searchParams.get("status") === "failed") return json(route, result([row("failed-filter", "failed")]));
      if (url.searchParams.get("cursor")) return json(route, result([row("second-page", "bounced")]));
      return json(route, result([row("first-page", "delayed"), row("delivered", "delivered")], "page-2"));
    }
    if (path === "/v1/regions") return json(route, []);
    if (path === "/v1/market-wallet") return json(route, { assetCode: "USD", availableUnits: "0", postedUnits: "0", heldUnits: "0", mode: "shadow", version: 0 });
    if (path === "/v1/market/config") return json(route, { usdCnyRateMicros: 7_000_000, fxRateFresh: true });
    if (path === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
    if (path.endsWith("/deliveries") || path.endsWith("/overview")) return json(route, { deliveries: [] });
    if (path.endsWith("/rooms")) return json(route, { rooms: [], totalUnread: 0 });
    if (path.endsWith("inbox-summary")) return json(route, { pendingRequests: 0 });
    if (path === "/v1/notifications") return json(route, { items: [], nextCursor: null, unread: 0, readThrough: null });
    return json(route, {});
  });
  await page.goto(`${base}/operations/?section=deliveries`);
  const panel = page.getByText("Marketplace email delivery", { exact: true }).locator("xpath=ancestor::*[.//table][1]");
  await panel.getByText("contract-first-page", { exact: true }).waitFor();
  for (const [label, value] of [["Gateway accepted", "410"], ["Delivered", "385"], ["Delayed", "19"], ["Failed / rejected", "6"]]) {
    assert.equal(await panel.locator("dl > div").filter({ has: page.locator("dt", { hasText: label }) }).locator("dd").textContent(), value);
  }
  const firstRow = panel.getByRole("row").filter({ hasText: "contract-first-page" });
  assert.match(await firstRow.textContent(), /Gateway accepted/);
  assert.match(await firstRow.textContent(), /Delivery delayed/);
  assert.ok(!(await firstRow.textContent()).includes("Delivered to mail server"));
  assert.equal(await panel.getByRole("button", { name: "Previous", exact: true }).isDisabled(), true);
  await panel.getByRole("button", { name: "Next", exact: true }).click();
  await panel.getByText("contract-second-page", { exact: true }).waitFor();
  assert.ok(requests.some((query) => query.includes("cursor=page-2")));
  assert.equal(await panel.getByRole("button", { name: "Next", exact: true }).isDisabled(), true);
  await panel.getByRole("button", { name: "Previous", exact: true }).click();
  await panel.getByText("contract-first-page", { exact: true }).waitFor();
  console.log("PASS API acceptance vs final delivery, counts and cursor navigation");

  // The HeroUI Select is a button/listbox, not an HTML select.
  await panel.getByRole("button").filter({ hasText: "All statuses" }).click();
  await page.getByRole("option", { name: "Failed", exact: true }).click();
  await panel.getByText("contract-failed-filter", { exact: true }).waitFor();
  assert.ok(requests.at(-1).includes("status=failed"));
  assert.ok(!requests.at(-1).includes("cursor="), "status changes reset pagination");
  fail = true;
  await panel.getByRole("button", { name: "Reload", exact: true }).click();
  await panel.getByText("Diagnostics unavailable", { exact: true }).waitFor();
  fail = false;
  await panel.getByRole("button", { name: "Reload", exact: true }).click();
  await panel.getByText("Diagnostics unavailable", { exact: true }).waitFor({ state: "hidden" });
  await panel.getByText("contract-failed-filter", { exact: true }).waitFor();
  console.log("PASS server-side filtering, page reset and failed-read recovery");

  await page.getByRole("button", { name: "Language", exact: true }).click();
  await page.getByText("市场邮件投递", { exact: true }).waitFor();
  await page.getByText("已配置签名回调", { exact: false }).waitFor();
  admin = false;
  const beforeNonAdmin = requests.length;
  await page.reload();
  await page.getByText("Admin access required", { exact: true }).waitFor();
  assert.equal(requests.length, beforeNonAdmin, "non-admins do not even load diagnostics");
  assert.equal(await page.getByText("Marketplace email delivery", { exact: true }).count(), 0);
  assert.deepEqual(writes, []);
  assert.deepEqual(errors, []);
  console.log("PASS bilingual copy, admin-only visibility and read-only interaction");

  // The shared navbar is exercised separately from the operator delivery view.
  const inbox = await browser.newPage({ viewport: { width: 375, height: 900 } });
  inbox.setDefaultTimeout(10_000);
  inbox.on("pageerror", error => errors.push(error.message));
  await inbox.addInitScript(() => {
    localStorage.setItem("cc_switch_router_locale_v1", "en");
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({ authDeviceId: "test", publicKey: "test", privateKey: "test",
      accessToken: "test-center", expiresAt: "2099-01-01T00:00:00Z" }));
  });
  const notice = (id, category, eventKind, payload = {}) => ({ id, category, eventKind, objectKind: "client",
    objectId: "client-one", payload, readAt: null, createdAt: at });
  let actor = "buyer", feedError = false, malformed = false, holdFeed = false, releaseFeed;
  const notices = [notice("market:wallet", "wallet", "topup_credited", { amountMinor: 100 }),
    notice("market:service", "service", "contract_suspended"),
    notice("client:offline", "client", "client_offline", { clientLabel: "<script>not-html</script>" })];
  const older = notice("client:older", "client", "client_recovered", { clientLabel: "older-client" });
  const system = notice("alert:owner", "system", "operator_alert_firing", { title: "FD pressure" });
  const centerWrites = [], feedRequests = [];
  await inbox.route("**/v1/**", async route => {
    const url = new URL(route.request().url()), path = url.pathname;
    if (path === "/v1/auth/home") return json(route, { hasActivity: true, entry: null, source: null });
    if (path === "/v1/auth/session/me") return json(route, { authenticated: !!actor, isAdmin: false,
      user: actor ? { id: actor, email: `${actor}@example.test` } : null, password: { hasPassword: true, version: 1 } });
    if (path === "/v1/notifications") {
      feedRequests.push(url.search);
      const rows = actor === "owner" ? [system] : [...notices, older];
      const body = { items: url.searchParams.has("cursor") ? [older] : rows.slice(0, actor === "owner" ? 1 : 3),
        nextCursor: actor !== "owner" && !url.searchParams.has("cursor") ? "older-page" : null,
        unread: rows.filter(n => !n.readAt).length, readThrough: "v1.mock-snapshot" };
      if (holdFeed) { holdFeed = false; await new Promise(ready => { releaseFeed = ready; }); }
      if (feedError) return json(route, { message: "secret raw upstream error" }, 503);
      if (malformed) return json(route, { ...body, items: [...body.items, null, { ...older, createdAt: "bad" }] });
      return json(route, body);
    }
    if (path.startsWith("/v1/notifications/") && path.endsWith("/read")) {
      assert.equal(route.request().method(), "POST");
      const id = decodeURIComponent(path.split("/")[3]);
      centerWrites.push({ id, body: route.request().postDataJSON() });
      const rows = actor === "owner" ? [system] : [...notices, older];
      for (const row of rows) if (id === "all" || row.id === id) row.readAt = at;
      return json(route, { unread: rows.filter(n => !n.readAt).length, readAt: at });
    }
    if (path === "/v1/regions") return json(route, []);
    if (path === "/v1/market-wallet") return json(route, { assetCode: "USD", availableUnits: "0", postedUnits: "0", heldUnits: "0", version: 0 });
    if (path === "/v1/market/config") return json(route, { usdCnyRateMicros: 7_000_000, fxRateFresh: true });
    if (path === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
    if (path.endsWith("/rooms")) return json(route, { rooms: [], totalUnread: 0 });
    if (path.endsWith("inbox-summary")) return json(route, { pendingRequests: 0 });
    return json(route, {});
  });
  await inbox.goto(`${base}/settings/`);
  const bell = () => inbox.getByRole("button", { name: /^Notifications(?:, \d+ unread)?$/ });
  await bell().click();
  const center = inbox.getByRole("region", { name: "Notification center", exact: true });
  await center.getByText(/^Client is offline/).waitFor();
  await center.getByText("Wallet & trading", { exact: true }).waitFor();
  await center.getByText("Rental services", { exact: true }).waitFor();
  await center.getByText("<script>not-html</script>", { exact: true }).waitFor();
  assert.equal(await center.locator("script").count(), 0);
  assert.equal(await center.getByText("Owner alerts", { exact: true }).count(), 0);
  for (const width of [320, 375, 768, 1440]) {
    await inbox.setViewportSize({ width, height: 900 });
    await inbox.waitForFunction(() => {
      const el = document.querySelector('[role="region"][aria-label="Notification center"]');
      const rect = el.getBoundingClientRect();
      return rect.left >= 15 && rect.right <= innerWidth - 15;
    });
  }
  await center.getByRole("button", { name: "Load more", exact: true }).click();
  await center.getByText("older-client", { exact: true }).waitFor();
  assert.ok(feedRequests.includes("?cursor=older-page"));
  const oldCount = await center.locator("li").count();
  await center.locator("li").filter({ hasText: "Client is offline" }).getByRole("button", { name: "Mark as read", exact: true }).click();
  await center.locator("li").filter({ hasText: "Client is offline" }).getByRole("button", { name: "Mark as read", exact: true }).waitFor({ state: "hidden" });
  assert.equal(await center.locator("li").count(), oldCount);
  await center.getByRole("button", { name: "Mark all read", exact: true }).click();
  await center.getByRole("button", { name: "Mark as read", exact: true }).first().waitFor({ state: "hidden" });
  await center.getByRole("button", { name: "Reload", exact: true }).waitFor({ state: "visible" });
  await inbox.waitForFunction(() => !document.querySelector('[role="region"][aria-label="Notification center"] button[aria-label="Reload"]').disabled);
  assert.deepEqual(centerWrites.at(-1), { id: "all", body: { through: "v1.mock-snapshot" } });
  assert.equal(await center.getByRole("button", { name: "Mark as read", exact: true }).count(), 0);
  await inbox.keyboard.press("Escape");
  assert.equal(await bell().evaluate(el => el === document.activeElement), true);
  await bell().click();
  await center.getByText(/^Client is offline/).waitFor();
  feedError = true;
  await center.getByRole("button", { name: "Reload", exact: true }).click();
  await center.getByRole("alert").waitFor();
  assert.doesNotMatch(await center.textContent(), /secret raw upstream/);
  feedError = false; malformed = true;
  await center.getByRole("button", { name: "Reload", exact: true }).click();
  await center.getByRole("alert").waitFor({ state: "hidden" });
  assert.equal(await center.locator("li").count(), 3);
  malformed = false;
  holdFeed = true;
  await center.getByRole("button", { name: "Reload", exact: true }).click();
  const heldDeadline = Date.now() + 5000;
  while (!releaseFeed && Date.now() < heldDeadline) await new Promise(ready => setTimeout(ready, 10));
  assert.equal(typeof releaseFeed, "function");
  actor = "owner";
  await inbox.evaluate(() => window.dispatchEvent(new Event("router-auth-changed")));
  await inbox.getByRole("button", { name: "owner@example.test", exact: true }).waitFor();
  await bell().click();
  await center.getByText("FD pressure", { exact: true }).waitFor();
  releaseFeed();
  await center.getByText("Owner alerts", { exact: true }).waitFor();
  assert.equal(await center.getByRole("link", { name: "View details", exact: true }).count(), 0,
    "Owner identity alone does not grant access to the administrator metrics page");
  assert.equal(await center.getByText(/^Client is offline/).count(), 0, "old-account response cannot repopulate the new inbox");
  await inbox.getByRole("button", { name: "Language", exact: true }).click();
  await inbox.getByRole("button", { name: /^通知，/ }).click();
  await inbox.getByRole("region", { name: "通知中心", exact: true }).getByText(/^系统告警/).waitFor();
  actor = "";
  await inbox.evaluate(() => window.dispatchEvent(new Event("router-auth-changed")));
  await inbox.getByRole("button", { name: /^通知/ }).waitFor({ state: "hidden" });
  assert.deepEqual(errors, []);
  console.log("PASS unified event categories, scoped account changes, read boundaries, pagination, recovery, keyboard, bilingual copy and four viewport widths");
} finally {
  await browser?.close();
  await new Promise((closed) => server.close(closed));
}
