import { chromium } from "playwright";
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";

// Start the frontend first. All business APIs are mocked; no SSH or payments occur.
const baseUrl = process.env.CHECKOUT_TEST_URL || "http://127.0.0.1:3127";
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
      available = "0",
      shortfall = "0",
      quotes = 0,
      commits = [],
      commitMode = "success",
      expiry = 120000,
      cancels = 0;
    let retiredMoneyRequests = 0;
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
      if (path === "/v1/auth/session/me")
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
      else if (path === "/v1/client-market/my-rentals") body = [];
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
                  requiredUnits: trial ? "0" : "86400000",
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
    await page.getByText("剩余试用 12 小时", { exact: true }).waitFor();
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
    trial = 0;
    shortfall = "86400000";
    await page.goto(baseUrl + "/client-market/?focusHost=host-1");
    await page.getByRole("heading", { name: "配置并确认" }).waitFor();
    await page
      .getByRole("button", { name: "充值后继续", exact: true })
      .waitFor();
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
    assert.deepEqual(errors, []);
    console.log("free checkout passed; no browser exceptions");
    console.log(captureScreenshots ? `Screenshots: ${artifacts}` : "DOM and interaction checks only; no screenshots");
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
