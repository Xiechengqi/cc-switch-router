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
    let rejectNextQuote = false;
    let loseNextReply = false;
    const commits = [];
    const leases = [];
    const receipts = new Map();
    const listing = {
      installationId: "online-client", providerId: "provider", label: "Test online Client",
      revision: 7, state: "listed", owned: false, cyclePriceMinor: 1000, freeDays: null,
      trialDays: 1, country: "JP", egressIp: "192.0.2.10", egressDiffers: true,
      hardware: { arch: "aarch64", cpu: 2, memoryBytes: 2 ** 32, diskBytes: 32 * 2 ** 30 },
      healthy: true, occupied: false, observedAt: Math.floor(Date.now()/1000), availableHours24: 22,
    };
    let retiredMoneyRequests = 0;
    await page.addInitScript(() => {
      localStorage.setItem(
        "cc_switch_router_locale_v1",
        new URL(location.href).searchParams.get("testLocale") || "zh-CN",
      );
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
          authenticated: true,
          isAdmin: false,
          user: { id: "buyer", email: "buyer@example.com" },
        };
      else if (path.startsWith("/v1/market-provider-finance/wallet-conversion") || path.includes("/withdrawals")) {
        retiredMoneyRequests += 1;
        return route.fulfill({ status: 410, contentType: "application/json", body: JSON.stringify({ code: "RETIRED" }) });
      }
      else if (path === "/v1/client-market/termination-workspace") body = { hosts: [], actionRequiredCount: 0, canAdjudicate: false, serverNow: new Date().toISOString() };
      else if (path === "/v1/client-market/me/rentals") body = emptyClientRentalsPage(req.url());
      else if (path.endsWith("/usage-history")) body = [];
      else if (path === "/v1/notifications") body = { items: [], nextCursor: null, unread: 0, readThrough: null };
      else if (path === "/v1/client-market/online/listings") body = {listings:[listing]};
      else if (path === "/v1/client-market/online/leases") body = {leases};
      else if (path === "/v1/client-market/online/installations/online-client/quote") {
        const input = req.postDataJSON();
        assert.equal(input.revision,listing.revision);
        quoteRequests++;
        const required = BigInt(listing.cyclePriceMinor) * 86400n;
        body = {id: "quote-" + quoteRequests, expiresAt: Math.floor(Date.now()/1000)+120,
          terms:{cyclePriceMinor:listing.cyclePriceMinor, freeDays:listing.freeDays, trialSeconds:listing.trialDays ? 498 : 0, autoRenew:input.autoRenew, terminationPolicy:{version:1,rateBps:1000}},
          walletFunding:listing.cyclePriceMinor ? {assetCode:"USD",availableUnits:available,heldUnits:"0",committedUnits:"0",requiredUnits:required.toString(),
            shortfallUnits:(required>BigInt(available)?required-BigInt(available):0n).toString()} : null};
      } else if (path.startsWith("/v1/client-market/online/quotes/") && path.endsWith("/commit")) {
        commits.push({path,input:req.postDataJSON()});
        if (rejectNextQuote) {
          rejectNextQuote=false;
          return route.fulfill({status:409,json:{message:"Client trial allowance changed; request a new quote"}});
        }
        if (!receipts.has(path)) {
          const id="lease-" + leases.length;
          leases.push({leaseId:id, installationId:listing.installationId,alias:"rent-" + id,state:"active",expiresAt:Math.floor(Date.now()/1000)+86400,
            clientUrl:"https://rental.example.test",events:[],terminationId:null});
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
      else if (path === "/v1/regions") body = [];
      else if (path.includes("inbox-summary")) body = { pendingRequests: 0 };
      else if (path.includes("rooms")) body = { rooms: [], totalUnread: 0 };
      else if (path === "/v1/announcement") body = { enabled: false, revision: "1", contentEn: "", contentZhCn: "" };
      else if (path.includes("config")) body = {};
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    });
    await installFinancialBrowserIdentity(page);
    await page.goto(baseUrl + "/client-market/?testLocale=en");
    const supply = page.getByRole("article").filter({has:page.getByRole("heading",{name:listing.label})});
    await supply.getByText("192.0.2.10").waitFor();
    await supply.getByText(/2 vCPU/).waitFor();
    for (const width of [320,375,768,1440]) {
      await page.setViewportSize({width,height:900});
      if (captureScreenshots) await page.screenshot({ path: `${artifacts}/online-supply-${width}.png` });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth>innerWidth),false,`listing overflow at ${width}`);
    }
    await supply.getByRole("button",{name:"Rent Client"}).click();
    const checkout = page.getByRole("dialog");
    const getQuote = checkout.getByRole("button",{name:"Get quote"});
    assert.equal(await getQuote.isEnabled(),false);
    await checkout.getByRole("checkbox",{name:/root access/}).check();
    await getQuote.click();
    await checkout.getByText(/compensation is 10%/).waitFor();
    await checkout.getByText(/8 min 18 sec/).waitFor();
    await checkout.getByText(/Add .* to continue/).waitFor();
    assert.equal(await checkout.getByRole("link",{name:"Platform wallet"}).getAttribute("href"),"/account/wallet/");
    assert.equal(commits.length,0);
    available="86400000";
    await checkout.getByRole("button",{name:/Reload|Refresh/i}).click();
    const confirm=checkout.getByRole("button",{name:"Confirm rental"});
    rejectNextQuote=true;
    await confirm.click();
    await checkout.getByText("Client trial allowance changed; request a new quote").waitFor();
    assert.equal(leases.length, 0);
    await getQuote.click();
    loseNextReply=true;
    await confirm.click();
    await checkout.getByText("Retry this quote").waitFor();
    await page.clock.setFixedTime(new Date(Date.now() + 180_000));
    await confirm.click();
    await checkout.waitFor({state:"hidden"});
    await page.getByText("rent-lease-0",{exact:true}).waitFor();
    assert.equal(quoteRequests,3);
    assert.equal(leases.length,1);
    assert.deepEqual(commits,["quote-2","quote-3","quote-3"].map(id => ({
      path:`/v1/client-market/online/quotes/${id}/commit`,
      input:{acknowledged:true},
    })));
    await page.clock.setFixedTime(new Date());
    for (const id of [listing.installationId, "lease-0"]) {
      await page.goto(baseUrl + "/client-market/?focus=" + encodeURIComponent(id) + "&testLocale=en");
      await page.waitForFunction(target => document.activeElement?.id === "market-client-" + target, id);
    }
    listing.cyclePriceMinor=0; listing.trialDays=0; listing.freeDays=7;
    await page.goto(baseUrl + "/client-market/?testLocale=zh-CN");
    await supply.getByRole("button",{name:"租用 Client"}).click();
    await checkout.getByRole("checkbox",{name:/root/}).check();
    await checkout.getByRole("button",{name:"获取报价"}).click();
    assert.equal(await checkout.getByRole("checkbox").count(),1);
    await checkout.getByRole("button",{name:"确认租用"}).click();
    await checkout.waitFor({state:"hidden"});
    assert.equal(retiredMoneyRequests,0);
    console.log("Online Client health, bilingual checkout, funding, requoting after a conflict and replay after expiry passed");

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
