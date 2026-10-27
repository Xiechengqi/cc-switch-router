import assert from "node:assert/strict";
import test from "node:test";
import {
  passwordErrorKey,
  shouldPromptForPassword,
  validLoginPassword,
  sameLoginContext,
} from "./password-auth";
import { passwordMessagesEn, passwordMessagesZh } from "./password-messages";

test("credential replies survive token rotation but cannot overwrite another login or explicit logout", () => {
  const started = {
    loginEpoch: "login-one",
    email: "u@example.test",
    accessToken: "old",
    refreshToken: "old-refresh",
  };
  assert.equal(
    sameLoginContext(started, {
      ...started,
      accessToken: "rotated",
      refreshToken: "rotated-refresh",
    }),
    true,
  );
  assert.equal(
    sameLoginContext(started, { ...started, loginEpoch: "login-two" }),
    false,
  );
  assert.equal(
    sameLoginContext(started, { ...started, email: "another@example.test" }),
    false,
  );
  assert.equal(
    sameLoginContext(started, { loginEpoch: null, email: null }),
    false,
  );
  assert.equal(
    sameLoginContext({ accessToken: "legacy" }, { accessToken: "legacy" }),
    true,
  );
  assert.equal(
    sameLoginContext({ accessToken: "legacy" }, { accessToken: "another" }),
    false,
  );
});

test("only accounts without a password or an explicit reset intent need the OTP completion prompt", () => {
  assert.equal(shouldPromptForPassword({}), false);
  assert.equal(shouldPromptForPassword({}, true), false);
  const password = {
    hasPassword: false,
    version: 0,
    loginEventId: "real-email-login",
  };
  assert.equal(shouldPromptForPassword({ password }), true);
  assert.equal(
    shouldPromptForPassword({ password: { ...password, hasPassword: true } }),
    false,
  );
  assert.equal(
    shouldPromptForPassword(
      { password: { ...password, hasPassword: true } },
      true,
    ),
    true,
  );
});

test("password policy accepts 6–128 Unicode characters without arbitrary composition rules", () => {
  for (const value of [
    "123456",
    "1234567",
    "12345678",
    "  ab  ",
    "  long password  ",
    "correct horse battery staple",
    "密".repeat(6),
    "🔑".repeat(6),
    "a".repeat(128),
    "🔑".repeat(128),
  ]) {
    assert.equal(validLoginPassword(value), true);
  }
  for (const value of [
    "",
    "short",
    "🔑".repeat(5),
    " ".repeat(6),
    "\u2003".repeat(6),
    "invalid\nlong password",
    "invalid\u007flong password",
    "abc\0def",
    "a".repeat(129),
    "🔑".repeat(129),
  ]) {
    assert.equal(validLoginPassword(value), false);
  }
});

test("password copy and structured errors have complete English and Chinese resources", () => {
  assert.deepEqual(
    Object.keys(passwordMessagesEn).sort(),
    Object.keys(passwordMessagesZh).sort(),
  );
  assert.equal(
    passwordMessagesZh["auth.password.unavailable"],
    "当前用户不能使用密码登录，请使用验证码登录后，再设置登录密码。",
  );
  assert.match(passwordMessagesEn["auth.password.policy"], /6–128/);
  assert.match(passwordMessagesZh["auth.password.policy"], /6–128/);
  for (const [key, english] of Object.entries(passwordMessagesEn)) {
    assert.ok(english.trim());
    assert.ok(
      passwordMessagesZh[key as keyof typeof passwordMessagesEn].trim(),
    );
  }
  assert.equal(
    passwordErrorKey("password_session_changed"),
    "auth.password.sessionChanged",
  );
  for (const unknown of [undefined, "OTHER", "__proto__", "constructor"])
    assert.equal(passwordErrorKey(unknown), undefined);
});
