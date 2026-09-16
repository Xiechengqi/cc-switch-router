"use client";

import * as React from "react";

import { getShareUserUsageBreakdown } from "@/lib/api";
import type { ShareUserUsageBreakdownMap } from "@/lib/types";

/**
 * Loads per-email usage equivalents for a Share as soon as the table is shown,
 * so the TOKEN column can render USD without waiting for a row expand.
 *
 * `onExpand` still exists for retrying a failed email after the nested panel
 * is opened. A failed fetch is not cached: `errors[email]` is set so the row
 * can retry.
 */
export function useShareUserUsageBreakdown(shareId?: string) {
  const [breakdown, setBreakdown] = React.useState<ShareUserUsageBreakdownMap>(
    {},
  );
  const [breakdownRevision, setBreakdownRevision] = React.useState<string>("");
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  const [loaded, setLoaded] = React.useState<Record<string, true>>({});
  const inflight = React.useRef(new Set<string>());
  const loadedKeys = React.useRef(new Set<string>());
  const shareRequestId = React.useRef(0);

  React.useEffect(() => {
    setBreakdown({});
    setBreakdownRevision("");
    setErrors({});
    setLoaded({});
    inflight.current.clear();
    loadedKeys.current.clear();
    if (!shareId) return;
    const requestId = ++shareRequestId.current;
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
        if (response.pricingRevision) {
          setBreakdownRevision(response.pricingRevision);
        }
      })
      .catch(() => {
        if (shareRequestId.current !== requestId) return;
        loadedKeys.current.clear();
        setBreakdown({});
        setLoaded({});
      })
      .finally(() => {
        inflight.current.delete("*");
      });
  }, [shareId]);

  const load = React.useCallback(
    (email: string, force = false) => {
      if (!shareId) return;
      const key = email.trim().toLowerCase();
      if (!key) return;
      if ((!force && loadedKeys.current.has(key)) || inflight.current.has(key)) return;
      if (force) loadedKeys.current.delete(key);
      inflight.current.add(key);
      setErrors((current) => {
        if (!(key in current)) return current;
        const next = { ...current };
        delete next[key];
        return next;
      });
      void getShareUserUsageBreakdown(shareId, key)
        .then((response) => {
          const row = response.rows.find(
            (item) => item.email.trim().toLowerCase() === key,
          );
          loadedKeys.current.add(key);
          setLoaded((current) => ({ ...current, [key]: true }));
          if (response.pricingRevision) {
            setBreakdownRevision(response.pricingRevision);
          }
          if (!row) return;
          setBreakdown((current) => ({ ...current, [key]: row }));
        })
        .catch((err) => {
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
          inflight.current.delete(key);
        });
    },
    [shareId],
  );

  const onExpand = React.useCallback((email: string) => load(email), [load]);
  const refresh = React.useCallback((email: string) => load(email, true), [load]);

  return { breakdown, breakdownRevision, onExpand, refresh, errors, loaded };
}
