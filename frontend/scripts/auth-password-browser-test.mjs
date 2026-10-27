import assert from "node:assert/strict";
import {
  createPublicKey,
  generateKeyPairSync,
  verify as verifySignature,
} from "node:crypto";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Real exported components and device signatures, mocked local APIs only.
// Never captures screenshots, sends email or touches deployed accounts.
const output = fileURLToPath(new URL("../out/", import.meta.url));
const server = createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(
      new URL(request.url, "http://localhost").pathname,
    );
    const path = resolve(
      output,
      `.${pathname.endsWith("/") ? `${pathname}index.html` : pathname}`,
    );
    if (!path.startsWith(`${resolve(output)}${sep}`))
      return response.writeHead(403).end();
    const types = {
      ".html": "text/html",
      ".js": "text/javascript",
      ".css": "text/css",
      ".txt": "text/plain",
      ".svg": "image/svg+xml",
      ".ttf": "font/ttf",
      ".woff2": "font/woff2",
    };
    response.writeHead(200, {
      "Content-Type": types[extname(path)] || "application/octet-stream",
      "Cache-Control": "no-store",
    });
    response.end(await readFile(path));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((ready, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", ready);
});
const base = `http://127.0.0.1:${server.address().port}`;
const pair = generateKeyPairSync("ed25519");
const identity = {
  authDeviceId: "password-browser",
  publicKey: pair.publicKey
    .export({ type: "spki", format: "der" })
    .subarray(-32)
    .toString("base64"),
  privateKey: pair.privateKey
    .export({ type: "pkcs8", format: "der" })
    .toString("base64"),
};
const user = { id: "password-buyer", email: "buyer@example.test" };
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome",
    headless: true,
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage();
  page.setDefaultTimeout(10_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript((initial) => {
    if (!localStorage.getItem("cc_switch_router_locale_v1"))
      localStorage.setItem("cc_switch_router_locale_v1", "en");
    if (!localStorage.getItem("cc_switch_router_auth_v2"))
      localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify(initial));
  }, identity);
  let actor = null;
  let state = {
    hasPassword: false,
    version: 0,
    loginEventId: null,
  };
  let credential = null;
  let token = 0;
  let otpEvents = 0;
  let codeRequests = 0;
  let passwordRequests = 0;
  let updates = 0;
  let busyNextSave = false;
  let rotateNextSave = true;
  let loseNextDevice = false;
  let holdNextPasswordLogin = false;
  let releasePasswordLogin;
  let publicKey = pair.publicKey;
  let holdNextSession = false;
  let releaseSession;
  const json = (route, body, status = 200) =>
    route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  const result = () => ({
    user: actor,
    password: { ...state },
    accessToken: `access-${++token}`,
    refreshToken: `refresh-${token}`,
    expiresAt: "2099-01-01T00:00:00Z",
    refreshExpiresAt: "2099-02-01T00:00:00Z",
  });
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/v1/auth/home") return json(route, { hasActivity: true, entry: null, source: null });
    if (path === "/v1/auth/session/me") {
      const response = actor
        ? {
            authenticated: true,
            isAdmin: false,
            user: actor,
            password: { ...state },
          }
        : { authenticated: false, isAdmin: false };
      if (holdNextSession) {
        holdNextSession = false;
        await new Promise((release) => {
          releaseSession = release;
        });
      }
      return json(route, response);
    }
    if (path === "/v1/auth/devices/register") {
      const body = route.request().postDataJSON();
      publicKey = createPublicKey({
        key: Buffer.concat([
          Buffer.from("302a300506032b6570032100", "hex"),
          Buffer.from(body.publicKey, "base64"),
        ]),
        type: "spki",
        format: "der",
      });
      return json(route, { authDeviceId: "recovered-device" });
    }
    if (path === "/v1/auth/email/request-code") {
      codeRequests++;
      return json(route, { maskedDestination: "b***@example.test" });
    }
    if (path === "/v1/auth/email/verify-code") {
      actor = user;
      state = {
        ...state,
        loginEventId: `email-${++otpEvents}`,
      };
      return json(route, result());
    }
    if (path === "/v1/auth/password/login") {
      passwordRequests++;
      if (loseNextDevice) {
        loseNextDevice = false;
        return json(route, { message: "auth device not found" }, 401);
      }
      const body = route.request().postDataJSON();
      const canonical = `namespace-flat-1\n${body.authDeviceId}\nauth_password_login\n${JSON.stringify({ email: body.email, password: body.password })}\n${body.timestampMs}\n${body.nonce}`;
      assert.equal(
        verifySignature(
          null,
          Buffer.from(canonical),
          publicKey,
          Buffer.from(body.signature, "base64"),
        ),
        true,
        "password login signs the complete canonical credential payload",
      );
      if (!credential)
        return json(
          route,
          { code: "password_unavailable", message: "account unavailable" },
          401,
        );
      if (body.password !== credential)
        return json(
          route,
          { code: "password_invalid", message: "wrong password" },
          401,
        );
      actor = user;
      state = {
        ...state,
        loginEventId: `password-${passwordRequests}`,
      };
      const response = result();
      if (holdNextPasswordLogin) {
        holdNextPasswordLogin = false;
        await new Promise((release) => {
          releasePasswordLogin = release;
        });
      }
      return json(route, response);
    }
    if (path === "/v1/auth/password") {
      updates++;
      const body = route.request().postDataJSON();
      assert.match(route.request().headers().authorization, /^Bearer access-/);
      assert.equal(body.expectedVersion, state.version);
      assert.deepEqual(Object.keys(body).sort(), ["expectedVersion", "newPassword"]);
      if (busyNextSave) {
        busyNextSave = false;
        return json(route, { code: "password_busy" }, 503);
      }
      if (rotateNextSave) {
        rotateNextSave = false;
        await page.evaluate(() => {
          const current = JSON.parse(
            localStorage.getItem("cc_switch_router_auth_v2"),
          );
          localStorage.setItem(
            "cc_switch_router_auth_v2",
            JSON.stringify({
              ...current,
              accessToken: "rotated-access",
              refreshToken: "rotated-refresh",
            }),
          );
          window.dispatchEvent(
            new StorageEvent("storage", { key: "cc_switch_router_auth_v2" }),
          );
        });
      }
      credential = body.newPassword;
      state = { ...state, hasPassword: true, version: state.version + 1 };
      return json(route, result());
    }
    if (path === "/v1/auth/session/logout") {
      actor = null;
      return json(route, {});
    }
    if (path === "/v1/market/config")
      return json(route, { usdCnyRateMicros: 7_000_000, fxRateFresh: true });
    if (path === "/v1/market-wallet")
      return json(route, { availableUnits: "0", hasWalletActivity: false });
    if (path === "/v1/regions")
      return json(route, [{ name: "test", url: base }]);
    if (path === "/v1/announcement")
      return json(route, {
        enabled: false,
        revision: "1",
        contentEn: "",
        contentZhCn: "",
      });
    if (path.endsWith("inbox-summary"))
      return json(route, { pendingRequests: 0 });
    if (path.endsWith("/rooms"))
      return json(route, { rooms: [], totalUnread: 0 });
    return json(route, {});
  });
  const dialog = () =>
    page.locator('[role="dialog"][data-slot="modal-dialog"]');
  const changeMenu = async (label) => {
    await page.getByRole("button", { name: user.email, exact: true }).click();
    await page.getByRole("menuitem", { name: label, exact: true }).click();
  };
  const verifyOtp = async () => {
    await dialog()
      .getByRole("textbox", { name: "Email", exact: true })
      .fill(user.email);
    await dialog()
      .getByRole("button", { name: "Send Code", exact: true })
      .click();
    await dialog()
      .getByRole("textbox", { name: "Code", exact: true })
      .fill("123456");
  };
  const setNew = async (value) => {
    await dialog().getByLabel("New password", { exact: true }).fill(value);
    await dialog()
      .getByLabel("Confirm new password", { exact: true })
      .fill(value);
    await dialog()
      .getByRole("button", { name: "Save password", exact: true })
      .click();
  };

  await page.goto(`${base}/account/api-keys/`);
  await page.getByRole("button", { name: "Login", exact: true }).click();
  assert.equal(
    await dialog()
      .getByRole("button", { name: "Email code", exact: true })
      .getAttribute("aria-pressed"),
    "true",
  );
  await dialog().getByRole("button", { name: "Password", exact: true }).click();
  await dialog()
    .getByRole("textbox", { name: "Email", exact: true })
    .fill(user.email);
  await dialog()
    .getByLabel("Password", { exact: true })
    .fill("not yet configured");
  await dialog().getByRole("button", { name: "Sign in", exact: true }).click();
  await dialog()
    .getByRole("alert")
    .filter({ hasText: "This account cannot use password login" })
    .waitFor();
  await dialog()
    .getByRole("button", { name: "Use an email code", exact: true })
    .click();
  assert.equal(
    await dialog()
      .getByRole("textbox", { name: "Email", exact: true })
      .inputValue(),
    user.email,
  );
  await verifyOtp();
  await page
    .getByRole("heading", { name: "Set login password", exact: true })
    .waitFor();
  await dialog().getByRole("button", { name: "Not now", exact: true }).click();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("router-auth-changed")),
  );
  await page.getByRole("button", { name: user.email, exact: true }).waitFor();
  await page.reload();
  await page.getByRole("button", { name: user.email, exact: true }).waitFor();
  assert.equal(
    await dialog().count(),
    0,
    "refresh and session bootstrap never reopen setup",
  );
  console.log(
    "PASS email-code default, unavailable-password recovery and one prompt per real OTP login",
  );

  // Dismissing setup does not suppress the next distinct OTP login.
  await changeMenu("Logout");
  await page.getByRole("button", { name: "Login", exact: true }).click();
  await verifyOtp();
  await page
    .getByRole("heading", { name: "Set login password", exact: true })
    .waitFor();
  await dialog().getByRole("button", { name: "Not now", exact: true }).click();
  await dialog().waitFor({ state: "hidden" });
  await changeMenu("Set login password");
  assert.equal(
    await dialog().getByLabel("Current password", { exact: true }).count(),
    0,
  );
  await setNew("short");
  await dialog()
    .getByRole("alert")
    .filter({ hasText: "Use 6–128 characters" })
    .waitFor();
  assert.equal(updates, 0, "a five-character password never reaches the API");
  await setNew("first6");
  await page
    .getByRole("heading", { name: "Password saved", exact: true })
    .waitFor();
  await dialog()
    .getByRole("button", { name: "Close", exact: true })
    .last()
    .click();
  await dialog().waitFor({ state: "hidden" });
  await changeMenu("Logout");
  await page.getByRole("button", { name: "Login", exact: true }).click();
  await dialog().getByRole("button", { name: "Password", exact: true }).click();
  await dialog()
    .getByRole("textbox", { name: "Email", exact: true })
    .fill(user.email);
  await dialog()
    .getByLabel("Password", { exact: true })
    .fill("incorrect password");
  await dialog().getByRole("button", { name: "Sign in", exact: true }).click();
  await dialog()
    .getByRole("alert")
    .filter({ hasText: "Email or password is incorrect" })
    .waitFor();
  assert.equal(
    await dialog().getByLabel("Password", { exact: true }).inputValue(),
    "",
  );
  loseNextDevice = true;
  await dialog().getByLabel("Password", { exact: true }).fill(credential);
  await dialog().getByRole("button", { name: "Sign in", exact: true }).click();
  await dialog().waitFor({ state: "hidden" });
  assert.equal(codeRequests, 2, "password login never sends an OTP email");
  await changeMenu("Change login password");
  assert.equal(await dialog().getByLabel("Current password", { exact: true }).count(), 0);
  await dialog()
    .getByText("A confirmation email will include your new password.", { exact: false })
    .waitFor();
  await setNew("next06");
  await page
    .getByRole("heading", { name: "Password saved", exact: true })
    .waitFor();
  await dialog()
    .getByRole("button", { name: "Close", exact: true })
    .last()
    .click();
  await dialog().waitFor({ state: "hidden" });
  console.log(
    "PASS signed password login, token rotation, stale-device recovery and two-field password change without old password or OTP",
  );

  await changeMenu("Logout");
  await page.getByRole("button", { name: "Login", exact: true }).click();
  await dialog().getByRole("button", { name: "Password", exact: true }).click();
  await dialog()
    .getByRole("button", { name: "Forgot password?", exact: true })
    .click();
  await verifyOtp();
  await page
    .getByRole("heading", { name: "Change login password", exact: true })
    .waitFor();
  assert.equal(
    await dialog().getByLabel("Current password", { exact: true }).count(),
    0,
  );
  busyNextSave = true;
  await setNew("third secure password");
  await dialog()
    .getByRole("alert")
    .filter({ hasText: "Password authentication is busy" })
    .waitFor();
  await setNew("third secure password");
  await page
    .getByRole("heading", { name: "Password saved", exact: true })
    .waitFor();
  await dialog()
    .getByRole("button", { name: "Close", exact: true })
    .last()
    .click();
  await dialog().waitFor({ state: "hidden" });
  assert.equal(
    updates,
    4,
    "each save emits one mutation, including a failed busy check",
  );
  console.log(
    "PASS forgotten-password email recovery and retryable change errors without adding a second verification step",
  );

  // An old in-flight session read must not restore a previous account.
  holdNextSession = true;
  await page.evaluate(() =>
    window.dispatchEvent(new Event("router-auth-changed")),
  );
  const deadline = Date.now() + 5000;
  while (!releaseSession && Date.now() < deadline)
    await new Promise((ready) => setTimeout(ready, 10));
  assert.equal(typeof releaseSession, "function");
  actor = { id: "another-user", email: "another@example.test" };
  await page.evaluate(() => {
    const state = JSON.parse(localStorage.getItem("cc_switch_router_auth_v2"));
    localStorage.setItem(
      "cc_switch_router_auth_v2",
      JSON.stringify({
        ...state,
        accessToken: "another-access",
        refreshToken: "another-refresh",
        email: "another@example.test",
      }),
    );
    window.dispatchEvent(
      new StorageEvent("storage", { key: "cc_switch_router_auth_v2" }),
    );
  });
  await page.getByRole("button", { name: actor.email, exact: true }).waitFor();
  const staleResponse = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/v1/auth/session/me",
  );
  releaseSession();
  await (await staleResponse).finished();
  await page.evaluate(
    () =>
      new Promise((done) =>
        requestAnimationFrame(() => requestAnimationFrame(done)),
      ),
  );
  await page.getByRole("button", { name: actor.email, exact: true }).waitFor();
  assert.equal(await dialog().count(), 0);
  console.log(
    "PASS cross-tab actor isolation and stale-session response protection",
  );

  // A completed password response also cannot overwrite a newer cross-tab login.
  await page.getByRole("button", { name: actor.email, exact: true }).click();
  await page.getByRole("menuitem", { name: "Logout", exact: true }).click();
  await page.getByRole("button", { name: "Login", exact: true }).click();
  await dialog().getByRole("button", { name: "Password", exact: true }).click();
  await dialog()
    .getByRole("textbox", { name: "Email", exact: true })
    .fill(user.email);
  await dialog().getByLabel("Password", { exact: true }).fill(credential);
  holdNextPasswordLogin = true;
  await dialog().getByRole("button", { name: "Sign in", exact: true }).click();
  const loginDeadline = Date.now() + 5000;
  while (!releasePasswordLogin && Date.now() < loginDeadline)
    await new Promise((ready) => setTimeout(ready, 10));
  assert.equal(typeof releasePasswordLogin, "function");
  actor = { id: "another-user", email: "another@example.test" };
  await page.evaluate(() => {
    const current = JSON.parse(
      localStorage.getItem("cc_switch_router_auth_v2"),
    );
    localStorage.setItem(
      "cc_switch_router_auth_v2",
      JSON.stringify({
        ...current,
        email: "another@example.test",
        accessToken: "newer-access",
        refreshToken: "newer-refresh",
      }),
    );
    window.dispatchEvent(
      new StorageEvent("storage", { key: "cc_switch_router_auth_v2" }),
    );
  });
  await page.getByRole("button", { name: actor.email, exact: true }).waitFor();
  releasePasswordLogin();
  await dialog()
    .getByRole("alert")
    .filter({ hasText: "Your sign-in session changed" })
    .waitFor();
  assert.equal(
    await page.evaluate(
      () =>
        JSON.parse(localStorage.getItem("cc_switch_router_auth_v2"))
          .accessToken,
    ),
    "newer-access",
  );
  await dialog().getByRole("button", { name: "Close", exact: true }).click();
  await dialog().waitFor({ state: "hidden" });

  await page.evaluate(() =>
    localStorage.setItem("cc_switch_router_locale_v1", "zh-CN"),
  );
  await page.setViewportSize({ width: 375, height: 812 });
  await page.reload();
  await page.getByRole("button", { name: actor.email, exact: true }).click();
  await page
    .getByRole("menuitem", { name: "修改登录密码", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "修改登录密码", exact: true })
    .waitFor();
  const box = await dialog().boundingBox();
  assert.ok(
    box && box.width <= 375 && box.x >= 0,
    "mobile password dialog fits the viewport",
  );
  assert.equal(await dialog().getByLabel("当前密码", { exact: true }).count(), 0);
  assert.equal(await dialog().locator('input[type="password"]').count(), 2);
  await dialog().getByText("使用 6–128 个字符", { exact: false }).waitFor();
  await dialog()
    .getByText("确认邮件将包含新密码明文", { exact: false })
    .waitFor();
  await dialog().getByRole("button", { name: "取消", exact: true }).click();
  await dialog().waitFor({ state: "hidden" });
  assert.deepEqual(errors, []);
  console.log(
    "PASS credential-response actor isolation, Chinese password copy and mobile dialog geometry; no screenshots",
  );
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
