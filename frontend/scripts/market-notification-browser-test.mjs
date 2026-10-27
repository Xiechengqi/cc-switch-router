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
    if (path === "/v1/market-notifications") return json(route, { items: [], nextCursor: null });
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
} finally {
  await browser?.close();
  await new Promise((closed) => server.close(closed));
}
