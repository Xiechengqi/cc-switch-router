import { installFinancialBrowserIdentity } from "./financial-browser-fixture.mjs";
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
    if (new URL(route.request().url()).pathname === "/v1/ui/context") {
      return route.fulfill({ contentType: "application/json", body: JSON.stringify({ mode: "router" }) });
    }
    const path = new URL(route.request().url()).pathname;
    if (path === "/v1/market-wallet/c2c/policy") return route.fulfill({ contentType: "application/json", body: JSON.stringify({
      policy: { version: 1, enabled: false, admissionMinor: 10000, minMinor: 100, maxMinor: 500000, paymentMinutes: 30 },
      available: false, blockers: ["C2C_DISABLED"], canManagePolicy: false, verified: true, termsVersion: 3, independentDisputeHandlerAvailable: false, claimGraceSeconds: 120, firstPurchaseMaxMinor: 10000,
    }) });
    if (path === "/v1/market-wallet/c2c/orders") return route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [], next: null }) });
    if (path === "/v1/client-market/terminations") return route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [], nextCursor: null }) });
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

  await installFinancialBrowserIdentity(page);
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

  // Fixed rules on the balance page: credit $5–$1,000 (fee extra), $20
  // preselected, one payment method means no picker, a confirming top-up needs
  // nothing from the buyer and is re-checked automatically.
  const view = (state, extra = {}) => ({ state, canPay: state === "awaiting_payment", canClose: state === "awaiting_payment",
    payUntil: state === "awaiting_payment" ? new Date(Date.now() + 1_800_000).toISOString() : null, ...extra });
  let topup = {
    id: "order-2",
    channelId: "channel-1",
    amountMinor: 525,
    creditAmountMinor: 500, platformFeeMinor: 25, platformFeeRateBps: 500, policyVersion: 1,
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
    buyerView: view("confirming"),
  };
  let recheckCalls = 0;
  let closeCalls = 0;
  let transferCalls = 0;
  let confirmCalls = 0;
  let withdrawCalls = 0;
  let topupCreates = 0;
  let rejectNextTopupQuote = false;
  let feeBps = 500;
  let policyVersion = 1;
  let channelSettingsVersion = 1;
  let primaryMaxCreditMinor = 100_000;
  let activeChannel = "channel-1";
  let delayedQuote = null;
  const quoteRequests = [];
  // The server returns the intersection of the fixed $5–$1,000 range and the
  // channel's own settings.
  const channelLimits = (id, displayName, minTopupMinor, maxTopupMinor) => {
    const lower = Math.max(500, minTopupMinor), upper = Math.min(100_000, maxTopupMinor);
    const minCreditMinor = lower <= upper ? lower : null;
    const maxCreditMinor = lower <= upper ? upper : null;
    return { id, displayName, gatewayKind: "dujiaopay", methodCategory: "crypto", orderMode: "cashier", methods: [],
      minTopupMinor, maxTopupMinor, minCreditMinor, maxCreditMinor,
      maintenanceFeeBps: feeBps, policyVersion, channelRevision: 1, channelSettingsVersion };
  };
  const transferKeys = [];
  const transferStepups = [];
  let transferAmount = "8640001";
  let pendingTransfers = [];
  const until = async (check, label) => {
    for (let i = 0; i < 100; i++) {
      if (check()) return;
      await page.waitForTimeout(50);
    }
    assert.fail(label);
  };
  await page.context().route("**/v1/market-wallet/transfer-capacity", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ transferableUnits: transferAmount, committedUnits: "0", dailyRemainingUnits: "86400000", enabled: true, verified: true, blockers: [] }) });
  });
  await page.context().route("**/v1/market-wallet/transfer-quotes", async (route) => {
    const input = route.request().postDataJSON();
    assert.equal(input.recipientEmail, "recipient@example.invalid");
    assert.equal(input.all, true);
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ id: "transfer-quote", senderEmail: "buyer@example.invalid", recipientEmail: input.recipientEmail, amountUnits: transferAmount, expiresAt: new Date(Date.now() + 900_000).toISOString(), firstRecipient: true, delivery: "delayed", delayHours: 24 }) });
  });
  await page.context().route("**/v1/market-wallet/transfer-quotes/*/code", async () => {
    assert.fail("retired financial OTP endpoint must not be called");
  });
  // First transfer to a new recipient: the server asks for the login password
  // (428), the first response after confirmation is uncertain (504) and the
  // retry replays the same business request with the same idempotency key.
  await page.context().route("**/v1/market-wallet/transfers", async (route) => {
    transferCalls += 1;
    const headers = route.request().headers();
    transferKeys.push(headers["idempotency-key"]);
    transferStepups.push(headers["x-router-stepup"] || null);
    assert.ok(headers["x-router-request-signature"], "transfers are signed by the login device");
    assert.deepEqual(route.request().postDataJSON(), { quoteId: "transfer-quote" });
    if (transferCalls === 1) {
      await route.fulfill({ status: 428, contentType: "application/json", body: JSON.stringify({
        code: "FINANCIAL_STEPUP_REQUIRED", message: "confirm", details: { challengeId: "transfer-challenge", method: "password", risk: "confirm" },
      }) });
      return;
    }
    if (transferCalls === 2) {
      assert.equal(headers["x-router-stepup"], "transfer-challenge");
      await route.fulfill({ status: 504, contentType: "application/json", body: JSON.stringify({ message: "response uncertain" }) });
      return;
    }
    const pending = { id: "11111111-1111-4111-8111-111111111111", senderEmail: "buyer@example.invalid", recipientEmail: "recipient@example.invalid",
      amountUnits: transferAmount, journalId: null, createdAt: new Date().toISOString(), status: "pending",
      releaseAt: new Date(Date.now() + 86_400_000).toISOString() };
    pendingTransfers = [pending];
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(pending) });
    transferAmount = "0";
  });
  await page.context().route("**/v1/market-wallet/security/confirm", async (route) => {
    confirmCalls += 1;
    assert.ok(route.request().headers()["x-router-request-signature"]);
    const { challengeId, password } = route.request().postDataJSON();
    assert.equal(challengeId, "transfer-challenge");
    if (password !== "correct horse") {
      await route.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ code: "FINANCIAL_PASSWORD_INVALID", message: "password is incorrect" }) });
      return;
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ stepupId: "transfer-challenge" }) });
  });
  await page.context().route("**/v1/market-wallet/transfers/pending", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: pendingTransfers }) });
  });
  await page.context().route("**/v1/market-wallet/transfers/pending/*/cancel", async (route) => {
    withdrawCalls += 1;
    assert.ok(route.request().headers()["x-router-request-signature"]);
    const withdrawn = { ...pendingTransfers[0], status: "cancelled" };
    pendingTransfers = [];
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(withdrawn) });
  });
  await page.context().route("**/v1/market-wallet/transfer-returns/awaiting", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [] }) });
  });
  await page.context().route("**/v1/market-wallet/topup-quotes", async (route) => {
    const { channelId, amountMinor } = route.request().postDataJSON();
    quoteRequests.push({ channelId, amountMinor });
    const channel = channelId === "channel-2" ? channelLimits(channelId, "Bounded channel", 1000, 5000) : channelLimits(channelId, "DujiaoPay", 500, primaryMaxCreditMinor);
    const feeMinor = Math.floor((amountMinor * feeBps + 5000) / 10000);
    const response = { id: `quote-${amountMinor}`, creditMinor: amountMinor, feeBps, feeMinor, payMinor: amountMinor + feeMinor, policyVersion, channelRevision: 1, channelSettingsVersion, expiresAt: new Date(Date.now() + 300_000).toISOString() };
    if (delayedQuote) {
      const pending = delayedQuote;
      delayedQuote = null;
      pending.started();
      await pending.wait;
    }
    if (amountMinor < channel.minCreditMinor || amountMinor > channel.maxCreditMinor) {
      await route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify({ code: "MARKET_TOPUP_AMOUNT_OUT_OF_RANGE", message: "credit outside range", details: channel }) });
      return;
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(response) });
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
    await route.fulfill({ contentType: "application/json", body: JSON.stringify([
      activeChannel === "channel-2" ? channelLimits(activeChannel, "Bounded channel", 1000, 5000)
        : activeChannel === "channel-3" ? channelLimits(activeChannel, "Unavailable channel", 200, 100)
          : channelLimits(activeChannel, "DujiaoPay", 500, primaryMaxCreditMinor),
      channelLimits("legacy-duplicate", "Hidden merchant name", 500, 1000),
      { ...channelLimits("unknown-category", "Unsupported method", 500, 1000), methodCategory: "unknown" },
    ]) });
  });
  await page.context().route("**/v1/market-wallet/topups", async (route) => {
    if (route.request().method() === "POST") {
      topupCreates += 1;
      const input = route.request().postDataJSON();
      assert.equal(input.quoteId, "quote-500");
      assert.equal(input.amountMinor, 500);
      assert.equal(input.channelId, "channel-1");
      if (rejectNextTopupQuote) {
        rejectNextTopupQuote = false; feeBps = 600; policyVersion += 1;
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
  await page.context().route("**/v1/notifications**", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      items: [], nextCursor: null, unread: 0, readThrough: null,
    }) });
  });
  await page.context().route("**/v1/market-provider-finance/me", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ isProvider: false, hasFinanceActivity: false, canPublishPaidOffers: false, automaticCredit: true,
      summary: { grossEarnedUnits: "0", commissionUnits: "0", netEarnedUnits: "0", pendingUnits: "0", readyUnits: "0", restrictedUnits: "0", creditedUnits: "0" } }) });
  });
  await page.context().route("**/v1/market-provider-finance/earnings", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [], nextCursor: null }) });
  });
  await page.context().route("**/v1/market-provider-finance/holds", async (route) => {
    await route.fulfill({ contentType: "application/json", body: "[]" });
  });
  await page.context().route("**/v1/market-access/dashboard", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ policies: [], counterparties: [], accessRequests: [] }) });
  });
  await page.context().route("**/v1/account/provider-contacts", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ contacts: [], updatedAt: new Date().toISOString() }) });
  });
  await page.context().route("**/v1/market-wallet/topups/order-2/recheck", async (route) => {
    recheckCalls += 1;
    assert.ok(route.request().headers()["x-router-request-signature"]);
    topup = { ...topup, buyerRecheckRequestedAt: new Date().toISOString(), buyerRecheckResult: "queued" };
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ queued: true, order: topup }) });
  });
  await page.context().route("**/v1/market-wallet/topups/order-1/close", async (route) => {
    closeCalls += 1;
    assert.ok(route.request().headers()["x-router-request-signature"]);
    topup = { ...topup, buyerAbandonedAt: new Date().toISOString(), checkoutUrl: null, buyerView: view("confirming") };
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(topup) });
  });
  await page.goto(`${baseUrl}/account/wallet/`);
  assert.equal(await page.getByRole("region", { name: "Client rental terminations", exact: true }).count(), 0, "termination actions belong to Client rentals, not balance history");
  for (const retired of ["Wallet security", "Transfer corrections"]) {
    assert.equal(await page.getByText(retired, { exact: true }).count(), 0, `${retired} is a fixed server rule, not a page section`);
  }
  const attention = page.getByRole("region", { name: "Needs attention", exact: true });
  await attention.getByText("Top-up $5.00 confirming", { exact: true }).waitFor();
  await attention.getByText("It credits automatically once the payment is confirmed. Nothing to do.", { exact: true }).waitFor();
  assert.equal(await attention.getByRole("button").count(), 0, "a confirming top-up offers no buyer actions");
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    assert.ok(overflow <= 1, `wallet page overflows by ${overflow}px at ${width}px`);
  }
  // Returning to the tab asks the server to look at confirming top-ups again.
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await until(() => recheckCalls === 1, "a confirming top-up is re-checked when the buyer returns");
  assert.equal(topup.status, "review_required");

  // The fixed $5–$1,000 credit range excludes the maintenance fee.
  // Invalid inputs never request a quote or consume pending-order capacity.
  const amountInput = page.locator("#market-wallet-topup-amount");
  const amountHelp = page.locator("#market-wallet-topup-amount-help");
  const payButton = (name) => page.getByRole("button", { name, exact: true });
  await payButton("Pay $21.00").waitFor();
  assert.equal(await amountInput.inputValue(), "20.00", "the common amount is preselected");
  assert.equal(await page.getByRole("button", { name: "$20", exact: true }).getAttribute("aria-pressed"), "true");
  await amountHelp.filter({ hasText: "Includes $1.00 maintenance fee · paid $21.00" }).waitFor();
  // Buyers never choose a merchant. Simulate an Owner changing which merchant
  // is active, then revalidate the form without losing its draft. Duplicate
  // and unknown categories collapse to one method, so no picker is shown.
  const selectChannel = async name => {
    activeChannel = { DujiaoPay: "channel-1", "Bounded channel": "channel-2", "Unavailable channel": "channel-3" }[name];
    assert.ok(activeChannel);
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await page.waitForFunction(id => document.querySelector("form[data-topup-channel]")?.dataset.topupChannel === id, activeChannel);
    assert.equal(await page.getByRole("button", { name: /Top-up method/ }).count(), 0, "one method needs no picker");
  };
  assert.equal(await page.getByRole("button", { name: /Top-up method/ }).count(), 0);
  const requestsBeforeInvalid = quoteRequests.length;
  for (const invalid of ["4.99", "1000.01"]) {
    await amountInput.fill(invalid);
    await amountInput.blur();
    await amountHelp.filter({ hasText: /\$5\.00 and \$1,000\.00/ }).waitFor();
    assert.equal(await amountInput.getAttribute("aria-invalid"), "true");
    assert.equal(await payButton("Continue to payment").isDisabled(), true);
    await page.waitForTimeout(600);
  }
  assert.equal(quoteRequests.length, requestsBeforeInvalid);
  assert.equal(topupCreates, 0);
  await amountInput.fill("960");
  await payButton("Pay $1,008.00").waitFor();
  assert.equal(quoteRequests.at(-1).amountMinor, 96_000);
  await amountInput.fill("1000");
  await payButton("Pay $1,050.00").waitFor();
  assert.equal(quoteRequests.at(-1).amountMinor, 100_000);

  await selectChannel("Bounded channel");
  await amountHelp.filter({ hasText: /\$10\.00 and \$50\.00/ }).waitFor();
  assert.equal(await amountInput.inputValue(), "1000", "switching never silently clamps the requested credit");
  assert.equal(await page.getByRole("button", { name: "$5", exact: true }).isDisabled(), true);
  assert.equal(await page.getByRole("button", { name: "$10", exact: true }).isDisabled(), false);
  assert.equal(await page.getByRole("button", { name: "$50", exact: true }).isDisabled(), false);
  assert.equal(await page.getByRole("button", { name: "$100", exact: true }).isDisabled(), true);
  await amountInput.fill("9.99");
  assert.equal(await amountInput.getAttribute("aria-invalid"), "true");
  await amountInput.fill("10");
  await payButton("Pay $10.50").waitFor();
  await amountInput.fill("50");
  await payButton("Pay $52.50").waitFor();
  await selectChannel("Unavailable channel");
  await amountHelp.filter({ hasText: /no available top-up range/ }).waitFor();
  assert.equal(await amountInput.isDisabled(), true);
  assert.equal(await payButton("Continue to payment").isDisabled(), true);

  // An old, delayed quote must not win after switching channels.
  await selectChannel("DujiaoPay");
  let releaseOldQuote;
  const oldQuoteStarted = new Promise((started) => {
    delayedQuote = { started, wait: new Promise((release) => { releaseOldQuote = release; }) };
  });
  await amountInput.fill("10");
  await oldQuoteStarted;
  await selectChannel("Bounded channel");
  await payButton("Pay $10.50").waitFor();
  releaseOldQuote();
  await page.waitForTimeout(100);
  assert.match(await amountHelp.textContent(), /paid \$10\.50/);
  assert.equal(await payButton("Pay $10.50").isDisabled(), false);

  // A fee update invalidates the old quote as soon as current channel data
  // arrives, even if no new payment is being created. Input remains untouched.
  await selectChannel("DujiaoPay");
  await amountInput.fill("1000");
  await payButton("Pay $1,050.00").waitFor();
  feeBps = 725; policyVersion += 1;
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await payButton("Pay $1,072.50").waitFor();
  assert.equal(await amountInput.inputValue(), "1000");
  assert.equal(await amountInput.getAttribute("aria-invalid"), "false");
  await amountHelp.filter({ hasText: "Includes $72.50 maintenance fee · paid $1,072.50" }).waitFor();
  for (const [rate, total] of [[0, "$1,000.00"], [10_000, "$2,000.00"]]) {
    feeBps = rate; policyVersion += 1;
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await payButton(`Pay ${total}`).waitFor();
    assert.equal(await amountInput.inputValue(), "1000");
    assert.equal(await amountInput.getAttribute("aria-invalid"), "false");
    assert.doesNotMatch(await amountHelp.textContent(), /Enter a balance credit/);
  }
  assert.equal(topupCreates, 0, "fee refresh never submits an order automatically");

  // If a quote sees a newer policy before channel polling does, refresh its
  // limits and request a matching quote before enabling payment.
  feeBps = 500; policyVersion += 1;
  await amountInput.fill("10");
  await payButton("Pay $10.50").waitFor();
  await amountHelp.filter({ hasText: /paid \$10\.50/ }).waitFor();

  // Only an explicit channel principal-limit change narrows the input range.
  // An error refreshes that range without clamping the entered principal.
  primaryMaxCreditMinor = 80_000; channelSettingsVersion += 1;
  await amountInput.fill("850");
  await amountHelp.filter({ hasText: /\$800\.00/ }).waitFor();
  assert.equal(await amountInput.inputValue(), "850");
  assert.equal(await amountInput.getAttribute("aria-invalid"), "true");
  assert.equal(topupCreates, 0);
  primaryMaxCreditMinor = 100_000; channelSettingsVersion += 1;
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await payButton("Pay $892.50").waitFor();

  // A committed order with no payable capability stays visible in the new
  // tab, which explains why it cannot pay instead of redirecting.
  topup = {
    id: "order-1", channelId: "channel-1", amountMinor: 525,
    creditAmountMinor: 500, platformFeeMinor: 25, platformFeeRateBps: 500, policyVersion: 1,
    status: "pending", checkoutUrl,
    createdAt: new Date().toISOString(), autoCloseAt: null,
    buyerAbandonedAt: null, reviewReason: null,
    buyerRecheckRequestedAt: null, buyerRecheckCompletedAt: null,
    buyerRecheckResult: null, lastGatewayStatus: "pending",
    lastGatewayCheckedAt: new Date().toISOString(),
    buyerView: view("awaiting_payment"),
  };
  canPay = false;
  expiresAt = new Date(Date.now() + 60_000).toISOString();
  await page.goto(`${baseUrl}/account/wallet/`);
  await amountInput.fill("5.00");
  const [blockedTab] = await Promise.all([
    page.waitForEvent("popup"),
    payButton("Pay $5.25").click(),
  ]);
  await blockedTab.getByText(/Payment or review evidence needs checking/i).waitFor();
  assert.match(blockedTab.url(), /\/wallet-checkout\/\?topup=order-1$/);
  assert.equal(externalVisits, 1);
  await blockedTab.close();

  // The unpaid order waits under Needs attention with one way to pay and one
  // way to stop; the checkout page re-checks payability before redirecting.
  await attention.getByText("Top-up $5.00 awaiting payment", { exact: true }).waitFor();
  canPay = true;
  const resumeLink = attention.getByRole("link", { name: "Continue to pay" });
  const resumedTabPromise = page.waitForEvent("popup");
  await resumeLink.click();
  const resumedTab = await resumedTabPromise;
  await resumedTab.waitForURL(checkoutUrl);
  assert.equal(externalVisits, 2);
  await resumedTab.close();
  await attention.getByRole("button", { name: "Don't pay", exact: true }).click();
  await page.getByText("Stop this top-up?", { exact: true }).waitFor();
  assert.equal(closeCalls, 0, "opening the confirmation does not stop the order");
  await page.getByRole("button", { name: "Don't pay", exact: true }).last().click();
  await attention.getByRole("status").filter({ hasText: /Stopped\. Any payment you already made still credits automatically/ }).waitFor();
  assert.equal(closeCalls, 1);
  assert.equal(await attention.getByText("Top-up $5.00 awaiting payment", { exact: true }).count(), 0);

  // No real transfer is sent. A first transfer is delayed and withdrawable;
  // the login password confirms it (one typo, then the right password) and an
  // uncertain response is retried with the same idempotency key, without OTP.
  await page.getByRole("button", { name: /^Send to someone/ }).click();
  const transferPanel = page.locator("#wallet-transfer");
  await transferPanel.getByLabel("Recipient email").fill("recipient@example.invalid");
  await transferPanel.getByRole("button", { name: "Send all available", exact: true }).click();
  await transferPanel.getByRole("button", { name: "Review send" }).click();
  const confirmation = page.getByRole("region", { name: "Confirm recipient & amount" });
  await confirmation.getByText("recipient@example.invalid", { exact: true }).waitFor();
  await confirmation.getByText("First transfer to this user: it credits in 24 hours and you can withdraw it until then.", { exact: true }).waitFor();
  assert.equal(await confirmation.getByText(/ledger units/).count(), 0, "internal ledger units are never shown");
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    assert.ok(await confirmation.evaluate((element) => element.scrollWidth <= element.clientWidth + 1), `transfer confirmation overflow at ${width}`);
  }
  assert.equal(await confirmation.locator('input[autocomplete="one-time-code"]').count(), 0);
  assert.equal(await confirmation.getByRole("button", { name: /verification|code/i }).count(), 0);
  await confirmation.getByRole("button", { name: /^Send \$/ }).click();
  const passwordDialog = page.getByRole("dialog", { name: "Confirm with your password" });
  await passwordDialog.getByText(/A transfer to a new recipient credits after 24 hours/).waitFor();
  await passwordDialog.getByLabel("Login password", { exact: true }).fill("typo");
  await passwordDialog.getByRole("button", { name: "Confirm", exact: true }).click();
  await passwordDialog.getByRole("alert").filter({ hasText: "The password is incorrect. Try again." }).waitFor();
  await passwordDialog.getByLabel("Login password", { exact: true }).fill("correct horse");
  await passwordDialog.getByRole("button", { name: "Confirm", exact: true }).click();
  await passwordDialog.waitFor({ state: "hidden" });
  await confirmation.getByRole("alert").waitFor();
  assert.equal(confirmCalls, 2);
  assert.equal(transferCalls, 2);
  await confirmation.getByRole("button", { name: /^Send \$/ }).click();
  await transferPanel.getByRole("status").filter({ hasText: /Sent for recipient@example\.invalid/ }).waitFor();
  assert.equal(transferCalls, 3);
  assert.ok(transferKeys[0]);
  assert.deepEqual(new Set(transferKeys).size, 1, "one idempotency key across confirmation and retry");
  assert.deepEqual(transferStepups, [null, "transfer-challenge", null]);

  // The pending transfer can be withdrawn by its sender before it credits.
  await page.keyboard.press("Escape");
  await attention.getByText(/^To recipient@example\.invalid · /).waitFor();
  await attention.getByText(/You can withdraw it until then/).waitFor();
  await attention.getByRole("button", { name: "Withdraw", exact: true }).click();
  await attention.getByRole("status").filter({ hasText: "Withdrawn. The amount is back in your available balance." }).waitFor();
  assert.equal(withdrawCalls, 1);
  assert.equal(await attention.getByText(/^To recipient@example\.invalid · /).count(), 0);

  // A changed policy refreshes the visible total but never silently submits
  // an order at the new price. A second user gesture is required.
  await page.setViewportSize({ width: 1280, height: 900 });
  rejectNextTopupQuote = true;
  await amountInput.fill("5.00");
  await payButton("Pay $5.25").click();
  await payButton("Pay $5.30").waitFor();
  await page.getByRole("alert").filter({ hasText: /Review the new total/ }).waitFor();
  assert.equal(topupCreates, 2);

  for (const retired of ["payments", "billing"]) {
    const response = await fetch(baseUrl + "/account/" + retired + "/");
    assert.equal(response.status, 404, "retired " + retired + " page must not return");
  }
  for (const [legacy, expected] of [["market-access/", "provider-settings/"]]) {
    await page.goto(`${baseUrl}/account/${legacy}`);
    await page.waitForURL(`${baseUrl}/account/${expected}?tab=access`);
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
  await page.goto(`${baseUrl}/account/provider-settings/?tab=access`);
  const accessForm = page.locator("form").filter({ has: page.getByRole("heading", { name: "Add a trusted buyer", exact: true }) });
  await accessForm.getByLabel("Buyer email", { exact: true }).fill("trusted@example.invalid");
  await accessForm.getByLabel("Client Host Market · Paid", { exact: true }).check();
  await accessForm.getByRole("button", { name: "Add", exact: true }).click();
  await page.waitForFunction(() => document.querySelector('input[type="email"]')?.value === "");
  assert.equal(accessUpdates, 1);
  assert.equal(await page.locator('a[href="/account/earnings/"]').count(), 0, "Provider income has no standalone navigation entry");

  // Model the worker's authoritative credit, never a browser-created transfer.
  let matured = false;
  let credited = "0";
  let retiredWrites = 0;
  const earning = () => ({
    id: "net-earning", amountUnits: "777600000", grossUnits: "864000000", commissionUnits: "86400000",
    commissionReversedUnits: "0", commissionRateBps: 1000, commissionPolicyVersion: 1,
    reversedUnits: "0", unlockedUnits: matured ? "777600000" : "0", heldUnits: "0",
    creditedUnits: credited, disputed: false,
    fulfilledAt: "2026-01-01T00:00:00Z", unlockAt: "2026-01-08T00:00:00Z",
  });
  for (const suffix of ["wallet-conversion-quotes", "wallet-conversions", "withdrawals", "destination"]) {
    await page.context().route("**/v1/market-provider-finance/" + suffix, async (route) => {
      retiredWrites += 1;
      await route.fulfill({ status: 410, contentType: "application/json", body: JSON.stringify({ code: "RETIRED" }) });
    });
  }
  await page.context().route("**/v1/market-provider-finance/me", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({
      isProvider: false, hasFinanceActivity: true, canPublishPaidOffers: false, automaticCredit: true,
      summary: { grossEarnedUnits: "864000000", commissionUnits: "86400000", netEarnedUnits: "777600000",
        pendingUnits: matured ? "0" : "777600000", readyUnits: matured && credited === "0" ? "777600000" : "0",
        restrictedUnits: "0", creditedUnits: credited },
    }) });
  });
  await page.context().route("**/v1/market-provider-finance/holds", async (route) => {
    await route.fulfill({ contentType: "application/json", body: "[]" });
  });
  await page.context().route("**/v1/market-provider-finance/earnings", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ items: [earning()], nextCursor: null }) });
  });
  // The Provider center opens on Rented out; income is its own tab.
  await page.goto(`${baseUrl}/account/provider-settings/?tab=income`);
  assert.equal(await page.getByRole("link", { name: "Provider earnings", exact: true }).count(), 0);
  const earningsPanel = page.getByRole("region", { name: "Provider earnings", exact: true });
  assert.equal(await earningsPanel.getByRole("region", { name: "Client rental terminations", exact: true }).count(), 0, "income is separate from rental actions");
  assert.equal(await earningsPanel.getByRole("alert").count(), 0);
  const earningSummary = earningsPanel.getByText("Revenue & service fees", { exact: true });
  assert.equal(await earningSummary.locator("..").getAttribute("open"), null, "detailed accounting starts collapsed");
  await earningSummary.click();
  await earningsPanel.getByRole("columnheader", { name: "Net earnings", exact: true }).waitFor();
  const earningTotals = earningsPanel.locator("dl").filter({ hasText: "Fulfilled revenue (net of refunds)" });
  await earningTotals.getByText("$100.00", { exact: true }).waitFor();
  await earningTotals.getByText("$10.00", { exact: true }).waitFor();
  await earningTotals.getByText("$90.00", { exact: true }).waitFor();
  await earningsPanel.getByText("Fee breakdown", { exact: true }).click();
  await earningsPanel.getByText("Gross $100.00", { exact: true }).waitFor();
  await earningsPanel.getByText("Service fee 10% · $10.00", { exact: true }).waitFor();
  const incomeRow = earningsPanel.getByRole("row").filter({ hasText: "Gross $100.00" });
  await incomeRow.getByText("Pending", { exact: true }).waitFor();
  assert.equal(await earningsPanel.getByRole("button", { name: /withdraw|move to balance/i }).count(), 0);
  await earningsPanel.getByRole("link", { name: "View balance", exact: true }).waitFor();
  assert.equal(await earningsPanel.getByRole("link").count(), 1, "wallet is the sole income shortcut");
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, "Provider income page overflows at " + width);
  }
  matured = true;
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await incomeRow.getByText("Awaiting automatic credit", { exact: true }).waitFor();
  credited = "777600000";
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await incomeRow.getByText("Credited to balance", { exact: true }).waitFor();
  await page.reload();
  await earningSummary.click();
  await incomeRow.getByText("Credited to balance", { exact: true }).waitFor();
  assert.equal(retiredWrites, 0, "viewing earnings never invokes retired payout or manual-credit endpoints");

  // Owner policy edits use integer basis points and versioned writes. A
  // conflict must reload the current policy, never silently overwrite it.
  let policy = { version: 1, maintenanceFeeBps: 500, transfersEnabled: true,
    transfersPaused: false, transferMaxMinor: 500_000, transferDailyMaxMinor: 500_000,
    balanceRefundable: false, canEdit: true };
  let trialPolicy = { version: 1, clientHours: 12, shareHours: 12, shareTokens: 1_000_000, shareValueUsdMicros: "1000000", canEdit: true };
  let trialWrites = 0;
  let conflictTrial = false;
  let policyWrites = 0;
  let conflictPolicy = false;
  let commissionPolicy = { version: 1, current: { policyVersion: 1, rateBps: 1000, effectiveAt: "2026-01-01T00:00:00Z" }, scheduled: null, increaseNoticeDays: 7, canEdit: true };
  let commissionWrites = 0;
  let conflictCommission = false;
  const categories = [{ id: "marketplace", label: "Market & Billing", description: "Market & Billing", fieldCount: 0 }];
  const sections = [{ id: "share_market", category: "marketplace", label: "Share Market", groups: [], panels: [] }, { id: "wallet_payments", category: "marketplace", label: "Wallet & payments", groups: [], panels: ["market_finance"] }];
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
      envPath: "/test/.env", schema: { categories, sections, fields: [], groups: [] }, values: [], pendingRestartKeys: [], unrecognizedKeys: [] };
    else if (path === "/v1/admin/market-finance/readiness") data = { mode: "enabled", version: 1,
      newOrdersPaused: false, newTopupsPaused: false, codeIntegrated: true,
      criticalCases: 0, staleTopupReviews: 0, verifiedLiveTopups: 1,
      paymentChannelBlockers: [], canEnable: false };
    else if (path === "/v1/admin/market-finance/overview") data = overview;
    else if (path === "/v1/admin/market-finance/c2c/policy") data = { policy: { version: 1, enabled: false,
      admissionMinor: 10000, minMinor: 100, maxMinor: 500000, paymentMinutes: 30 }, canManage: true, canResolve: true, };
    else if (path === "/v1/admin/market-finance/c2c/orders" || path === "/v1/admin/market-finance/c2c/sellers") data = { items: [], next: null };
    else if (path === "/v1/admin/market/client-termination-policy") data = { version: 1, rateBps: 1000, canEdit: true };
    else if (path === "/v1/admin/market/client-terminations") data = { items: [], nextCursor: null };
    else if (path === "/v1/admin/market/trial-policy") {
      if (route.request().method() === "PUT") {
        trialWrites += 1;
        const input = route.request().postDataJSON();
        assert.deepEqual(Object.keys(input).sort(), ["clientHours", "expectedVersion", "shareHours", "shareTokens", "shareValueUsdMicros"]);
        assert.equal(input.expectedVersion, trialPolicy.version);
        if (conflictTrial) {
          conflictTrial = false;
          trialPolicy = { ...trialPolicy, version: trialPolicy.version + 1, shareHours: 6 };
          return route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ message: "trial policy changed" }) });
        }
        trialPolicy = { ...trialPolicy, ...input, version: trialPolicy.version + 1 };
      }
      data = trialPolicy;
    }
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
        assert.equal(input.defaultClientTrialHours, undefined, "trial policy is independent from wallet fees");
        assert.equal(input.canEdit, undefined);
        assert.equal(input.balanceRefundable, undefined);
        assert.equal(input.earningsConversionsPaused, undefined);
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
  // Owner-only top-up limits and circuit breaker now live in Settings, not on
  // the buyer's balance page. Saving is a signed finance write with CAS.
  let risk = { version: 1, topupUserDayMinor: 200000, topupChannelDayMinor: 2000000,
    topupGlobalDayMinor: 5000000, topupGlobalHourMinor: 1000000, incomingUserDayMinor: 1000000,
    chainVerificationRequired: false, topupCircuitOpen: true };
  let riskWrites = 0, rejectRisk = true;
  await page.context().route("**/v1/admin/market-finance/risk-policy", async (route) => {
    if (route.request().method() === "PUT") {
      riskWrites++;
      assert.ok(route.request().headers()["x-router-request-signature"]);
      const input = route.request().postDataJSON();
      assert.equal(input.version, risk.version);
      if (rejectRisk) {
        rejectRisk = false; risk = { ...risk, version: 2 };
        return route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ message: "risk policy changed; reload" }) });
      }
      risk = { ...input, version: risk.version + 1 };
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(risk) });
  });
  await page.goto(`${baseUrl}/settings/`);
  await page.getByRole("button", { name: "Market & Billing", exact: true }).click();
  await page.getByRole("tab", { name: /^Wallet & payments/ }).click();
  await page.getByRole("region", { name: "Client rental terminations", exact: true }).getByText("No Client termination records.", { exact: true }).waitFor();
  const policyPanel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Balance & sending", exact: true }) }).last();
  const feeInput = policyPanel.getByLabel("Top-up maintenance fee (%)", { exact: true });
  const trialPanel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Market trial rules", exact: true }) }).last();
  const trialInput = trialPanel.getByLabel("Default Client trial (hours)", { exact: true });
  assert.equal(await trialInput.inputValue(), "12");
  await trialInput.fill("0");
  await trialPanel.getByLabel("Default Share trial Tokens (M)", { exact: true }).fill("0");
  await trialPanel.getByRole("button", { name: "Save", exact: true }).click();
  await trialPanel.getByRole("status").filter({ hasText: /Saved/ }).waitFor();
  assert.equal(trialWrites, 1);
  assert.equal(trialPolicy.clientHours, 0);
  assert.equal(trialPolicy.shareTokens, 0);
  assert.equal(policyWrites, 0, "trial edits must not change the wallet fee version");
  conflictTrial = true;
  await trialPanel.getByLabel("Default Share trial (hours)", { exact: true }).fill("24");
  await trialPanel.getByRole("button", { name: "Save", exact: true }).click();
  await trialPanel.getByRole("alert").filter({ hasText: /changed elsewhere/ }).waitFor();
  assert.equal(await trialPanel.getByRole("button", { name: "Save", exact: true }).isEnabled(), false);
  await trialPanel.getByRole("button", { name: "Refresh", exact: true }).click();
  await page.waitForFunction(() => document.querySelector('input[aria-label="Default Share trial (hours)"]')?.value === "6");
  await feeInput.fill("5.25");
  await policyPanel.getByRole("button", { name: "Save", exact: true }).click();
  await policyPanel.getByRole("status").filter({ hasText: /Saved/ }).waitFor();
  assert.equal(policyWrites, 1);
  assert.equal(trialPolicy.clientHours, 0);
  assert.equal(await trialInput.inputValue(), "0");
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
  const riskPanel = () => page.locator("section").filter({ has: page.getByRole("heading", { name: "Aggregate credit limits", exact: true }) }).last();
  await riskPanel().getByLabel("Top-ups per user / 24 hours (USD)", { exact: true }).fill("2500");
  await riskPanel().getByRole("button", { name: "Save risk policy", exact: true }).click();
  await riskPanel().getByRole("alert").filter({ hasText: "risk policy changed" }).waitFor();
  assert.equal(await riskPanel().getByLabel("Top-ups per user / 24 hours (USD)", { exact: true }).inputValue(), "2500", "a conflict keeps the Owner's draft");
  // The panel has no in-place reload; a page reload reads the current version.
  await page.reload();
  await riskPanel().getByLabel("Top-ups per user / 24 hours (USD)", { exact: true }).waitFor();
  await riskPanel().getByLabel("Top-ups per user / 24 hours (USD)", { exact: true }).fill("2500");
  await riskPanel().getByLabel("Pause new top-ups and automatic credits", { exact: true }).uncheck();
  await riskPanel().getByRole("button", { name: "Save risk policy", exact: true }).click();
  await riskPanel().getByRole("status").filter({ hasText: "Saved" }).waitFor();
  assert.equal(riskWrites, 2);
  assert.equal(risk.topupUserDayMinor, 250000);
  assert.equal(risk.topupCircuitOpen, false);
  policy = { ...policy, canEdit: false };
  commissionPolicy = { ...commissionPolicy, canEdit: false };
  await page.reload();
  await policyPanel.getByText("Only the Router owner can change this policy.", { exact: true }).waitFor();
  assert.equal(await feeInput.isDisabled(), true);
  assert.equal(await policyPanel.getByRole("button", { name: "Save", exact: true }).isDisabled(), true);
  assert.equal(await commissionInput.isDisabled(), true);
  assert.equal(await commissionPanel.getByRole("button", { name: "Save", exact: true }).isDisabled(), true);

  // Retired: wallet security and transfer-correction requests. Requests made
  // earlier still wait for the recipient under Needs attention, and any
  // received transfer can be sent back as an ordinary transfer.
  const time = new Date().toISOString();
  const json = (route, data, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(data) });
  const receivedId = "33333333-3333-4333-8333-333333333333";
  const otherId = "44444444-4444-4444-8444-444444444444";
  let awaiting = [
    { id: "return-1", transferId: receivedId, reason: "Wrong registered recipient", version: 1, createdAt: time, senderEmail: "sender@example.invalid", amountUnits: "8640000" },
    { id: "return-2", transferId: otherId, reason: "Sent twice by mistake", version: 3, createdAt: time, senderEmail: "other@example.invalid", amountUnits: "17280000" },
  ];
  const decisions = [];
  await page.context().route("**/v1/market-wallet/transfer-returns/awaiting", route => json(route, { items: awaiting }));
  await page.context().route("**/v1/market-wallet/transfers/*/return/*", async route => {
    const match = new URL(route.request().url()).pathname.match(/transfers\/([^/]+)\/return\/(approve|decline)$/);
    assert.ok(match);
    const [, id, decision] = match;
    assert.equal(route.request().method(), "POST");
    assert.ok(route.request().headers()["x-router-request-signature"]);
    const request = awaiting.find(item => item.transferId === id);
    assert.equal(route.request().postDataJSON().expectedVersion, request.version);
    decisions.push([id, decision]);
    awaiting = awaiting.filter(item => item.transferId !== id);
    await json(route, { id: request.id, transferId: id, status: decision === "approve" ? "approved" : "declined" });
  });
  await page.context().route("**/v1/market-wallet/transfers/*/return", async route => {
    assert.notEqual(route.request().method(), "POST", "new return requests are retired");
    await json(route, null);
  });
  await page.context().route("**/v1/market-wallet/transactions", route => json(route, { items: [{
    id: "entry-received", sourceKind: "wallet_transfer", action: "transfer", deltaUnits: "8640000", heldDeltaUnits: "0",
    createdAt: time, transferId: receivedId, counterpartyEmail: "sender@example.invalid", c2cOrderId: null,
  }], nextCursor: null }));
  await page.goto(`${baseUrl}/account/wallet/`);
  for (const retired of ["Wallet security", "Transfer corrections", "Add Passkey", "Request return"]) {
    assert.equal(await page.getByText(retired, { exact: true }).count(), 0, `${retired} is gone`);
  }
  const needs = page.getByRole("region", { name: "Needs attention", exact: true });
  await needs.getByText("sender@example.invalid asks you to return $1.00", { exact: true }).waitFor();
  await needs.getByText("Wrong registered recipient", { exact: true }).waitFor();
  await needs.getByRole("button", { name: "Return $1.00", exact: true }).click();
  await needs.getByText("sender@example.invalid asks you to return $1.00", { exact: true }).waitFor({ state: "detached" });
  await needs.getByText("other@example.invalid asks you to return $2.00", { exact: true }).waitFor();
  await needs.getByRole("button", { name: "Decline", exact: true }).click();
  await needs.getByText("other@example.invalid asks you to return $2.00", { exact: true }).waitFor({ state: "detached" });
  assert.deepEqual(decisions, [[receivedId, "approve"], [otherId, "decline"]]);
  const activity = page.getByRole("region", { name: "Activity", exact: true });
  await activity.getByText("Received from sender@example.invalid", { exact: true }).waitFor();
  await activity.getByRole("button", { name: "Send back", exact: true }).click();
  const sendBack = page.getByRole("dialog", { name: "Send back to sender@example.invalid" });
  await sendBack.waitFor();
  assert.equal(await sendBack.getByLabel("Recipient email").inputValue(), "sender@example.invalid");
  assert.equal(await sendBack.locator("#wallet-transfer input[inputmode=decimal]").inputValue(), "1.00");
  await page.keyboard.press("Escape");
  await sendBack.waitFor({ state: "hidden" });
  for (const width of [320, 375, 414, 768]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `wallet attention/activity page overflows at ${width}`);
  }
  assert.deepEqual(pageErrors, []);
  console.log("wallet checkout: fixed $5–$1,000 range with $20 default, dynamic fees, stale-quote races, safe payment tabs, automatic recheck, Don't pay, password-confirmed delayed transfer with idempotent retry and withdraw, automatic earnings credit, Owner policies, risk-policy CAS in Settings, legacy return decisions and send-back passed; four widths; no screenshots or real payments");
} finally {
  await browser?.close();
  if (server) await new Promise((resolveStop, rejectStop) => {
    server.close((error) => error ? rejectStop(error) : resolveStop());
  });
}
