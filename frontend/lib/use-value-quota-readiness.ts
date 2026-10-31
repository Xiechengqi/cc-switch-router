"use client";
import * as React from "react";
import { readAuthState } from "./auth";
import { previewValueQuota, valueCapabilityReady, valueReadinessError, type ValueQuotaPreview } from "./share-value-quota";

export const valueReadinessActor = () => {
  const auth = readAuthState();
  return `${auth.email ?? ""}:${auth.loginEpoch ?? ""}:${!!auth.accessToken}`;
};
/** Separate capability polling from the explicit, two-step policy confirmation. */
export function useValueQuotaReadiness(shareId?: string, revision?: number) {
  const [actor, setActor] = React.useState(valueReadinessActor);
  const [attempt, retry] = React.useReducer(n => n + 1, 0);
  const key = `${actor}:${shareId ?? ""}:${revision ?? ""}`;
  const [snapshot, setSnapshot] = React.useState<{key:string; preview?:ValueQuotaPreview; error?:ReturnType<typeof valueReadinessError>; loading:boolean}>();
  React.useEffect(() => {
    const update = () => setActor(valueReadinessActor());
    window.addEventListener("router-auth-changed", update); window.addEventListener("storage", update);
    return () => { window.removeEventListener("router-auth-changed", update); window.removeEventListener("storage", update); };
  }, []);
  React.useEffect(() => {
    if (!shareId || revision == null || !readAuthState().accessToken) return;
    const controller = new AbortController();
    let pending = false;
    const refresh = async () => {
      if (pending || document.visibilityState === "hidden" || controller.signal.aborted || actor !== valueReadinessActor()) return;
      pending = true;
      setSnapshot(previous=>previous?.key===key?{...previous,error:undefined,loading:true}:{key,loading:true});
      try {
        const preview = await previewValueQuota(shareId, revision, {}, controller.signal);
        if (!controller.signal.aborted && actor === valueReadinessActor()) setSnapshot({key, preview, loading:false,
          error: !preview.checks ? "upgrade" : undefined});
      } catch (error) {
        if (!controller.signal.aborted && actor === valueReadinessActor()) setSnapshot({key, loading:false, error:valueReadinessError(error)});
      } finally { pending = false; }
    };
    void refresh();
    const visible = () => { if (document.visibilityState === "visible") void refresh(); };
    document.addEventListener("visibilitychange", visible);
    const timer = window.setInterval(() => void refresh(), 30_000);
    return () => {controller.abort(); window.clearInterval(timer); document.removeEventListener("visibilitychange", visible);};
  }, [shareId, revision, actor, key, attempt]);
  const current = snapshot?.key === key && actor === valueReadinessActor() ? snapshot : undefined;
  return {key, preview:current?.preview, error:current?.error, loading:current?.loading ?? !!shareId,
    ready:valueCapabilityReady(current?.preview), retry};
}
