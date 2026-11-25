// Run against a disposable Router, a connected Client, and two distinct verified
// local test accounts. Supply the fixture and the seller's local password on stdin.
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { readFileSync } from "node:fs";

const fixture = JSON.parse(readFileSync(0, "utf8"));
const base = new URL(fixture.origin).origin;
const browser = await chromium.launch({
  executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome",
  args: ["--no-sandbox", "--no-proxy-server"],
});
async function participant(email, accessToken) {
  const context = await browser.newContext({ locale: "en-US" });
  await context.addInitScript(
    ({ base, email, accessToken }) => {
      if (location.origin !== base) return;
      localStorage.setItem("cc_switch_router_locale_v1", "en");
      localStorage.setItem(
        "cc_switch_router_auth_v2",
        JSON.stringify({
          email,
          accessToken,
          loginEpoch: email,
          expiresAt: new Date(Date.now() + 3600000).toISOString(),
        }),
      );
    },
    { base, email, accessToken },
  );
  const page = await context.newPage();
  page.setDefaultTimeout(45000);
  return page;
}
const seller = await participant(fixture.sellerEmail, fixture.sellerToken);
const buyer = await participant(fixture.buyerEmail, fixture.buyerToken);
const localOwner = fixture.ownerLocalOrigin ? await browser.newPage({ locale: "en-US" }) : null;
const workspaceRequests = [];
buyer.context().on("response", (r) => {
  const url = new URL(r.url());
  if (url.hostname.startsWith("rent-")) workspaceRequests.push([r.status(), url.pathname]);
});
try {
  if (localOwner) {
    await localOwner.goto(fixture.ownerLocalOrigin);
    const password = localOwner.locator('input[type="password"]');
    await password.fill(fixture.sellerPassword);
    await password.press("Enter");
    await password.waitFor({ state: "hidden" });
  }
  await seller.goto(
    base + "/client-market/?view=selling&add=1&installation=" + encodeURIComponent(fixture.installationId),
  );
  const listing = seller.getByRole("dialog");
  await listing.getByText(/✓ Egress address verified/).waitFor();
  await listing.getByRole("checkbox").check();
  const publish = listing.getByRole("button", { name: "Publish listing" });
  await publish.click();
  await listing.waitFor({ state: "hidden" });
  await seller.getByText("Available", { exact: true }).first().waitFor();
  if (localOwner) {
    await localOwner.bringToFront();
    await localOwner.locator("aside").filter({ hasText: /Client Market|租用/ }).waitFor({ timeout: 70_000 });
  }
  console.log("PASS seller preflight and publication");
  await buyer.goto(base + "/client-market/");
  await buyer
    .getByRole("button", { name: /Show Clients in/ })
    .first()
    .click();
  const card = buyer.locator('[id="market-client-' + fixture.installationId + '"]');
  await card.getByRole("button", { name: /Continue/ }).click();
  const quote = buyer.getByRole("dialog");
  await quote.getByRole("checkbox", { name: /root/i }).check();
  await quote.getByRole("button", { name: "Confirm rental" }).click();
  await buyer.waitForURL((u) => u.searchParams.get("view") === "rentals" && !!u.searchParams.get("focus"));
  const lease = new URL(buyer.url()).searchParams.get("focus");
  const rental = buyer.locator('[id="market-client-' + lease + '"]');
  const open = rental.getByRole("link", { name: "Open workspace" });
  const opened = buyer.waitForEvent("popup");
  await open.click();
  const workspace = await opened;
  workspace.setDefaultTimeout(35000);
  await workspace.getByRole("heading", { name: /Tenant workspace|租户工作区/ }).waitFor();
  if (localOwner) {
    await localOwner.bringToFront();
    const assistance = localOwner.getByRole("heading", { name: /Owner assistance|所有者协助/ });
    const password = localOwner.locator('input[type="password"]');
    await assistance.or(password).first().waitFor({ timeout: 45_000 });
    if (!(await assistance.isVisible())) {
      // Direct /api/auth/login sessions expire when the Client restarts into its leased workspace.
      await password.fill(fixture.sellerPassword);
      await password.press("Enter");
    }
    await assistance.waitFor();
    await localOwner.getByRole("button", { name: /^(Restart|重启)$/ }).waitFor();
    console.log("PASS open local owner console followed listing and local password login restored assistance after restart");
  }
  assert.match(new URL(workspace.url()).hostname, /^rent-[0-9a-f]{20}\./);
  assert.equal(new URL(workspace.url()).pathname, "/");
  assert.equal(await workspace.locator('meta[name="cc-lease-ticket"]').count(), 0);
  assert.equal(
    await workspace.evaluate(
      () =>
        JSON.stringify(localStorage).includes("accessToken") &&
        !!JSON.parse(localStorage.getItem("cc_switch_router_auth_v2") || "{}").accessToken,
    ),
    false,
  );
  await workspace.reload();
  await workspace.getByRole("heading", { name: /Tenant workspace|租户工作区/ }).waitFor();
  const sessionStarted = Date.now();
  console.log("PASS checkout receipt focus, standalone alias login and refresh");
  const account = await buyer.context().newPage();
  await account.goto(base + "/account/rentals/?market=client&focus=" + encodeURIComponent(lease));
  const accountCard = account.getByTestId("client-rentals-current");
  const accountPopup = account.waitForEvent("popup");
  await accountCard.getByRole("link", { name: "Open workspace" }).click();
  const fromAccount = await accountPopup;
  await fromAccount.getByRole("heading", { name: /Tenant workspace|租户工作区/ }).waitFor();
  assert.equal(new URL(fromAccount.url()).origin, new URL(workspace.url()).origin);
  await fromAccount.getByRole("button", { name: /^(Sign out|退出登录)$/i }).click();
  await fromAccount.getByRole("heading", { name: /Sign in to the workspace|登录租户工作区/ }).waitFor();
  await fromAccount.close();
  await account.close();
  console.log("PASS workspace access and independent window sign-out from the buyer's account rental record");
  await seller.getByRole("button", { name: "Owner assistance", exact: true }).click();
  const assistance = seller.frameLocator('iframe[src*="routerOrigin="]');
  await assistance.getByRole("heading", { name: /Owner assistance|所有者协助/ }).waitFor();
  await assistance.getByLabel(/Owner password for maintenance|运维需要所有者密码/).fill(fixture.sellerPassword);
  await assistance.getByRole("button", { name: /Unlock maintenance|解锁运维/ }).click();
  await assistance.getByRole("button", { name: /^(Restart|重启)$/ }).waitFor();
  const resize = seller.waitForResponse((r) => new URL(r.url()).pathname === "/web-api/terminal/resize" && r.ok());
  const terminal = assistance.getByRole("button", { name: /^(Owner terminal|所有者终端)$/ });
  await terminal.click();
  const dimensions = (await resize).request().postDataJSON();
  assert.ok(dimensions.c > 1 && dimensions.r > 1);
  await terminal.click();
  await assistance.getByText(/^(Host settings|主机设置)$/).click();
  const interval = assistance.getByRole("spinbutton");
  const before = Number(await interval.inputValue());
  for (const value of [before < 1440 ? before + 1 : before - 1, before]) {
    await interval.fill(String(value));
    const save = seller.waitForResponse((r) => new URL(r.url()).pathname === "/web-api/market/maintenance" && r.ok());
    seller.once("dialog", (dialog) => dialog.accept());
    await assistance.getByRole("button", { name: /Save host settings|保存主机设置/ }).click();
    assert.equal((await (await save).json()).upgradePolicy.autoUpgradeCheckIntervalMinutes, value);
  }
  await seller.goto(base + "/client-market/?view=selling");
  console.log("PASS seller's embedded assistance, independent owner verification, terminal resize and host settings");
  if (process.env.WORKSPACE_LONG_SESSION === "1") {
    await workspace.waitForResponse(
      (r) => new URL(r.url()).pathname === "/web-api/auth/router-renew" && r.ok(),
      { timeout: 630_000 },
    );
    await workspace.waitForFunction((since) => Date.now() - since >= 605_000, sessionStarted, {
      timeout: 610_000,
      polling: 1000,
    });
    const context = await workspace.waitForResponse((r) => new URL(r.url()).pathname === "/web-api/context" && r.ok());
    assert.equal((await context.json()).workspaceRole, "tenant");
    console.log("PASS standalone session renewed and remained usable beyond ten minutes");
  }
  await rental.getByRole("button", { name: "End rental" }).click();
  const end = buyer.getByRole("dialog");
  const downloading = buyer.waitForEvent("download");
  await end.getByRole("button", { name: "Export workspace backup" }).click();
  const download = await downloading;
  assert.equal(await download.failure(), null);
  assert.equal(download.suggestedFilename(), "workspace-" + lease + ".zip");
  await end.getByRole("button", { name: "Preview settlement" }).click();
  await end.getByRole("button", { name: "End rental" }).click();
  await end.waitFor({ state: "hidden" });
  await workspace.getByRole("heading", { name: /Lease ended|租约已结束/ }).waitFor({ timeout: 65000 });
  await seller.getByText("Available", { exact: true }).first().waitFor({ timeout: 90000 });
  console.log("PASS backup, settlement confirmation, revocation and inventory recovery");
} catch (error) {
  console.error("Workspace HTTP responses", workspaceRequests.slice(-25));
  throw error;
} finally {
  await browser.close();
}
