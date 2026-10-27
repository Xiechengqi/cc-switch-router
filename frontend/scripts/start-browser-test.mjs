import assert from "node:assert/strict";
import { chromium } from "playwright";

// Exercises the start page against the mocked home API. No screenshots or payments.
const baseUrl = process.env.START_TEST_URL || "http://localhost:3456";
const browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox"] });

async function openPage(locale) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const puts = [];
  page.on("pageerror", (error) => { throw error; });
  await page.addInitScript((selected) => {
    localStorage.setItem("cc_switch_router_locale_v1", selected);
    localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({
      authDeviceId: "start-test", publicKey: "test", privateKey: "test",
      accessToken: "start-test", expiresAt: "2099-01-01T00:00:00Z",
    }));
  }, locale);
  let home = { hasActivity: false, entry: null, source: null };
  let failHome = false;
  const json = (route, body, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(/\/(?:v1|share-api)\//, async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    if (!path.startsWith("/v1/") && !path.startsWith("/share-api/")) return route.continue();
    if (path === "/v1/auth/home") {
      if (route.request().method() === "PUT") {
        if (failHome) return json(route, { message: "unavailable" }, 503);
        const body = route.request().postDataJSON();
        puts.push(body);
        home = { hasActivity: false, entry: body.entry, source: body.source };
        return json(route, home);
      }
      if (failHome) return json(route, { message: "unavailable" }, 503);
      return json(route, home);
    }
    if (path === "/v1/auth/session/me") return json(route, { authenticated: true, isAdmin: false, user: { id: "empty", email: "empty@example.invalid" } });
    if (path === "/v1/regions") return json(route, []);
    if (path === "/v1/announcement") return json(route, { enabled: false, revision: "1", contentEn: "", contentZhCn: "" });
    if (path.endsWith("inbox-summary")) return json(route, { pendingRequests: 0 });
    if (path === "/v1/market-wallet") return json(route, { assetCode: "USD", postedUnits: "0", heldUnits: "0", availableUnits: "0", version: 1, mode: "enabled", canTopup: false, hasWalletActivity: false });
    if (path === "/v1/market/config") return json(route, { usdCnyRateMicros: 7_000_000, fxRateFresh: true });
    if (path === "/v1/client-market/hosts" || path === "/v1/client-market/my-rentals") return json(route, []);
    if (path === "/v1/share-market/listings") return json(route, { listings: [], trialHours: 0 });
    if (path === "/v1/share-market/me/listings" || path === "/v1/share-market/me/rented-listings") return json(route, { listings: [] });
    if (path.startsWith("/v1/share-market/me/subscriptions")) return json(route, { subscriptions: [] });
    if (path === "/v1/share-market/owned-shares") return json(route, []);
    if (path === "/v1/market-providers") return json(route, { generation: {
      id: "start", algorithmVersion: "test", publishedAt: new Date().toISOString(), stale: false,
      weights: { serviceQuality: 50, effectiveChoice: 20, fulfillment: 20, supplyBreadth: 10 },
      minimumIndependentBuyers: 3, minimumObservationDays: 7,
    }, providers: [] });
    if (path === "/v1/market-wallet/channels") return json(route, []);
    if (path.endsWith("/topups") || path.endsWith("/transactions") || path.endsWith("/orders")) return json(route, { items: [], nextCursor: null });
    if (path === "/v1/dashboard") return json(route, { clients: [], shares: [], stats: { clients: 0, countries: 0, activeShares: 0, inFlightRequests: 0 }, map: { points: [] } });
    if (path.includes("/chat/rooms")) return json(route, { rooms: [], totalUnread: 0 });
    if (path === "/share-api/context") return json(route, { message: "not a share" }, 404);
    return json(route, {});
  });
  await page.route("**/share-api/**", (route) => json(route, { message: "not a share" }, 404));
  return { page, puts, setHome: (value) => { home = value; }, setFail: (value) => { failHome = value; } };
}

