import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";

import { MarketClientBadge, MarketClientStatusLine } from "./client-market-client-context";
import { LocaleProvider } from "@/components/i18n/locale-provider";

test("market origin remains visible without private rental access", () => {
  const html = renderToStaticMarkup(
    <LocaleProvider>
      <MarketClientBadge context={{ hint: "unknown", manageAs: null }} />
      <MarketClientStatusLine context={{ hint: "unknown", manageAs: null }} installationId="client-1" />
    </LocaleProvider>,
  );
  assert.match(html, /Client Market/);
  assert.match(html, /Offline does not mean the rental has ended/);
  assert.doesNotMatch(html, /href=/);
});

test("renter and Provider links use the market's focused mine view", () => {
  for (const [role, label] of [["client", "Manage rental"], ["provider", "Manage Host"]] as const) {
    const html = renderToStaticMarkup(
      <LocaleProvider>
        <MarketClientStatusLine
          context={{ hint: "long-active", manageAs: role }}
          installationId="client-1"
        />
      </LocaleProvider>,
    );
    assert.match(html, /Long offline/);
    assert.match(html, new RegExp(label));
    assert.match(html, /href="\/client-market\/\?tab=mine&amp;focus=client-1"/);
  }
});
