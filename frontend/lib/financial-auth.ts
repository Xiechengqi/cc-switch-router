import { askFinancialPassword, requireFinancialPassword, type FinancialConfirmRisk } from "@/lib/financial-confirm";

/** Financial signatures use exact request bytes; the device key is possession
 * proof. Risky operations are additionally confirmed with the login password. */
export type FinancialDevice = { authDeviceId: string; privateKey: string };
const encoder = new TextEncoder();
function bytes64(bytes: Uint8Array) { return btoa(Array.from(bytes, b => String.fromCharCode(b)).join("")); }
function decode64(value: string) { return Uint8Array.from(atob(value.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)); }
async function sha256(value: BufferSource) { return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", value)), b => b.toString(16).padStart(2, "0")).join(""); }
function requestUrl(input: RequestInfo | URL) { return new URL(input instanceof Request ? input.url : String(input), typeof location === "undefined" ? "https://router.invalid" : location.href); }
export function isFinancialWrite(input: RequestInfo | URL, init: RequestInit = {}) {
  const method = (init.method || (input instanceof Request ? input.method : "GET")).toUpperCase();
  const url = requestUrl(input);
  if (typeof location !== "undefined" && url.origin !== location.origin) return false;
  return !["GET", "HEAD", "OPTIONS"].includes(method) && (url.pathname.startsWith("/v1/market-wallet/") || url.pathname.startsWith("/v1/admin/market-") || url.pathname.startsWith("/v1/market-provider-finance/"));
}
export async function signedFinancialFetch(input: RequestInfo | URL, init: RequestInit, token: string, device: FinancialDevice, guard: () => void): Promise<Response> {
  const url = requestUrl(input);
  const request = new Request(input instanceof Request ? input.clone() : url, init);
  const method = request.method;
  const bytes = await request.clone().arrayBuffer();
  const accessHash = await sha256(encoder.encode(token));
  const bodyHash = await sha256(bytes);
  const privateKey = await crypto.subtle.importKey("pkcs8", decode64(device.privateKey), { name: "Ed25519" }, false, ["sign"]);
  const send = async (stepup?: string) => {
    guard();
    const headers = new Headers(request.headers);
    headers.set("Authorization", `Bearer ${token}`);
    headers.delete("x-router-verified-financial-proof");
    const timestamp = Date.now().toString();
    const nonce = crypto.randomUUID();
    const canonical = `market-finance-v1\n${url.origin}\n${device.authDeviceId}\n${accessHash}\n${method}\n${url.pathname}${url.search}\n${bodyHash}\n${headers.get("Idempotency-Key") || ""}\n${timestamp}\n${nonce}`;
    headers.set("x-router-device", device.authDeviceId);
    headers.set("x-router-request-time", timestamp);
    headers.set("x-router-request-nonce", nonce);
    headers.set("x-router-request-signature", bytes64(new Uint8Array(await crypto.subtle.sign("Ed25519", privateKey, encoder.encode(canonical)))));
    if (stepup) headers.set("x-router-stepup", stepup); else headers.delete("x-router-stepup");
    guard();
    return fetch(request.clone(), { ...init, method, headers, body: bytes });
  };
  const response = await send();
  if (response.status === 403) {
    const denied = await response.clone().json().catch(() => null);
    // Accounts without a password are guided to set one; the request still fails.
    if (denied?.code === "FINANCIAL_PASSWORD_REQUIRED") requireFinancialPassword();
    return response;
  }
  if (response.status !== 428) return response;
  const challenge = await response.clone().json().catch(() => null);
  if (challenge?.code !== "FINANCIAL_STEPUP_REQUIRED" || challenge?.details?.method !== "password") return response;
  const challengeId = String(challenge.details.challengeId || "");
  const risk: FinancialConfirmRisk = challenge.details.risk === "immediate" ? "immediate" : "confirm";
  let error: string | undefined;
  // A mistyped password asks again for the same challenge; the server's
  // password attempt budget bounds retries.
  for (let attempt = 0; attempt < 5; attempt++) {
    guard();
    const password = await askFinancialPassword({ risk, error });
    guard();
    const verified = await signedFinancialFetch("/v1/market-wallet/security/confirm", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ challengeId, password }) }, token, device, guard);
    if (verified.ok) {
      const result = await verified.json().catch(() => ({}));
      guard();
      return send(String(result.stepupId || challengeId));
    }
    const failure = await verified.clone().json().catch(() => ({}));
    if (failure?.code !== "FINANCIAL_PASSWORD_INVALID") return verified;
    error = failure.code;
  }
  return response;
}
