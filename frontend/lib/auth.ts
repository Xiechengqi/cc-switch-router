"use client";

import type { SessionStatus } from "@/lib/types";

const AUTH_KEY = "cc_switch_router_auth_v1";

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

export async function refreshAccessToken() {
  const state = readAuthState();
  if (!state.refreshToken || !state.installationId) return false;
  const response = await fetch("/v1/auth/session/refresh", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      refreshToken: state.refreshToken,
      installationId: state.installationId,
    }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) return false;
  mergeAuthState({
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    expiresAt: data.expiresAt,
    refreshExpiresAt: data.refreshExpiresAt,
  });
  return true;
}

export async function authFetch(input: RequestInfo | URL, init: RequestInit = {}) {
  const headers = new Headers(init.headers || {});
  const bearer = authBearerHeaders();
  Object.entries(bearer).forEach(([key, value]) => headers.set(key, value));
  let response = await fetch(input, { ...init, headers });
  if (response.status === 401 && (await refreshAccessToken())) {
    const retryHeaders = new Headers(init.headers || {});
    Object.entries(authBearerHeaders()).forEach(([key, value]) => retryHeaders.set(key, value));
    response = await fetch(input, { ...init, headers: retryHeaders });
  }
  return response;
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
