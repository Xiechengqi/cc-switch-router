import assert from "node:assert/strict";
import test from "node:test";
import { currentPageHref } from "./url-search";

test("query edits keep the page and, unless dropped, its hash", () => {
  const href = "https://router.example/account/share/?tab=user&focus=sub-1#income";
  assert.equal(currentPageHref(href, (params) => params.set("tab", "provider")), "/account/share/?tab=provider&focus=sub-1#income");
  assert.equal(currentPageHref(href, (params) => params.set("tab", "access"), { dropHash: true }), "/account/share/?tab=access&focus=sub-1");
});

test("removing the last parameter leaves no dangling question mark", () => {
  assert.equal(currentPageHref("https://router.example/client-market/?provider=mp_1", (params) => params.delete("provider")), "/client-market/");
});
