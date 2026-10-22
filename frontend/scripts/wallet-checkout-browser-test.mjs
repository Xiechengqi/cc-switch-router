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

// Read computed colors only, never capture or rasterize the page. A single
// color swatch lets Chromium normalize rgb/oklch into the same sRGB space.
async function assertCommissionContrast(panel) {
  const measurements = await panel.evaluate((root) => {
    const swatch = document.createElement("canvas");
    swatch.width = swatch.height = 1;
    const context = swatch.getContext("2d", { willReadFrequently: true });
    const rgba = (color) => {
      context.clearRect(0, 0, 1, 1);
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data].map((value) => value / 255);
    };
    const surface = (element) => {
      if (!element) return [1, 1, 1];
      const color = rgba(getComputedStyle(element).backgroundColor);
      const behind = color[3] < 1 ? surface(element.parentElement) : color;
      return color.slice(0, 3).map((value, i) => value * color[3] + behind[i] * (1 - color[3]));
    };
    const luminance = (color) => color.slice(0, 3).map((value) => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
      .reduce((sum, value, i) => sum + value * [0.2126, 0.7152, 0.0722][i], 0);
    const contrast = (a, b) => {
      const light = [luminance(a), luminance(b)].sort((a, b) => b - a);
      return (light[0] + 0.05) / (light[1] + 0.05);
    };
    const samples = [...root.querySelectorAll("h3,p,span,input,button")]
      .filter((element) => element.getClientRects().length && !element.closest(":disabled") &&
        (element.tagName === "INPUT" || [...element.childNodes].some((node) => node.nodeType === Node.TEXT_NODE && node.textContent.trim())))
      .map((element) => ({ text: (element.value || element.textContent).trim().slice(0, 70),
        minimum: 4.5, ratio: contrast(rgba(getComputedStyle(element).color), surface(element)) }));
    const focused = root.querySelector(":focus-visible");
    if (focused) samples.push({ text: "keyboard focus outline against its offset surface", minimum: 3,
      ratio: contrast(rgba(getComputedStyle(focused).outlineColor), surface(focused.parentElement)) });
    return samples;
  });
  assert.ok(measurements.length > 0);
  for (const { text, ratio, minimum } of measurements) assert.ok(ratio >= minimum, `commission contrast ${ratio.toFixed(2)}: ${text}`);
}

