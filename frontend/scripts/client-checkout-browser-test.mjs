import { chromium } from "playwright";
import assert from "node:assert/strict";
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
    let trial = 43200,
      trialAllowance = 43200,
      available = "0",
      shortfall = "86400000",
      quotes = 0,
      commits = [],
      commitMode = "success",
      expiry = 120000,
      cancels = 0;
    let retiredMoneyRequests = 0;
    let rentals = [];
    const host = {
      id: "host-1",
      providerId: "provider",
      hostOwnerEmail: "provider@example.com",
      ip: "192.0.2.1",
      hostname: "Test Host",
      port: 22,
      countryCode: "JP",
      status: "idle",
      pricingModel: "prepaid_calendar_month",
      cyclePriceMinor: 1000,
      currency: "USD",
      offerRevision: 1,
      contacts: [],
      eligibility: { allowed: true },
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    };
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
      let body = {};
      if (path === "/v1/auth/home") body = { hasActivity: true, entry: null, source: null };
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
      else if (path === "/v1/client-market/hosts") body = [host];
      else if (path === "/v1/client-market/my-rentals") body = rentals;
      else if (path.endsWith("/usage-history")) body = [];
      else if (path === "/v1/notifications") body = { items: [], nextCursor: null, unread: 0, readThrough: null };
      else if (path === "/v1/client-market/quotes") {
        quotes++;
        body = {
          id: "quote-" + quotes,
          status: "active",
          expiresAt: new Date(Date.now() + expiry).toISOString(),
          fundingRail: "platform_wallet",
          funding: [],
          recurringFunding: [],
          items: [
            {
              ...host,
              id: "item-" + quotes,
              hostId: host.id,
              trialSecondsRemaining: trial,
              trialAllowanceSeconds: trialAllowance,
              trialSecondsConsumed: Math.max(0, trialAllowance - trial),
            },
          ],
          walletFunding:
            host.cyclePriceMinor == null
              ? undefined
              : {
                  assetCode: "USD",
                  availableUnits: available,
                  heldUnits: "0",
                  committedUnits: "0",
                  requiredUnits: (BigInt(host.cyclePriceMinor) * 86_400n).toString(),
                  shortfallUnits: shortfall,
                },
        };
      } else if (path.includes("subdomain-availability"))
        body = { available: true };
      else if (path.endsWith("/commit")) {
        commits.push(req.postDataJSON());
        if (commitMode === "unknown") {
          commitMode = "success";
          return route.fulfill({
            status: 503,
            contentType: "application/json",
            body: JSON.stringify({ message: "result unknown" }),
          });
        }
        if (commitMode === "conflict") {
          commitMode = "success";
          shortfall = "86400000";
          available = "0";
          return route.fulfill({
            status: 409,
            contentType: "application/json",
            body: JSON.stringify({
              code: "MARKET_WALLET_REQUIRED",
              message: "balance changed",
            }),
          });
        }
        body = { batchId: "batch", jobIds: ["job"] };
      } else if (path.includes("/batches/"))
        body = {
          id: "batch",
          status: "succeeded",
          jobs: [
            {
              id: "job",
              status: "succeeded",
              phase: "complete",
              subdomain: "example-client",
              clientUrl: "https://example.test",
              log: "installed",
              createdAt: "",
              updatedAt: "",
            },
          ],
        };
      else if (path.endsWith("/cancel")) {
        cancels++;
        body = { ok: true };
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
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page
      .getByRole("heading", { name: "配置并确认" })
      .waitFor({ timeout: 60000 });
    console.log("opened", quotes);
    if (captureScreenshots) await page.screenshot({ path: artifacts + "/initial.png", fullPage: true });
    assert.equal(await page.locator('img[alt="Binance Pay"]').count(), 0);
    const trialSummary = page.locator("[data-client-trial]");
    const trialRules = trialSummary.locator("details");
    await trialSummary.getByText("免费试用", { exact: true }).waitFor();
    assert.equal(await trialSummary.locator(":scope > dl dd").textContent(), "12小时");
    await trialSummary.getByText(/试用免费，结束后从平台余额支付 .*10\.00/).waitFor();
    await trialSummary.getByText("须备足首期余额；扣款失败即回收 Client 并永久清理数据。", { exact: true }).waitFor();
    const sharedTrialRule = trialRules.getByText(/同账号在此 Host 共用一份试用额度/);
    assert.equal(await trialRules.getAttribute("open"), null);
    assert.equal(await sharedTrialRule.isVisible(), false, "detailed rules are collapsed by default");
    const expandTrialRules = trialRules.locator("summary");
    await expandTrialRules.focus();
    await page.keyboard.press("Enter");
    await sharedTrialRule.waitFor();
    assert.equal(await trialRules.locator("dd").first().textContent(), "12小时");
    assert.equal(await trialRules.locator("dd").last().textContent(), "0秒");
    await trialRules.getByText("仅计算实际可用的服务时长。", { exact: true }).waitFor();
    await page.keyboard.press("Enter");
    await sharedTrialRule.waitFor({ state: "hidden" });
    await page.getByRole("button", { name: "充值后继续", exact: true }).waitFor();
    assert.equal(await page.getByRole("button", { name: "开始试用并创建", exact: true }).count(), 0);
    assert.equal(commits.length, 0, "an unfunded trial cannot submit a rental");
    available = "86400000";
    shortfall = "0";
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await page
      .getByLabel("Web 登录密码", { exact: true })
      .fill("test-password-123");
    for (const width of [320, 375, 414, 768]) {
      await page.setViewportSize({ width, height: 900 });
      if (captureScreenshots) await page.screenshot({
        path: `${artifacts}/checkout-${width}.png`,
        fullPage: true,
      });
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > window.innerWidth,
      );
      assert.equal(overflow, false, `overflow at ${width}`);
      const button = page.getByRole("button", {
        name: "开始试用并创建",
        exact: true,
      });
      assert.equal(await button.isVisible(), true);
    }
    await page
      .getByRole("button", { name: "开始试用并创建", exact: true })
      .click();
    await page
      .getByRole("link", { name: "打开 Client" })
      .waitFor({ timeout: 10000 });
    assert.equal(commits.length, 1);
    assert.equal(commits[0].items[0].autoRenew, false);
    console.log("trial creation and responsive widths passed");
    await page
      .getByRole("button", { name: "关闭", exact: true })
      .last()
      .click();
    trial = 4500;
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    assert.equal(await trialRules.locator("dd").last().textContent(), "10小时 45分钟");
    assert.equal(await trialSummary.locator(":scope > dl dd").textContent(), "1小时 15分钟");
    assert.equal(await trialRules.getAttribute("open"), null);
    // The reported example must show the actual remaining time and quoted price,
    // rather than repeating the full allowance and the already-used time.
    trialAllowance = 3600;
    trial = 498;
    host.cyclePriceMinor = 5;
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    assert.equal(await trialSummary.locator(":scope > dl dd").textContent(), "8分钟 18秒");
    assert.equal(await trialRules.locator("dd").last().textContent(), "51分钟 42秒");
    await trialSummary.getByText(/试用免费，结束后从平台余额支付 .*0\.05/).waitFor();
    if (captureScreenshots) await page.screenshot({ path: artifacts + "/concise-trial.png", fullPage: true });
    trialAllowance = 43200;
    host.cyclePriceMinor = 1000;
    trial = 1;
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    assert.equal(await trialSummary.locator(":scope > dl dd").textContent(), "1秒");
    trial = 0;
    available = "0";
    shortfall = "86400000";
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await page
      .getByRole("button", { name: "充值后继续", exact: true })
      .waitFor();
    await trialSummary.getByText(/试用额度已用尽，开通后支付 .*10\.00/).waitFor();
    assert.equal(await trialSummary.locator(":scope > dl").count(), 0, "an exhausted trial does not show a redundant zero timer");
    assert.equal(await trialRules.getAttribute("open"), null);
    await page.getByLabel("访问子域名", { exact: true }).fill("saved-domain");
    await page.getByLabel("后续按月从平台余额自动续费").check();
    await page.getByRole("button", { name: "充值后继续", exact: true }).click();
    await page.waitForURL("**/account/wallet/**");
    const url = new URL(page.url());
    const returnTo = url.searchParams.get("returnTo");
    assert(returnTo.includes("saved-domain"));
    assert(returnTo.includes("draftAutoRenew=1"));
    assert(!page.url().includes("test-password"));
    available = "86400000";
    shortfall = "0";
    await page.goto(baseUrl + returnTo);
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    assert.equal(
      await page.getByLabel("访问子域名", { exact: true }).inputValue(),
      "saved-domain",
    );
    assert.equal(
      await page.getByLabel("Web 登录密码", { exact: true }).inputValue(),
      "",
    );
    assert(await page.getByLabel("后续按月从平台余额自动续费").isChecked());
    console.log("topup return and non-sensitive draft restoration passed");
    // A rejected wallet transaction must preserve input and explicitly requote.
    await page
      .getByLabel("Web 登录密码", { exact: true })
      .fill("wallet-conflict-password");
    commitMode = "conflict";
    await page
      .getByRole("button", { name: "确认租用并创建", exact: true })
      .click();
    await page
      .getByRole("button", { name: "重新确认资源", exact: true })
      .waitFor();
    assert.equal(
      await page.getByLabel("Web 登录密码", { exact: true }).inputValue(),
      "wallet-conflict-password",
    );
    await page
      .getByRole("button", { name: "重新确认资源", exact: true })
      .click();
    await page
      .getByRole("button", { name: "充值后继续", exact: true })
      .waitFor();
    console.log("wallet conflict preserves draft and updates funding passed");

    // Income is already part of ordinary wallet funds after the worker runs.
    // Checkout must never ask for a manual earning-credit confirmation.
    available = "0"; shortfall = "86400000";
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await page.getByRole("button", { name: "充值后继续", exact: true }).waitFor();
    assert.equal(await page.getByTestId("earning-conversion").count(), 0);
    const commitsBeforeCredit = commits.length;
    available = "86400000"; shortfall = "0";
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await page.getByRole("button", { name: "确认租用并创建", exact: true }).waitFor();
    assert.equal(commits.length, commitsBeforeCredit);
    assert.equal(retiredMoneyRequests, 0);
    console.log("automatically credited income uses the ordinary purchase balance without manual conversion or a surprise purchase");

    // A lost response must replay the identical request even after quote expiry.
    trial = 43200;
    shortfall = "0";
    expiry = 4000;
    commitMode = "unknown";
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await page
      .getByLabel("Web 登录密码", { exact: true })
      .fill("retry-password");
    await page
      .getByRole("button", { name: "开始试用并创建", exact: true })
      .click();
    await page.getByRole("button", { name: "重试", exact: true }).waitFor();
    assert(await page.getByLabel("Web 登录密码", { exact: true }).isDisabled());
    await page.waitForTimeout(4500);
    await page.getByRole("button", { name: "重试", exact: true }).click();
    await page.getByRole("link", { name: "打开 Client" }).waitFor();
    assert.deepEqual(commits.at(-1), commits.at(-2));
    console.log(
      "unknown submission retries identical payload after expiry passed",
    );

    // Normal expiry keeps the form and password, but requires a fresh quote.
    expiry = 2500;
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await page
      .getByLabel("Web 登录密码", { exact: true })
      .fill("expiry-password");
    await page
      .getByRole("button", { name: "重新确认资源", exact: true })
      .waitFor({ timeout: 10000 });
    assert.equal(
      await page.getByLabel("Web 登录密码", { exact: true }).inputValue(),
      "expiry-password",
    );
    expiry = 120000;
    await page
      .getByRole("button", { name: "重新确认资源", exact: true })
      .click();
    await page
      .getByRole("button", { name: "开始试用并创建", exact: true })
      .waitFor();
    assert.equal(
      await page.getByLabel("Web 登录密码", { exact: true }).inputValue(),
      "expiry-password",
    );
    console.log("expiry retains draft and requires explicit requote passed");

    trialAllowance = 0;
    trial = 0;
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await trialSummary.getByText(/无免费试用，开通后支付 .*10\.00/).waitFor();
    assert.equal(await trialSummary.locator("dl, details").count(), 0, "a Host without a trial has no trial metrics or rules");
    assert.equal(await trialSummary.getByText(/试用额度已用尽/).count(), 0);
    trialAllowance = 43200;
    trial = 4500;
    await page.goto(baseUrl + "/client-market/?focusHost=host-1&testLocale=en");
    await page.getByRole("heading", { name: "Configure and confirm" }).waitFor();
    await trialSummary.getByText("Free trial", { exact: true }).waitFor();
    assert.equal(await trialSummary.locator(":scope > dl dd").textContent(), "1 hr 15 min");
    await trialSummary.getByText(/Free during the trial, then .*10\.00/).waitFor();
    await trialSummary.getByText(/Enough balance for the first period is required.*permanently deletes Client data/).waitFor();
    assert.equal(await trialRules.getAttribute("open"), null);
    for (const width of [320, 375, 414, 768]) {
      await page.setViewportSize({ width, height: 640 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false, `English trial overflow at ${width}`);
    }
    console.log("concise bilingual trial summary, keyboard-accessible collapsed rules, dynamic prices and full/partial/one-second/exhausted/disabled allowances passed");

    // Free checkout has no wallet or renewal controls, including English rendering.
    host.cyclePriceMinor = undefined;
    host.pricingModel = "free";
    host.freeDurationDays = 7;
    trial = 0;
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    assert.equal(
      await page.getByLabel("后续按月从平台余额自动续费").count(),
      0,
    );
    assert.equal(
      await page.getByRole("button", { name: "创建", exact: true }).count(),
      1,
    );
    assert.equal(await trialSummary.count(), 0, "free offers do not display paid trial terms");
    await page.goto(baseUrl + "/client-market/?focusHost=host-1&testLocale=en");
    await page
      .getByRole("heading", { name: "Configure and confirm" })
      .waitFor();
    await page
      .getByLabel("Web login password", { exact: true })
      .fill("english-password");
    for (const width of [320, 375, 414, 768]) {
      await page.setViewportSize({ width, height: 640 });
      assert.equal(
        await page.evaluate(
          () => document.documentElement.scrollWidth > window.innerWidth,
        ),
        false,
      );
      assert(
        await page
          .getByRole("button", { name: "Create", exact: true })
          .isVisible(),
      );
    }
    await page.getByRole("button", { name: "Create", exact: true }).click();
    await page
      .getByRole("link", { name: "Open Client", exact: true })
      .waitFor();
    console.log("English free creation and short viewport widths passed");

    Object.assign(host, { status: "allocated", installationId: "client-one", isClientOwner: true,
      clientSubdomain: "rental-client", pricingModel: "prepaid_calendar_month", cyclePriceMinor: 5,
      clientConnection: { state: "online", since: "2026-10-28T00:00:00Z", lastHeartbeatAt: "2026-10-28T00:00:00Z" } });
    rentals = [{ installationId: "client-one", hostId: host.id, providerId: host.providerId,
      hostOwnerEmail: host.hostOwnerEmail, clientOwnerEmail: "buyer@example.com", status: "active",
      pricingModel: "prepaid_calendar_month", cyclePriceMinor: 5, offerRevision: 1, isClientOwner: true,
      canRelease: true, canFinalizeRelease: false, hostCleanupPending: false, providerTerminalAccessActive: false,
      canManageProviderTerminal: false, updatedAt: "2026-10-28T00:00:00Z", walletContract: {
        id: "wallet-contract", status: "active", trialSecondsRemaining: 0, renewalPolicy: "manual",
        periodEnd: "2026-11-28T06:02:00Z", nextRentalTrial: { epoch: 1, allowanceSeconds: 43200,
          consumedSeconds: 0, remainingSeconds: 43200, lastResetAt: "2026-10-28T00:00:00Z" },
      } }];
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(baseUrl + "/client-market/?tab=mine&testLocale=en");
    const rentalRow = page.locator("#client-market-host-client-one");
    await rentalRow.waitFor();
    const statusCell = rentalRow.locator("td").first();
    assert.doesNotMatch(await statusCell.textContent(), /Ends on|Renews on|Platform wallet/);
    await statusCell.getByText("Client online", { exact: true }).waitFor();
    const expandRental = rentalRow.getByRole("button", { name: "Show rental details and usage history", exact: true });
    await expandRental.focus();
    await page.keyboard.press("Enter");
    const rentalDetails = page.getByRole("region", { name: "Current rental", exact: true });
    // A named section is an accessible region when it has an aria-label.
    await rentalDetails.getByText(/^Ends on /).waitFor();
    await rentalDetails.locator("[data-next-rental-trial]").waitFor();
    assert.equal(await rentalDetails.getByRole("link", { name: "Platform wallet", exact: true }).count(), 0);
    const collapseRental = rentalRow.getByRole("button", { name: "Hide rental details and usage history", exact: true });
    assert.equal(await collapseRental.getAttribute("aria-expanded"), "true");
    assert.equal(await page.locator(`[id="${await collapseRental.getAttribute("aria-controls")}"]`).count(), 1);
    await collapseRental.click();
    await rentalDetails.waitFor({ state: "hidden" });
    for (const [status, renewal, hint] of [
      ["active", "automatic", /^Renews on /],
      ["trialing", "automatic", /Trial/],
      ["suspended", "automatic", /Paused/],
    ]) {
      rentals[0].walletContract.status = status;
      rentals[0].walletContract.renewalPolicy = renewal;
      rentals[0].walletContract.trialSecondsRemaining = status === "trialing" ? 3600 : 0;
      await page.reload();
      await rentalRow.waitFor();
      if (status === "active") assert.doesNotMatch(await statusCell.textContent(), /Renews on/);
      else await statusCell.getByText(hint).waitFor();
      await rentalRow.click();
      await rentalDetails.getByText(hint).waitFor();
    }
    rentals[0].status = "release_failed";
    rentals[0].cleanupFailureKind = "cleanup_ssh_unreachable";
    await page.reload();
    await rentalRow.getByText(/SSH|Host|unreachable|offline/).first().waitFor();
    console.log("Client rows keep status/health and exception actions visible; expiry, renewal and restored trials live in expandable details");
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
