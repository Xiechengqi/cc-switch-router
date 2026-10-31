"use client";
import * as React from "react";
import { readAuthState } from "./auth";
import { previewValueQuota, valueCapabilityReady, valueReadinessError, type ValueQuotaPreview } from "./share-value-quota";

export const valueReadinessActor = () => {
  const auth = readAuthState();
  return `${auth.email ?? ""}:${auth.loginEpoch ?? ""}:${!!auth.accessToken}`;
};
const POLL_MS = 30_000;
/**
 * Infrastructure readiness, separate from a policy preview. `enabled` checks
 * once (e.g. when a quota dialog opens, so the `$` option can be disabled);
 * `poll` keeps checking only while USD is selected or already in use.
 */
export function useValueQuotaReadiness(shareId?: string, revision?: number, {enabled = true, poll = true}: {enabled?: boolean; poll?: boolean} = {}) {
  const [actor, setActor] = React.useState(valueReadinessActor);
  const [attempt, retry] = React.useReducer(n => n + 1, 0);
  const key = `${actor}:${shareId ?? ""}:${revision ?? ""}`;
  const [snapshot, setSnapshot] = React.useState<{key:string; preview?:ValueQuotaPreview; error?:ReturnType<typeof valueReadinessError>; loading:boolean; checkedAt?:number}>();
  const checkedAt = React.useRef<{key:string; at:number; attempt:number}>(undefined);
  React.useEffect(() => {
    const update = () => setActor(valueReadinessActor());
    window.addEventListener("router-auth-changed", update); window.addEventListener("storage", update);
    return () => { window.removeEventListener("router-auth-changed", update); window.removeEventListener("storage", update); };
  }, []);
  React.useEffect(() => {
    if (!enabled || !shareId || revision == null || !readAuthState().accessToken) return;
    const controller = new AbortController();
    let pending = false;
    const refresh = async (force: boolean) => {
      if (pending || document.visibilityState === "hidden" || controller.signal.aborted || actor !== valueReadinessActor()) return;
      const last = checkedAt.current;
      // Toggling between one-shot and polling must not refetch a fresh answer.
      if (!force && last?.key === key && last.attempt === attempt && Date.now() - last.at < POLL_MS) return;
      pending = true;
      setSnapshot(previous=>previous?.key===key?{...previous,error:undefined,loading:true}:{key,loading:true});
      try {
        const preview = await previewValueQuota(shareId, revision, {}, controller.signal);
        if (!controller.signal.aborted && actor === valueReadinessActor()) {
          checkedAt.current = {key, at: Date.now(), attempt};
          setSnapshot({key, preview, loading:false, error: !preview.checks ? "upgrade" : undefined});
        }
      } catch (error) {
        if (!controller.signal.aborted && actor === valueReadinessActor()) {
          checkedAt.current = {key, at: Date.now(), attempt};
          setSnapshot(previous => ({key, loading:false, error:valueReadinessError(error),
            // Keep the last good checklist visible beside the failure.
            preview: previous?.key === key ? previous.preview : undefined}));
        }
      } finally { pending = false; }
    };
    void refresh(false);
    if (!poll) return () => controller.abort();
    const visible = () => { if (document.visibilityState === "visible") void refresh(false); };
    document.addEventListener("visibilitychange", visible);
    const timer = window.setInterval(() => void refresh(true), POLL_MS);
    return () => {controller.abort(); window.clearInterval(timer); document.removeEventListener("visibilitychange", visible);};
  }, [shareId, revision, actor, key, attempt, enabled, poll]);
  const current = snapshot?.key === key && actor === valueReadinessActor() ? snapshot : undefined;
  const ready = !current?.error && valueCapabilityReady(current?.preview);
  return {key, preview:current?.preview, error:current?.error, loading:enabled && (current?.loading ?? !!shareId),
    /** True only after a completed check said no; unknown is not "unavailable". */
    unavailable: !!current && !ready && (!current.loading || !!current.preview),
    ready, retry};
}
