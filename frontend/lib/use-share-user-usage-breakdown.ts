"use client";

import * as React from "react";

import { ApiError, getShareUserUsageBreakdown } from "@/lib/api";
import { useLoginContextKey } from "@/lib/use-login-context";
import type { ShareUserUsageBreakdownMap, ShareUserUsageBreakdownResponse } from "@/lib/types";

/** Who may price an unpriced model; both fields are absent on older Routers. */
export type SharePriceCapabilities = Pick<ShareUserUsageBreakdownResponse, "canSetPrice" | "canSetGlobalPrice">;

function priceCapabilities(response: ShareUserUsageBreakdownResponse): SharePriceCapabilities {
  return { canSetPrice: response.canSetPrice, canSetGlobalPrice: response.canSetGlobalPrice };
}

/** The breakdown needs a session; signed out there is simply no USD to show. */
const signedOut = (error: unknown) => error instanceof ApiError && error.status === 401;

/**
 * Loads per-email usage equivalents for a Share as soon as the table is shown,
 * so the TOKEN column can render USD without waiting for a row expand.
 *
 * `onExpand` still exists for retrying a failed email after the nested panel
 * is opened. A failed fetch is not cached: `errors[email]` is set so the row
 * can retry. A 401 is not a failure: the email is marked loaded without a row,
 * so the USD parts stay hidden. Everything resets when the sign-in changes.
 */
export function useShareUserUsageBreakdown(shareId?: string) {
  const [breakdown, setBreakdown] = React.useState<ShareUserUsageBreakdownMap>(
    {},
  );
  const [breakdownRevision, setBreakdownRevision] = React.useState<string>("");
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  const [loaded, setLoaded] = React.useState<Record<string, true>>({});
  const [capabilities, setCapabilities] = React.useState<SharePriceCapabilities>({});
  const inflight = React.useRef(new Set<string>());
  const loadedKeys = React.useRef(new Set<string>());
  const shareRequestId = React.useRef(0);
  const unauthorized = React.useRef(false);
  const loginContext = useLoginContextKey();

  React.useEffect(() => {
    setBreakdown({});
    setBreakdownRevision("");
    setErrors({});
    setLoaded({});
    setCapabilities({});
    inflight.current.clear();
    loadedKeys.current.clear();
    unauthorized.current = false;
    // Also fences per-email responses still in flight for the previous
    // Share or sign-in.
    const requestId = ++shareRequestId.current;
    if (!shareId) return;
    inflight.current.add("*");
    void getShareUserUsageBreakdown(shareId)
      .then((response) => {
        if (shareRequestId.current !== requestId) return;
        const next: ShareUserUsageBreakdownMap = {};
        const nextLoaded: Record<string, true> = {};
        for (const row of response.rows || []) {
          const key = row.email.trim().toLowerCase();
          if (!key) continue;
          next[key] = row;
          nextLoaded[key] = true;
          loadedKeys.current.add(key);
        }
        setBreakdown(next);
        setLoaded(nextLoaded);
        setErrors({});
        setCapabilities(priceCapabilities(response));
        if (response.pricingRevision) {
          setBreakdownRevision(response.pricingRevision);
        }
      })
      .catch((err) => {
        if (shareRequestId.current !== requestId) return;
        loadedKeys.current.clear();
        setBreakdown({});
        setLoaded({});
        if (signedOut(err)) unauthorized.current = true;
      })
      .finally(() => {
        if (shareRequestId.current === requestId) inflight.current.delete("*");
      });
  }, [shareId, loginContext]);

  const load = React.useCallback(
    (email: string, force = false) => {
      if (!shareId) return;
      const key = email.trim().toLowerCase();
      if (!key) return;
      if ((!force && loadedKeys.current.has(key)) || inflight.current.has(key)) return;
      if (unauthorized.current) {
        loadedKeys.current.add(key);
        setLoaded((current) => (key in current ? current : { ...current, [key]: true }));
        return;
      }
      if (force) loadedKeys.current.delete(key);
      const requestId = shareRequestId.current;
      inflight.current.add(key);
      setErrors((current) => {
        if (!(key in current)) return current;
        const next = { ...current };
        delete next[key];
        return next;
      });
      void getShareUserUsageBreakdown(shareId, key)
        .then((response) => {
          if (shareRequestId.current !== requestId) return;
          const row = response.rows.find(
            (item) => item.email.trim().toLowerCase() === key,
          );
          loadedKeys.current.add(key);
          setLoaded((current) => ({ ...current, [key]: true }));
          setCapabilities(priceCapabilities(response));
          if (response.pricingRevision) {
            setBreakdownRevision(response.pricingRevision);
          }
          if (!row) return;
          setBreakdown((current) => ({ ...current, [key]: row }));
        })
        .catch((err) => {
          if (shareRequestId.current !== requestId) return;
          if (signedOut(err)) {
            unauthorized.current = true;
            loadedKeys.current.add(key);
            setLoaded((current) => ({ ...current, [key]: true }));
            return;
          }
          loadedKeys.current.delete(key);
          setLoaded((current) => {
            if (!(key in current)) return current;
            const next = { ...current };
            delete next[key];
            return next;
          });
          setErrors((current) => ({
            ...current,
            [key]: err instanceof Error ? err.message : String(err),
          }));
        })
        .finally(() => {
          if (shareRequestId.current === requestId) inflight.current.delete(key);
        });
    },
    [shareId],
  );

  const onExpand = React.useCallback((email: string) => load(email), [load]);
  const refresh = React.useCallback((email: string) => load(email, true), [load]);

  return { breakdown, breakdownRevision, onExpand, refresh, errors, loaded, priceCapabilities: capabilities };
}
