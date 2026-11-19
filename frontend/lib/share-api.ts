import { parseRequestMetering } from "./share-request-metering";
import { parseGrantCapabilityPreview, type GrantCapabilityInput } from "./share-grant-capabilities";
import { parseCapabilityPreview } from "./share-capabilities";
import { parseJson } from "@/lib/api";
import type { ShareApiAuthResponse, ShareApiContextResponse, ShareApiShareResponse, ShareSettingsPatch, ShareEditView } from "@/lib/types";

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

function assertCurrentShareCredentials(started: ReturnType<typeof readShareApiCredentials>) {
  const current = readShareApiCredentials();
  if (started.email !== current.email || started.token !== current.token) {
    throw new DOMException("Share sign-in session changed", "AbortError");
  }
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
  const started = readShareApiCredentials();
  const result = await parseShareJson<ShareApiAuthResponse>(
    await fetch(`/share-api/auth/me${shareEmailParam(email)}`, {
      cache: "no-store",
      headers: shareAuthHeaders(token),
    }),
  );
  assertCurrentShareCredentials(started);
  return result;
}

export async function getSharePageShare(email?: string, token?: string) {
  const started = readShareApiCredentials();
  const result = await parseShareJson<ShareApiShareResponse>(
    await fetch(`/share-api/share${shareEmailParam(email)}`, {
      cache: "no-store",
      headers: shareAuthHeaders(token),
    }),
  );
  assertCurrentShareCredentials(started);
  return result;
}

export async function updateSharePageSettings(
  patch: ShareSettingsPatch,
  baseConfigRevision?: number,
  operationId?: string,
  started = readShareApiCredentials(),
) {
  assertCurrentShareCredentials(started);
  const result = await parseShareJson<{ ok: boolean; edit: ShareEditView; appliedSynchronously: boolean }>(
    await fetch(`/share-api/share/settings${shareEmailParam(started.email)}`, {
      method: "PATCH",
      headers: {
        "Content-Type": "application/json",
        ...shareAuthHeaders(started.token),
      },
      body: JSON.stringify({ patch, baseConfigRevision, operationId }),
    }),
  );
  assertCurrentShareCredentials(started);
  if (operationId && result?.edit?.id !== operationId) throw new Error("Invalid Share settings response");
  return result;
}

export async function getSharePageSettingsEdit(editId: string) {
  const { email } = readShareApiCredentials();
  return parseShareJson<ShareEditView>(await fetch(
    `/share-api/share/settings-edits/${encodeURIComponent(editId)}${shareEmailParam(email)}`,
    { headers: shareAuthHeaders(), cache: "no-store" },
  ));
}

async function fetchCapabilityPreview(_shareId:string,input:unknown,signal?:AbortSignal):Promise<unknown> {
  const started=readShareApiCredentials();
  const result=await parseShareJson<import("./share-capabilities").CapabilityPreview>(await fetch(`/share-api/share/capability-preview${shareEmailParam(started.email)}`,{method:"POST",headers:{"Content-Type":"application/json",...shareAuthHeaders(started.token)},body:JSON.stringify(input),signal,cache:"no-store"}));
  assertCurrentShareCredentials(started);return result;
}

export async function previewSharePageCapabilities(shareId: string, input: unknown, signal?: AbortSignal) {
  return parseCapabilityPreview(await fetchCapabilityPreview(shareId, input, signal), shareId, input);
}

export async function previewSharePageGrantCapabilities(shareId: string, input: GrantCapabilityInput, signal?: AbortSignal) {
  return parseGrantCapabilityPreview(await fetchCapabilityPreview(shareId, input, signal), shareId, input);
}

export async function getSharePageRequestMetering(shareId: string, requestId: string, signal?: AbortSignal) {
  const started = readShareApiCredentials();
  const response = await fetch(`/share-api/share/request-metering${shareEmailParam(started.email)}`, {
    method: "POST", headers: { ...shareAuthHeaders(started.token), "Content-Type": "application/json" },
    body: JSON.stringify({requestId}), signal, cache: "no-store",
  });
  const result = await parseShareJson<unknown>(response);
  assertCurrentShareCredentials(started);
  return parseRequestMetering(result, shareId, requestId);
}

export async function getSharePageRequestLogs(signal?: AbortSignal) {
  const started = readShareApiCredentials();
  const result = await parseShareJson<import("./types").ShareRequestLogsPage>(
    await fetch(`/share-api/share/request-logs${shareEmailParam(started.email)}`, {
      headers: shareAuthHeaders(started.token), signal, cache: "no-store",
    }),
  );
  assertCurrentShareCredentials(started);
  return result.logs;
}
