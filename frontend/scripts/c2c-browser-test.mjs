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
  const errors = []; const orders = []; const keys = []; const loginCodeRequests = []; let counter = 0; let quoteCounter = 0; let quoteLifetimeMs = 300000; let uncertain = false; let heartbeat = 0; let released = 0; let resolution = 0; let manualAcceptRequests = 0;
  let policy = { version: 1, enabled: true, admissionMinor: 10000, minMinor: 100, maxMinor: 500000, paymentMinutes: 30 };
  let profile = null; let sellerUnits = "1728000000"; let available = true; let pauseCount = 0; let financeFailure = false; let sellerReadFailure = false; let heldIncome = false;
  let topupMode = "shadow"; let publicationReady = true; let retiredSetupRequests = 0; let offerWrites = 0;
  const publicationHost = { id: "publication-host", providerId: "seller", hostOwnerEmail: "seller@example.invalid", ip: "192.0.2.50", hostname: "Publication Host", port: 22,
    countryCode: "JP", status: "idle", isHostOwner: true, pricingModel: "prepaid_calendar_month", cyclePriceMinor: 1000, billingInterval: "calendar_month",
    currency: "USD", offerRevision: 1, paymentMethodKinds: [], contacts: [], eligibility: { allowed: true }, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() };
  const description = "Bank transfer · contact @seller · https://seller.example.invalid/pay · <b>plain text</b>";
  const seller = { sellerId: "seller", version: 1, displayName: "C2C-123456abcdef", description, currency: "USD", priceBps: 10000, minMinor: 100, maxMinor: 10000, availableMinor: 20000, completedOrders: 0 };
  const overview = Object.fromEntries(["walletPostedUnits", "walletHeldUnits", "unfulfilledUnits", "providerPendingUnits", "providerAvailableUnits", "providerHeldUnits", "buyerRefundHeldUnits", "ownerLossUnits", "maintenanceRevenueUnits"].map(k => [k, "0"]));
  Object.assign(overview, { estimatedGatewayCostMinor: 0, creditedTopupMinor: 0, grossTopupMinor: 0, openCases: 0, openCriticalCases: 0, heldEarningLots: 0 });
  const makeOrder = (q, actor) => {
    const createdAt = new Date().toISOString();
    return { id: `order-${++counter}`, quoteId: q.id, buyerId: actor, sellerId: "seller", amountUnits: String(q.creditMinor * 86400), status: "awaiting_payment", version: 1,
      flowVersion: 3, acceptanceDeadline: null, acceptedAt: null, paymentDeadline: new Date(Date.parse(createdAt) + q.paymentMinutes * 60000).toISOString(), paidAt: null, releasedAt: null, latePaymentReported: false, latePaymentResolved: false,
      createdAt, updatedAt: createdAt, sellerName: seller.displayName, payMinor: q.payMinor, currency: q.currency, fxMicros: 1000000, priceBps: 10000,
      description: q.description, legacyPayment: null };
  };
  const makeLegacyOrder = (q, actor) => ({ ...makeOrder(q, actor), flowVersion: 2, status: "awaiting_seller", paymentDeadline: null, acceptanceDeadline: new Date(Date.now() + 600000).toISOString() });
  const acceptOrder = o => Object.assign(o, { status: "awaiting_payment", version: o.version + 1, acceptedAt: new Date().toISOString(), paymentDeadline: new Date(Date.now() + 1800000).toISOString() });
  const disputeHandlers = [];
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
      if (p.startsWith("/v1/auth/") && p !== "/v1/auth/session/me") loginCodeRequests.push(p);
      if (p === "/v1/auth/session/me") return json(route, { authenticated: true, isAdmin: admin, user: { id: actor, email: `${actor}@example.invalid` } });
      if (p === "/v1/regions") return json(route, []);
      if (p === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
      if (p === "/v1/map-display") return json(route, { showFlows: true, showHeat: true, viewport: { visibleStartPx: 90 }, revision: "1" });
      if (p === "/v1/market-billing/config") return json(route, { usdCnyRateMicros: 7000000, fxRateFresh: true });
      if (p === "/v1/market-wallet") return json(route, { assetCode: "USD", postedUnits: actor === "seller" ? "1728000000" : "0", heldUnits: "0", availableUnits: actor === "seller" ? "1728000000" : "0", version: 1, mode: topupMode, canTopup: false, hasWalletActivity: false });
      if (p === "/v1/market-wallet/transfer-capacity") return json(route, { transferableUnits: actor === "seller" ? sellerUnits : "0", committedUnits: "0", dailyRemainingUnits: "43200000000", enabled: true, verified: true, blockers: [] });
      if (p === "/v1/market-wallet/commission-policy") return json(route, { version: 1, current: { policyVersion: 1, rateBps: 1000, effectiveAt: "2026-01-01T00:00:00Z" }, scheduled: null, increaseNoticeDays: 7, canEdit: false });
      if (["/v1/account/payment-profile", "/v1/market-billing/dashboard"].includes(p)) { retiredSetupRequests++; return json(route, { message: "retired setup" }, 410); }
      if (p === "/v1/client-market/hosts") return json(route, [publicationHost]);
      if (p === "/v1/client-market/my-rentals") return json(route, []);
      if (p === "/v1/client-market/hosts/publication-host/offer" && request.method() === "PATCH") {
        assert.deepEqual(body, { cyclePriceMinor: 1000, currency: "USD" }); offerWrites++; publicationHost.offerRevision++;
        return json(route, { ...publicationHost, hostId: publicationHost.id });
      }
      if (p.endsWith("/channels") || p === "/v1/admin/market-payment-channels") return json(route, []);
      const blockers = [...(!policy.enabled ? ["C2C_DISABLED"] : []), ...(!available ? ["C2C_TRANSFERS_PAUSED"] : [])];
      if (p === "/v1/market-wallet/c2c/policy") return json(route, { policy, available: !blockers.length, blockers, canManagePolicy: admin, verified: true, termsVersion: 3, independentDisputeHandlerAvailable: disputeHandlers.some(h => h.enabled), claimGraceSeconds: 120, firstPurchaseMaxMinor: 10000 });
      if (p === "/v1/market-wallet/c2c/sellers") {
        const items = actor === "seller" || !policy.enabled || !available || profile?.autoPauseReason || profile?.autoAcceptTermsVersion === 0 ? [] : [seller];
        return json(route, { items, next: null, emptyReason: items.length ? null : !policy.enabled || !available ? "platform_paused" : "no_other_sellers" });
      }
      if (p === "/v1/market-wallet/c2c/seller/heartbeat") { heartbeat++; return json(route, { message: "retired" }, 410); }
      if (p === "/v1/market-wallet/c2c/seller/resume") {
        assert.equal(profile.autoAcceptTermsVersion, 3);
        assert.equal(body.expectedVersion, profile.version);
        profile = { ...profile, autoPauseReason: null, autoPausedAt: null, autoPauseOrderId: null, version: profile.version + 1 };
        return json(route, profile);
      }
      if (p === "/v1/market-wallet/c2c/seller/pause") {
        assert.equal(actor, "seller"); assert.equal(body.expectedVersion, profile.version);
        assert.deepEqual(Object.keys(body), ["expectedVersion"]);
        profile = { ...profile, enabled: false, autoPauseReason: null, autoPausedAt: null, autoPauseOrderId: null, version: profile.version + 1 }; pauseCount++;
        return json(route, profile);
      }
      if (p === "/v1/market-wallet/c2c/seller") {
        if (request.method() === "PUT") {
          assert.equal(actor, "seller"); assert.equal(body.acknowledgeExternalPayment, true); assert.equal(body.priceBps, 10000);
          assert.equal(body.autoAcceptTermsVersion, 3);
          assert.equal(body.displayName, undefined); assert.equal(body.contact, undefined); assert.equal(body.paymentMethod, undefined); assert.equal(body.paymentDetails, undefined);
          if (body.expectedVersion !== (profile?.version ?? 0)) return json(route, { code: "C2C_STALE", message: "stale version" }, 409);
          profile = { ...body, userId: actor, displayName: seller.displayName, version: (profile?.version ?? 0) + 1, suspended: false,
            autoPauseReason: profile?.autoPauseReason ?? null, autoPausedAt: profile?.autoPausedAt ?? null, autoPauseOrderId: profile?.autoPauseOrderId ?? null,
            autoAcceptConsentedAt: profile?.autoAcceptConsentedAt ?? new Date().toISOString() };
          return json(route, profile);
        }
        if (sellerReadFailure) return json(route, { message: "Seller settings temporarily unavailable" }, 503);
        const sellerBlockers = [...blockers, ...(!profile?.enabled && BigInt(sellerUnits) < BigInt(policy.admissionMinor) * 86400n ? ["C2C_ADMISSION"] : [])];
        return json(route, { seller: actor === "seller" ? profile : null, state: actor !== "seller" || !profile ? "not_selling" : !profile.enabled ? "paused" : profile.autoAcceptTermsVersion === 0 ? "confirmation_required" : profile.autoPauseReason ? "auto_paused" : !blockers.length ? "selling" : "unavailable", pendingOrders: orders.filter(o => o.sellerId === actor && ["awaiting_seller", "awaiting_payment", "paid", "disputed"].includes(o.status)).length, admissionBalanceUnits: sellerUnits, availableUnits: sellerUnits, blockers: sellerBlockers, canEnable: !sellerBlockers.length });
      }
      if (p === "/v1/market-wallet/c2c/quotes") {
        latestQuote = { id: `quote-${++quoteCounter}`, sellerId: "seller", sellerName: seller.displayName, sellerVersion: 1, creditMinor: body.amountMinor, payMinor: body.amountMinor, currency: "USD", fxMicros: 1000000, priceBps: 10000, description, expiresAt: new Date(Date.now() + quoteLifetimeMs).toISOString(), flowVersion: 3, paymentMinutes: policy.paymentMinutes };
        return json(route, latestQuote);
      }
      if (p === "/v1/market-wallet/c2c/orders" && request.method() === "POST") {
        assert.equal(body.acknowledgeExternalPayment, true); assert.equal(body.termsVersion, 3); keys.push(request.headers()["idempotency-key"]);
        const previous = orders.find(o => o.quoteId === body.quoteId); const o = previous ?? makeOrder(latestQuote, actor); if (!previous) orders.unshift(o);
        if (uncertain) { uncertain = false; return json(route, { message: "response lost" }, 504); }
        return json(route, o);
      }
      if (p === "/v1/market-wallet/c2c/orders" || p === "/v1/admin/market-finance/c2c/orders") {
        const items = orders.filter(o => (admin || o.buyerId === actor || o.sellerId === actor) && (!url.searchParams.get("quoteId") || o.quoteId === url.searchParams.get("quoteId")) && (url.searchParams.get("status") !== "active" || ["awaiting_seller", "awaiting_payment", "paid", "disputed"].includes(o.status)) && (url.searchParams.get("status") !== "exceptions" || o.status === "disputed" || (o.latePaymentReported && !o.latePaymentResolved)));
        return json(route, { items: items.map(o => ({ ...o, legacyPayment: null })), next: null });
      }
      const match = p.match(/^\/v1\/market-wallet\/c2c\/orders\/([^/]+)(?:\/(.+))?$/);
      if (match) {
        const o = orders.find(o => o.id === match[1]); if (!o) return json(route, { message: "not found" }, 404);
        const action = match[2]; if (!action) return json(route, o);
        if (action === "events") return json(route, { items: [{ id: "event", actorId: actor, action: o.status === "awaiting_payment" ? "created" : o.status, note: "", createdAt: o.createdAt }], next: null });
        if (action === "accept") { assert.equal(actor, o.sellerId); assert.equal(o.flowVersion, 2); manualAcceptRequests++; acceptOrder(o); return json(route, o); }
        if (action === "reject") { assert.equal(actor, o.sellerId); o.status = "cancelled"; o.version++; return json(route, o); }
        if (action === "release-code") assert.fail("retired financial OTP endpoint must not be called");
        if (action === "release") {
          assert.equal(actor, "seller"); assert.equal(body.acknowledgeReceived, true);
          assert.deepEqual(body, { expectedVersion: o.version, acknowledgeReceived: true });
          o.status = "released"; o.releasedAt = new Date().toISOString(); released++;
        } else if (action === "paid") { assert.equal(actor, o.buyerId); o.status = "paid"; o.paidAt = new Date().toISOString(); }
        else if (action === "cancel") { assert.equal(body.acknowledgeUnpaid, true); o.status = "cancelled"; }
        else if (action === "dispute") { assert.ok(body.note); o.status = "disputed"; }
        else if (action === "late-payment") { assert.ok(body.note); o.latePaymentReported = true; if (o.status === "awaiting_seller") { o.status = "disputed"; o.paidAt = new Date().toISOString(); } }
        o.version++; o.updatedAt = new Date().toISOString(); return json(route, o);
      }
      if (p === "/v1/admin/settings") return json(route, { revision: "1", generatedAt: new Date().toISOString(), envPath: "/test/.env", schema: { categories: [{ id: "marketplace", label: "Marketplace", description: "Marketplace", fieldCount: 0 }], fields: [], groups: [] }, values: [], pendingRestartKeys: [] });
      if (p === "/v1/admin/market-finance/readiness") return json(route, { mode: "shadow", version: 1, newOrdersPaused: false, newTopupsPaused: false, codeIntegrated: true, criticalCases: 0, staleTopupReviews: 0, verifiedLiveTopups: 1, paymentChannelBlockers: [], canEnable: false });
      if (p === "/v1/admin/market-finance/overview") return json(route, overview);
      if (p === "/v1/admin/market-finance/wallet-policy") return json(route, { version: 1, maintenanceFeeBps: 500, transfersEnabled: true, transfersPaused: false, transferMaxMinor: 500000, transferDailyMaxMinor: 500000, balanceRefundable: false, canEdit: true });
      if (p === "/v1/admin/market-finance/commission-policy") return json(route, { version: 1, current: { policyVersion: 1, rateBps: 1000, effectiveAt: "2026-01-01T00:00:00Z" }, scheduled: null, increaseNoticeDays: 7, canEdit: true });
      if (p === "/v1/admin/market-finance/c2c/policy") {
        if (request.method() === "PUT") { assert.equal(body.version, policy.version); policy = { ...body, version: policy.version + 1 }; return json(route, policy); }
        return json(route, { policy, canManage: actor === "owner", canResolve: actor === "owner" || disputeHandlers.some(h => h.enabled && h.userId === actor), });
      }
      if (p === "/v1/admin/market-finance/c2c/dispute-handlers") {
        assert.equal(actor, "owner");
        if (request.method() === "PUT") {
          const previous = disputeHandlers.find(h => h.email === body.email);
          assert.equal(body.expectedVersion, previous?.version ?? 0);
          if (previous) Object.assign(previous, { enabled: body.enabled, version: previous.version + 1 });
          else disputeHandlers.push({ userId: "handler", email: body.email, version: 1, enabled: true, isAdmin: true, updatedAt: new Date().toISOString() });
          return json(route, { ok: true });
        }
        return json(route, { items: disputeHandlers, next: null });
      }
      if (p === "/v1/admin/market-finance/c2c/sellers") return json(route, { items: profile ? [profile] : [], next: null });
      const adminOrder = p.match(/^\/v1\/admin\/market-finance\/c2c\/orders\/([^/]+)(?:\/(.+))?$/);
      if (adminOrder) {
        const o = orders.find(o => o.id === adminOrder[1]);
        if (adminOrder[2] === "events") return json(route, { items: [], next: null });
        if (adminOrder[2] === "resolve") { assert.ok(body.attestExternalEvidence); assert.ok(body.evidence); assert.equal(body.expectedVersion, o.version); resolution++; o.status = "cancelled"; o.version++; return json(route, o); }
        return json(route, { order: o, events: { items: [], next: null }, evidenceValid: true, canResolve: actor !== o.sellerId && actor !== o.buyerId && (actor === "owner" || disputeHandlers.some(h => h.enabled && h.userId === actor)) });
      }
      if (p.endsWith("inbox-summary")) return json(route, { pendingRequests: 0 });
      if (p === "/v1/market-provider-finance/me") {
        if (financeFailure) return json(route, { message: "Income temporarily unavailable" }, 503);
        return json(route, { isProvider: actor === "seller", canPublishPaidOffers: actor === "seller" && publicationReady, hasFinanceActivity: actor === "seller", automaticCredit: true,
          summary: { grossEarnedUnits: "1728000000", commissionUnits: "172800000", netEarnedUnits: "1555200000", pendingUnits: heldIncome ? "423360000" : "432000000", readyUnits: "0", restrictedUnits: heldIncome ? "8640000" : "0", creditedUnits: "1123200000" } });
      }
      if (p === "/v1/market-provider-finance/earnings") return json(route, { items: [], nextCursor: null });
      if (p === "/v1/market-provider-finance/holds") return json(route, heldIncome && actor === "seller" ? [{ reasonKind: "dispute", reasonRef: "contract", providerUserId: actor, lots: 1, heldUnits: "8640000", createdAt: new Date().toISOString() }] : []);
      if (p === "/v1/market-access/dashboard") return json(route, { policies: [], counterparties: [], accessRequests: [] });
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
  await page.goto(`${base}/account/wallet/`); await page.getByRole("button", { name: "C2C", exact: true }).click();
  await page.getByRole("button", { name: "Buy balance", exact: true }).click();
  await page.locator("[data-c2c-buy]").getByRole("button", { name: new RegExp(seller.displayName) }).click();
  await page.getByLabel("Wallet credit to buy (USD)").fill("10");
  for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "[data-c2c-buy]"); }
  const publicDescription = page.locator("[data-c2c-buy] [data-c2c-description]");
  assert.equal(await publicDescription.textContent(), description);
  assert.equal(await publicDescription.locator("b,iframe,img").count(), 0, "description is inert text, not seller HTML");
  assert.equal(await publicDescription.getByRole("link").getAttribute("target"), "_blank");
  assert.equal(await publicDescription.getByRole("link").getAttribute("rel"), "noopener noreferrer nofollow ugc");
  await page.getByRole("button", { name: "Review purchase", exact: true }).click(); let dialog = page.getByRole("dialog", { name: "Review purchase" });
  await dialog.waitFor(); assert.equal(await dialog.getByRole("button", { name: "Create order" }).isDisabled(), true);
  await dialog.getByRole("checkbox").check();
  for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "dialog[open]"); }
  uncertain = true; await dialog.getByRole("button", { name: "Create order" }).click();
  await page.getByRole("button", { name: "Recover order" }).waitFor(); assert.equal(orders.length, 1); assert.equal(keys.length, 1);
  await page.reload(); await page.getByRole("button", { name: "C2C", exact: true }).click();
  await page.getByRole("button", { name: "Buy balance", exact: true }).click(); await page.getByRole("button", { name: "Recover order" }).click();
  dialog = page.getByRole("dialog", { name: "Order details" }); await dialog.getByText("Awaiting payment", { exact: true }).waitFor();
  assert.equal(await dialog.getByRole("button", { name: "I have paid", exact: true }).count(), 1);
  assert.equal(Date.parse(orders[0].paymentDeadline) - Date.parse(orders[0].createdAt), 1800000);
  assert.equal(orders[0].acceptedAt, null, "standing authorization is not fake manual acceptance");
  const acceptor = await open("seller"); await acceptor.page.goto(`${base}/account/wallet/?c2cOrder=${orders[0].id}`);
  const acceptDialog = acceptor.page.getByRole("dialog", { name: "Order details" });
  await acceptDialog.getByText("Awaiting payment", { exact: true }).waitFor();
  assert.equal(await acceptDialog.getByRole("button", { name: "Accept", exact: true }).count(), 0);
  assert.equal(await acceptDialog.getByRole("button", { name: "Decline", exact: true }).count(), 0);
  assert.equal(manualAcceptRequests, 0, "new orders never issue a manual acceptance request");
  await acceptor.context.close(); await page.goto(`${base}/account/wallet/?c2cOrder=${orders[0].id}`); dialog = page.getByRole("dialog", { name: "Order details" });
  await dialog.locator("[data-c2c-description]").waitFor(); assert.equal(await dialog.locator("[data-c2c-description]").textContent(), description); assert.equal(orders.length, 1); assert.equal(keys.length, 1);
  for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "dialog[open]"); }
  await dialog.getByRole("button", { name: "I have paid", exact: true }).click(); await dialog.getByRole("checkbox").check(); await dialog.getByRole("button", { name: "I have paid", exact: true }).click();
  await dialog.getByText("Payment declared", { exact: true }).waitFor(); assert.equal(await dialog.getByRole("button", { name: "Cancel unpaid order" }).count(), 0);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  policy.enabled = false; sellerUnits = "0";
  const sell = await open("seller"); await sell.page.clock.install(); await sell.page.goto(`${base}/account/wallet/`); await sell.page.getByRole("button", { name: "Sell balance", exact: true }).click();
  const sellerPanel = sell.page.locator("[data-c2c-orders]");
  let editor = sell.page.getByRole("dialog", { name: "Sell balance", exact: true });
  await editor.getByLabel("C2C description", { exact: true }).fill(description);
  assert.equal(await sellerPanel.locator(":scope > ul > li").count(), 1, "order list stays mounted behind seller settings");
  assert.equal(await editor.locator("input:visible:not([type=checkbox])").count(), 1, "only public description is expanded by default");
  assert.equal(await editor.getByRole("button", { name: "Start selling", exact: true }).isDisabled(), true);
  await editor.getByRole("checkbox", { name: /I will verify/ }).check();
  assert.equal(await editor.getByRole("button", { name: "Start selling", exact: true }).isDisabled(), true, "disabled C2C is explicit, not a missing email OTP");
  await editor.getByText(/Opening threshold/).waitFor();
  assert.equal(await editor.getByRole("link", { name: "Open wallet settings", exact: true }).count(), 0, "ordinary sellers have no owner-only shortcut");
  const management = await open("owner", "en", true);
  await management.page.goto(`${base}/account/wallet/?c2cSell=1`);
  const managementLink = management.page.getByRole("dialog", { name: "Sell balance", exact: true }).getByRole("link", { name: "Open wallet settings", exact: true });
  await managementLink.waitFor();
  assert.equal(await managementLink.getAttribute("href"), "/settings/?panel=market_finance");
  await managementLink.click();
  await management.page.locator("[data-c2c-admin]").getByText("C2C settings", { exact: true }).waitFor();
  await management.context.close();
  sellerReadFailure = true;
  await editor.getByRole("button", { name: "Refresh eligibility", exact: true }).click();
  await editor.getByRole("alert").filter({ hasText: /Eligibility could not be refreshed/ }).waitFor();
  assert.equal(await editor.getByLabel("C2C description", { exact: true }).inputValue(), description);
  assert.equal(await editor.getByRole("checkbox", { name: /I will verify/ }).isChecked(), true);
  sellerReadFailure = false; policy.enabled = true; sellerUnits = String(BigInt(policy.admissionMinor) * 86400n);
  await editor.getByRole("button", { name: "Refresh eligibility", exact: true }).click();
  await sell.page.waitForFunction(() => [...document.querySelectorAll("dialog[open] button")].some(b => b.textContent.trim() === "Start selling" && !b.disabled));
  assert.equal(await editor.getByLabel("C2C description", { exact: true }).inputValue(), description, "eligibility refresh preserves the draft");
  assert.equal(await editor.getByRole("checkbox", { name: /I will verify/ }).isChecked(), true);
  for (const width of [320, 375, 414, 768]) { await sell.page.setViewportSize({ width, height: 900 }); await geometry(sell.page, "dialog[open]"); }
  await editor.getByRole("button", { name: "Start selling", exact: true }).click(); await sellerPanel.getByText("Seller settings saved.").waitFor();
  assert.deepEqual(loginCodeRequests, [], "a logged-in seller publishes without a second email login challenge");
  await editor.waitFor({ state: "hidden" });
  await sell.page.clock.fastForward(61000); assert.equal(heartbeat, 0, "selling never sends browser heartbeats");
  await sellerPanel.getByRole("button", { name: "Sell settings", exact: true }).click(); editor = sell.page.getByRole("dialog", { name: "Sell settings", exact: true });
  await editor.getByLabel("C2C description", { exact: true }).fill("Unsaved local draft");
  profile = { ...profile, description: "Another session changed the settings", version: profile.version + 1 };
  const poll = sell.page.waitForResponse(r => r.url().endsWith("/c2c/seller") && r.request().method() === "GET");
  await sell.page.clock.fastForward(31000); await poll;
  assert.equal(await editor.getByLabel("C2C description", { exact: true }).inputValue(), "Unsaved local draft", "polling preserves the draft and original version");
  await editor.getByRole("checkbox", { name: /I will verify/ }).check();
  await editor.getByRole("status").filter({ hasText: /changed/ }).waitFor();
  assert.equal(await editor.getByRole("button", { name: "Save", exact: true }).isDisabled(), true, "polling detects stale reviewed terms without overwriting the draft");
  sellerReadFailure = true;
  await editor.getByRole("button", { name: "Reload settings", exact: true }).click();
  await editor.getByRole("alert").filter({ hasText: /could not be confirmed/ }).waitFor();
  assert.equal(await editor.getByLabel("C2C description", { exact: true }).inputValue(), "Unsaved local draft", "failed reload stays in the editor without dropping draft");
  sellerReadFailure = false; await editor.getByRole("button", { name: "Reload settings", exact: true }).click();
  await sell.page.waitForFunction(() => document.querySelector("[data-c2c-seller-editor] input")?.value === "Another session changed the settings");
  assert.equal(await editor.getByRole("checkbox").isChecked(), false);
  await editor.getByRole("button", { name: "Close", exact: true }).click();
  // Old sellers must confirm once; confirmation below admission preserves
  // the old pause, which still requires an explicit Resume action.
  profile = { ...profile, autoAcceptTermsVersion: 0, autoAcceptConsentedAt: null, autoPauseReason: "acceptance_timeout", autoPausedAt: new Date().toISOString(), autoPauseOrderId: orders[0].id, version: profile.version + 1 };
  sellerUnits = String(2000 * 86400); await sell.page.reload();
  await sellerPanel.getByText(/Automatic orders need confirmation/).waitFor();
  assert.equal(await sellerPanel.getByRole("button", { name: "Resume selling", exact: true }).count(), 0);
  await sellerPanel.getByRole("button", { name: "Confirm automatic orders", exact: true }).click();
  editor = sell.page.getByRole("dialog", { name: "Sell settings", exact: true });
  assert.equal(await editor.getByRole("checkbox").isChecked(), false);
  await editor.getByRole("checkbox").check();
  await editor.getByRole("button", { name: "Confirm automatic orders", exact: true }).click();
  await editor.waitFor({ state: "hidden" });
  assert.equal(profile.autoAcceptTermsVersion, 3);
  assert.equal(profile.autoPauseReason, "acceptance_timeout", "consent cannot silently resume an old pause");
  await sellerPanel.getByRole("button", { name: "Resume selling", exact: true }).click();
  await sellerPanel.getByText("Accepting orders", { exact: true }).waitFor();
  assert.equal(profile.autoPauseReason, null); assert.equal(profile.enabled, true);
  await sell.page.goto(`${base}/account/provider-settings/`);
  await page.goto(`${base}/account/wallet/`); await page.getByRole("button", { name: "C2C", exact: true }).click();
  await page.getByRole("button", { name: "Buy balance", exact: true }).click();
  await page.locator("[data-c2c-buy]").getByRole("button", { name: new RegExp(seller.displayName) }).waitFor();
  assert.equal(heartbeat, 0, "seller remains listed after navigating away without any heartbeat");
  sellerUnits = "0"; available = false;
  await sell.page.goto(`${base}/account/wallet/`); await sellerPanel.getByRole("button", { name: "Pause", exact: true }).click();
  await sellerPanel.getByText("Selling paused", { exact: true }).waitFor(); assert.equal(pauseCount, 1, "pause independent of balance, policy and editor validation");
  await sell.page.clock.fastForward(61000); assert.equal(heartbeat, 0, "pause needs no browser presence");
  sellerUnits = "1728000000"; available = true;
  await sell.page.goto(`${base}/account/wallet/?c2cOrder=${orders[0].id}`); const releaseDialog = sell.page.getByRole("dialog", { name: "Order details" });
  assert.equal(await releaseDialog.locator('input[autocomplete="one-time-code"]').count(), 0);
  assert.equal(await releaseDialog.getByRole("button", { name: "Send release code", exact: true }).count(), 0);
  assert.equal(await releaseDialog.getByRole("button", { name: "Confirm receipt & release", exact: true }).isDisabled(), true);
  await releaseDialog.getByRole("checkbox", { name: /I personally verified/ }).check();
  await releaseDialog.getByRole("button", { name: "Confirm receipt & release", exact: true }).click();
  await releaseDialog.getByText("Credit delivered", { exact: true }).first().waitFor(); assert.equal(released, 1);
  const withdrawn = makeLegacyOrder({ ...latestQuote, id: "withdraw-quote" }, "buyer"); orders.unshift(withdrawn);
  await page.goto(`${base}/account/wallet/?c2cOrder=${withdrawn.id}`); dialog = page.getByRole("dialog", { name: "Order details" });
  await dialog.getByText("Awaiting seller", { exact: true }).waitFor();
  for (const width of [320, 375, 414, 768]) { await page.setViewportSize({ width, height: 900 }); await geometry(page, "dialog[open]"); }
  await dialog.getByRole("checkbox").check(); await dialog.getByRole("button", { name: "Withdraw application", exact: true }).click();
  await dialog.getByText("Cancelled", { exact: true }).waitFor(); assert.equal(withdrawn.acceptedAt, null);
  const declined = makeLegacyOrder({ ...latestQuote, id: "decline-quote" }, "buyer"); orders.unshift(declined);
  await sell.page.goto(`${base}/account/wallet/?c2cOrder=${declined.id}`);
  await sell.page.getByRole("dialog", { name: "Order details" }).getByRole("button", { name: "Decline", exact: true }).click();
  await sell.page.getByRole("dialog").getByText("Cancelled", { exact: true }).waitFor(); assert.equal(declined.paymentDeadline, null);
  const early = makeLegacyOrder({ ...latestQuote, id: "early-quote" }, "buyer"); orders.unshift(early);
  await page.goto(`${base}/account/wallet/?c2cOrder=${early.id}`); dialog = page.getByRole("dialog", { name: "Order details" });
  await dialog.locator("summary").filter({ hasText: "Already paid before acceptance?" }).click();
  await dialog.getByLabel("Explanation / evidence reference").fill("External transfer made prematurely. Check evidence independently.");
  await dialog.getByRole("button", { name: "Report payment exception", exact: true }).click(); await dialog.getByText("In dispute", { exact: true }).waitFor();
  await sell.page.goto(`${base}/account/wallet/?c2cOrder=${early.id}`);
  const exceptional = sell.page.getByRole("dialog", { name: "Order details" }); await exceptional.getByText("In dispute", { exact: true }).waitFor();
  assert.equal(await exceptional.getByRole("button", { name: "Confirm receipt & release", exact: true }).count(), 0, "never-accepted exceptional claims need independent resolution");
  const legacy = makeLegacyOrder({ ...latestQuote, id: "legacy-accept-quote" }, "buyer"); orders.unshift(legacy);
  await sell.page.goto(`${base}/account/wallet/?c2cOrder=${legacy.id}`);
  const legacyDialog = sell.page.getByRole("dialog", { name: "Order details" });
  await legacyDialog.getByRole("button", { name: "Accept", exact: true }).click();
  await legacyDialog.getByText("Awaiting payment", { exact: true }).waitFor();
  assert.equal(manualAcceptRequests, 1, "only a historical v2 application still needs acceptance");
  assert.equal(Date.parse(legacy.paymentDeadline) - Date.parse(legacy.acceptedAt), 1800000);
  await page.goto(`${base}/account/wallet/?c2cOrder=${legacy.id}`); dialog = page.getByRole("dialog", { name: "Order details" });
  await dialog.getByRole("button", { name: "Cancel unpaid order", exact: true }).click();
  await dialog.getByRole("checkbox").check(); await dialog.getByRole("button", { name: "Cancel unpaid order", exact: true }).click();
  await dialog.getByText("Cancelled", { exact: true }).waitFor();
  // A separate unpaid purchase can be cancelled; expiry hides instructions.
  orders.unshift(makeOrder({ ...latestQuote, id: "automatic-quote" }, "buyer")); const unpaid = orders[0];
  await page.goto(`${base}/account/wallet/?c2cOrder=${unpaid.id}`); dialog = page.getByRole("dialog", { name: "Order details" }); await dialog.locator("[data-c2c-description]").waitFor();
  await dialog.getByRole("button", { name: "Cancel unpaid order", exact: true }).click(); await dialog.getByRole("checkbox").check(); await dialog.getByRole("button", { name: "Cancel unpaid order", exact: true }).click(); await dialog.getByText("Cancelled", { exact: true }).waitFor();
  unpaid.status = "awaiting_payment"; unpaid.version++; unpaid.paymentDeadline = new Date(Date.now() - 30000).toISOString();
  await page.reload(); dialog = page.getByRole("dialog", { name: "Order details" }); await dialog.getByText(/Do not pay now/).waitFor(); assert.equal(await dialog.locator("[data-c2c-description]").count(), 0);
  unpaid.status = "disputed"; unpaid.paidAt = new Date().toISOString(); unpaid.version++;
  const owner = await open("owner", "en", true); await owner.page.goto(`${base}/settings/?panel=market_finance`);
  const adminPanel = owner.page.locator("[data-c2c-admin]"); await adminPanel.getByText("C2C settings", { exact: true }).waitFor();
  assert.equal(new URL(owner.page.url()).searchParams.has("panel"), false, "settings shortcut is consumed once");
  await owner.page.getByRole("button", { name: "Overview", exact: true }).click();
  await owner.page.waitForFunction(() => JSON.parse(localStorage.getItem("cc_switch_router_settings_navigation_v1"))?.activeSection === "overview");
  await owner.page.reload();
  await owner.page.getByRole("button", { name: "Overview", exact: true }).waitFor();
  assert.equal(await adminPanel.isVisible(), false, "refresh preserves a later section choice instead of replaying the shortcut");
  await owner.page.goto(`${base}/settings/?panel=market_finance`);
  await adminPanel.getByText("C2C settings", { exact: true }).waitFor();
  await adminPanel.getByLabel("Opening balance threshold (USD)").fill("150"); await adminPanel.getByRole("button", { name: "Save", exact: true }).click(); assert.equal(policy.admissionMinor, 15000);
  await adminPanel.locator("summary").filter({ hasText: "C2C disputes & late payments" }).click(); await adminPanel.getByRole("button", { name: new RegExp(unpaid.id) }).click();
  const adjudication = owner.page.getByRole("dialog", { name: "Evidence-based resolution" }); await adjudication.getByLabel("Evidence-based resolution").selectOption("return_to_seller");
  await adjudication.getByLabel("Explanation / evidence reference").fill("Both parties confirm no payment. External reference recorded."); await adjudication.getByRole("checkbox").check();
  for (const width of [320, 375, 414, 768]) { await owner.page.setViewportSize({ width, height: 900 }); await geometry(owner.page, "dialog[open]"); }
  await adjudication.getByRole("button", { name: "Save", exact: true }).click(); await adjudication.getByText("Cancelled", { exact: true }).waitFor(); assert.equal(resolution, 1);
  await adjudication.getByRole("button", { name: "Close", exact: true }).click();
  const handlerSection = adminPanel.locator("details").filter({ hasText: "Independent C2C dispute handlers" });
  await handlerSection.locator("summary").click();
  await handlerSection.getByLabel("Administrator email", { exact: true }).fill("handler@example.invalid");
  await handlerSection.getByRole("button", { name: "Authorize", exact: true }).click();
  await handlerSection.getByRole("button", { name: "Revoke", exact: true }).waitFor();
  const handler = await open("handler", "en", true); await handler.page.goto(`${base}/settings/?panel=market_finance&c2cOrder=${early.id}`);
  let handlerDialog = handler.page.getByRole("dialog", { name: "Evidence-based resolution" });
  await handlerDialog.getByText("In dispute", { exact: true }).waitFor();
  assert.equal(await handlerDialog.getByLabel("Evidence-based resolution").isDisabled(), false);
  assert.equal(await handler.page.locator("[data-c2c-admin]").getByLabel("Opening balance threshold (USD)").isDisabled(), true);
  await handlerSection.getByRole("button", { name: "Revoke", exact: true }).click();
  await handlerSection.getByText(/Revoked/).waitFor(); await handler.page.reload();
  handlerDialog = handler.page.getByRole("dialog", { name: "Evidence-based resolution" });
  await handlerDialog.getByText(/Only the Owner/).waitFor();
  assert.equal(await handlerDialog.getByLabel("Evidence-based resolution").isDisabled(), true); await handler.context.close();
  const ownerTrade = { ...makeOrder({ ...latestQuote, id: "owner-trade" }, "buyer"), sellerId: "owner", status: "disputed", paidAt: new Date().toISOString(), latePaymentReported: true };
  orders.unshift(ownerTrade); await owner.page.goto(`${base}/settings/?panel=market_finance&c2cOrder=${ownerTrade.id}`);
  const ownerConflict = owner.page.getByRole("dialog", { name: "Evidence-based resolution" });
  await ownerConflict.getByText("You are a party to this order and cannot adjudicate it.", { exact: true }).waitFor();
  assert.equal(await ownerConflict.getByLabel("Evidence-based resolution").isDisabled(), true);
  // Independent resolutions finish these mock exceptional trades before
  // exercising the separate empty-order storage recovery scenario below.
  for (const o of [early, ownerTrade]) { o.status = "cancelled"; o.latePaymentResolved = true; o.version++; }
  // Corrupt local recovery data must not silently enable another order.
  await page.evaluate(() => sessionStorage.setItem("router_c2c_order_v1:buyer", "invalid-json"));
  await page.goto(`${base}/account/wallet/`); await page.getByRole("button", { name: "C2C", exact: true }).click();
  await page.getByRole("button", { name: "Buy balance", exact: true }).click();
  const buyPanel = page.locator("[data-c2c-buy]"); await buyPanel.getByRole("button", { name: "Check existing orders", exact: true }).waitFor();
  assert.equal(await buyPanel.getByRole("button", { name: new RegExp(seller.displayName) }).count(), 0);
  await buyPanel.getByRole("button", { name: "Check existing orders", exact: true }).click();
  await buyPanel.getByRole("button", { name: new RegExp(seller.displayName) }).click(); await buyPanel.getByLabel("Wallet credit to buy (USD)").fill("10");
  quoteLifetimeMs = -1000; await buyPanel.getByRole("button", { name: "Review purchase", exact: true }).click(); dialog = page.getByRole("dialog", { name: "Review purchase" });
  await dialog.getByText(/This quote expired/).waitFor(); await dialog.getByRole("checkbox").check(); assert.equal(await dialog.getByRole("button", { name: "Create order", exact: true }).isDisabled(), true);
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
    await skewDialog.locator("[data-c2c-description]").waitFor(); assert.equal(await skewDialog.getByRole("button", { name: "I have paid", exact: true }).isDisabled(), false);
    skewOrder.paymentDeadline = new Date(Date.now() - 30000).toISOString(); await skew.page.reload();
    await skewDialog.getByText(/Do not pay now/).waitFor(); assert.equal(await skewDialog.locator("[data-c2c-description]").count(), 0);
    await skew.context.close();
  }
  const zh = await open("buyer", "zh-CN"); await zh.page.goto(`${base}/account/wallet/`); await zh.page.getByRole("button", { name: "C2C", exact: true }).click(); await zh.page.getByRole("button", { name: "购入余额", exact: true }).click();
  await zh.page.locator("[data-c2c-buy]").getByRole("button", { name: new RegExp(seller.displayName) }).waitFor();
  for (const width of [320, 375, 414, 768]) { await zh.page.setViewportSize({ width, height: 900 }); await geometry(zh.page, "[data-c2c-buy]"); }
  const zhSeller = await open("seller", "zh-CN"); await zhSeller.page.goto(`${base}/account/wallet/`);
  await zhSeller.page.locator("[data-c2c-orders]").getByRole("button", { name: "出售设置", exact: true }).click();
  const zhEditor = zhSeller.page.getByRole("dialog", { name: "出售设置", exact: true });
  await zhEditor.getByLabel("C2C 描述", { exact: true }).fill("支持支付宝和微信，Telegram：@seller，说明：https://seller.example.invalid");
  for (const expanded of [false, true]) {
    if (expanded) await zhEditor.locator("summary").click();
    for (const dark of [false, true]) {
      await zhSeller.page.evaluate(dark => document.documentElement.classList.toggle("dark", dark), dark);
      for (const width of [320, 375, 414, 768]) { await zhSeller.page.setViewportSize({ width, height: 900 }); await geometry(zhSeller.page, "dialog[open]"); }
    }
  }
  await zhEditor.getByLabel("C2C 描述", { exact: true }).fill(""); await zhEditor.getByRole("combobox").focus();
  assert.equal(await zhEditor.getByLabel("C2C 描述", { exact: true }).getAttribute("aria-invalid"), "true");
  await zhEditor.getByText("请填写不超过 300 字的单行描述。", { exact: true }).waitFor();
  await zhSeller.page.keyboard.press("Escape"); await zhEditor.waitFor({ state: "hidden" });
  await zhSeller.context.close();
  // Publication and transfer controls must stay available without official
  // recharge. No legacy payment profile is consulted in any mode.
  const editOffer = async () => {
    await sell.page.locator('button[aria-label="Host actions"]').click();
    await sell.page.getByRole("menuitem", { name: "Edit offer", exact: true }).click();
    return sell.page.getByRole("dialog", { name: "Edit Host offer", exact: true });
  };
  const waitOfferReady = () => sell.page.waitForFunction(() => [...document.querySelectorAll('[role="dialog"] button')].some(button => button.textContent.trim() === "Save" && !button.disabled));
  for (const mode of ["disabled", "shadow", "enabled"]) {
    topupMode = mode;
    await sell.page.goto(`${base}/account/wallet/`);
    await sell.page.getByRole("button", { name: "Transfer", exact: true }).click();
    await sell.page.locator("#wallet-transfer form").waitFor();
    await sell.page.goto(`${base}/client-market/`);
    const offerDialog = await editOffer(); await waitOfferReady();
    await offerDialog.getByRole("button", { name: "Save", exact: true }).click();
    await offerDialog.waitFor({ state: "hidden" });
  }
  assert.equal(offerWrites, 3);
  publicationReady = false; topupMode = "disabled";
  await sell.page.goto(`${base}/client-market/`);
  let offerDialog = await editOffer();
  await offerDialog.getByText("Claim an active Provider identity first.", { exact: true }).waitFor();
  assert.equal(await offerDialog.getByRole("button", { name: "Save", exact: true }).isDisabled(), true);
  assert.equal(await offerDialog.getByRole("link", { name: "Open Provider settings", exact: true }).getAttribute("href"), "/account/provider-settings/");
  await offerDialog.getByRole("button", { name: "Cancel", exact: true }).click();
  publicationReady = true; financeFailure = true;
  offerDialog = await editOffer(); await offerDialog.getByRole("alert").waitFor();
  financeFailure = false; await offerDialog.getByRole("button", { name: "Retry", exact: true }).click(); await waitOfferReady();
  await offerDialog.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal(retiredSetupRequests, 0); topupMode = "shadow";
  // Income is embedded in Provider settings, including empty/error states.
  assert.equal((await page.request.get(`${base}/account/earnings/`)).status(), 404, "no obsolete page or redirect export");
  financeFailure = true; await sell.page.goto(`${base}/account/provider-settings/#income`);
  const income = sell.page.locator("#income"); await income.getByRole("alert").waitFor();
  financeFailure = false; await income.getByRole("button", { name: "Retry", exact: true }).click();
  await income.getByText("$130.00", { exact: true }).waitFor();
  assert.equal(await sell.page.locator('nav a[href*="earnings"]').count(), 0);
  assert.equal(await income.getByRole("button", { name: /withdraw|convert|transfer/i }).count(), 0);
  assert.equal(await income.locator("details[open]").count(), 0, "earnings details folded initially");
  heldIncome = true; await sell.page.reload(); await income.locator("aside").waitFor();
  for (const dark of [false, true]) {
    await sell.page.evaluate(dark => document.documentElement.classList.toggle("dark", dark), dark);
    for (const width of [320, 375, 414, 768]) { await sell.page.setViewportSize({ width, height: 900 }); await geometry(sell.page, "#income"); }
  }
  heldIncome = false;
  await page.goto(`${base}/account/provider-settings/`); await page.locator("#income").getByText("No earnings yet.", { exact: true }).waitFor();
  assert.equal(await page.locator("#income dl").count(), 0, "empty income does not show a wall of zeros");
  const guest = await open("guest");
  await guest.context.route("**/v1/auth/session/me", route => route.fulfill({ contentType: "application/json", body: JSON.stringify({ authenticated: false, isAdmin: false, user: null }) }));
  let guestFinanceRequests = 0;
  await guest.context.route("**/v1/market-provider-finance/**", route => { guestFinanceRequests++; return route.fulfill({ status: 401, contentType: "application/json", body: "{}" }); });
  await guest.page.goto(`${base}/account/provider-settings/`);
  await guest.page.locator("#income").getByText(/sign in/i).waitFor();
  assert.equal(guestFinanceRequests, 0, "anonymous income has a sign-in state and never fetches private finances");
  assert.deepEqual(errors, []);
  console.log("C2C browser checks passed: automatic reservation/payment without manual acceptance, explicit seller upgrade consent preserving pauses, legacy withdrawal/rejection and early payment protection, persistent selling without heartbeat, independent dispute grants and party conflict, public descriptions, seller draft conflict, Provider income, recovery, login-authorized release without financial OTP, bilingual DOM geometry, contrast, keyboard and clock skew; no screenshots or real operations.");
} finally { await browser?.close(); await new Promise(resolveClose => server.close(resolveClose)); }
