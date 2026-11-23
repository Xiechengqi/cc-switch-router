/** The same-page address after editing its query string. */
export function currentPageHref(href: string, update: (params: URLSearchParams) => void, { dropHash = false } = {}) {
  const url = new URL(href);
  update(url.searchParams);
  const search = url.searchParams.toString();
  return `${url.pathname}${search ? `?${search}` : ""}${dropHash ? "" : url.hash}`;
}

const NEXT_HISTORY_KEYS = ["__NA", "__PRIVATE_NEXTJS_INTERNALS_TREE"] as const;

/**
 * History state for a same-page query edit.
 *
 * Next 16 treats a `replaceState` whose data has no `__NA` as an external
 * history change. It then restores the route, and a later Back/Forward reloads
 * the document. Copy the markers that are already on the entry so the patched
 * `replaceState` keeps the original call. Never invent them: an entry written
 * before hydration has no tree, and a fake `__NA` makes Back look restorable.
 */
export function historyStateForQueryRewrite(state: unknown) {
  if (!state || typeof state !== "object") return state ?? null;
  const current = state as Record<string, unknown>;
  const next: Record<string, unknown> = { ...current };
  for (const key of NEXT_HISTORY_KEYS) {
    if (key in current) next[key] = current[key];
  }
  return next;
}

/**
 * Rewrite the current page's query (tabs, filters, focus) without a router
 * navigation. No RSC payload is fetched, so the edit cannot fall back to a
 * full page load.
 *
 * `useSearchParams` does not see this write. Callers keep the value they just
 * set in their own state; the address is what a reload or a shared link reads.
 */
export function replaceCurrentSearch(update: (params: URLSearchParams) => void, options?: { dropHash?: boolean }) {
  window.history.replaceState(
    historyStateForQueryRewrite(window.history.state),
    "",
    currentPageHref(window.location.href, update, options),
  );
}
