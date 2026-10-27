import type { AuthResult } from "./types";

type LoginContext = {
  loginEpoch?: string | null;
  email?: string | null;
  accessToken?: string | null;
  refreshToken?: string | null;
};

// Token rotation is not a new login. Explicit login/logout changes the epoch,
// including when signing into the same account again in another tab.
export function sameLoginContext(started: LoginContext, current: LoginContext) {
  if (started.loginEpoch)
    return (
      started.loginEpoch === current.loginEpoch &&
      started.email === current.email
    );
  return (
    started.accessToken === current.accessToken &&
    started.refreshToken === current.refreshToken
  );
}

export function shouldPromptForPassword(
  result: Pick<AuthResult, "password">,
  resetIntent = false,
) {
  return !!result.password && (resetIntent || !result.password.hasPassword);
}

export function validLoginPassword(password: string) {
  const length = Array.from(password).length;
  return (
    length >= 6 &&
    length <= 128 &&
    !/[\p{Cc}]/u.test(password) &&
    password.trim().length > 0
  );
}

export const passwordErrorKeys = {
  password_unavailable: "auth.password.unavailable",
  password_invalid: "auth.password.invalid",
  password_changed: "auth.password.changedElsewhere",
  password_policy: "auth.password.policy",
  password_busy: "auth.password.busy",
  password_rate_limited: "auth.password.rateLimited",
  password_session_changed: "auth.password.sessionChanged",
} as const;

export function passwordErrorKey(code?: string) {
  return code && Object.hasOwn(passwordErrorKeys, code)
    ? passwordErrorKeys[code as keyof typeof passwordErrorKeys]
    : undefined;
}
