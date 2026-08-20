import assert from "node:assert/strict";
import test from "node:test";

import { summarizeShareAvailability } from "@/components/dashboard/operational-status";
import type { ShareView } from "@/lib/types";

function share(overrides: Partial<ShareView> = {}): ShareView {
  return {
    shareId: "share-1",
    capacityPoolId: "share-1",
    shareName: "Share 1",
    freeAccess: false,
    subdomain: "share-1",
    appType: "codex",
    tokenLimit: 1_000,
    parallelLimit: 3,
    tokensUsed: 0,
    requestsCount: 0,
    shareStatus: "active",
    createdAt: "2026-08-20T00:00:00Z",
    expiresAt: "2026-08-25T00:00:00Z",
    isOnline: true,
    routeState: "active",
    activeRequests: 0,
    onlineRate24h: 100,
    ...overrides,
  };
}

test("service-ready advisory Shares remain available and still count as warnings", () => {
  const summary = summarizeShareAvailability([
    share({
      operationalSummary: {
        state: "degraded",
        primaryReason: { code: "expires_soon", severity: "warning" },
        additionalReasonCount: 1,
      },
      serviceReadiness: {
        ready: true,
        additionalBlockerCount: 0,
      },
    }),
  ]);

  assert.equal(summary.enabledCount, 1);
  assert.equal(summary.availableCount, 1);
  assert.equal(summary.degradedCount, 1);
  assert.equal(summary.issueCount, 0);
});

test("an authoritative blocker excludes a degraded Share from availability", () => {
  const summary = summarizeShareAvailability([
    share({
      operationalSummary: {
        state: "degraded",
        primaryReason: { code: "expires_soon", severity: "warning" },
        additionalReasonCount: 1,
      },
      serviceReadiness: {
        ready: false,
        primaryBlocker: { code: "provider_unavailable", severity: "critical" },
        additionalBlockerCount: 0,
      },
    }),
  ]);

  assert.equal(summary.availableCount, 0);
  assert.equal(summary.degradedCount, 1);
  assert.equal(summary.issueCount, 1);
});

test("paused Shares do not become the availability denominator", () => {
  const summary = summarizeShareAvailability([
    share({
      shareStatus: "paused",
      isOnline: false,
      routeState: "offline",
      operationalSummary: { state: "disabled", additionalReasonCount: 0 },
      serviceReadiness: { ready: false, additionalBlockerCount: 0 },
    }),
  ]);

  assert.equal(summary.enabledCount, 0);
  assert.equal(summary.availableCount, 0);
  assert.equal(summary.issueCount, 0);
});

test("legacy responses retain the previous strict operational-state fallback", () => {
  const summary = summarizeShareAvailability([
    share({ shareId: "online", operationalSummary: { state: "online", additionalReasonCount: 0 } }),
    share({ shareId: "degraded", operationalSummary: { state: "degraded", additionalReasonCount: 0 } }),
  ]);

  assert.equal(summary.enabledCount, 2);
  assert.equal(summary.availableCount, 1);
  assert.equal(summary.degradedCount, 1);
  assert.equal(summary.issueCount, 1);
});
