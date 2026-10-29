"use client";

import * as React from "react";
import { getMarketAccessInboxSummary } from "@/lib/api";
import { useCachedResource } from "@/lib/cached-resource";

const INBOX_SUMMARY_POLL_MS = 30_000;

/** Pending buyer-access requests for the signed-in provider. The top bar, the
 * account sidebar and the provider center read one shared request and poll. */
export function useMarketAccessInboxSummary(actor: string | null | undefined) {
  const { data, mutate, refresh } = useCachedResource(
    actor ? `market-access-inbox-summary:${actor}` : null,
    () => getMarketAccessInboxSummary(),
    { pollMs: INBOX_SUMMARY_POLL_MS },
  );
  const setPendingRequests = React.useCallback((count: number) => {
    mutate((current) => (current?.pendingRequests === count ? current : { ...current, pendingRequests: count }));
  }, [mutate]);
  return { pendingRequests: actor ? data?.pendingRequests ?? 0 : 0, setPendingRequests, refresh };
}
