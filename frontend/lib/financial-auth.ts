/** Financial signatures use exact request bytes; the device key is possession
 * proof. Independent user verification is supplied only by a wallet Passkey. */
export type FinancialDevice = { authDeviceId: string; privateKey: string };
const encoder = new TextEncoder();
function bytes64(bytes: Uint8Array) { return btoa(Array.from(bytes, b => String.fromCharCode(b)).join("")); }
function decode64(value: string) { return Uint8Array.from(atob(value.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)); }
function url64(value: ArrayBuffer) { return bytes64(new Uint8Array(value)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""); }
async function sha256(value: BufferSource) { return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", value)), b => b.toString(16).padStart(2, "0")).join(""); }
function requestUrl(input: RequestInfo | URL) { return new URL(input instanceof Request ? input.url : String(input), typeof location === "undefined" ? "https://router.invalid" : location.href); }
export function isFinancialWrite(input: RequestInfo | URL, init: RequestInit = {}) {
  const method = (init.method || (input instanceof Request ? input.method : "GET")).toUpperCase();
  const url = requestUrl(input);
  if (typeof location !== "undefined" && url.origin !== location.origin) return false;
  return !["GET", "HEAD", "OPTIONS"].includes(method) && (url.pathname.startsWith("/v1/market-wallet/") || url.pathname.startsWith("/v1/admin/market-") || url.pathname.startsWith("/v1/market-provider-finance/"));
}
type CredentialOptions = { publicKey: Record<string, unknown> };
export function passkeyRequestOptions(options: CredentialOptions): CredentialRequestOptions {
  const key = options.publicKey;
  return { publicKey: { ...key, challenge: decode64(key.challenge as string), allowCredentials: (key.allowCredentials as { id: string; type: PublicKeyCredentialType; transports?: AuthenticatorTransport[] }[] | undefined)?.map(c => ({ ...c, id: decode64(c.id) })) } as PublicKeyCredentialRequestOptions };
}
export function passkeyCreationOptions(options: CredentialOptions): CredentialCreationOptions {
  const key = options.publicKey;
  return { publicKey: { ...key, challenge: decode64(key.challenge as string), user: { ...(key.user as object), id: decode64((key.user as { id: string }).id) }, excludeCredentials: (key.excludeCredentials as { id: string; type: PublicKeyCredentialType }[] | undefined)?.map(c => ({ ...c, id: decode64(c.id) })) } as unknown as PublicKeyCredentialCreationOptions };
}
export function serializePasskey(credential: PublicKeyCredential) {
  const response = credential.response;
  return { id: credential.id, rawId: url64(credential.rawId), type: credential.type, extensions: credential.getClientExtensionResults(), response: response instanceof AuthenticatorAttestationResponse ? { attestationObject: url64(response.attestationObject), clientDataJSON: url64(response.clientDataJSON), transports: response.getTransports() } : { authenticatorData: url64((response as AuthenticatorAssertionResponse).authenticatorData), clientDataJSON: url64(response.clientDataJSON), signature: url64((response as AuthenticatorAssertionResponse).signature), userHandle: (response as AuthenticatorAssertionResponse).userHandle ? url64((response as AuthenticatorAssertionResponse).userHandle!) : null } };
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
  if (response.status !== 428) return response;
  const challenge = await response.clone().json().catch(() => null);
  if (challenge?.code !== "FINANCIAL_STEPUP_REQUIRED") return response;
  if (!globalThis.PublicKeyCredential || !navigator.credentials) throw new Error("This operation requires a wallet Passkey in a supported browser.");
  guard();
  const credential = await navigator.credentials.get(passkeyRequestOptions(challenge.details.options)) as PublicKeyCredential | null;
  if (!credential) throw new Error("Financial confirmation was cancelled.");
  guard();
  const verified = await signedFinancialFetch("/v1/market-wallet/security/authenticate", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ challengeId: challenge.details.challengeId, credential: serializePasskey(credential) }) }, token, device, guard);
  const result = await verified.json().catch(() => ({}));
  if (!verified.ok) throw new Error(result.message || "Passkey verification failed.");
  guard();
  return send(result.stepupId);
}
