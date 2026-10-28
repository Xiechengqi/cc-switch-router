"use client";
import * as React from "react";
import { getClientTerminationWorkspace, type ClientTerminationWorkspace } from "@/lib/client-termination";

export function useClientTerminationWorkspace(actor?: string) {
  const [loaded, setLoaded] = React.useState<{ actor: string; value: ClientTerminationWorkspace } | null>(null);
  const [error, setError] = React.useState(false);
  const [revision, setRevision] = React.useState(0);
  const request = React.useRef<AbortController | null>(null);
  const generation = React.useRef(0);
  const signature = React.useRef("");
  const refresh = React.useCallback(async (force = false) => {
    if (!actor || request.current && !force) return;
    request.current?.abort();
    const controller = new AbortController(); request.current = controller;
    const gen = generation.current;
    try {
      const value = await getClientTerminationWorkspace(controller.signal);
      if (controller.signal.aborted || gen !== generation.current) return;
      const nextSignature = JSON.stringify([value.hosts, value.actionRequiredCount, value.canAdjudicate]);
      if (force || signature.current !== nextSignature) { signature.current = nextSignature; setRevision(n => n + 1); }
      setLoaded({ actor, value }); setError(false);
    } catch { if (!controller.signal.aborted && gen === generation.current) setError(true); }
    finally { if (request.current === controller) request.current = null; }
  }, [actor]);
  React.useEffect(() => {
    generation.current++; request.current?.abort(); request.current = null; signature.current = "";
    setLoaded(null); setError(false); void refresh();
    const tick = () => { if (document.visibilityState === "visible") void refresh(); };
    const timer = window.setInterval(tick, 20000);
    document.addEventListener("visibilitychange", tick);
    return () => { generation.current++; request.current?.abort(); window.clearInterval(timer); document.removeEventListener("visibilitychange", tick); };
  }, [refresh]);
  return { workspace: loaded && loaded.actor === actor ? loaded.value : null, error, revision, refresh };
}
