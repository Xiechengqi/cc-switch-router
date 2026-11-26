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
  const publish = listing.getByRole("button", { name: "List for free", exact: true });
  await publish.waitFor();
  await listing.getByRole("checkbox", { name: /renter controls the console/i }).check();
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
  await card.getByRole("button", { name: "Rent", exact: true }).click();
  const quote = buyer.getByRole("dialog");
  await quote.getByRole("checkbox", { name: /owner can read/i }).check();
  await quote.getByRole("button", { name: "Confirm free rental" }).click();
  await buyer.waitForURL((u) => u.pathname === "/account/rentals/" && u.searchParams.get("market") === "client" && !!u.searchParams.get("focus"));
  const lease = new URL(buyer.url()).searchParams.get("focus");
  const rental = buyer.getByTestId("client-rentals-current").getByTestId("client-rental-detail");
  const open = rental.getByRole("link", { name: "Open workspace" });
  const opened = buyer.waitForEvent("popup");
  await open.click();
  const workspace = await opened;
  workspace.setDefaultTimeout(35000);
  await workspace.getByRole("heading", { name: /Tenant workspace|租户工作区/ }).waitFor();
  if (localOwner) {
    await localOwner.bringToFront();
    const response = await localOwner.request.get(new URL("/web-api/context", fixture.ownerLocalOrigin).href);
    assert.equal(response.status(), 403);
    assert.equal((await response.json()).code, "workspace_leased");
    await localOwner.getByText(/renter controls this Client|此 Client 已出租/i).waitFor();
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
  async function assertPhysicalTicketsClosed(page) {
    for (const kind of ["console","terminal"]) {
      const response=await page.request.post(base + "/v1/auth/client-web-tickets", {
        headers:{authorization:"Bearer " + fixture.sellerToken},
        data:{installationId:fixture.installationId, kind, nonce:crypto.randomUUID()},
      });
      assert.equal(response.status(),403);
    }
  }
  await assertPhysicalTicketsClosed(seller);
  assert.equal(await workspace.getByRole("button", {name:/web terminal/i}).count(),0);
  const terminalResponse = await workspace.request.get(new URL("/web-api/terminal/stream",workspace.url()).href);
  assert.equal(terminalResponse.status(),403);
  await seller.goto(base + "/client-market/?view=selling");
  await seller.getByText(fixture.buyerEmail,{exact:false}).first().waitFor();
  console.log("PASS physical tickets and Web Terminal closed during the lease");
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
  if (localOwner) {
    await localOwner.reload();
    const password=localOwner.locator('input[type="password"]');
    await password.fill(fixture.sellerPassword);
    await password.press("Enter");
    await password.waitFor({state:"hidden"});
  }
  console.log("PASS backup, settlement confirmation, revocation and inventory recovery");
  await seller.goto(base + "/client-market/");
  const ownCard = seller.locator('[id="market-client-' + fixture.installationId + '"]');
  await ownCard.getByText("Free for you").waitFor();
  const quoteReady = seller.waitForResponse((response) => response.url().endsWith("/quote") && response.ok());
  await ownCard.getByRole("button", { name: "Rent", exact: true }).click();
  const selfQuote = await (await quoteReady).json();
  assert.equal(selfQuote.terms.selfRental, true);
  assert.equal(selfQuote.terms.cyclePriceMinor, 0);
  assert.equal(selfQuote.walletFunding, null);
  const selfDialog = seller.getByRole("dialog");
  await selfDialog.getByRole("button", { name: "Confirm free rental" }).click();
  await seller.waitForURL((u) => u.pathname === "/account/rentals/" && !!u.searchParams.get("focus"));
  const ownRental = seller.getByTestId("client-rentals-current").getByTestId("client-rental-detail");
  const selfPopup = seller.waitForEvent("popup");
  await ownRental.getByRole("link", { name: "Open workspace" }).click();
  const selfWorkspace = await selfPopup;
  await selfWorkspace.getByRole("heading", { name: /Tenant workspace|租户工作区/ }).waitFor();
  const management = await seller.context().newPage();
  await management.goto(base + "/client-market/?view=selling");
  await assertPhysicalTicketsClosed(management);
  await ownRental.getByRole("button", { name: "End rental" }).click();
  await selfDialog.getByRole("button", { name: "Preview settlement" }).click();
  await selfDialog.getByRole("button", { name: "End rental" }).click();
  await selfDialog.waitFor({ state: "hidden" });
  await selfWorkspace.getByRole("heading", { name: /Lease ended|租约已结束/ }).waitFor({ timeout: 65000 });
  await management.goto(base + "/client-market/?view=selling");
  await management.getByText("Available", { exact: true }).first().waitFor({ timeout: 90000 });
  console.log("PASS same account rents its own listing for free, uses only the tenant console, and releases the Client");

} catch (error) {
  console.error("Workspace HTTP responses", workspaceRequests.slice(-25));
  throw error;
} finally {
  await browser.close();
}
