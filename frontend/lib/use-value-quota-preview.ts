"use client";
import * as React from "react";
import { prepareQuotaHistory, previewValueQuota, type ValueQuotaPreview } from "./share-value-quota";
import type { ShareUserPolicy } from "./types";

export type ValuePreviewState = {intent: string; preview?: ValueQuotaPreview; error?: unknown; loading: boolean; preparedRecords?: number};
/** Stable identity for "this exact policy set in this readiness scope". */
export function valuePreviewIntent(scope: string, policies: Record<string, ShareUserPolicy> | undefined) {
  if (!policies) return "";
  return JSON.stringify([scope, Object.keys(policies).sort().map(email => [email, policies[email]])]);
}
/**
 * Debounced explicit-policy preview. Any change to the policies (another
 * user, another amount, dialog closed) drops the previous answer at once, so
 * a stale preview can never be shown or confirmed.
 */
export function useValueQuotaPreview(shareId: string | undefined, revision: number | undefined, scope: string,
  policies: Record<string, ShareUserPolicy> | undefined, delayMs = 400) {
  const intent = shareId && revision != null ? valuePreviewIntent(`${scope}:${shareId}:${revision}`, policies) : "";
  const [state, setState] = React.useState<ValuePreviewState>({intent: "", loading: false});
  const request = React.useRef<{intent: string; controller: AbortController; promise: Promise<ValueQuotaPreview | undefined>}>(undefined);
  const latest = React.useRef({intent, shareId, revision, policies});
  latest.current = {intent, shareId, revision, policies};

  const run = React.useCallback((): Promise<ValueQuotaPreview | undefined> => {
    const {intent: wanted, shareId: id, revision: rev, policies: body} = latest.current;
    if (!wanted || !id || rev == null || !body) return Promise.resolve(undefined);
    if (request.current?.intent === wanted) return request.current.promise;
    request.current?.controller.abort();
    const controller = new AbortController();
    setState({intent: wanted, loading: true});
    const promise = (async () => {
      let preview = await previewValueQuota(id, rev, body, controller.signal);
      let users = preview.rows.filter(row => row.conversionRequired && row.targetUnit === "usdEquivalent"
        && ["history_incomplete", "history_preparing"].includes(row.reasonCode ?? "")).map(row => row.email);
      const prepared=new Map<string,number>();
      while (users.length && !controller.signal.aborted && latest.current.intent === wanted) {
        const page = await prepareQuotaHistory(id, rev, users.slice(0, 10), controller.signal);
        page.items.forEach(item=>prepared.set(item.email,item.preparation.processed));
        const processed=Array.from(prepared.values()).reduce((sum,n)=>sum+n,0);
        setState({intent: wanted, loading: true, preparedRecords: processed});
        const complete = new Set(page.items.filter(item => item.preparation.state === "complete").map(item => item.email));
        users = users.filter(email => !complete.has(email));
        if (!page.items.length) throw new Error("quota_history_invalid_response");
      }
      if (!controller.signal.aborted) preview = await previewValueQuota(id, rev, body, controller.signal);
      return preview;
    })().then(preview => {
      if (controller.signal.aborted || latest.current.intent !== wanted) return undefined;
      setState({intent: wanted, preview, loading: false});
      return preview;
    }, error => {
      if (controller.signal.aborted || latest.current.intent !== wanted) return undefined;
      setState({intent: wanted, error, loading: false});
      return undefined;
    }).finally(() => { if (request.current?.controller === controller) request.current = undefined; });
    request.current = {intent: wanted, controller, promise};
    return promise;
  }, []);

  React.useEffect(() => {
    request.current?.controller.abort(); request.current = undefined;
    setState({intent, loading: !!intent});
    if (!intent) return;
    const timer = window.setTimeout(() => void run(), delayMs);
    return () => window.clearTimeout(timer);
  }, [intent, delayMs, run]);
  React.useEffect(() => () => request.current?.controller.abort(), []);

  const current = state.intent === intent ? state : {intent, loading: !!intent};
  return {...current, intent,
    /** Resolve the preview for the current policies now, reusing an in-flight or settled answer. */
    fetchNow: (): Promise<ValueQuotaPreview | undefined> => current.preview && current.preview.rows.every(row => !row.credentialExpiresAtMs || row.credentialExpiresAtMs > Date.now() + 5_000) ? Promise.resolve(current.preview) : run()};
}
