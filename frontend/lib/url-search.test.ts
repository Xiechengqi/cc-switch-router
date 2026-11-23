import assert from "node:assert/strict";
import test from "node:test";
import { currentPageHref, replaceCurrentSearch } from "./url-search";

test("query edits keep the page and, unless dropped, its hash", () => {
  const href = "https://router.example/account/share/?tab=user&focus=sub-1#income";
  assert.equal(currentPageHref(href, (params) => params.set("tab", "provider")), "/account/share/?tab=provider&focus=sub-1#income");
  assert.equal(currentPageHref(href, (params) => params.set("tab", "access"), { dropHash: true }), "/account/share/?tab=access&focus=sub-1");
});

test("removing the last parameter leaves no dangling question mark", () => {
  assert.equal(currentPageHref("https://router.example/client-market/?provider=mp_1", (params) => params.delete("provider")), "/client-market/");
});

test("query rewrites keep Next history markers and only change the query", () => {
  const tree = { tree: ["", {}], renderedSearch: "" };
  const state = { __NA: true, __PRIVATE_NEXTJS_INTERNALS_TREE: tree, scroll: 12 };
  let written: unknown;
  const history: { state: unknown; replaceState: (data: unknown, unused: string, url?: string) => void } = {
    state,
    replaceState(data: unknown, _unused: string, url?: string) {
      written = data;
      history.state = data;
      location.href = `https://router.example${url}`;
    },
  };
  const location = {
    href: "https://router.example/client-market/?terminationId=term-1#rentals",
  };
  const previous = globalThis.window;
  Object.assign(globalThis, { window: { history, location } });
  try {
    replaceCurrentSearch((params) => params.delete("terminationId"));
    assert.equal(location.href, "https://router.example/client-market/#rentals");
    assert.equal((written as { __NA?: boolean }).__NA, true);
    assert.equal((written as { __PRIVATE_NEXTJS_INTERNALS_TREE?: unknown }).__PRIVATE_NEXTJS_INTERNALS_TREE, tree);
    assert.equal((written as { scroll?: number }).scroll, 12);
  } finally {
    if (previous === undefined) {
      delete (globalThis as { window?: unknown }).window;
    } else {
      Object.assign(globalThis, { window: previous });
    }
  }
});

test("an empty history entry does not gain a fabricated Next marker", () => {
  let written: unknown = "unset";
  const history = {
    state: null as unknown,
    replaceState(data: unknown, _unused: string, url?: string) {
      written = data;
      location.href = `https://router.example${url}`;
    },
  };
  const location = { href: "https://router.example/providers/?market=share" };
  const previous = globalThis.window;
  Object.assign(globalThis, { window: { history, location } });
  try {
    replaceCurrentSearch((params) => params.delete("market"));
    assert.equal(written, null);
    assert.equal(location.href, "https://router.example/providers/");
  } finally {
    if (previous === undefined) {
      delete (globalThis as { window?: unknown }).window;
    } else {
      Object.assign(globalThis, { window: previous });
    }
  }
});
