"use client";

import type { AuthResult, SessionStatus } from "@/lib/types";
import { sameLoginContext } from "@/lib/password-auth";

const AUTH_KEY = "cc_switch_router_auth_v2";
const PROTOCOL_EPOCH = "namespace-flat-1";
const ACCESS_TOKEN_REFRESH_SKEW_MS = 10_000;
const AUTH_REFRESH_LOCK_NAME = "cc-switch-router-auth-refresh-v2";
const AUTH_DEVICE_IDENTITY_LOCK_NAME = "cc-switch-router-auth-device-identity-v1";
const FALLBACK_REFRESH_RECHECK_MS = 200;

let accessTokenRefresh: Promise<boolean> | null = null;
let authDeviceIdentityInitialization: Promise<AuthDeviceIdentity> | null = null;

export type AuthState = {
  authDeviceId?: string | null;
  publicKey?: string | null;
  privateKey?: string | null;
  email?: string | null;
  loginEpoch?: string | null;
  accessToken?: string | null;
  refreshToken?: string | null;
  expiresAt?: string | null;
  refreshExpiresAt?: string | null;
};

export type AuthDeviceIdentity = {
  authDeviceId: string;
  publicKey: string;
  privateKey: string;
};

function isBrowser() {
  return typeof window !== "undefined" && typeof localStorage !== "undefined";
}

export function readAuthState(): AuthState {
  if (!isBrowser()) return {};
  try {
    return JSON.parse(localStorage.getItem(AUTH_KEY) || "{}") || {};
  } catch {
    return {};
  }
}

export function writeAuthState(state: AuthState) {
  if (!isBrowser()) return;
  localStorage.setItem(AUTH_KEY, JSON.stringify(state));
}

export function mergeAuthState(patch: AuthState) {
  const next = { ...readAuthState(), ...patch };
  writeAuthState(next);
  window.dispatchEvent(new CustomEvent("router-auth-changed", { detail: next }));
  return next;
}

export function clearSessionTokens({ preserveLoginEpoch = false }: { preserveLoginEpoch?: boolean } = {}) {
  const state = readAuthState();
  mergeAuthState({
    authDeviceId: state.authDeviceId || null,
    publicKey: state.publicKey || null,
    privateKey: state.privateKey || null,
    email: preserveLoginEpoch ? state.email : null,
    loginEpoch: preserveLoginEpoch ? state.loginEpoch : null,
    accessToken: null,
    refreshToken: null,
    expiresAt: null,
    refreshExpiresAt: null,
  });
}

function bytesToBase64(bytes: Uint8Array) {
  let binary = "";
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return btoa(binary);
}

function base64ToBytes(value: string) {
  return Uint8Array.from(atob(value), (ch) => ch.charCodeAt(0));
}

function platformLabel() {
  const ua = navigator.userAgent || "";
  if (/Mac/i.test(ua)) return "web-macos";
  if (/Windows/i.test(ua)) return "web-windows";
  if (/Linux/i.test(ua)) return "web-linux";
  return "web";
}

async function generateAuthDeviceKeys() {
  const keyPair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const publicKey = bytesToBase64(new Uint8Array(await crypto.subtle.exportKey("raw", keyPair.publicKey)));
  const privateKey = bytesToBase64(new Uint8Array(await crypto.subtle.exportKey("pkcs8", keyPair.privateKey)));
  return { publicKey, privateKey };
}

async function importPrivateKey(privateKeyBase64: string) {
  return crypto.subtle.importKey("pkcs8", base64ToBytes(privateKeyBase64), { name: "Ed25519" }, false, ["sign"]);
}

async function signCanonicalPayload(
  privateKeyBase64: string,
  authDeviceId: string,
  action: string,
  payloadJson: string,
  timestampMs: number,
  nonce: string,
) {
  const body = `${PROTOCOL_EPOCH}\n${authDeviceId}\n${action}\n${payloadJson}\n${timestampMs}\n${nonce}`;
  const privateKey = await importPrivateKey(privateKeyBase64);
  return bytesToBase64(
    new Uint8Array(await crypto.subtle.sign("Ed25519", privateKey, new TextEncoder().encode(body))),
  );
}