try {
  const zh = await openPage("zh-CN");
  await zh.page.goto(`${baseUrl}/`);
  await zh.page.getByRole("heading", { name: "先做哪一件？" }).waitFor();
  assert.equal(await zh.page.getByRole("navigation", { name: "控制台分区" }).count(), 0);
  const recommended = zh.page.getByRole("button", { name: /租一个客户端/ });
  const other = zh.page.getByRole("button", { name: "买一段共享额度" });
  const sizes = await Promise.all([recommended, other].map((item) => item.evaluate((node) => node.getBoundingClientRect().height)));
  assert.ok(sizes[0] > sizes[1], "recommended choice is larger");
  await recommended.click();
  await zh.page.waitForURL(/\/client-market\/$/);
  assert.deepEqual(zh.puts.at(-1), { entry: "client-market", source: "choice" });
  await zh.page.goto(`${baseUrl}/`);
  await zh.page.waitForURL(/\/client-market\/$/);
  await zh.page.goto(`${baseUrl}/clients/`);
  await zh.page.getByRole("heading", { name: "客户端", level: 1 }).waitFor();
  assert.equal(zh.page.url().includes("/clients/"), true);

  zh.setHome({ hasActivity: false, entry: null, source: null });
  await zh.page.goto(`${baseUrl}/`);
  await zh.page.getByRole("button", { name: "先跳过", exact: true }).click();
  await zh.page.waitForURL(/\/clients\/$/);
  assert.deepEqual(zh.puts.at(-1), { entry: "clients", source: "skip" });
  await zh.page.getByRole("link", { name: "去 Client 市场租一个" }).waitFor();
  await zh.page.getByRole("link", { name: "去平台余额充值" }).waitFor();

  zh.setHome({ hasActivity: true, entry: null, source: null });
  await zh.page.goto(`${baseUrl}/`);
  await zh.page.waitForURL(/\/clients\/$/);
  assert.equal(zh.puts.some((item) => item.source === "activity"), false);

  const beforeTask = zh.puts.length;
  await zh.page.goto(`${baseUrl}/?topup=1`);
  await zh.page.waitForURL(/\/account\/wallet\/\?topup=1$/);
  assert.equal(zh.puts.length, beforeTask);

  zh.setHome({ hasActivity: false, entry: null, source: null });
  zh.setFail(true);
  await zh.page.goto(`${baseUrl}/`);
  await zh.page.waitForURL(/\/clients\/$/);
  assert.equal(zh.puts.length, beforeTask);
  await zh.page.close();

  const en = await openPage("en");
  await en.page.setViewportSize({ width: 390, height: 844 });
  await en.page.goto(`${baseUrl}/`);
  await en.page.getByRole("heading", { name: "What do you want to do first?" }).waitFor();
  assert.equal(await en.page.getByRole("navigation", { name: "Dashboard sections" }).count(), 0);
  const overflow = await en.page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
  assert.equal(overflow, false);
  await en.page.getByRole("button", { name: "Skip for now", exact: true }).click();
  await en.page.waitForURL(/\/clients\/$/);
  await en.page.getByRole("link", { name: "CC-Switch Router" }).click();
  await en.page.waitForURL(/\/clients\/$/);

  en.setHome({ hasActivity: false, entry: "wallet", source: "choice" });
  await en.page.goto(`${baseUrl}/account/home/`);
  await en.page.locator("section").getByRole("heading", { name: "Home entry" }).waitFor();
  await en.page.getByRole("radio", { name: "Clients", exact: true }).check();
  await en.page.getByRole("button", { name: "Save home entry" }).click();
  await en.page.getByText("Saved.", { exact: true }).waitFor();
  assert.deepEqual(en.puts.at(-1), { entry: "clients", source: "choice" });
  await en.page.goto(`${baseUrl}/`);
  await en.page.waitForURL(/\/clients\/$/);
  assert.equal(await en.page.getByRole("heading", { name: "What do you want to do first?" }).count(), 0);
  console.log("start page checks passed");
} finally {
  await browser.close();
}
