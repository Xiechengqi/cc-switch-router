"use client";

import type { SessionStatus } from "@/lib/types";

const AUTH_KEY = "cc_switch_router_auth_v1";
const ACCESS_TOKEN_REFRESH_SKEW_MS = 10_000;
const AUTH_REFRESH_LOCK_NAME = "cc-switch-router-auth-refresh-v1";
const FALLBACK_REFRESH_RECHECK_MS = 200;

let accessTokenRefresh: Promise<boolean> | null = null;

export type AuthState = {
  installationId?: string | null;
  publicKey?: string | null;
  privateKey?: string | null;
  email?: string | null;
  accessToken?: string | null;
  refreshToken?: string | null;
  expiresAt?: string | null;
  refreshExpiresAt?: string | null;
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

export function clearSessionTokens() {
  const state = readAuthState();
  mergeAuthState({
    installationId: state.installationId || null,
    publicKey: state.publicKey || null,
    privateKey: state.privateKey || null,
    email: null,
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

async function generateInstallationKeys() {
  const keyPair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const publicKey = bytesToBase64(new Uint8Array(await crypto.subtle.exportKey("raw", keyPair.publicKey)));
  const privateKey = bytesToBase64(new Uint8Array(await crypto.subtle.exportKey("pkcs8", keyPair.privateKey)));
  return { publicKey, privateKey };
}

async function importPrivateKey(privateKeyBase64: string) {
  return crypto.subtle.importKey("pkcs8", base64ToBytes(privateKeyBase64), { name: "Ed25519" }, false, ["sign"]);
}

async function registerInstallationIdentity(publicKey: string) {
  const response = await fetch("/v1/installations/register", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      publicKey,
      platform: platformLabel(),
      appVersion: "router-dashboard-next",
      instanceNonce: crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`,
    }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.message || `installation register failed (${response.status})`);
  return data.installationId as string;
}

export async function ensureInstallationIdentity() {
  const state = readAuthState();
  if (state.installationId && state.publicKey && state.privateKey) {
    return {
      installationId: state.installationId,
      publicKey: state.publicKey,
      privateKey: state.privateKey,
    };
  }
  const keys = await generateInstallationKeys();
  const installationId = await registerInstallationIdentity(keys.publicKey);
  const next = mergeAuthState({ installationId, publicKey: keys.publicKey, privateKey: keys.privateKey });
  return {
    installationId,
    publicKey: next.publicKey!,
    privateKey: next.privateKey!,
  };
}

export function shouldResetInstallationIdentity(message: string) {
  return /installation|public key|signature/i.test(message || "");
}

export function resetInstallationIdentityState() {
  const state = readAuthState();
  mergeAuthState({
    ...state,
    installationId: null,
    publicKey: null,
    privateKey: null,
  });
}

export async function signAuthPayload(action: string, payload: Record<string, unknown>) {
  const identity = await ensureInstallationIdentity();
  const timestampMs = Date.now();
  const nonce = crypto.randomUUID ? crypto.randomUUID() : `${timestampMs}-${Math.random()}`;
  const payloadJson = JSON.stringify(payload);
  const body = `${identity.installationId}\n${action}\n${payloadJson}\n${timestampMs}\n${nonce}`;
  const privateKey = await importPrivateKey(identity.privateKey);
  const signature = bytesToBase64(new Uint8Array(await crypto.subtle.sign("Ed25519", privateKey, new TextEncoder().encode(body))));
  return {
    installationId: identity.installationId,
    timestampMs,
    nonce,
    signature,
  };
}

export function authBearerHeaders() {
  const state = readAuthState();
  return state.accessToken ? { Authorization: `Bearer ${state.accessToken}` } : {};
}

function accessTokenExpiresSoon(state: AuthState) {
  if (!state.accessToken || !state.refreshToken || !state.installationId || !state.expiresAt) return false;
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
    current.installationId !== expected.installationId ||
    current.accessToken !== expected.accessToken ||
    current.refreshToken !== expected.refreshToken;
  if (!changed) return null;
  return !!current.installationId && !!current.accessToken && !!current.refreshToken;
}

function waitForFallbackRefreshRecheck() {
  return new Promise<void>((resolve) => setTimeout(resolve, FALLBACK_REFRESH_RECHECK_MS));
}

async function handleUnauthorizedRefresh(state: AuthState, crossTabLockHeld: boolean) {
  if (!crossTabLockHeld) await waitForFallbackRefreshRecheck();
  const changedResult = changedSessionResult(state);
  if (changedResult !== null) return changedResult;
  clearSessionTokens();
  return false;
}

async function refreshSessionWithState(state: AuthState, crossTabLockHeld: boolean) {
  if (!state.refreshToken || !state.installationId) return false;

  let response: Response;
  try {
    response = await fetch("/v1/auth/session/refresh", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        refreshToken: state.refreshToken,
        installationId: state.installationId,
      }),
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
  if (!requestedState.refreshToken || !requestedState.installationId) return false;

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

export async function authFetch(input: RequestInfo | URL, init: RequestInit = {}) {
  let state = readAuthState();
  if (accessTokenExpiresSoon(state)) {
    await refreshAccessToken();
    state = readAuthState();
  }

  const attemptedAccessToken = bearerTokenForRequest(state);
  const response = await requestWithAccessToken(input, init, attemptedAccessToken);
  if (response.status !== 401) return response;

  let currentAccessToken = bearerTokenForRequest(readAuthState());
  if (currentAccessToken && currentAccessToken !== attemptedAccessToken) {
    return requestWithAccessToken(input, init, currentAccessToken);
  }

  if (await refreshAccessToken()) {
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
  const state = readAuthState();
  const params = new URLSearchParams();
  if (state.installationId) params.set("installationId", state.installationId);
  const response = await authFetch(`/v1/auth/session/me${params.toString() ? `?${params}` : ""}`);
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

export async function requestEmailCode(email: string) {
  const signed = await signAuthPayload("auth_request_code", { email, purpose: "login" });
  const response = await fetch("/v1/auth/email/request-code", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, ...signed }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.message || `request code failed (${response.status})`);
  return data as { maskedDestination: string };
}

export async function verifyEmailCode(email: string, code: string) {
  const identity = await ensureInstallationIdentity();
  const response = await fetch("/v1/auth/email/verify-code", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, code, installationId: identity.installationId }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.message || `verify failed (${response.status})`);
  mergeAuthState({
    email: data.user?.email || email,
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    expiresAt: data.expiresAt,
    refreshExpiresAt: data.refreshExpiresAt,
  });
  return data;
}