async function registerAuthDeviceIdentity(publicKey: string, privateKey: string) {
  const platform = platformLabel();
  const appVersion = "router-dashboard-next";
  const kind = "browser";
  const instanceNonce = crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`;
  const timestampMs = Date.now();
  const registrationCanonical = `${PROTOCOL_EPOCH}\nregister_auth_device\n${publicKey}\n${kind}\n${platform}\n${appVersion}\n${instanceNonce}\n${timestampMs}`;
  const registrationSignature = bytesToBase64(
    new Uint8Array(
      await crypto.subtle.sign(
        "Ed25519",
        await importPrivateKey(privateKey),
        new TextEncoder().encode(registrationCanonical),
      ),
    ),
  );
  const response = await fetch("/v1/auth/devices/register", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      protocolEpoch: PROTOCOL_EPOCH,
      publicKey,
      kind,
      platform,
      appVersion,
      instanceNonce,
      timestampMs,
      signature: registrationSignature,
    }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.message || `auth device register failed (${response.status})`);
  return data.authDeviceId as string;
}

function authDeviceIdentityFromState(state: AuthState): AuthDeviceIdentity | null {
  if (state.authDeviceId && state.publicKey && state.privateKey) {
    return {
      authDeviceId: state.authDeviceId,
      publicKey: state.publicKey,
      privateKey: state.privateKey,
    };
  }
  return null;
}

async function createAuthDeviceIdentityIfMissing(): Promise<AuthDeviceIdentity> {
  const existing = authDeviceIdentityFromState(readAuthState());
  if (existing) return existing;

  const keys = await generateAuthDeviceKeys();
  const authDeviceId = await registerAuthDeviceIdentity(keys.publicKey, keys.privateKey);

  const identityCreatedElsewhere = authDeviceIdentityFromState(readAuthState());
  if (identityCreatedElsewhere) return identityCreatedElsewhere;

  const next = mergeAuthState({ authDeviceId, publicKey: keys.publicKey, privateKey: keys.privateKey });
  return {
    authDeviceId,
    publicKey: next.publicKey!,
    privateKey: next.privateKey!,
  };
}

async function initializeAuthDeviceIdentity(): Promise<AuthDeviceIdentity> {
  const lockManager = typeof navigator !== "undefined" ? navigator.locks : undefined;
  if (!lockManager) return createAuthDeviceIdentityIfMissing();

  let lockCallbackStarted = false;
  try {
    return await lockManager.request(AUTH_DEVICE_IDENTITY_LOCK_NAME, async () => {
      lockCallbackStarted = true;
      return createAuthDeviceIdentityIfMissing();
    });
  } catch (error) {
    if (lockCallbackStarted) throw error;
    return createAuthDeviceIdentityIfMissing();
  }
}

export async function ensureAuthDeviceIdentity(): Promise<AuthDeviceIdentity> {
  const existing = authDeviceIdentityFromState(readAuthState());
  if (existing) return existing;
  if (authDeviceIdentityInitialization) return authDeviceIdentityInitialization;

  const pending = initializeAuthDeviceIdentity();
  authDeviceIdentityInitialization = pending;
  try {
    return await pending;
  } finally {
    if (authDeviceIdentityInitialization === pending) authDeviceIdentityInitialization = null;
  }
}

function shouldResetAuthDeviceIdentity(message: string) {
  return /auth device|public key|signature/i.test(message || "");
}

async function replaceAuthDeviceIdentity(
  expectedAuthDeviceId: string,
): Promise<AuthDeviceIdentity> {
  const replaceIfCurrent = async () => {
    const current = authDeviceIdentityFromState(readAuthState());
    if (current && current.authDeviceId !== expectedAuthDeviceId) return current;
    mergeAuthState({
      authDeviceId: null,
      publicKey: null,
      privateKey: null,
    });
    return createAuthDeviceIdentityIfMissing();
  };

  const lockManager = typeof navigator !== "undefined" ? navigator.locks : undefined;
  if (!lockManager) return replaceIfCurrent();

  let lockCallbackStarted = false;
  try {
    return await lockManager.request(AUTH_DEVICE_IDENTITY_LOCK_NAME, async () => {
      lockCallbackStarted = true;
      return replaceIfCurrent();
    });
  } catch (error) {
    if (lockCallbackStarted) throw error;
    return replaceIfCurrent();
  }
}

async function signAuthPayloadWithIdentity(
  identity: AuthDeviceIdentity,
  action: string,
  payload: Record<string, unknown>,
) {
  const timestampMs = Date.now();
  const nonce = crypto.randomUUID ? crypto.randomUUID() : `${timestampMs}-${Math.random()}`;
  const payloadJson = JSON.stringify(payload);
  const signature = await signCanonicalPayload(
    identity.privateKey,
    identity.authDeviceId,
    action,
    payloadJson,
    timestampMs,
    nonce,
  );
  return {
    authDeviceId: identity.authDeviceId,
    timestampMs,
    nonce,
    signature,
  };
}

export async function signAuthPayload(action: string, payload: Record<string, unknown>) {
  const identity = await ensureAuthDeviceIdentity();
  return signAuthPayloadWithIdentity(identity, action, payload);
}

export function authBearerHeaders() {
  const state = readAuthState();
  return state.accessToken ? { Authorization: `Bearer ${state.accessToken}` } : {};
}

function accessTokenExpiresSoon(state: AuthState) {
  if (!state.accessToken || !state.refreshToken || !state.expiresAt) return false;
  const expiresAt = Date.parse(state.expiresAt);
  return Number.isFinite(expiresAt) && expiresAt <= Date.now() + ACCESS_TOKEN_REFRESH_SKEW_MS;
}

function accessTokenIsExpired(state: AuthState) {
  if (!state.accessToken || !state.expiresAt) return false;
  const expiresAt = Date.parse(state.expiresAt);
  return Number.isFinite(expiresAt) && expiresAt <= Date.now();
}

function bearerTokenForRequest(state: AuthState) {
  if (!state.accessToken || accessTokenIsExpired(state)) return null;
  return state.accessToken;
}

function changedSessionResult(expected: AuthState, current = readAuthState()): boolean | null {
  const changed =
    current.accessToken !== expected.accessToken ||
    current.refreshToken !== expected.refreshToken;
  if (!changed) return null;
  return !!current.accessToken && !!current.refreshToken;
}

function waitForFallbackRefreshRecheck() {
  return new Promise<void>((resolve) => setTimeout(resolve, FALLBACK_REFRESH_RECHECK_MS));
}

async function handleUnauthorizedRefresh(state: AuthState, crossTabLockHeld: boolean) {
  if (!crossTabLockHeld) await waitForFallbackRefreshRecheck();
  const changedResult = changedSessionResult(state);
  if (changedResult !== null) return changedResult;
  // Server-side expiry/revocation is not an explicit logout intent. Keep the
  // context marker so an in-flight successful password change can accept its
  // replacement tokens; the server still validates every credential version.
  clearSessionTokens({ preserveLoginEpoch: true });
  return false;
}

async function refreshSessionWithState(state: AuthState, crossTabLockHeld: boolean) {
  if (!state.refreshToken) return false;

  let response: Response;
  try {
    response = await fetch("/v1/auth/session/refresh", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refreshToken: state.refreshToken }),
    });
  } catch {
    return changedSessionResult(state) ?? false;
  }

  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    if (response.status === 401) return handleUnauthorizedRefresh(state, crossTabLockHeld);
    return changedSessionResult(state) ?? false;
  }

  const changedResult = changedSessionResult(state);
  if (changedResult !== null) return changedResult;
  if (
    typeof data.accessToken !== "string" ||
    typeof data.refreshToken !== "string" ||
    typeof data.expiresAt !== "string" ||
    typeof data.refreshExpiresAt !== "string"
  ) {
    return false;
  }
  mergeAuthState({
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    expiresAt: data.expiresAt,
    refreshExpiresAt: data.refreshExpiresAt,
  });
  return true;
}

async function performAccessTokenRefresh() {
  const requestedState = readAuthState();
  if (!requestedState.refreshToken) return false;

  const lockManager = typeof navigator !== "undefined" ? navigator.locks : undefined;
  if (!lockManager) return refreshSessionWithState(requestedState, false);

  try {
    return await lockManager.request(AUTH_REFRESH_LOCK_NAME, async () => {
      const lockedState = readAuthState();
      const changedResult = changedSessionResult(requestedState, lockedState);
      if (changedResult !== null) return changedResult;
      return refreshSessionWithState(lockedState, true);
    });
  } catch {
    const fallbackState = readAuthState();
    const changedResult = changedSessionResult(requestedState, fallbackState);
    if (changedResult !== null) return changedResult;
    return refreshSessionWithState(fallbackState, false);
  }
}

export async function refreshAccessToken() {
  if (accessTokenRefresh) return accessTokenRefresh;
  const pending = performAccessTokenRefresh();
  accessTokenRefresh = pending;
  try {
    return await pending;
  } finally {
    if (accessTokenRefresh === pending) accessTokenRefresh = null;
  }
}

function requestWithAccessToken(input: RequestInfo | URL, init: RequestInit, accessToken?: string | null) {
  const headers = new Headers(init.headers || {});
  if (accessToken) headers.set("Authorization", `Bearer ${accessToken}`);
  return fetch(input, { ...init, headers });
}

function checkLoginContext(expected: AuthState | undefined, current = readAuthState()) {
  if (expected && !sameLoginContext(expected, current)) {
    throw new AuthError("Your sign-in session changed. Sign in again.", "password_session_changed");
  }
}

export async function authFetch(input: RequestInfo | URL, init: RequestInit = {}, expectedContext?: AuthState) {
  let state = readAuthState();
  checkLoginContext(expectedContext, state);
  if (accessTokenExpiresSoon(state)) {
    await refreshAccessToken();
    state = readAuthState();
    checkLoginContext(expectedContext, state);
  }

  const attemptedAccessToken = bearerTokenForRequest(state);
  const response = await requestWithAccessToken(input, init, attemptedAccessToken);
  if (response.status !== 401) return response;

  checkLoginContext(expectedContext);
  let currentAccessToken = bearerTokenForRequest(readAuthState());
  if (currentAccessToken && currentAccessToken !== attemptedAccessToken) {
    return requestWithAccessToken(input, init, currentAccessToken);
  }

  const refreshed = await refreshAccessToken();
  checkLoginContext(expectedContext);
  if (refreshed) {
    currentAccessToken = bearerTokenForRequest(readAuthState());
    if (currentAccessToken) return requestWithAccessToken(input, init, currentAccessToken);
  }

  // Another tab may have completed token rotation while this tab's refresh was rejected.
  currentAccessToken = bearerTokenForRequest(readAuthState());
  if (currentAccessToken && currentAccessToken !== attemptedAccessToken) {
    return requestWithAccessToken(input, init, currentAccessToken);
  }
  return requestWithAccessToken(input, init, null);
}

export async function sessionStatus(): Promise<SessionStatus> {
  const response = await authFetch("/v1/auth/session/me");
  if (!response.ok) return { authenticated: false, isAdmin: false };
  return response.json();
}

export async function logoutSession() {
  const headers = new Headers();
  Object.entries(authBearerHeaders()).forEach(([key, value]) => headers.set(key, value));
  await fetch("/v1/auth/session/logout", {
    method: "POST",
    headers,
  }).catch(() => undefined);
}

async function requestEmailCodeWithIdentity(
  email: string,
  identity: AuthDeviceIdentity,
) {
  const signed = await signAuthPayloadWithIdentity(identity, "auth_request_code", { email, purpose: "login" });
  const response = await fetch("/v1/auth/email/request-code", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, ...signed }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.message || `request code failed (${response.status})`);
  return {
    ...(data as { maskedDestination: string }),
    identity,
  };
}

export async function requestEmailCode(email: string) {
  let identity = await ensureAuthDeviceIdentity();
  try {
    return await requestEmailCodeWithIdentity(email, identity);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!shouldResetAuthDeviceIdentity(message)) throw error;
    identity = await replaceAuthDeviceIdentity(identity.authDeviceId);
    return requestEmailCodeWithIdentity(email, identity);
  }
}

export async function verifyEmailCode(email: string, code: string, identity: AuthDeviceIdentity): Promise<AuthResult> {
  const started = readAuthState();
  const response = await fetch("/v1/auth/email/verify-code", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, code, authDeviceId: identity.authDeviceId }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.message || `verify failed (${response.status})`);
  return acceptAuthResult(data, identity, started);
}

export class AuthError extends Error {
  constructor(message: string, public readonly code?: string) { super(message); }
}

function acceptAuthResult(data: AuthResult, identity: AuthDeviceIdentity | undefined, started: AuthState) {
  checkLoginContext(started);
  mergeAuthState({
    ...(identity || {}), email: data.user.email,
    loginEpoch: crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`,
    accessToken: data.accessToken, refreshToken: data.refreshToken,
    expiresAt: data.expiresAt, refreshExpiresAt: data.refreshExpiresAt,
  });
  return data;
}

