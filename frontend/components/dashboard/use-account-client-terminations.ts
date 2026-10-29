"use client";

import * as React from "react";
import { getClientTerminations, type ClientTerminationPage } from "@/lib/client-termination";

/** One paginated history per account/role, never one request per rental row. */
export function useAccountClientTerminations(actor: string | undefined, role: "buyer" | "provider", focus?: string | null) {
  const [page, setPage] = React.useState<ClientTerminationPage>({ items: [], nextCursor: null });
  const [error, setError] = React.useState("");
  const [loading, setLoading] = React.useState(false);
  const [loadedScope, setLoadedScope] = React.useState("");
  const scope = `${actor}:${role}`;
  const pageRef = React.useRef(page);
  const request = React.useRef<AbortController | null>(null);
  const focusCursors = React.useRef(new Set<string>());
  const load = React.useCallback(async (more = false) => {
    if (!actor || request.current) return;
    const controller = new AbortController(); request.current = controller;
    const previous = pageRef.current;
    if (more && !previous.nextCursor) { request.current = null; return; }
    setLoading(true);
    try {
      let next = await getClientTerminations(role, more ? previous.nextCursor! : undefined, controller.signal);
      if (!Array.isArray(next.items)) throw new Error("Invalid termination records");
      const items = [...next.items];
      const anchor = more ? null : previous.items.at(-1)?.id;
      const seen = new Set<string>();
      while (!controller.signal.aborted && anchor && next.nextCursor && !items.some(item => item.id === anchor)) {
        if (seen.has(next.nextCursor)) throw new Error("Repeated termination cursor");
        seen.add(next.nextCursor);
        next = await getClientTerminations(role, next.nextCursor, controller.signal);
        items.push(...next.items);
      }
      if (controller.signal.aborted) return;
      const unique = new Map((more ? [...previous.items, ...items] : items).map(item => [item.id, item]));
      const value = { items: [...unique.values()], nextCursor: next.nextCursor };
      pageRef.current = value; setPage(value); setLoadedScope(`${actor}:${role}`); setError("");
    } catch (cause) {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (request.current === controller) { request.current = null; setLoading(false); }
    }
  }, [actor, role]);
  React.useEffect(() => {
    pageRef.current = { items: [], nextCursor: null }; setPage(pageRef.current); setError("");
    request.current?.abort(); request.current = null; focusCursors.current.clear();
    void load();
    const refresh = () => { if (document.visibilityState === "visible") void load(); };
    const timer = window.setInterval(refresh, 20_000);
    window.addEventListener("focus", refresh);
    return () => { request.current?.abort(); request.current = null; window.clearInterval(timer); window.removeEventListener("focus", refresh); };
  }, [load]);
  React.useEffect(() => { focusCursors.current.clear(); }, [focus]);
  React.useEffect(() => {
    if (!focus || loading || !page.nextCursor || page.items.some(item => item.installationId === focus)
      || focusCursors.current.has(page.nextCursor)) return;
    focusCursors.current.add(page.nextCursor); void load(true);
  }, [focus, page, loading, load]);
  return { ...(loadedScope === scope ? page : { items: [], nextCursor: null }), error, loading, reload: () => load(), more: () => load(true) };
}
