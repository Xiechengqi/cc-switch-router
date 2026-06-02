import { parseJson } from "@/lib/api";
import type { MarketsResponse, ShareApiAuthResponse, ShareApiContextResponse, ShareApiShareResponse, ShareSettingsPatch, ShareEditView } from "@/lib/types";

const TOKEN_KEY = "cc_switch_share_api_token_v1";
const EMAIL_KEY = "cc_switch_share_api_email_v1";

export class ShareApiError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function parseShareJson<T>(response: Response): Promise<T> {
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new ShareApiError(response.status, data?.message || `HTTP ${response.status}`);
  }
  return data as T;
}

export function readShareApiCredentials() {
  if (typeof sessionStorage === "undefined") return { email: "", token: "" };
  return {
    email: sessionStorage.getItem(EMAIL_KEY) || "",
    token: sessionStorage.getItem(TOKEN_KEY) || "",
  };
}

export function writeShareApiCredentials(email: string, token: string) {
  sessionStorage.setItem(EMAIL_KEY, email.trim().toLowerCase());
  sessionStorage.setItem(TOKEN_KEY, token.trim());
}

export function clearShareApiCredentials() {
  sessionStorage.removeItem(EMAIL_KEY);
  sessionStorage.removeItem(TOKEN_KEY);
}

function shareAuthHeaders(token?: string): Record<string, string> {
  const actual = token || readShareApiCredentials().token;
  return actual ? { Authorization: `Bearer ${actual}` } : {};
}

function shareEmailParam(email?: string) {
  const actual = (email || readShareApiCredentials().email || "").trim().toLowerCase();
  return actual ? `?email=${encodeURIComponent(actual)}` : "";
}

export async function getShareContext() {
  return parseShareJson<ShareApiContextResponse>(await fetch("/share-api/context", { cache: "no-store" }));
}

export async function getShareApiAuth(email?: string, token?: string) {
  return parseShareJson<ShareApiAuthResponse>(
    await fetch(`/share-api/auth/me${shareEmailParam(email)}`, {
      cache: "no-store",
      headers: shareAuthHeaders(token),
    }),
  );
}

export async function getSharePageShare(email?: string, token?: string) {
  return parseShareJson<ShareApiShareResponse>(
    await fetch(`/share-api/share${shareEmailParam(email)}`, {
      cache: "no-store",
      headers: shareAuthHeaders(token),
    }),
  );
}

export async function updateSharePageSettings(patch: ShareSettingsPatch) {
  const { email } = readShareApiCredentials();
  return parseShareJson<{ ok: boolean; edit: ShareEditView; appliedSynchronously: boolean }>(
    await fetch(`/share-api/share/settings${shareEmailParam(email)}`, {
      method: "PATCH",
      headers: {
        "Content-Type": "application/json",
        ...shareAuthHeaders(),
      },
      body: JSON.stringify({ patch }),
    }),
  );
}

export async function getPublicMarkets() {
  return parseJson<MarketsResponse>(await fetch("/v1/markets", { cache: "no-store" }));
}