async function loginWithPasswordIdentity(email: string, password: string, identity: AuthDeviceIdentity, started: AuthState): Promise<AuthResult> {
  checkLoginContext(started);
  const signed = await signAuthPayloadWithIdentity(identity, "auth_password_login", { email, password });
  checkLoginContext(started);
  const response = await fetch("/v1/auth/password/login", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, password, ...signed }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new AuthError(data.message || "Password login failed", data.code || (response.status === 429 ? "password_rate_limited" : undefined));
  return acceptAuthResult(data, identity, started);
}

export async function loginWithPassword(email: string, password: string): Promise<AuthResult> {
  const started = readAuthState();
  let identity = await ensureAuthDeviceIdentity();
  try {
    return await loginWithPasswordIdentity(email, password, identity, started);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!(error instanceof AuthError) || error.code || !shouldResetAuthDeviceIdentity(message)) throw error;
    checkLoginContext(started);
    identity = await replaceAuthDeviceIdentity(identity.authDeviceId);
    return loginWithPasswordIdentity(email, password, identity, started);
  }
}

export async function updateLoginPassword(newPassword: string, expectedVersion: number, currentPassword?: string): Promise<AuthResult> {
  const started = readAuthState();
  const response = await authFetch("/v1/auth/password", {
    method: "PUT", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ newPassword, currentPassword, expectedVersion }),
  }, started);
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new AuthError(data.message || "Password update failed", data.code || (response.status === 429 ? "password_rate_limited" : undefined));
  return acceptAuthResult(data, undefined, started);
}
