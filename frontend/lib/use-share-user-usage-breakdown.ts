"use client";

import * as React from "react";

import { getShareUserUsageBreakdown } from "@/lib/api";
import type { ShareUserUsageBreakdownMap } from "@/lib/types";

/**
 * Lazy-loads per-email usage breakdowns for a Share.
 *
 * The parent never fetches on mount: `onExpand` fires from the table's first
 * expand of that email, and subsequent expands reuse the cached row. A failed
 * fetch is not cached: `errors[email]` is set so the row can retry.
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

  React.useEffect(() => {
    setBreakdown({});
    setBreakdownRevision("");
    setErrors({});
    setLoaded({});
    inflight.current.clear();
    loadedKeys.current.clear();
  }, [shareId]);

  const onExpand = React.useCallback(
    (email: string) => {
      if (!shareId) return;
      const key = email.trim().toLowerCase();
      if (!key) return;
      if (loadedKeys.current.has(key) || inflight.current.has(key)) return;
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

  return { breakdown, breakdownRevision, onExpand, errors, loaded };
}
