import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Local exported UI and mocked APIs only. No screenshots or real money operations.
const outputDir = fileURLToPath(new URL("../out/", import.meta.url));
let baseUrl = process.env.NAVBAR_WALLET_TEST_URL;
let server;
if (!baseUrl) {
  server = createServer(async (request, response) => {
    try {
      const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
      const path = resolve(outputDir, `.${pathname.endsWith("/") ? `${pathname}index.html` : pathname}`);
      if (!path.startsWith(`${resolve(outputDir)}${sep}`)) return response.writeHead(403).end();
      const body = await readFile(path);
      const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".txt": "text/plain", ".svg": "image/svg+xml", ".ttf": "font/ttf", ".woff2": "font/woff2" };
      response.writeHead(200, { "Content-Type": types[extname(path)] || "application/octet-stream", "Cache-Control": "no-store" });
      response.end(body);
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((ready, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", ready); });
  baseUrl = `http://127.0.0.1:${server.address().port}`;
}

let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.clock.install();
  await page.addInitScript(() => {
    localStorage.setItem("cc_switch_router_locale_v1", "en");
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({
      authDeviceId: "test", publicKey: "test", privateKey: "test",
      accessToken: "test-navbar-wallet", expiresAt: "2099-01-01T00:00:00Z",
    }));
  });

  let actor = null;
  let available = "216000000"; // $25 available, distinct from the $40 posted balance.
  let walletCalls = 0;
  let responseMode = "ready";
  let holdNext = false;
  let releaseHeld;
  const json = (route, body, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/v1/auth/session/me") return json(route, actor
      ? { authenticated: true, isAdmin: false, user: { id: actor, email: `${actor}@example.invalid` } }
      : { authenticated: false, isAdmin: false });
    if (path === "/v1/market-wallet") {
      walletCalls++;
      const body = { assetCode: "USD", postedUnits: "345600000", heldUnits: "129600000", availableUnits: available,
        version: walletCalls, mode: "enabled", canTopup: false, hasWalletActivity: true };
      if (holdNext) {
        holdNext = false;
        await new Promise((release) => { releaseHeld = release; });
      }
      if (responseMode === "error") return json(route, { message: "offline" }, 503);
      return json(route, responseMode === "malformed" ? { availableUnits: "not-money" } : body);
    }
    if (path === "/v1/regions") return json(route, [{ name: "Long Router region name", url: baseUrl }]);
    if (path === "/v1/market-billing/config") return json(route, { usdCnyRateMicros: 7_000_000, fxRateFresh: true });
    if (path === "/v1/market-wallet/c2c/policy") return json(route, { policy: { version: 1, enabled: false,
      admissionMinor: 10000, minMinor: 100, maxMinor: 500000, paymentMinutes: 30 }, available: false,
      blockers: ["C2C_DISABLED"], canManagePolicy: false, verified: true, termsVersion: 1, claimGraceSeconds: 120, firstPurchaseMaxMinor: 10000 });
    if (path === "/v1/market-wallet/c2c/orders") return json(route, { items: [], next: null });
    if (path === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
    if (path.endsWith("inbox-summary")) return json(route, { pendingRequests: 0 });
    if (path === "/v1/market-providers") return json(route, { generation: {
      id: "test", algorithmVersion: "test", publishedAt: new Date().toISOString(), stale: false,
      weights: { serviceQuality: 50, effectiveChoice: 20, fulfillment: 20, supplyBreadth: 10 },
      minimumIndependentBuyers: 3, minimumObservationDays: 7,
    }, providers: [] });
    if (path.endsWith("/channels")) return json(route, []);
    if (path.endsWith("/topups") || path.endsWith("/transactions")) return json(route, { items: [], nextCursor: null });
    if (path.endsWith("/rooms")) return json(route, { rooms: [], totalUnread: 0 });
    return json(route, {});
  });
  const balance = page.locator("[data-wallet-balance]");
  const expectAmount = async (amount) => {
    await balance.filter({ hasText: amount }).waitFor();
    assert.match(await balance.getAttribute("aria-label"), new RegExp(amount.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  };
  const changed = (user) => page.evaluate((id) => window.dispatchEvent(new CustomEvent("router-market-balance-changed", { detail: id })), user);
  const heldResponse = async () => {
    const deadline = Date.now() + 5_000;
    while (!releaseHeld && Date.now() < deadline) await new Promise((ready) => setTimeout(ready, 10));
    assert.equal(typeof releaseHeld, "function", "expected an in-flight wallet read");
    const release = releaseHeld;
    releaseHeld = undefined;
    return release;
  };

  await page.goto(`${baseUrl}/account/api-keys/`);
  await page.getByRole("button", { name: "Login", exact: true }).waitFor();
  assert.equal(await balance.count(), 0);
  assert.equal(walletCalls, 0);
  actor = "buyer";
  holdNext = true;
  await page.evaluate(() => window.dispatchEvent(new Event("router-auth-changed")));
  await balance.locator("[aria-live]").filter({ hasText: "…" }).waitFor();
  assert.equal(await balance.getAttribute("aria-busy"), "true");
  (await heldResponse())();
  await expectAmount("$25.00");
  assert.equal(walletCalls, 1);
  assert.equal(await balance.getAttribute("href"), "/account/wallet/");
  console.log("PASS anonymous privacy, loading state, and available (not posted) balance");

  available = "181440000";
  await changed("buyer");
  await expectAmount("$21.00");
  const beforeForeignSignal = walletCalls;
  await changed("another-user");
  await page.clock.runFor(50);
  assert.equal(walletCalls, beforeForeignSignal);
  available = "172800000";
  await page.evaluate(() => window.dispatchEvent(new StorageEvent("storage", {
    key: "router_market_balance_revision_v1", newValue: JSON.stringify({ actor: "buyer", nonce: "another-tab" }),
  })));
  await expectAmount("$20.00");

  // Queue a new read if the balance changed while the previous read was in flight.
  holdNext = true;
  await changed("buyer");
  const releaseQueued = await heldResponse();
  available = "164160000";
  await changed("buyer");
  releaseQueued();
  await expectAmount("$19.00");
  console.log("PASS local/cross-tab invalidation, actor isolation, and in-flight refresh queue");

  available = "155520000";
  await page.clock.fastForward(15_000);
  await expectAmount("$18.00");
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  const beforeHidden = walletCalls;
  available = "146880000";
  await changed("buyer");
  await page.clock.fastForward(45_000);
  assert.equal(walletCalls, beforeHidden);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expectAmount("$17.00");
  console.log("PASS 15-second external updates and background-page polling suspension");

  responseMode = "error";
  await changed("buyer");
  await balance.locator("[aria-live]").filter({ hasText: "—" }).waitFor();
  assert.match(await balance.getAttribute("aria-label"), /Balance unavailable/);
  responseMode = "malformed";
  await changed("buyer");
  await page.clock.runFor(100);
  assert.equal(await balance.getAttribute("data-state"), "error");
  responseMode = "ready";
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await expectAmount("$17.00");
  await page.getByRole("button", { name: "Show prices in CNY" }).click();
  await expectAmount("≈¥119.00");
  await page.getByRole("button", { name: "Show prices in USD" }).click();
  await expectAmount("$17.00");
  await page.getByRole("button", { name: "Language", exact: true }).click();
  assert.match(await balance.getAttribute("aria-label"), /平台余额/);
  console.log("PASS failed/malformed response recovery, USD/CNY conversion, and bilingual labels");

  available = "9223372036854775807";
  await changed("buyer");
  await balance.filter({ hasText: "$1,067,519,911,673." }).waitFor();
  for (const width of [320, 375, 414, 768, 1024, 1280, 1536]) {
    await page.setViewportSize({ width, height: 900 });
    const layout = await balance.evaluate((link) => {
      const header = link.closest("header");
      const rect = link.getBoundingClientRect();
      const headerRect = header.getBoundingClientRect();
      const actions = link.parentElement;
      const nextRect = link.nextElementSibling.getBoundingClientRect();
      return { width: rect.width, left: rect.left, right: rect.right, headerLeft: headerRect.left, headerRight: headerRect.right,
        nextLeft: nextRect.left, actionsRight: actions.getBoundingClientRect().right,
        overflow: document.documentElement.scrollWidth > window.innerWidth };
    });
    assert.ok(layout.width >= 40, `${width}px readable balance target: ${JSON.stringify(layout)}`);
    assert.ok(layout.left >= layout.headerLeft && layout.right <= layout.headerRight + 1, `${width}px balance inside header`);
    assert.ok(layout.nextLeft >= layout.right - 1 && layout.actionsRight <= width, `${width}px actions do not overlap`);
    assert.equal(layout.overflow, false, `${width}px no page overflow`);
  }
  await page.getByRole("button", { name: /CNY/ }).focus();
  await page.keyboard.press("Shift+Tab");
  assert.ok(await balance.evaluate((link) => link === document.activeElement && getComputedStyle(link).outlineStyle !== "none"));
  console.log("PASS long/precise balances at seven viewport widths and keyboard focus");

  // An aborted response belonging to the previous account must never overwrite the next account.
  holdNext = true;
  await changed("buyer");
  const releasePreviousActor = await heldResponse();
  actor = "second";
  available = "8640000";
  await page.evaluate(() => window.dispatchEvent(new Event("router-auth-changed")));
  await expectAmount("$1.00");
  releasePreviousActor();
  await page.clock.runFor(100);
  await expectAmount("$1.00");
  actor = null;
  await page.evaluate(() => window.dispatchEvent(new Event("router-auth-changed")));
  await page.getByRole("button", { name: "登录", exact: true }).waitFor();
  assert.equal(await balance.count(), 0);
  const beforeLoggedOut = walletCalls;
  await page.clock.fastForward(30_000);
  assert.equal(walletCalls, beforeLoggedOut);
  console.log("PASS account-switch response isolation and logout cleanup");

  actor = "buyer";
  available = "86400000";
  await page.evaluate(() => window.dispatchEvent(new Event("router-auth-changed")));
  await expectAmount("$10.00");
  await page.getByRole("navigation", { name: "控制台分区" }).getByRole("link", { name: "服务商", exact: true }).click();
  await page.waitForURL(`${baseUrl}/providers/`);
  const shortcut = page.getByRole("complementary", { name: "平台钱包", exact: true });
  await shortcut.filter({ hasText: "$10.00" }).waitFor();
  const beforeSharedPoll = walletCalls;
  available = "77760000";
  await changed("buyer");
  await expectAmount("$9.00");
  await shortcut.filter({ hasText: "$9.00" }).waitFor();
  assert.equal(walletCalls, beforeSharedPoll + 1, "navigation and shortcut share one wallet request");
  responseMode = "error";
  await changed("buyer");
  await shortcut.filter({ hasText: "余额暂不可用" }).waitFor();
  assert.ok(!(await shortcut.textContent()).includes("$9.00"), "shortcut must not advertise a stale spendable balance");
  responseMode = "ready";
  await changed("buyer");
  await expectAmount("$9.00");
  await balance.click();
  await page.waitForURL(`${baseUrl}/account/wallet/`);
  await expectAmount("$9.00");
  assert.deepEqual(errors, []);
  console.log("PASS shared shortcut request, wallet navigation, retained balance across routes, and no browser exceptions");
} finally {
  await browser?.close();
  if (server) await new Promise((closed) => server.close(closed));
}
