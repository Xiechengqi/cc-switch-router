import { SHARE_LOCAL_TEST_ORIGIN } from "./share-local-test";

// Bump when the destination or credential-handling contract changes.
const CONSENT_VERSION = 1;

function consentKey(accountId: string, targetOrigin: string) {
  return `router-share-local-test-consent:${JSON.stringify([CONSENT_VERSION, accountId, targetOrigin])}`;
}

/** Store only consent, never an API Key, login credential or generated URL.
 * localStorage already isolates the Router origin; account + target + version
 * prevent another account/site/notice from inheriting this decision. */
export function hasShareLocalTestConsent(
  accountId: string,
  storage?: Pick<Storage, "getItem">,
  targetOrigin = SHARE_LOCAL_TEST_ORIGIN,
): boolean {
  if (!accountId) return false;
  try {
    return (storage ?? window.localStorage).getItem(consentKey(accountId, targetOrigin)) === "accepted";
  } catch {
    return false;
  }
}

export function rememberShareLocalTestConsent(
  accountId: string,
  storage?: Pick<Storage, "setItem">,
  targetOrigin = SHARE_LOCAL_TEST_ORIGIN,
): boolean {
  if (!accountId) return false;
  try {
    (storage ?? window.localStorage).setItem(consentKey(accountId, targetOrigin), "accepted");
    return true;
  } catch {
    return false;
  }
}

export function forgetShareLocalTestConsent(
  accountId: string,
  storage?: Pick<Storage, "removeItem">,
  targetOrigin = SHARE_LOCAL_TEST_ORIGIN,
): void {
  if (!accountId) return;
  try {
    (storage ?? window.localStorage).removeItem(consentKey(accountId, targetOrigin));
  } catch {
    // Unavailable storage must not prevent using the explicit confirmation.
  }
}
