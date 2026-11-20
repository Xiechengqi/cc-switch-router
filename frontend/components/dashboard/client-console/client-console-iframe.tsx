"use client";

import * as React from "react";
import {
  type ConsoleWindow,
  consoleWindowKindLabelKey,
} from "@/components/dashboard/client-console/client-console-manager";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { authFetch, readAuthState } from "@/lib/auth";

export function ClientConsoleIframe({ window, paused = false }: { window: ConsoleWindow; paused?: boolean }) {
  const { t } = useLocaleText();
  const frameRef = React.useRef<HTMLIFrameElement>(null);
  const [src, setSrc] = React.useState<string>();

  React.useEffect(() => {
    let target: URL;
    try { target = new URL(window.url); }
    catch { setSrc(window.url); return; }
    target.searchParams.set("embed", "1");
    target.searchParams.set("routerOrigin", globalThis.window.location.origin);
    // Authentication finishes before the terminal navigation reaches its gate.
    target.searchParams.set("view", "providers");
    setSrc(target.toString());
    const origin = target.origin;
    let active = true;
    let handshakeSeen = false;
    let login = readAuthState();
    const pending = new Map<string, Promise<unknown>>();
    const onMessage = (event: MessageEvent) => {
      if (event.source !== frameRef.current?.contentWindow || event.origin !== origin
        || event.data?.type !== "cc-client-login-request" || typeof event.data.nonce !== "string"
        || !/^[0-9a-f-]{36}$/i.test(event.data.nonce)) return;
      const nonce = event.data.nonce;
      handshakeSeen = true;
      if (pending.has(nonce)) return;
      const request = (async () => {
        try {
          const response = await authFetch("/v1/auth/client-web-tickets", {
            method: "POST", headers: { "content-type": "application/json" },
            body: JSON.stringify({ installationId: window.clientId, kind: window.kind, nonce }),
          });
          const data = response.ok ? await response.json() : null;
          if (active && event.source === frameRef.current?.contentWindow) {
            frameRef.current?.contentWindow?.postMessage({
              type: "cc-client-login-ticket", nonce, kind: window.kind,
              ticket: data?.origin === origin ? data.ticket : null,
            }, origin);
          }
        } catch {
          if (active) frameRef.current?.contentWindow?.postMessage({ type: "cc-client-login-ticket", nonce, kind: window.kind, ticket: null }, origin);
        } finally { pending.delete(nonce); }
      })();
      pending.set(nonce, request);
    };
    const onAuthChanged = () => {
      const next = readAuthState();
      if (login.email !== next.email || login.loginEpoch !== next.loginEpoch || (!!login.accessToken && !next.accessToken)) {
        frameRef.current?.contentWindow?.postMessage({ type: "cc-client-login-reset" }, origin);
      }
      login = next;
    };
    globalThis.window.addEventListener("message", onMessage);
    globalThis.window.addEventListener("router-auth-changed", onAuthChanged);
    globalThis.window.addEventListener("storage", onAuthChanged);
    const legacyFallback = globalThis.window.setTimeout(() => {
      if (active && !handshakeSeen && window.kind === "terminal") setSrc(window.url);
    }, 5_000);
    return () => {
      active = false;
      globalThis.window.clearTimeout(legacyFallback);
      globalThis.window.removeEventListener("message", onMessage);
      globalThis.window.removeEventListener("router-auth-changed", onAuthChanged);
      globalThis.window.removeEventListener("storage", onAuthChanged);
    };
  }, [window.clientId, window.kind, window.url, window.reloadKey]);

  return (
    <iframe
      key={`${window.id}-${window.reloadKey}`}
      ref={frameRef}
      src={src}
      title={`${t(consoleWindowKindLabelKey(window.kind))} ${window.title}`}
      className="h-full w-full rounded-xl border-0 bg-white"
      allow="clipboard-read; clipboard-write"
      sandbox="allow-downloads allow-forms allow-modals allow-popups allow-same-origin allow-scripts"
      referrerPolicy="same-origin"
      aria-hidden={paused}
      tabIndex={paused ? -1 : undefined}
      style={paused ? { contentVisibility: "hidden" } : undefined}
    />
  );
}
