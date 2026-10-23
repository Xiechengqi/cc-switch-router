import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Exported local UI, mocked APIs, no screenshots, messages or real payments.
const output = resolve(fileURLToPath(new URL("../out/", import.meta.url)));
const server = createServer(async (req, res) => {
  try {
    const p = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
    const path = resolve(output, `.${p.endsWith("/") ? `${p}index.html` : p}`);
    if (!path.startsWith(output + sep)) return res.writeHead(403).end();
    const body = await readFile(path);
    res.writeHead(200, { "Content-Type": ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".txt": "text/plain", ".svg": "image/svg+xml", ".ttf": "font/ttf", ".woff2": "font/woff2" })[extname(path)] || "application/octet-stream" }); res.end(body);
  } catch { res.writeHead(404).end(); }
});
await new Promise((ready, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", ready); });
const base = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });
  const errors = []; const orders = []; const keys = []; let counter = 0; let quoteCounter = 0; let quoteLifetimeMs = 300000; let uncertain = false; let heartbeat = 0; let released = 0; let resolution = 0;
  let policy = { version: 1, enabled: true, admissionMinor: 10000, minMinor: 100, maxMinor: 500000, paymentMinutes: 30 };
  let profile = null;
  const seller = { sellerId: "seller", version: 1, displayName: "Ordinary verified user", paymentMethod: "Bank transfer", currency: "USD", priceBps: 10000, minMinor: 100, maxMinor: 10000, availableMinor: 20000, completedOrders: 0 };
  const overview = Object.fromEntries(["walletPostedUnits", "walletHeldUnits", "unfulfilledUnits", "providerPendingUnits", "providerAvailableUnits", "providerHeldUnits", "buyerRefundHeldUnits", "ownerLossUnits", "maintenanceRevenueUnits"].map(k => [k, "0"]));
  Object.assign(overview, { estimatedGatewayCostMinor: 0, creditedTopupMinor: 0, grossTopupMinor: 0, openCases: 0, openCriticalCases: 0, heldEarningLots: 0 });
  const makeOrder = (q, actor) => ({ id: `order-${++counter}`, quoteId: q.id, buyerId: actor, sellerId: "seller", amountUnits: String(q.creditMinor * 86400), status: "awaiting_payment", version: 1,
    paymentDeadline: new Date(Date.now() + 1800000).toISOString(), paidAt: null, releasedAt: null, latePaymentReported: false, latePaymentResolved: false,
    createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), sellerName: seller.displayName, payMinor: q.payMinor, currency: q.currency, fxMicros: 1000000, priceBps: 10000,
    contact: "Private contact for order parties", paymentMethod: "Bank transfer", paymentDetails: "Private account 123" });
  let latestQuote;
  const open = async (actor, locale = "en", admin = false) => {
    const context = await browser.newContext(); const page = await context.newPage(); page.on("pageerror", e => { errors.push(e.message); console.error(e.stack); });
    await page.addInitScript(({ locale, actor }) => {
      localStorage.setItem("cc_switch_router_locale_v1", locale);
      localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({ accessToken: `c2c-${actor}`, expiresAt: "2099-01-01T00:00:00Z", authDeviceId: "test", publicKey: "test", privateKey: "test" }));
    }, { locale, actor });
    const json = (route, value, status = 200) => route.fulfill({ status, contentType: "application/json", headers: { "x-router-server-time": new Date().toISOString() }, body: JSON.stringify(value) });
    await context.route("**/v1/**", async route => {
      const request = route.request(); const url = new URL(request.url()); const p = url.pathname; const body = request.postData() ? request.postDataJSON() : {};
      if (p === "/v1/auth/session/me") return json(route, { authenticated: true, isAdmin: admin, user: { id: actor, email: `${actor}@example.invalid` } });
      if (p === "/v1/regions") return json(route, []);
      if (p === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
      if (p === "/v1/map-display") return json(route, { showFlows: true, showHeat: true, viewport: { visibleStartPx: 90 }, revision: "1" });
      if (p === "/v1/market-billing/config") return json(route, { usdCnyRateMicros: 7000000, fxRateFresh: true });
      if (p === "/v1/market-wallet") return json(route, { assetCode: "USD", postedUnits: actor === "seller" ? "1728000000" : "0", heldUnits: "0", availableUnits: actor === "seller" ? "1728000000" : "0", version: 1, mode: "enabled", canTopup: true, hasWalletActivity: false });
      if (p.endsWith("/channels") || p === "/v1/admin/market-payment-channels") return json(route, []);
      if (p === "/v1/market-wallet/c2c/policy") return json(route, { policy, available: policy.enabled, verified: true, termsVersion: 1, claimGraceSeconds: 120, firstPurchaseMaxMinor: 10000 });
      if (p === "/v1/market-wallet/c2c/sellers") return json(route, { items: [seller], next: null });
      if (p === "/v1/market-wallet/c2c/seller/heartbeat") { heartbeat++; return json(route, { ok: true }); }
      if (p === "/v1/market-wallet/c2c/seller") {
        if (request.method() === "PUT") {
          assert.equal(actor, "seller"); assert.equal(body.acknowledgeExternalPayment, true); assert.equal(body.priceBps, 10000);
          profile = { ...body, userId: actor, version: (profile?.version ?? 0) + 1, suspended: false, heartbeatAt: new Date().toISOString() };
          return json(route, profile);
        }
        return json(route, { seller: profile, admissionBalanceUnits: "1728000000", availableUnits: "1728000000" });
      }
      if (p === "/v1/market-wallet/c2c/quotes") {
        latestQuote = { id: `quote-${++quoteCounter}`, sellerId: "seller", sellerName: seller.displayName, sellerVersion: 1, creditMinor: body.amountMinor, payMinor: body.amountMinor, currency: "USD", fxMicros: 1000000, priceBps: 10000, paymentMethod: "Bank transfer", expiresAt: new Date(Date.now() + quoteLifetimeMs).toISOString() };
        return json(route, latestQuote);
      }
      if (p === "/v1/market-wallet/c2c/orders" && request.method() === "POST") {
        assert.equal(body.acknowledgeExternalPayment, true); assert.equal(body.termsVersion, 1); keys.push(request.headers()["idempotency-key"]);
        const previous = orders.find(o => o.quoteId === body.quoteId); const o = previous ?? makeOrder(latestQuote, actor); if (!previous) orders.unshift(o);
        if (uncertain) { uncertain = false; return json(route, { message: "response lost" }, 504); }
        return json(route, o);
      }
      if (p === "/v1/market-wallet/c2c/orders" || p === "/v1/admin/market-finance/c2c/orders") {
        const items = orders.filter(o => (admin || o.buyerId === actor || o.sellerId === actor) && (!url.searchParams.get("quoteId") || o.quoteId === url.searchParams.get("quoteId")) && (url.searchParams.get("status") !== "active" || ["awaiting_payment", "paid", "disputed"].includes(o.status)) && (url.searchParams.get("status") !== "exceptions" || o.status === "disputed" || (o.latePaymentReported && !o.latePaymentResolved)));
        return json(route, { items: items.map(o => ({ ...o, contact: null, paymentDetails: null })), next: null });
      }
      const match = p.match(/^\/v1\/market-wallet\/c2c\/orders\/([^/]+)(?:\/(.+))?$/);
      if (match) {
        const o = orders.find(o => o.id === match[1]); if (!o) return json(route, { message: "not found" }, 404);
        const action = match[2]; if (!action) return json(route, o);
        if (action === "events") return json(route, { items: [{ id: "event", actorId: actor, action: o.status === "awaiting_payment" ? "created" : o.status, note: "", createdAt: o.createdAt }], next: null });
        if (action === "release-code") return json(route, { id: "challenge", expiresAt: new Date(Date.now() + 600000).toISOString(), resendAfterSeconds: 60 });
        if (action === "release") {
          assert.equal(actor, "seller"); assert.equal(body.acknowledgeReceived, true);
          if (body.code !== "123456") return json(route, { code: "C2C_CODE_INVALID", message: "wrong code" }, 409);
          o.status = "released"; o.releasedAt = new Date().toISOString(); released++;
        } else if (action === "paid") { assert.equal(actor, o.buyerId); o.status = "paid"; o.paidAt = new Date().toISOString(); }
        else if (action === "cancel") { assert.equal(body.acknowledgeUnpaid, true); o.status = "cancelled"; }
        else if (action === "dispute") { assert.ok(body.note); o.status = "disputed"; }
        else if (action === "late-payment") { assert.ok(body.note); o.latePaymentReported = true; }
        o.version++; o.updatedAt = new Date().toISOString(); return json(route, o);
      }
      if (p === "/v1/admin/settings") return json(route, { revision: "1", generatedAt: new Date().toISOString(), envPath: "/test/.env", schema: { categories: [{ id: "marketplace", label: "Marketplace", description: "Marketplace", fieldCount: 0 }], fields: [], groups: [] }, values: [], pendingRestartKeys: [] });
      if (p === "/v1/admin/market-finance/readiness") return json(route, { mode: "enabled", version: 1, newOrdersPaused: false, newTopupsPaused: false, codeIntegrated: true, legacyBlockers: [], criticalCases: 0, staleTopupReviews: 0, verifiedLiveTopups: 1, paymentChannelBlockers: [], canEnable: false });
      if (p === "/v1/admin/market-finance/overview") return json(route, overview);
      if (p === "/v1/admin/market-finance/wallet-policy") return json(route, { version: 1, maintenanceFeeBps: 500, transfersEnabled: true, transfersPaused: false, transferMaxMinor: 500000, transferDailyMaxMinor: 500000, balanceRefundable: false, canEdit: true });
      if (p === "/v1/admin/market-finance/commission-policy") return json(route, { version: 1, current: { policyVersion: 1, rateBps: 1000, effectiveAt: "2026-01-01T00:00:00Z" }, scheduled: null, increaseNoticeDays: 7, canEdit: true });
      if (p === "/v1/admin/market-finance/c2c/policy") {
        if (request.method() === "PUT") { assert.equal(body.version, policy.version); policy = { ...body, version: policy.version + 1 }; return json(route, policy); }
        return json(route, { policy, canManage: true, emailReady: true });
      }
      if (p === "/v1/admin/market-finance/c2c/sellers") return json(route, { items: profile ? [profile] : [], next: null });
      const adminOrder = p.match(/^\/v1\/admin\/market-finance\/c2c\/orders\/([^/]+)(?:\/(.+))?$/);
      if (adminOrder) {
        const o = orders.find(o => o.id === adminOrder[1]);
        if (adminOrder[2] === "events") return json(route, { items: [], next: null });
        if (adminOrder[2] === "resolve") { assert.ok(body.attestExternalEvidence); assert.ok(body.evidence); assert.equal(body.expectedVersion, o.version); resolution++; o.status = "cancelled"; o.version++; return json(route, o); }
        return json(route, { order: o, events: { items: [], next: null }, evidenceValid: true });
      }
      if (p.endsWith("inbox-summary")) return json(route, { pendingRequests: 0 });
      if (p.endsWith("/rooms")) return json(route, { rooms: [], totalUnread: 0 });
      if (p.includes("market-notifications")) return json(route, { items: [], nextCursor: null, unread: 0 });
      if (["topups", "transactions", "cases"].some(s => p.endsWith(`/${s}`))) return json(route, { items: [], nextCursor: null });
      if (p.startsWith("/v1/admin/")) return json(route, []);
      return json(route, {});
    });
    return { page, context };
  };
  const geometry = async (page, selector) => {
    const issues = await page.locator(selector).evaluate(root => {
      const issues = [];
      if (document.documentElement.scrollWidth > innerWidth + 1) issues.push("page horizontal overflow");
      for (const el of [root, ...root.querySelectorAll("button,input,select,textarea,p,dt,dd,h2,h3")]) {
        if (!el.getClientRects().length) continue;
        const r = el.getBoundingClientRect();
        if (r.left < -1 || r.right > innerWidth + 1) issues.push(`${el.tagName}: ${(el.textContent || "").slice(0, 50)} overflow`);
        if (["BUTTON", "INPUT", "SELECT"].includes(el.tagName) && !["checkbox", "radio"].includes(el.type) && r.height < 43.9) issues.push(`short target ${el.tagName}`);
        // Seller selectors contain multiple distinct rows; action labels
        // themselves must remain single-line at every breakpoint.
        if (el.matches("button[data-state]") && getComputedStyle(el).whiteSpace !== "nowrap") issues.push("action label can wrap");
      }
      return issues;
    });
    assert.deepEqual(issues, [], selector);
    await contrast(page, selector);
  };
  // Normalize computed colors using one solid swatch, not a page capture.
  const contrast = async (page, selector) => {
    const checks = await page.locator(selector).evaluate(root => {
      const canvas = document.createElement("canvas"); canvas.width = canvas.height = 1;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      const rgb = color => { ctx.clearRect(0, 0, 1, 1); ctx.fillStyle = color; ctx.fillRect(0, 0, 1, 1); return [...ctx.getImageData(0, 0, 1, 1).data].map(v => v / 255); };
      const bg = el => { if (!el) return [1, 1, 1]; const c = rgb(getComputedStyle(el).backgroundColor); const behind = c[3] < 1 ? bg(el.parentElement) : c; return c.slice(0, 3).map((v, i) => v * c[3] + behind[i] * (1 - c[3])); };
      const lum = c => c.slice(0, 3).map(v => v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4).reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0);
      const ratio = (a, b) => { const l = [lum(a), lum(b)].sort((a, b) => b - a); return (l[0] + 0.05) / (l[1] + 0.05); };
      const checks = [...root.querySelectorAll("h2,h3,p,span,button,input,textarea,select,dt,dd")].filter(el => el.getClientRects().length && !el.closest(":disabled") && (["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName) || [...el.childNodes].some(n => n.nodeType === Node.TEXT_NODE && n.textContent.trim()))).map(el => ({ label: (el.value || el.textContent).slice(0, 65), ratio: ratio(rgb(getComputedStyle(el).color), bg(el)), minimum: 4.5 }));
      const focused = root.querySelector(":focus-visible");
      if (focused) checks.push({ label: "keyboard focus outline", ratio: ratio(rgb(getComputedStyle(focused).outlineColor), bg(focused.parentElement)), minimum: 3 });
      return checks;
    });
    assert.ok(checks.length > 0);
    for (const c of checks) assert.ok(c.ratio >= c.minimum, `${c.label}: contrast ${c.ratio.toFixed(2)} < ${c.minimum}`);
  };
  const buyer = await open("buyer"); const page = buyer.page;
  await page.goto(`${base}/account/wallet/`); await page.getByRole("button", { name: "Buy from users", exact: true }).click();
  await page.locator("[data-c2c-buy]").getByRole("button", { name: /Ordinary verified user/ }).click();
  await page.getByLabel("Wallet credit to buy (USD)").fill("10");
  for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "[data-c2c-buy]"); }
  assert.equal(await page.getByText("Private account 123").count(), 0, "private payment info before reservation");
  await page.getByRole("button", { name: "Review purchase", exact: true }).click(); let dialog = page.getByRole("dialog", { name: "Review purchase" });
  await dialog.waitFor(); assert.equal(await dialog.getByRole("button", { name: "Reserve & create order" }).isDisabled(), true);
  await dialog.getByRole("checkbox").check();
  for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "dialog[open]"); }
  uncertain = true; await dialog.getByRole("button", { name: "Reserve & create order" }).click();
  await page.getByRole("button", { name: "Recover order" }).waitFor(); assert.equal(orders.length, 1); assert.equal(keys.length, 1);
  await page.reload(); await page.getByRole("button", { name: "Buy from users", exact: true }).click(); await page.getByRole("button", { name: "Recover order" }).click();
  dialog = page.getByRole("dialog", { name: "Order details" }); await dialog.getByText("Private account 123").waitFor(); assert.equal(orders.length, 1); assert.equal(keys.length, 1);
  for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "dialog[open]"); }
  await dialog.getByRole("button", { name: "I have paid", exact: true }).click(); await dialog.getByRole("checkbox").check(); await dialog.getByRole("button", { name: "I have paid", exact: true }).click();
  await dialog.getByText("Payment declared", { exact: true }).waitFor(); assert.equal(await dialog.getByRole("button", { name: "Cancel unpaid order" }).count(), 0);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  const sell = await open("seller"); await sell.page.goto(`${base}/account/wallet/`); await sell.page.getByRole("button", { name: "Sell balance", exact: true }).click();
  const sellerPanel = sell.page.locator("[data-c2c-orders]");
  await sellerPanel.getByLabel("Seller name", { exact: true }).fill("Ordinary verified user"); await sellerPanel.getByLabel("Payment method", { exact: true }).fill("Bank transfer");
  await sellerPanel.getByLabel("Contact seller", { exact: true }).fill("Private contact"); await sellerPanel.getByLabel("Payment details", { exact: true }).fill("Private account 123");
  await sellerPanel.getByLabel("Accept new orders", { exact: true }).check(); await sellerPanel.getByRole("checkbox", { name: /I will verify/ }).check();
  for (const width of [320, 375, 414, 768]) { await sell.page.setViewportSize({ width, height: 900 }); await geometry(sell.page, "[data-c2c-orders]"); }
  const firstHeartbeat = sell.page.waitForResponse(r => r.url().endsWith("/seller/heartbeat"));
  await sellerPanel.getByRole("button", { name: "Save", exact: true }).click(); await sellerPanel.getByText("Seller settings saved.").waitFor();
  await firstHeartbeat; assert.ok(heartbeat >= 1);
  await sell.page.goto(`${base}/account/wallet/?c2cOrder=${orders[0].id}`); const releaseDialog = sell.page.getByRole("dialog", { name: "Order details" });
  await releaseDialog.getByRole("checkbox", { name: /I personally verified/ }).check(); await releaseDialog.getByRole("button", { name: "Send release code", exact: true }).click();
  await releaseDialog.getByLabel("6-digit release code").fill("000000"); await releaseDialog.getByRole("button", { name: "Confirm receipt & release", exact: true }).click();
  await releaseDialog.getByRole("alert").waitFor(); assert.equal(released, 0);
  await releaseDialog.getByLabel("6-digit release code").fill("123456"); await releaseDialog.getByRole("button", { name: "Confirm receipt & release", exact: true }).click();
  await releaseDialog.getByText("Credit delivered", { exact: true }).first().waitFor(); assert.equal(released, 1);
  // A separate unpaid purchase can be cancelled; expiry hides instructions.
  orders.unshift(makeOrder({ ...latestQuote, id: "manual-quote" }, "buyer")); const unpaid = orders[0];
  await page.goto(`${base}/account/wallet/?c2cOrder=${unpaid.id}`); dialog = page.getByRole("dialog", { name: "Order details" }); await dialog.getByText("Private account 123").waitFor();
  await dialog.getByRole("button", { name: "Cancel unpaid order", exact: true }).click(); await dialog.getByRole("checkbox").check(); await dialog.getByRole("button", { name: "Cancel unpaid order", exact: true }).click(); await dialog.getByText("Cancelled", { exact: true }).waitFor();
  unpaid.status = "awaiting_payment"; unpaid.version++; unpaid.paymentDeadline = new Date(Date.now() - 30000).toISOString();
  await page.reload(); dialog = page.getByRole("dialog", { name: "Order details" }); await dialog.getByText(/Do not pay now/).waitFor(); assert.equal(await dialog.getByText("Private account 123").count(), 0);
  unpaid.status = "disputed"; unpaid.paidAt = new Date().toISOString(); unpaid.version++;
  const owner = await open("owner", "en", true); await owner.page.goto(`${base}/settings/`); await owner.page.getByRole("button", { name: "Marketplace", exact: true }).click();
  await owner.page.getByRole("tab", { name: "Platform wallet", exact: true }).click();
  const adminPanel = owner.page.locator("[data-c2c-admin]"); await adminPanel.getByText("C2C settings", { exact: true }).waitFor();
  await adminPanel.getByLabel("Opening balance threshold (USD)").fill("150"); await adminPanel.getByRole("button", { name: "Save", exact: true }).click(); assert.equal(policy.admissionMinor, 15000);
  await adminPanel.locator("summary").filter({ hasText: "C2C disputes & late payments" }).click(); await adminPanel.getByRole("button", { name: new RegExp(unpaid.id) }).click();
  const adjudication = owner.page.getByRole("dialog", { name: "Evidence-based resolution" }); await adjudication.getByLabel("Evidence-based resolution").selectOption("return_to_seller");
  await adjudication.getByLabel("Explanation / evidence reference").fill("Both parties confirm no payment. External reference recorded."); await adjudication.getByRole("checkbox").check();
  for (const width of [320, 375, 414, 768]) { await owner.page.setViewportSize({ width, height: 900 }); await geometry(owner.page, "dialog[open]"); }
  await adjudication.getByRole("button", { name: "Save", exact: true }).click(); await adjudication.getByText("Cancelled", { exact: true }).waitFor(); assert.equal(resolution, 1);
  // Corrupt local recovery data must not silently enable another order.
  await page.evaluate(() => sessionStorage.setItem("router_c2c_order_v1:buyer", "invalid-json"));
  await page.goto(`${base}/account/wallet/`); await page.getByRole("button", { name: "Buy from users", exact: true }).click();
  const buyPanel = page.locator("[data-c2c-buy]"); await buyPanel.getByRole("button", { name: "Check existing orders", exact: true }).waitFor();
  assert.equal(await buyPanel.getByRole("button", { name: /Ordinary verified user/ }).count(), 0);
  await buyPanel.getByRole("button", { name: "Check existing orders", exact: true }).click();
  await buyPanel.getByRole("button", { name: /Ordinary verified user/ }).click(); await buyPanel.getByLabel("Wallet credit to buy (USD)").fill("10");
  quoteLifetimeMs = -1000; await buyPanel.getByRole("button", { name: "Review purchase", exact: true }).click(); dialog = page.getByRole("dialog", { name: "Review purchase" });
  await dialog.getByText(/This quote expired/).waitFor(); await dialog.getByRole("checkbox").check(); assert.equal(await dialog.getByRole("button", { name: "Reserve & create order", exact: true }).isDisabled(), true);
  quoteLifetimeMs = 300000; await dialog.getByRole("button", { name: "Refresh quote", exact: true }).click();
  await dialog.getByRole("button", { name: "Refresh quote", exact: true }).waitFor({ state: "hidden" }); assert.equal(await dialog.getByRole("checkbox").isChecked(), false);
  for (const dark of [false, true]) {
    await page.evaluate(dark => document.documentElement.classList.toggle("dark", dark), dark);
    for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "dialog[open]"); await contrast(page, "dialog[open]"); }
  }
  await dialog.getByRole("checkbox").focus(); await page.keyboard.press("Tab");
  assert.equal(await page.evaluate(() => !!document.activeElement?.closest("dialog[open]")), true);
  await contrast(page, "dialog[open]"); await page.keyboard.press("Escape"); await dialog.waitFor({ state: "hidden" });
  const skewOrder = makeOrder({ ...latestQuote, id: "skew-quote" }, "buyer"); orders.unshift(skewOrder);
  for (const delta of [-86400000, 86400000]) {
    const skew = await open("buyer"); await skew.page.clock.install({ time: new Date(Date.now() + delta) });
    skewOrder.paymentDeadline = new Date(Date.now() + 1800000).toISOString();
    await skew.page.goto(`${base}/account/wallet/?c2cOrder=${skewOrder.id}`); const skewDialog = skew.page.getByRole("dialog", { name: "Order details" });
    await skewDialog.getByText("Private account 123").waitFor(); assert.equal(await skewDialog.getByRole("button", { name: "I have paid", exact: true }).isDisabled(), false);
    skewOrder.paymentDeadline = new Date(Date.now() - 30000).toISOString(); await skew.page.reload();
    await skewDialog.getByText(/Do not pay now/).waitFor(); assert.equal(await skewDialog.getByText("Private account 123").count(), 0);
    await skew.context.close();
  }
  const zh = await open("buyer", "zh-CN"); await zh.page.goto(`${base}/account/wallet/`); await zh.page.getByRole("button", { name: "C2C 购入", exact: true }).click();
  await zh.page.locator("[data-c2c-buy]").getByRole("button", { name: /Ordinary verified user/ }).waitFor();
  for (const width of [320, 375, 414, 768]) { await zh.page.setViewportSize({ width, height: 900 }); await geometry(zh.page, "[data-c2c-buy]"); }
  assert.deepEqual(errors, []);
  console.log("C2C browser checks passed: lost/corrupt recovery, expired quote refresh, buyer payment/cancel, seller admission/heartbeat/code/release, owner policy/adjudication, bilingual 320/375/414/768 DOM geometry, light/dark contrast, keyboard dialog controls and +/-24h clock skew; no screenshots or real operations.");
} finally { await browser?.close(); await new Promise(resolveClose => server.close(resolveClose)); }
