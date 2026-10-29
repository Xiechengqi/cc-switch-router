/** The same-page address after editing its query string. */
export function currentPageHref(href: string, update: (params: URLSearchParams) => void, { dropHash = false } = {}) {
  const url = new URL(href);
  update(url.searchParams);
  const search = url.searchParams.toString();
  return `${url.pathname}${search ? `?${search}` : ""}${dropHash ? "" : url.hash}`;
}

/**
 * Rewrite the current page's query (tabs, filters, focus) without a router
 * navigation. The App Router still adopts the new URL, so `useSearchParams`
 * re-renders, but unlike `router.replace` no RSC payload is fetched: the
 * switch is instant and cannot fall back to a full page load.
 *
 * The `null` state is deliberate: Next copies its own history state and
 * applies the URL. Passing `window.history.state` would bypass the router
 * and leave `useSearchParams` stale. Some pages still do that on purpose for
 * query keys they track in local state; reading the live address here keeps
 * those keys intact.
 */
export function replaceCurrentSearch(update: (params: URLSearchParams) => void, options?: { dropHash?: boolean }) {
  window.history.replaceState(null, "", currentPageHref(window.location.href, update, options));
}