try {
  browser = await chromium.launch({
    executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome",
    headless: true,
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage();
  const pageErrors = [];
  page.context().on("page", (tab) => tab.on("pageerror", (error) => pageErrors.push(error.message)));
  page.on("pageerror", (error) => pageErrors.push(error.message));
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
      creditAmountMinor: 100, platformFeeMinor: 0, platformFeeRateBps: 0, policyVersion: 1,
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
    amountMinor: 105,
    creditAmountMinor: 100, platformFeeMinor: 5, platformFeeRateBps: 500, policyVersion: 1,
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
  let transferCalls = 0;
  let topupCreates = 0;
  let rejectNextTopupQuote = false;
  let feeBps = 500;
  const transferKeys = [];
  let transferAmount = "8640001";
  await page.context().route("**/v1/market-wallet/transfer-capacity", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ transferableUnits: transferAmount, committedUnits: "0", dailyRemainingUnits: "86400000", enabled: true, verified: true }) });
  });
  await page.context().route("**/v1/market-wallet/transfer-quotes", async (route) => {
    const input = route.request().postDataJSON();
    assert.equal(input.recipientEmail, "recipient@example.invalid");
    assert.equal(input.all, true);
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ id: "transfer-quote", senderEmail: "buyer@example.invalid", recipientEmail: input.recipientEmail, amountUnits: transferAmount, expiresAt: new Date(Date.now() + 900_000).toISOString() }) });
  });
  await page.context().route("**/v1/market-wallet/transfer-quotes/*/code", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ id: "transfer-code", expiresAt: new Date(Date.now() + 600_000).toISOString(), resendAfterSeconds: 60 }) });
  });
  await page.context().route("**/v1/market-wallet/transfers", async (route) => {
    transferCalls += 1;
    transferKeys.push(route.request().headers()["idempotency-key"]);
    const input = route.request().postDataJSON();
    assert.equal(input.quoteId, "transfer-quote");
    assert.equal(input.challengeId, "transfer-code");
    if (input.code !== "123456") {
      await route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ code: "WALLET_TRANSFER_CODE_INVALID", message: "invalid" }) });
      return;
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ id: "transfer-1", senderEmail: "buyer@example.invalid", recipientEmail: "recipient@example.invalid", amountUnits: transferAmount, journalId: "journal-1", createdAt: new Date().toISOString() }) });
    transferAmount = "0";
  });
  await page.context().route("**/v1/market-wallet/topup-quotes", async (route) => {
    const { amountMinor } = route.request().postDataJSON();
    const feeMinor = Math.floor((amountMinor * feeBps + 5000) / 10000);
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ id: `quote-${amountMinor}`, creditMinor: amountMinor, feeBps, feeMinor, payMinor: amountMinor + feeMinor, policyVersion: 1, channelRevision: 1, channelSettingsVersion: 1, expiresAt: new Date(Date.now() + 300_000).toISOString() }) });
  });
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
      topupCreates += 1;
      const input = route.request().postDataJSON();
      assert.equal(input.quoteId, "quote-100");
      assert.equal(input.amountMinor, 100);
      assert.equal(input.channelId, "channel-1");
      if (rejectNextTopupQuote) {
        rejectNextTopupQuote = false; feeBps = 600;
        await route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ code: "MARKET_TOPUP_QUOTE_CHANGED", message: "review new total" }) });
        return;
      }
      await route.fulfill({ contentType: "application/json", body: JSON.stringify(topup) });
      return;
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [topup], nextCursor: null }) });
  });
  await page.context().route("**/v1/market-wallet/transactions", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [], nextCursor: null }) });
  });
  await page.context().route("**/v1/market-notifications**", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      items: [], nextCursor: null, unread: 0,
    }) });
  });
  await page.context().route("**/v1/market-provider-finance/me", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ isProvider: false }) });
  });
  await page.context().route("**/v1/market-provider-finance/withdrawals", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [], nextCursor: null }) });
  });
  await page.context().route("**/v1/market-access/dashboard", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ policies: [], counterparties: [], accessRequests: [] }) });
  });
  await page.context().route("**/v1/account/provider-contacts", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ contacts: [], updatedAt: new Date().toISOString() }) });
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
    id: "order-1", channelId: "channel-1", amountMinor: 105,
    creditAmountMinor: 100, platformFeeMinor: 5, platformFeeRateBps: 500, policyVersion: 1,
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
  await page.locator("#market-wallet-topup-amount").fill("1.00");
  const [blockedTab] = await Promise.all([
    page.waitForEvent("popup"),
    page.getByRole("button", { name: "Pay $1.05", exact: true }).click(),
  ]);
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

  // No real transfer is sent. The confirmation must retain exact sub-cent
  // units, the frozen recipient, and one idempotency key across a wrong code.
  const transferPanel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Transfer to a user", exact: true }) }).last();
  await transferPanel.getByLabel("Recipient email").fill("recipient@example.invalid");
  await transferPanel.getByLabel("Transfer all available").check();
  await transferPanel.getByRole("button", { name: "Review transfer" }).click();
  const transferDialog = page.getByRole("dialog", { name: "Confirm recipient & amount" });
  await transferDialog.getByText("recipient@example.invalid", { exact: true }).waitFor();
  await transferDialog.getByText("Exact amount: 8640001 ledger units", { exact: true }).waitFor();
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    assert.ok(await transferDialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1), `transfer dialog overflow at ${width}`);
  }
  await transferDialog.getByRole("button", { name: "Email verification code", exact: true }).click();
  await transferDialog.locator('input[autocomplete="one-time-code"]').fill("000000");
  await transferDialog.getByRole("button", { name: /^Transfer \$/ }).click();
  await transferDialog.getByRole("alert").filter({ hasText: /Incorrect verification code/ }).waitFor();
  await transferDialog.locator('input[autocomplete="one-time-code"]').fill("123456");
  await transferDialog.getByRole("button", { name: /^Transfer \$/ }).click();
  await transferPanel.getByRole("status").filter({ hasText: /Transferred/ }).waitFor();
  assert.equal(transferCalls, 2);
  assert.ok(transferKeys[0]);
  assert.equal(transferKeys[0], transferKeys[1]);

  // A changed policy refreshes the visible total but never silently submits
  // an order at the new price. A second user gesture is required.
  rejectNextTopupQuote = true;
  await page.getByRole("button", { name: "Pay $1.05", exact: true }).click();
  await page.getByRole("button", { name: "Pay $1.06", exact: true }).waitFor();
  await page.getByRole("alert").filter({ hasText: /Review the new total/ }).waitFor();
  assert.equal(topupCreates, 2);

  for (const [legacy, expected] of [["payments/", "provider-settings/"], ["market-access/", "provider-settings/"], ["billing/?tab=receivables", "earnings/"]]) {
    await page.goto(`${baseUrl}/account/${legacy}`);
    await page.waitForURL(`${baseUrl}/account/${expected}`);
  }

  // Updating one permission through the new Provider settings must preserve
  // the buyer's other allow/deny scopes and must not grant legacy credit.
  const accessData = { policies: [], accessRequests: [], counterparties: [{
    id: "trusted-buyer", buyerEmail: "trusted@example.invalid", buyerUserId: "trusted",
    revision: 7, status: "active", accessRules: [
      { productKind: "share", pricingKind: "free", decision: "allow" },
      { productKind: "share", pricingKind: "paid", decision: "deny" },
      { productKind: "client_host", pricingKind: "free", decision: "inherit" },
      { productKind: "client_host", pricingKind: "paid", decision: "deny" },
    ],
  }] };
  let accessUpdates = 0;
  await page.context().route("**/v1/market-access/dashboard", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(accessData) });
  });
  await page.context().route("**/v1/market-access/counterparties/trusted-buyer", async (route) => {
    accessUpdates += 1;
    const input = route.request().postDataJSON();
    assert.equal(input.expectedRevision, 7);
    assert.deepEqual(Object.keys(input).sort(), ["accessRules", "expectedRevision"]);
    assert.deepEqual(input.accessRules, [
      { productKind: "share", pricingKind: "free", decision: "allow" },
      { productKind: "share", pricingKind: "paid", decision: "deny" },
      { productKind: "client_host", pricingKind: "free", decision: "inherit" },
      { productKind: "client_host", pricingKind: "paid", decision: "allow" },
    ]);
    accessData.counterparties[0].accessRules = input.accessRules;
    accessData.counterparties[0].revision += 1;
    await route.fulfill({ contentType: "application/json", body: "{}" });
  });
  await page.goto(`${baseUrl}/account/provider-settings/`);
  const accessForm = page.locator("form").filter({ has: page.getByRole("heading", { name: "Add a trusted buyer", exact: true }) });
  await accessForm.getByLabel("Buyer email", { exact: true }).fill("trusted@example.invalid");
  await accessForm.getByLabel("Client Host Market · Paid", { exact: true }).check();
  await accessForm.getByRole("button", { name: "Add", exact: true }).click();
  await page.waitForFunction(() => document.querySelector('input[type="email"]')?.value === "");
  assert.equal(accessUpdates, 1);
  assert.equal(await page.locator('a[href="/account/earnings/"]').count(), 0, "ordinary buyers do not get a Provider income tab");

  let canWithdraw = true;
  await page.context().route("**/v1/market-provider-finance/me", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      isProvider: false, hasFinanceActivity: true, canWithdraw, destination: null,
      summary: { grossEarnedUnits: "864000000", commissionUnits: "86400000", netEarnedUnits: "777600000",
        pendingUnits: "0", availableUnits: "777600000", withdrawableUnits: "777600000", withdrawalHeldUnits: "0", paidUnits: "0" },
    }) });
  });
  await page.context().route("**/v1/market-provider-finance/holds", async (route) => {
    await route.fulfill({ contentType: "application/json", body: "[]" });
  });
  await page.context().route("**/v1/market-provider-finance/earnings", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [{
      id: "net-earning", amountUnits: "777600000", grossUnits: "864000000", commissionUnits: "86400000",
      commissionReversedUnits: "0", commissionRateBps: 1000, commissionPolicyVersion: 1,
      reversedUnits: "0", unlockedUnits: "777600000", heldUnits: "0", paidUnits: "0", disputed: false,
      fulfilledAt: "2026-01-01T00:00:00Z", unlockAt: "2026-01-08T00:00:00Z",
    }], nextCursor: null }) });
  });
  await page.reload();
  await page.getByRole("link", { name: "Earnings & withdrawals", exact: true }).click();
  await page.getByRole("heading", { name: "Provider earnings", exact: true }).waitFor();
  await page.getByRole("columnheader", { name: "Net earnings", exact: true }).waitFor();
  const earningSummary = page.getByText("Revenue & service fees", { exact: true });
  assert.equal(await earningSummary.locator("..").getAttribute("open"), null, "detailed accounting starts collapsed");
  await earningSummary.click();
  const earningTotals = page.locator("dl").filter({ hasText: "Fulfilled revenue (net of refunds)" });
  await earningTotals.getByText("$100.00", { exact: true }).waitFor();
  await earningTotals.getByText("$10.00", { exact: true }).waitFor();
  await earningTotals.getByText("$90.00", { exact: true }).waitFor();
  await page.getByText("Fee breakdown", { exact: true }).click();
  await page.getByText("Gross $100.00", { exact: true }).waitFor();
  await page.getByText("Service fee 10% · $10.00", { exact: true }).waitFor();
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `Provider net earnings overflow at ${width}`);
  }
  canWithdraw = false;
  await page.reload();
  await page.getByText(/Earnings history remains visible/).waitFor();

  // Owner policy edits use integer basis points and versioned writes. A
  // conflict must reload the current policy, never silently overwrite it.
  let policy = { version: 1, maintenanceFeeBps: 500, transfersEnabled: true,
    transfersPaused: false, transferMaxMinor: 500_000, transferDailyMaxMinor: 500_000,
    balanceRefundable: false, canEdit: true };
  let policyWrites = 0;
  let conflictPolicy = false;
  let commissionPolicy = { version: 1, current: { policyVersion: 1, rateBps: 1000, effectiveAt: "2026-01-01T00:00:00Z" }, scheduled: null, increaseNoticeDays: 7, canEdit: true };
  let commissionWrites = 0;
  let conflictCommission = false;
  const categories = [{ id: "marketplace", label: "Marketplace", description: "Marketplace", fieldCount: 0 }];
  const overview = Object.fromEntries(["walletPostedUnits", "walletHeldUnits", "unfulfilledUnits", "providerPendingUnits",
    "providerAvailableUnits", "providerHeldUnits", "buyerRefundHeldUnits", "ownerLossUnits", "maintenanceRevenueUnits"].map((key) => [key, "0"]));
  Object.assign(overview, { estimatedGatewayCostMinor: 0, creditedTopupMinor: 0, grossTopupMinor: 0,
    openCases: 0, openCriticalCases: 0, heldEarningLots: 0 });
  await page.context().route("**/v1/auth/session/me", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      authenticated: true, isAdmin: true, user: { id: "buyer", email: "buyer@example.invalid" },
    }) });
  });
  await page.context().route("**/v1/map-display", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      showFlows: true, showHeat: true, viewport: { visibleStartPx: 90 }, revision: "1",
    }) });
  });
  await page.context().route("**/v1/admin/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    let data = [];
    if (path === "/v1/admin/settings") data = { revision: "1", generatedAt: new Date().toISOString(),
      envPath: "/test/.env", schema: { categories, fields: [], groups: [] }, values: [], pendingRestartKeys: [] };
    else if (path === "/v1/admin/market-finance/readiness") data = { mode: "enabled", version: 1,
      newOrdersPaused: false, newWithdrawalsPaused: false, newTopupsPaused: false, codeIntegrated: true,
      legacyBlockers: [], criticalCases: 0, staleTopupReviews: 0, verifiedLiveTopups: 1,
      paymentChannelBlockers: [], canEnable: false };
    else if (path === "/v1/admin/market-finance/overview") data = overview;
    else if (path === "/v1/admin/market-finance/commission-policy") {
      if (route.request().method() === "PUT") {
        commissionWrites += 1;
        const input = route.request().postDataJSON();
        assert.deepEqual(Object.keys(input).sort(), ["expectedVersion", "rateBps"]);
        assert.equal(input.expectedVersion, commissionPolicy.version);
        if (conflictCommission) {
          conflictCommission = false;
          commissionPolicy = { ...commissionPolicy, version: commissionPolicy.version + 1 };
          await route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ message: "commission policy changed" }) });
          return;
        }
        assert.equal(input.rateBps, 1025);
        commissionPolicy = { ...commissionPolicy, version: commissionPolicy.version + 1,
          scheduled: { policyVersion: commissionPolicy.version + 1, rateBps: input.rateBps, effectiveAt: new Date(Date.now() + 7 * 86400000).toISOString() } };
      }
      data = commissionPolicy;
    }
    else if (["topups", "cases"].some((kind) => path === `/v1/admin/market-finance/${kind}`)) data = { items: [], nextCursor: null };
    else if (path === "/v1/admin/market-finance/wallet-policy") {
      if (route.request().method() === "PUT") {
        policyWrites += 1;
        const input = route.request().postDataJSON();
        assert.equal(input.expectedVersion, policy.version);
        assert.equal(input.maintenanceFeeBps, conflictPolicy ? 600 : 525);
        assert.equal(input.canEdit, undefined);
        assert.equal(input.balanceRefundable, undefined);
        if (conflictPolicy) {
          conflictPolicy = false;
          policy = { ...policy, version: policy.version + 1, maintenanceFeeBps: 700 };
          await route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ message: "wallet policy changed" }) });
          return;
        }
        policy = { ...policy, ...input, version: policy.version + 1 };
      }
      data = policy;
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(data) });
  });
  await page.goto(`${baseUrl}/settings/`);
  await page.getByRole("button", { name: "Marketplace", exact: true }).click();
  await page.getByRole("tab", { name: "Platform wallet", exact: true }).click();
  const policyPanel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Balance & transfers", exact: true }) }).last();
  const feeInput = policyPanel.getByLabel("Top-up maintenance fee (%)", { exact: true });
  await feeInput.fill("5.25");
  await policyPanel.getByRole("button", { name: "Save", exact: true }).click();
  await policyPanel.getByRole("status").filter({ hasText: /Saved/ }).waitFor();
  assert.equal(policyWrites, 1);
  conflictPolicy = true;
  await feeInput.fill("6");
  assert.equal(await policyPanel.getByRole("status").count(), 0, "editing clears stale saved feedback");
  await policyPanel.getByRole("button", { name: "Save", exact: true }).click();
  await policyPanel.getByRole("alert").filter({ hasText: /changed elsewhere/ }).waitFor();
  await policyPanel.getByRole("button", { name: "Reload", exact: true }).click();
  await page.waitForFunction(() => [...document.querySelectorAll("input")].some((input) => input.value === "7"));
  assert.equal(await feeInput.inputValue(), "7");
  assert.equal(policyWrites, 2);
  const commissionPanel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Provider service fee", exact: true }) }).last();
  const commissionInput = commissionPanel.getByLabel("Platform service fee (%)", { exact: true });
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    await commissionPanel.scrollIntoViewIfNeeded();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  }
  await assertCommissionContrast(commissionPanel);
  await commissionInput.focus();
  await page.keyboard.press("Tab");
  const focus = await commissionPanel.getByRole("button", { name: "Save", exact: true }).evaluate((button) => {
    const style = getComputedStyle(button);
    return { visible: button.matches(":focus-visible"), width: parseFloat(style.outlineWidth), style: style.outlineStyle };
  });
  assert.equal(focus.visible, true);
  assert.ok(focus.width >= 2 && focus.style !== "none", "service fee save retains a visible keyboard focus ring");
  await assertCommissionContrast(commissionPanel);
  await commissionInput.fill("100.01");
  await commissionPanel.getByRole("button", { name: "Save", exact: true }).click();
  await commissionPanel.getByRole("alert").filter({ hasText: /0–100/ }).waitFor();
  await assertCommissionContrast(commissionPanel);
  assert.equal(commissionWrites, 0);
  await commissionInput.fill("10.25");
  await commissionPanel.getByRole("button", { name: "Save", exact: true }).click();
  await commissionPanel.getByRole("status").filter({ hasText: /Policy saved/ }).waitFor();
  await assertCommissionContrast(commissionPanel);
  assert.equal(commissionWrites, 1);
  assert.equal(policyWrites, 2, "commission writes do not edit top-up policy");
  conflictCommission = true;
  await commissionInput.fill("12");
  await commissionPanel.getByRole("button", { name: "Save", exact: true }).click();
  await commissionPanel.getByRole("alert").filter({ hasText: /changed elsewhere/ }).waitFor();
  assert.equal(await commissionInput.isDisabled(), true);
  await commissionPanel.getByRole("button", { name: "Retry", exact: true }).click();
  await page.waitForFunction(() => [...document.querySelectorAll("input")].some((input) => input.value === "10.25" && !input.disabled));
  assert.equal(await commissionInput.inputValue(), "10.25");
  policy = { ...policy, canEdit: false };
  commissionPolicy = { ...commissionPolicy, canEdit: false };
  await page.reload();
  await policyPanel.getByText("Only the Router owner can change this policy.", { exact: true }).waitFor();
  assert.equal(await feeInput.isDisabled(), true);
  assert.equal(await policyPanel.getByRole("button", { name: "Save", exact: true }).isDisabled(), true);
  assert.equal(await commissionInput.isDisabled(), true);
  assert.equal(await commissionPanel.getByRole("button", { name: "Save", exact: true }).isDisabled(), true);
  assert.deepEqual(pageErrors, []);
  console.log("wallet checkout: safe new tabs, fee change, recheck/stop, exact transfer, OTP retry, redirects, scoped access merge, historical Provider finance and Owner policy edits passed; four widths; no screenshots or real payments");
} finally {
  await browser?.close();
  if (server) await new Promise((resolveStop, rejectStop) => {
    server.close((error) => error ? rejectStop(error) : resolveStop());
  });
}
