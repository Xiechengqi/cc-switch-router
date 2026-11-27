import { installFinancialBrowserIdentity } from "./financial-browser-fixture.mjs";
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { emptyClientRentalsPage, isRetiredClientMarketPath } from "./client-rental-fixtures.mjs";
import { mkdir, readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

// Serve the production export by default. All business APIs are mocked; no
// SSH or payments occur. An explicit URL can still target a development server.
let baseUrl = process.env.CHECKOUT_TEST_URL;
let server;
if (!baseUrl) {
  const output = resolve(fileURLToPath(new URL("../out/", import.meta.url)));
  server = createServer(async (req, res) => {
    try {
      const pathname = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
      const path = resolve(output, `.${pathname.endsWith("/") ? `${pathname}index.html` : pathname}`);
      if (!path.startsWith(output + sep)) return res.writeHead(403).end();
      const body = await readFile(path);
      res.writeHead(200, { "Content-Type": ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".txt": "text/plain", ".svg": "image/svg+xml", ".woff2": "font/woff2" })[extname(path)] || "application/octet-stream", "Cache-Control": "no-store" });
      res.end(body);
    } catch { res.writeHead(404).end(); }
  });
  await new Promise((ready, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", ready); });
  baseUrl = `http://127.0.0.1:${server.address().port}`;
}
const artifacts =
  process.env.CHECKOUT_ARTIFACTS || "/tmp/client-checkout-review";
const captureScreenshots = process.env.CHECKOUT_SCREENSHOTS === "1";
if (captureScreenshots) await mkdir(artifacts, { recursive: true });
(async () => {
  const browser = await chromium.launch({
    executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome",
    headless: true,
    args: ["--no-sandbox"],
  });
  try {
    const page = await browser.newPage({
      viewport: { width: 768, height: 900 },
    });
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    page.on("console", (message) => {
      // HTTP failures are deliberately exercised below; application errors are not.
      if (message.type() === "error" && !message.text().startsWith("Failed to load resource:")) errors.push(message.text());
    });
    let available = "0";
    let quoteRequests = 0;
    let ttl = 120;
    let authenticated = true;
    let delistRequests = 0;
    let rejectNextQuote = false;
    let loseNextReply = false;
    const commits = [];
    const leases = [];
    const receipts = new Map();
    const listing = {
      installationId: "online-client", providerId: "provider", label: "Test online Client",
      revision: 7, state: "listed", owned: false, cyclePriceMinor: 1000, freeDays: null,
      trialDays: 1, country: "JP", egressIp: "192.0.2.xx", egressDiffers: true,
      hardware: { arch: "aarch64", cpu: 2, memoryBytes: 2 ** 32, diskBytes: 32 * 2 ** 30 },
      ownerRental: true, exclusiveManagement: true, onlineRate24h: 98.75, observedMinutes24h: 80, observationCoverage24h: 5.56,
      healthy: true, occupied: false, observedAt: Math.floor(Date.now()/1000), availability: "available",
    };
    let retiredMoneyRequests = 0;
    await page.addInitScript(() => {
      localStorage.setItem(
        "cc_switch_router_locale_v1",
        new URL(location.href).searchParams.get("testLocale") || "zh-CN",
      );
      if (!localStorage.getItem("cc_switch_router_client_market_shop_browse_v1"))
        localStorage.setItem("cc_switch_router_client_market_shop_browse_v1", JSON.stringify({ scope: "mine", country: "JP", rentableOnly: true }));
      localStorage.setItem(
        "cc_switch_router_auth_v2",
        JSON.stringify({
          authDeviceId: "test",
          publicKey: "test",
          privateKey: "test",
          accessToken: "test",
          expiresAt: "2099-01-01T00:00:00Z",
        }),
      );
    });
    await page.route("**/v1/**", async (route) => {
      const req = route.request(),
        path = new URL(req.url()).pathname;
      if (isRetiredClientMarketPath(path)) {
        errors.push(path);
        return route.fulfill({ status: 410 });
      }
      let body = {};
      if (path === "/v1/ui/context") body = { mode: "router" };
      else if (path === "/v1/auth/session/me")
        body = {
          authenticated,
          isAdmin: false,
          user: authenticated ? { id: "buyer", email: "buyer@example.com" } : null,
        };
      else if (path.startsWith("/v1/market-provider-finance/wallet-conversion") || path.includes("/withdrawals")) {
        retiredMoneyRequests += 1;
        return route.fulfill({ status: 410, contentType: "application/json", body: JSON.stringify({ code: "RETIRED" }) });
      }
      else if (path === "/v1/client-market/termination-workspace") body = { hosts: [], actionRequiredCount: 0, canAdjudicate: false, serverNow: new Date().toISOString() };
      else if (req.method() === "DELETE" && path.endsWith("/listing")) {
        delistRequests += 1;
        body = { state: "unlocking" };
      } else if (path.startsWith("/v1/client-market/me/rentals")) {
        const list = emptyClientRentalsPage(req.url());
        const now = list.serverNow;
        const records = leases.map((lease) => ({
          group: "current", id: "lease:" + lease.leaseId, installationId: lease.leaseId, clientSubdomain: lease.alias,
          providerEmail: "seller@example.com", countryCode: "JP", provenance: "order_snapshot", createdAt: now, activatedAt: now,
          status: lease.cyclePriceMinor ? "paid" : "free", pricingModel: lease.cyclePriceMinor ? "prepaid_calendar_month" : "free",
          cyclePriceMinor: lease.cyclePriceMinor || null, endedAt: null, cleanupConfirmedAt: null, contractId: null,
          cleanupStatus: "not_required", attention: "none", terminationId: null, dailyRateMinor: null, offerRevision: 7,
          trialSecondsRemaining: null, renewalPolicy: null, periodEnd: null, expiresAt: null, freeDurationDays: null,
          contractStatus: null, money: null,
        }));
        if (path.endsWith("/rentals")) {
          const focus = new URL(req.url()).searchParams.get("focus");
          body = list.view === "current" ? { ...list, items: records, total: records.length,
            focusedId: records.find((r) => r.installationId === focus)?.id ?? null } : list;
        } else {
          const record = records.find((r) => r.id === decodeURIComponent(path.split("/").pop()));
          body = { record, rental: { leaseId: record.installationId, contractId: null, canDispute: false, openDispute: false },
            periods: [], periodsNextCursor: null, events: [], eventsNextCursor: null, trialConsumedSeconds: 0, serverNow: now, settlement: null };
        }
      }
      else if (path.endsWith("/usage-history")) body = [];
      else if (path === "/v1/notifications") body = { items: [], nextCursor: null, unread: 0, readThrough: null };
      else if (path === "/v1/client-market/online/listings") body = {listings:[listing]};
      else if (path === "/v1/client-market/online/leases") body = {leases};
      else if (path === "/v1/client-market/online/installations/online-client/quote") {
        const input = req.postDataJSON();
        assert.equal(input.revision,listing.revision);
        quoteRequests++;
        const required = BigInt(listing.cyclePriceMinor) * 86400n;
        body = {
          id: "quote-" + quoteRequests,
          expiresAt: Math.floor(Date.now() / 1000) + ttl,
          terms: {
            selfRental: listing.owned, listingPriceMinor: listing.cyclePriceMinor,
            cyclePriceMinor: listing.owned ? 0 : listing.cyclePriceMinor,
            freeDays: listing.owned ? null : listing.freeDays,
            trialSeconds: !listing.owned && listing.trialDays ? 498 : 0,
            autoRenew: !listing.owned && input.autoRenew,
            terminationPolicy: { version: 1, rateBps: 1000 },
          },
          walletFunding: !listing.owned && listing.cyclePriceMinor
            ? {
                assetCode: "USD",
                availableUnits: available,
                heldUnits: "0",
                committedUnits: "0",
                requiredUnits: required.toString(),
                shortfallUnits: (required > BigInt(available) ? required - BigInt(available) : 0n).toString(),
              }
            : null,
        };
      } else if (path.startsWith("/v1/client-market/online/quotes/") && path.endsWith("/commit")) {
        commits.push({path,input:req.postDataJSON()});
        if (rejectNextQuote) {
          rejectNextQuote=false;
          return route.fulfill({status:409,json:{message:"Client trial allowance changed; request a new quote"}});
        }
        if (!receipts.has(path)) {
          const id="lease-" + leases.length;
          leases.push({
            leaseId: id,
            installationId: listing.installationId,
            alias: "rent-" + id,
            state: "active",
            expiresAt: Math.floor(Date.now() / 1000) + 86400,
            clientUrl: "https://rental.example.test",
            activatedAt: Math.floor(Date.now() / 1000),
            createdAt: Math.floor(Date.now() / 1000),
            cyclePriceMinor: listing.owned ? 0 : listing.cyclePriceMinor,
            events: [],
            terminationId: null,
          });
          receipts.set(path,{leaseId:id,state:"starting"});
        }
        if (loseNextReply) {
          loseNextReply=false;
          return route.fulfill({status:503,json:{message:"Retry this quote"}});
        }
        body=receipts.get(path);
      } else if (path === "/v1/market-wallet")
        body = {
          mode: "enabled",
          assetCode: "USD",
          postedUnits: available,
          availableUnits: available,
          heldUnits: "0",
          version: 1,
          canTopup: true,
        };
      else if (path === "/v1/market-wallet/topups") body = { items: [], nextCursor: null };
      else if (path === "/v1/market-wallet/channels" || path === "/v1/regions") body = [];
      else if (path.includes("inbox-summary")) body = { pendingRequests: 0 };
      else if (path.includes("rooms")) body = { rooms: [], totalUnread: 0 };
      else if (path === "/v1/announcement")
        body = { enabled: false, revision: "1", contentEn: "", contentZhCn: "" };
      else if (path.includes("config")) body = {};
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    });
    await installFinancialBrowserIdentity(page);
    await page.goto(baseUrl + "/client-market/?testLocale=en&country=JP");
    const supply = page.locator("#market-client-online-client");
    await supply.getByText(/2 cores · 4 GiB · aarch64/).waitFor();
    await supply.getByRole("img", { name: "Japan" }).waitFor();
    await supply.getByText("98.8%").waitFor();
    // The egress network is the card's lead fact; the full IP is shown only to the tenant.
    await supply.getByText("192.0.2.xx", { exact: true }).waitFor();
    assert.equal(await supply.getByText("Network details").count(), 0);
    assert.equal(await page.getByRole("option", { name: "Mine", exact: true }).count(), 0);
    assert.equal(await supply.getByText(/root access|24 h$|running time/i).count(), 0);
    for (const width of [320, 375, 768, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      if (captureScreenshots) await page.screenshot({ path: artifacts + "/online-supply-" + width + ".png" });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    }
    await supply.getByRole("button", { name: "Rent", exact: true }).click();
    const checkout = page.getByRole("dialog");
    // A trial defaults to trial only: no balance, no renewal switch.
    await checkout.getByText(/8 min 18 sec/).first().waitFor();
    assert.equal(await checkout.getByRole("checkbox", { name: /renew/i }).count(), 0);
    await checkout.getByRole("button", { name: "Start trial" }).waitFor();
    await checkout.getByRole("radio", { name: /Keep after trial/ }).check();
    await checkout.getByText(/short\.$/).waitFor();
    assert.equal(await checkout.getByText(/Add .* to continue/).count(), 0);
    if (captureScreenshots) await checkout.screenshot({ path: artifacts + "/paid-checkout.png" });
    assert.equal(await checkout.getByRole("link", { name: "Platform wallet" }).count(), 0);
    const renewal = checkout.getByRole("checkbox", { name: /renew/i });
    await Promise.all([
      page.waitForResponse(
        (r) => r.url().endsWith("/quote") && r.request().postDataJSON().autoRenew === true,
      ),
      renewal.check(),
    ]);
    assert.equal(commits.length, 0);
    await checkout.getByRole("button", { name: "Cover shortfall" }).click();
    await checkout.getByText(/Add .* to continue/).waitFor();
    available = "86400000";
    await page.evaluate(() =>
      window.dispatchEvent(new CustomEvent("router-market-balance-changed", { detail: "buyer" })),
    );
    const confirm = checkout.getByRole("button", { name: "Confirm rental" });
    const ack = checkout.getByRole("checkbox", { name: /seller can read/ });
    await ack.check();
    rejectNextQuote = true;
    await confirm.click();
    await checkout.getByText(/quote has been updated/i).waitFor();
    assert.equal(await ack.isChecked(), false);
    assert.equal(leases.length, 0);
    await ack.check();
    loseNextReply = true;
    await confirm.click();
    await checkout.getByText(/order receipt has not arrived/i).waitFor();
    await page.clock.setFixedTime(new Date(Date.now() + 180000));
    await page.reload();
    await page
      .getByRole("status")
      .filter({ hasText: /order receipt has not arrived/i })
      .getByRole("button", { name: /Retry/i })
      .click();
    try {
      // The buyer stays on the market page; the new rental is highlighted under "My Clients".
      await page.locator("#my-client-lease-0").waitFor();
    } catch (cause) {
      console.error({
        url: page.url(),
        commits,
        errors,
        text: (await page.locator("main").allTextContents()).join(" ").slice(-1600),
      });
      throw cause;
    }
    assert.equal(leases.length, 1);
    assert.equal(commits.at(-1).path, commits.at(-2).path);
    assert.notEqual(commits.at(-1).path, commits[0].path);
    assert.equal(commits.at(-1).input.afterTrial, "continue");
    assert.equal(new URL(page.url()).pathname, "/client-market/");
    await page.clock.setFixedTime(new Date());
    listing.cyclePriceMinor = 0;
    listing.trialDays = 0;
    listing.freeDays = 7;
    ttl = 2;
    await page.goto(baseUrl + "/client-market/?testLocale=zh-CN&country=JP");
    await supply.getByRole("button", { name: "租用", exact: true }).click();
    assert.equal(await checkout.getByRole("checkbox").count(), 1);
    ttl = 120;
    await checkout.getByText(/报价已更新/).waitFor();
    await checkout.getByRole("checkbox", { name: /读取这台机器上的数据/ }).check();
    await checkout.getByRole("button", { name: "确认免费租用" }).click();
    await checkout.waitFor({ state: "hidden" });
    await page.locator("#my-client-lease-1").waitFor();
    listing.cyclePriceMinor = 4200;
    listing.freeDays = null;
    listing.trialDays = 3;
    listing.owned = true;
    listing.onlineRate24h = null;
    listing.observedMinutes24h = 0;
    await page.goto(baseUrl + "/client-market/?testLocale=zh-CN");
    await supply.getByText("自租免费").waitFor();
    await supply.getByText("暂无观测").waitFor();
    await page.setViewportSize({ width: 375, height: 720 });
    await supply.getByRole("button", { name: "租用", exact: true }).click();
    await checkout.getByText(/由你的工作区独占/).waitFor();
    assert.equal(await checkout.getByRole("checkbox").count(), 0);
    assert.equal(await checkout.getByText(/42\.00|钱包/).count(), 0);
    if (captureScreenshots) await checkout.screenshot({ path: artifacts + "/self-checkout-mobile.png" });
    await checkout.getByRole("button", { name: "确认免费租用" }).click();
    await page.locator("#my-client-lease-2").waitFor();
    assert.equal(leases.at(-1).cyclePriceMinor, 0);
    assert.equal(delistRequests, 0);
    listing.ownerRental = false;
    await page.goto(baseUrl + "/client-market/?testLocale=zh-CN");
    await page.getByText("当前没有可租的 Client。").waitFor();
    await page.getByRole("button", { name: "仅可租" }).click();
    await supply.getByText("Client 升级后可租用").waitFor();
    assert.equal(await supply.getByRole("button", { name: "租用", exact: true }).count(), 0);
    await page.goto(baseUrl + "/client-market/?view=rentals&focus=lease-2");
    await page.waitForURL(/account\/rentals\/.*focus=lease-2/);
    listing.owned = false;
    authenticated = false;
    await page.goto(baseUrl + "/client-market/?testLocale=zh-CN&country=JP");
    await supply.getByRole("button", { name: "登录后租用" }).waitFor();
    assert.equal(retiredMoneyRequests, 0);
    console.log(
      "PASS checkout in both languages, renewal terms, in-place funding, expiry, retry and receipt focus",
    );
    assert.deepEqual(errors, []);
    console.log("free checkout passed; no browser exceptions");
    console.log(captureScreenshots ? `Screenshots: ${artifacts}` : "DOM and interaction checks only; no screenshots");
  } finally {
    await browser.close();
    if (server) await new Promise((done) => server.close(done));
  }
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
