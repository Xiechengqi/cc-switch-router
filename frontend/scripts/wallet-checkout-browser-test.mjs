import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Gateway and Router API calls are mocked. The local exported frontend is
// served automatically; no order, transfer, or screenshot is created.
const outputDir = resolve(fileURLToPath(new URL("../out/", import.meta.url)));
let baseUrl = process.env.WALLET_CHECKOUT_TEST_URL;
let server;
if (!baseUrl) {
  server = createServer(async (request, response) => {
    let pathname;
    try {
      pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    } catch {
      response.writeHead(400).end();
      return;
    }
    const filePath = resolve(outputDir, `.${pathname.endsWith("/") ? `${pathname}index.html` : pathname}`);
    if (!filePath.startsWith(`${outputDir}${sep}`)) {
      response.writeHead(403).end();
      return;
    }
    try {
      const body = await readFile(filePath);
      const types = {
        ".css": "text/css", ".html": "text/html", ".js": "text/javascript",
        ".svg": "image/svg+xml", ".txt": "text/plain", ".woff2": "font/woff2",
      };
      response.writeHead(200, {
        "Content-Type": types[extname(filePath)] || "application/octet-stream",
        "Content-Length": body.length,
        "Cache-Control": "no-store",
      });
      response.end(request.method === "HEAD" ? undefined : body);
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((resolveStart, rejectStart) => {
    server.once("error", rejectStart);
    server.listen(0, "127.0.0.1", resolveStart);
  });
  baseUrl = `http://127.0.0.1:${server.address().port}`;
}

let browser;

try {
  browser = await chromium.launch({
    executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome",
    headless: true,
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage();
  await page.addInitScript(() => {
    localStorage.setItem("cc_switch_router_locale_v1", "en");
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({
      authDeviceId: "test-device",
      publicKey: "test-public-key",
      privateKey: "test-private-key",
      accessToken: "test-wallet-checkout",
      expiresAt: "2099-01-01T00:00:00Z",
    }));
  });
  const checkoutUrl = "https://www.dujiaopay.com/checkout/ct_test";
  let canPay = true;
  let expiresAt = new Date(Date.now() + 60_000).toISOString();
  let gatewayCalls = 0;
  let externalVisits = 0;
  // Context routes also cover the newly opened checkout tab. Page routes
  // cannot reliably intercept a popup's first navigation.
  await page.context().route("**/v1/**", async (route) => {
    await route.fulfill({ contentType: "application/json", body: "{}" });
  });
  await page.context().route("**/v1/market-wallet/topups/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    assert.equal(route.request().headers().authorization, "Bearer test-wallet-checkout");
    if (path.endsWith("/gateway-status")) {
      gatewayCalls += 1;
      await route.fulfill({ contentType: "application/json", body: JSON.stringify({
        gatewayStatus: "pending",
        canPay,
        payabilityReason: canPay ? "payable" : "payment_evidence",
        canCancel: false,
        canAbandon: false,
        selectionDeadline: null,
        expiresAt,
        serverNow: new Date().toISOString(),
      }) });
      return;
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      id: "order-1",
      channelId: "channel-1",
      amountMinor: 100,
      status: "pending",
      checkoutUrl,
      createdAt: new Date().toISOString(),
      autoCloseAt: null,
      buyerAbandonedAt: null,
      reviewReason: null,
    }) });
  });
  await page.context().route("https://www.dujiaopay.com/checkout/**", async (route) => {
    externalVisits += 1;
    await route.fulfill({ contentType: "text/html", body: "<title>Mock checkout</title>" });
  });

  await page.goto(`${baseUrl}/wallet-checkout/?topup=order-1`);
  await page.waitForURL(checkoutUrl);
  assert.equal(gatewayCalls, 1);
  assert.equal(externalVisits, 1);

  canPay = false;
  await page.goto(`${baseUrl}/wallet-checkout/?topup=order-1`);
  await page.getByText(/cannot be safely opened for payment/i).waitFor();
  assert.equal(gatewayCalls, 2);
  assert.equal(externalVisits, 1);

  canPay = true;
  expiresAt = new Date(Date.now() - 1_000).toISOString();
  await page.goto(`${baseUrl}/wallet-checkout/?topup=order-1`);
  await page.getByText(/cannot be safely opened for payment/i).waitFor();
  assert.equal(gatewayCalls, 3);
  assert.equal(externalVisits, 1);

  await page.goto(`${baseUrl}/wallet-checkout/`);
  await page.getByText(/cannot be safely opened for payment/i).waitFor();
  assert.equal(gatewayCalls, 3);
  assert.equal(externalVisits, 1);

  // A review hold must give the buyer an observable check result and a safe
  // way to stop using Router's payment link without claiming gateway closure.
  let topup = {
    id: "order-2",
    channelId: "channel-1",
    amountMinor: 100,
    status: "review_required",
    checkoutUrl: null,
    createdAt: new Date().toISOString(),
    autoCloseAt: null,
    buyerAbandonedAt: null,
    reviewReason: "terminal_payment_marker",
    buyerRecheckRequestedAt: null,
    buyerRecheckCompletedAt: null,
    buyerRecheckResult: null,
    lastGatewayStatus: "expired",
    lastGatewayCheckedAt: new Date().toISOString(),
  };
  let recheckCalls = 0;
  let stopCalls = 0;
  await page.context().route("**/v1/auth/session/me", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      authenticated: true, user: { id: "buyer", email: "buyer@example.invalid" }, isAdmin: false,
    }) });
  });
  await page.context().route("**/v1/market-wallet", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      assetCode: "USD", postedUnits: "0", heldUnits: "0", availableUnits: "0",
      version: 1, mode: "enabled", canTopup: true, hasWalletActivity: false,
    }) });
  });
  await page.context().route("**/v1/market-wallet/channels", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify([{ id: "channel-1", displayName: "DujiaoPay", gatewayKind: "dujiaopay" }]) });
  });
  await page.context().route("**/v1/market-wallet/topups", async (route) => {
    if (route.request().method() === "POST") {
      await route.fulfill({ contentType: "application/json", body: JSON.stringify(topup) });
      return;
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [topup], nextCursor: null }) });
  });
  await page.context().route("**/v1/market-wallet/transactions", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [], nextCursor: null }) });
  });
  await page.context().route("**/v1/market-provider-finance/me", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ isProvider: false }) });
  });
  await page.context().route("**/v1/market-provider-finance/withdrawals", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [], nextCursor: null }) });
  });
  await page.context().route("**/v1/market-wallet/topups/order-2/recheck", async (route) => {
    recheckCalls += 1;
    topup = { ...topup, buyerRecheckRequestedAt: new Date().toISOString(), buyerRecheckResult: "queued" };
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ queued: true, order: topup }) });
  });
  await page.context().route("**/v1/market-wallet/topups/order-2/stop", async (route) => {
    stopCalls += 1;
    topup = { ...topup, buyerAbandonedAt: new Date().toISOString(), checkoutUrl: null };
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(topup) });
  });
  await page.goto(`${baseUrl}/account/wallet/`);
  await page.getByRole("button", { name: "Check payment" }).waitFor();
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    assert.ok(overflow <= 1, `wallet page overflows by ${overflow}px at ${width}px`);
  }
  await page.getByRole("button", { name: "Check payment" }).click();
  await page.getByText(/Payment verification is scheduled/i).waitFor();
  assert.equal(recheckCalls, 1);
  await page.getByRole("button", { name: "Stop paying" }).click();
  await page.getByRole("button", { name: "Stop paying", exact: true }).last().click();
  await page.getByRole("status").filter({ hasText: /You stopped paying in Router/i }).first().waitFor();
  assert.equal(stopCalls, 1);
  assert.equal(topup.status, "review_required");

  // A committed order with no payable capability stays visible in the new
  // tab. The wallet must not pretend it is ready to choose a payment method.
  topup = {
    id: "order-1", channelId: "channel-1", amountMinor: 100,
    status: "pending", checkoutUrl,
    createdAt: new Date().toISOString(), autoCloseAt: null,
    buyerAbandonedAt: null, reviewReason: null,
    buyerRecheckRequestedAt: null, buyerRecheckCompletedAt: null,
    buyerRecheckResult: null, lastGatewayStatus: "pending",
    lastGatewayCheckedAt: new Date().toISOString(),
  };
  canPay = false;
  expiresAt = new Date(Date.now() + 60_000).toISOString();
  await page.goto(`${baseUrl}/account/wallet/`);
  await page.getByLabel("Amount (USD)").fill("1.00");
  const blockedTabPromise = page.waitForEvent("popup");
  await page.getByRole("button", { name: "Continue to payment" }).click();
  const blockedTab = await blockedTabPromise;
  await blockedTab.getByText(/Payment or review evidence needs checking/i).waitFor();
  assert.match(blockedTab.url(), /\/wallet-checkout\/\?topup=order-1$/);
  await page.getByText("Payment unavailable").waitFor();
  assert.equal(await page.getByRole("link", { name: "Continue payment" }).count(), 0);
  assert.equal(externalVisits, 1);
  await blockedTab.close();

  canPay = true;
  await page.getByRole("button", { name: "Refresh gateway status" }).click();
  const resumeLink = page.getByRole("link", { name: "Continue payment" });
  await resumeLink.waitFor();
  const resumedTabPromise = page.waitForEvent("popup");
  await resumeLink.click();
  const resumedTab = await resumedTabPromise;
  await resumedTab.waitForURL(checkoutUrl);
  assert.equal(externalVisits, 2);
  await resumedTab.close();
} finally {
  await browser?.close();
  if (server) await new Promise((resolveStop, rejectStop) => {
    server.close((error) => error ? rejectStop(error) : resolveStop());
  });
}
