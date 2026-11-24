import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "@/components/i18n/locale-provider";
import { HostHardwareSummary, HostHealthSummary } from "./host-health";
import { canBootstrapHost, hostHealthDisplayState, hostHealthRecord } from "@/lib/host-health-view";
import { sortHosts } from "./host-utils";
import { mergeHosts } from "@/lib/client-market-refresh";
import { hostHealthMessagesEn, hostHealthMessagesZh } from "@/lib/host-health-messages";
import { getClientMarketHostHealth, manageClientMarketHostHealth } from "@/lib/api";
import type { ClientMarketHost, HostHealth } from "@/lib/types";

const health: HostHealth = {
  state: "healthy", lastReceivedAt: "2026-11-01T10:59:59Z",
  sampledAt: "2026-11-01T10:59:58Z", nextDueAt: "2026-11-01T11:59:00Z",
  received24h: 22, observable24h: 24, consecutiveMissed: 0, supervision: "nohup", snapshot: null,
};

test("main-row and quote summary expose counts, time and restart limits", () => {
  const html = renderToStaticMarkup(<LocaleProvider><HostHealthSummary health={health} /></LocaleProvider>);
  assert.match(html, /22\/24 observable hours/);
  assert.match(html, /Reporting normally/);
  assert.match(html, /Automatic restart is not configured/);
  assert.match(html, /dateTime="2026-11-01T10:59:59Z"/);
  assert.doesNotMatch(html, /<ol/);
});

test("hourly health refreshes without a business updatedAt change", () => {
  const host = { id: "host", status: "idle", hostHealth: health, updatedAt: "same" } as ClientMarketHost;
  assert.equal(mergeHosts([host], [structuredClone(host)])[0], host);
  const changed = { ...host, hostHealth: { ...health, state: "missed" as const, received24h: 21 } };
  assert.equal(mergeHosts([host], [changed])[0], changed);
  assert.deepEqual(Object.keys(hostHealthMessagesEn).sort(), Object.keys(hostHealthMessagesZh).sort());
});

test("partial hardware and clock restart limits stay visible in rows and quotes", () => {
  const unknown = { value: null, unknownReason: "scope_unverified" as const };
  const partial: HostHealth = { ...health, clockSynchronized: false, snapshot: {
    os: "Linux", arch: "x86_64", effectiveVcpu: { value: 2, unknownReason: null },
    memoryTotalBytes: { value: 2 ** 30, unknownReason: null }, dataVolumeTotalBytes: unknown,
    memoryAvailableBytes: unknown, dataVolumeAvailableBytes: unknown, cpuUsagePct: unknown,
  } };
  for (const variant of ["table", "checkout"] as const) {
    const html = renderToStaticMarkup(<LocaleProvider><HostHealthSummary health={partial} variant={variant} /><HostHardwareSummary health={partial} /></LocaleProvider>);
    assert.match(html, /Linux-x86_64/);
    assert.doesNotMatch(html, /Disk unknown|磁盘/);
    assert.match(html, /1 GiB RAM/);
    assert.match(html, variant === "table" ? /Clock unsynced/ : /Time sync is off/);
    assert.match(html, variant === "table" ? /No autostart/ : /Automatic restart is not configured/);
  }
});

test("unenrolled and queued rentals cannot look like a scheduled first report", () => {
  const empty = { ...health, state: "awaiting_first" as const, lastReceivedAt: null, nextDueAt: null, observable24h: 0, received24h: 0 };
  assert.equal(hostHealthDisplayState(empty), "not_enabled");
  assert.equal(hostHealthRecord(empty), null);
  const html = renderToStaticMarkup(<LocaleProvider><HostHealthSummary health={empty} variant="table" /><HostHardwareSummary health={empty} /></LocaleProvider>);
  assert.match(html, /Monitoring not enabled/);
  assert.match(html, /Specifications pending/);
  assert.doesNotMatch(html, /Waiting for first report|0\/0|No observable|<ol/);
  assert.equal(hostHealthDisplayState({ ...empty, deployment: { state: "queued", reason: null, canBootstrap: false, deployedCommit: null } }), "queued");
});

test("monitor enrollment requires the owner capability and current rental identity", () => {
  const host = { id: "host", status: "allocated", isHostOwner: true, installationId: "lease", hostHealth: { ...health, machineEpoch: 1, deployment: { state: "not_enabled", reason: null, canBootstrap: true, deployedCommit: null } } } as ClientMarketHost;
  assert.equal(canBootstrapHost(host), true);
  assert.equal(canBootstrapHost({ ...host, isHostOwner: false }), false);
  assert.equal(canBootstrapHost({ ...host, installationId: undefined }), false);
  assert.equal(canBootstrapHost({ ...host, status: "draining" }), false);
  assert.equal(canBootstrapHost({ ...host, hostHealth: { ...host.hostHealth!, machineEpoch: 0 } }), false);
});

test("health and column ordering use one sort preference and unknown records stay honest", () => {
  const host = (id: string, received24h: number, observable24h: number) => ({ id, ip: id, hostOwnerEmail: "owner", status: "idle", hostHealth: { ...health, received24h, observable24h } } as ClientMarketHost);
  const hosts = [host("c", 0, 0), host("b", 22, 24), host("a", 24, 24), host("d", 1, 1)];
  assert.deepEqual(sortHosts(hosts, { key: "health", dir: "desc" }).map(h => h.id), ["a", "d", "b", "c"]);
  assert.deepEqual(sortHosts(hosts, { key: "ip", dir: "desc" }).map(h => h.id), ["d", "c", "b", "a"]);
});

test("health reads bypass cache and maintenance bodies match each strict endpoint", async () => {
  const originals = new Map(["window", "localStorage", "fetch"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  Object.defineProperty(globalThis, "window", { configurable: true, value: new EventTarget() });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => JSON.stringify({ accessToken: "fixture", expiresAt: "2099-01-01T00:00:00Z" }) } });
  try {
    const calls: { url: string; body: unknown; cache?: RequestCache }[] = [];
    globalThis.fetch = async (url, init) => {
      calls.push({ url: String(url), body: init?.body ? JSON.parse(String(init.body)) : null, cache: init?.cache });
      return Response.json({ queued: true });
    };
    await getClientMarketHostHealth("host/one");
    const options = { expectedMachineEpoch: 3, compromised: true };
    await manageClientMarketHostHealth("host/one", "retry", options);
    await manageClientMarketHostHealth("host/one", "rotate-key", options);
    await manageClientMarketHostHealth("host/one", "replace", options);
    await manageClientMarketHostHealth("host/one", "bootstrap", { ...options, expectedInstallationId: "current-lease" });
    assert.equal(calls[0].url, "/v1/client-market/hosts/host%2Fone/health");
    assert.equal(calls[0].cache, "no-store");
    assert.deepEqual(calls.slice(1).map(call => call.body), [{}, { compromised: true }, { expectedMachineEpoch: 3 }, { expectedInstallationId: "current-lease", expectedMachineEpoch: 3 }]);
  } finally {
    for (const [key, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});

