import type { ProviderModelProbe } from "./types";

export const SHARE_LOCAL_TEST_ORIGIN = "https://llm-api-test.xiecq.top";

export type ShareLocalTestInput = {
  app: string;
  operation: string;
  baseUrl: string;
  apiToken: string;
  probe?: ProviderModelProbe;
  authenticated: boolean;
  canExecute: boolean;
  isBound: boolean;
  tokenLoading?: boolean;
  tokenError?: boolean;
};

function shareEndpoint(baseUrl: string): URL | null {
  try {
    const base = new URL(baseUrl);
    if (base.protocol !== "https:" || base.username || base.password || base.search || base.hash
      || !["", "/"].includes(base.pathname)) return null;
    return new URL("/v1/chat/completions", base);
  } catch {
    return null;
  }
}

export function shareLocalTestUnavailable(input: ShareLocalTestInput) {
  if (!input.authenticated) return "dashboard.connectDialog.test.needAuth" as const;
  if (!input.canExecute) return "dashboard.connectDialog.test.needPermission" as const;
  if (!input.isBound) return "dashboard.connectDialog.test.notBound" as const;
  if (input.operation !== "text" || input.app !== "codex") {
    return "dashboard.connectDialog.localTest.unsupported" as const;
  }
  if (!input.probe?.requestedModel.trim()) return "dashboard.connectDialog.test.probeUnavailable" as const;
  if (input.probe.apiType !== "openai") return "dashboard.connectDialog.localTest.unsupported" as const;
  if (input.tokenLoading) return "dashboard.connectDialog.localTest.keyLoading" as const;
  if (input.tokenError || !input.apiToken || /[\s\u0000-\u001f\u007f•]/u.test(input.apiToken)) {
    return "dashboard.connectDialog.localTest.keyUnavailable" as const;
  }
  if (!shareEndpoint(input.baseUrl)) return "dashboard.connectDialog.localTest.apiUnavailable" as const;
  return null;
}

/** Build only inside the explicit user gesture. Never put this URL in
 * rendered hrefs, application history, storage, error messages, or telemetry. */
export function buildShareLocalTestUrl(input: ShareLocalTestInput): string | null {
  if (shareLocalTestUnavailable(input)) return null;
  const url = new URL("/", SHARE_LOCAL_TEST_ORIGIN);
  const fragment = new URLSearchParams();
  fragment.set("api", shareEndpoint(input.baseUrl)!.href);
  fragment.set("key", input.apiToken);
  // This is the model shown to the caller, not the Provider's wire/upstream model.
  fragment.set("model", input.probe!.requestedModel);
  // Fragments stay in the browser; unlike query parameters they are not sent
  // with the initial request to the static host/CDN. The target JS can read it.
  url.hash = fragment.toString();
  return url.href;
}

export function openShareLocalTest(input: ShareLocalTestInput, isCurrent: () => boolean): boolean {
  if (!isCurrent()) return false;
  const url = buildShareLocalTestUrl(input);
  if (!url) return false;
  // A synchronous user gesture avoids popup blocking caused by awaiting a Key
  // request here. A null result with noopener does NOT prove a blocked popup.
  try {
    window.open(url, "_blank", "noopener,noreferrer");
    return true;
  } catch {
    // Browser exception messages may contain the full URL and therefore Key.
    return false;
  }
}
