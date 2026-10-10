import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";

import { ShareUsageEquivalent, ShareUsageTable, visibleShareEmailUsage } from "./share-email-usage-table";
import { interpolate, messages } from "@/lib/i18n";
import type { ShareUsageByEmailResponse } from "@/lib/types";

const t = (key: keyof typeof messages.en, values?: Record<string, string | number>) =>
  interpolate(messages.en[key], values);

test("email usage total keeps tokens first and official-price equivalent below", () => {
  const usage: ShareUsageByEmailResponse = {
    shareId: "share-1",
    period: "24h",
    days: 1,
    totalTokens: 150,
    equivalentUsdMicros: "250000",
    pricedCoveragePercent: 80,
    rows: [
      {
        email: "owner@example.com", role: "owner",
        inputTokens: 90, outputTokens: 10, cacheReadTokens: 0,
        cacheCreationTokens: 0, totalTokens: 100, percent: 66.67,
        equivalentUsdMicros: "250000", pricedCoveragePercent: 80,
        daily: [],
      },
      {
        email: "other@example.com", role: "gateway",
        inputTokens: 50, outputTokens: 0, cacheReadTokens: 0,
        cacheCreationTokens: 0, totalTokens: 50, percent: 33.33,
        equivalentUsdMicros: null, pricedCoveragePercent: 0,
        daily: [],
      },
    ],
  };
  const html = renderToStaticMarkup(<ShareUsageTable usage={usage} t={t} locale="en" />);
  assert.match(html, /100<\/div><span[^>]*>≈ \$0\.25<span[^>]*> · 80%<\/span>/);
  assert.match(html, /50<\/div><span[^>]*>—<\/span>/);
  assert.match(html, /amount excludes unpriced usage/);
  assert.doesNotMatch(html, /\$0\.00/);
});

test("summary labels the equivalent and keeps partial coverage visible", () => {
  const html = renderToStaticMarkup(
    <ShareUsageEquivalent amount="$1.25" coverage={75} t={t} summary />,
  );
  assert.match(html, /Equivalent USD ≈ \$1\.25/);
  assert.match(html, /· 75%/);
  assert.match(renderToStaticMarkup(<ShareUsageEquivalent amount={null} coverage={0} t={t} summary />), /Equivalent USD —/);
});

test("switching Share or period never shows a stale equivalent", () => {
  const usage = {
    shareId: "share-1", period: "24h", totalTokens: 100,
    equivalentUsdMicros: "250000", pricedCoveragePercent: 100,
    days: 1, rows: [],
  } satisfies ShareUsageByEmailResponse;
  assert.equal(visibleShareEmailUsage(usage, "share-1", "24h", false, ""), usage);
  assert.equal(visibleShareEmailUsage(usage, "share-1", "1w", false, ""), null);
  assert.equal(visibleShareEmailUsage(usage, "share-2", "24h", false, ""), null);
  assert.equal(visibleShareEmailUsage(usage, "share-1", "24h", true, ""), null);
  assert.equal(visibleShareEmailUsage(usage, "share-1", "24h", false, "failed"), null);
});
