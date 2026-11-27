import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { clientOpsAccess } from "@/components/dashboard/client-ops";
import type { DashboardClient } from "@/lib/types";

function client(overrides: Partial<DashboardClient> = {}): DashboardClient {
  return {
    installation: { id: "inst-1" },
    clientTunnel: { subdomain: "c1", tunnelUrl: "https://c1.example.com", enabled: true, online: true },
    ...overrides,
  } as DashboardClient;
}

const none = { console: false, terminal: false, upgrade: false, takeover: false };

test("the owner of an unrented Client gets every operation", () => {
  const owner = client({ canOpenClientConsole: true, canOpenClientTerminal: true, canUpgrade: true });
  assert.deepEqual(clientOpsAccess(owner, true), { console: true, terminal: true, upgrade: true, takeover: true });
});

test("visitors get no operations, so the card shows no Ops button", () => {
  assert.deepEqual(clientOpsAccess(client(), false), none);
  // The Router flags decide, not the mere presence of a tunnel.
  assert.deepEqual(clientOpsAccess(client({ canOpenClientTerminal: false, canOpenClientConsole: false }), false), none);
});

test("the tenant gets the console and upgrade but never the terminal", () => {
  const tenant = client({
    rental: { state: "active", selfRental: false, exclusiveManagement: true, controlId: "lease-1" },
    canOpenClientConsole: true,
    // Even a stray flag from the Router must not show the seller's shell.
    canOpenClientTerminal: true,
    canUpgrade: true,
  });
  assert.deepEqual(clientOpsAccess(tenant, true), { console: true, terminal: false, upgrade: true, takeover: false });
});

test("the owner sees no operations while the Client is rented out", () => {
  const rentedOut = client({
    rental: { state: "active", selfRental: false, exclusiveManagement: true, tenantEmail: "tenant@example.com" },
    canOpenClientConsole: true,
    canOpenClientTerminal: true,
    canUpgrade: true,
  });
  assert.deepEqual(clientOpsAccess(rentedOut, true), none);
});

test("the subdomain switch follows the Router's owner check, not the email match alone", () => {
  // The tunnel email matches the viewer, but the Router does not treat them as the owner.
  assert.equal(clientOpsAccess(client({ canOpenClientConsole: false }), true).takeover, false);
  assert.equal(clientOpsAccess(client({ canOpenClientConsole: true }), true).takeover, true);
});

test("console and terminal need a tunnel address", () => {
  const offline = client({ clientTunnel: undefined, canOpenClientConsole: true, canOpenClientTerminal: true });
  assert.equal(clientOpsAccess(offline, false).console, false);
  assert.equal(clientOpsAccess(offline, false).terminal, false);
});

const board = readFileSync(new URL("./client-board.tsx", import.meta.url), "utf8");
const menu = readFileSync(new URL("./client-ops-menu.tsx", import.meta.url), "utf8");

test("the card groups operations behind Ops and keeps logs and chat public", () => {
  assert.match(board, /<ClientOpsMenu key=\{client\.rental\?\.controlId \|\| client\.installation\.id\} client=\{client\} onOpenTakeover=\{onOpenTakeover\} \/>/);
  assert.doesNotMatch(board, /ClientConsoleButton|ClientTerminalButton|ClientTakeoverButton|ClientUpgradeButton/);
  assert.match(board, /<ClientLogsButton onOpen=\{onOpenLogs\} \/>/);
  assert.match(board, /<ClientChatButton client=\{client\} \/>/);
});

test("the name row shows only who manages the Client: the tenant while rented, else the owner", () => {
  assert.match(board, /const manager = rental\s*\? \{ role: t\("dashboard\.renter"\), email: rental\.tenantEmail \|\| t\("clientMarket\.shop\.rentedOut"\) \}\s*: owner && owner !== "-"\s*\? \{ role: t\("dashboard\.owner"\), email: owner \}/);
  assert.match(board, /title=\{`\$\{manager\.role\} \$\{manager\.email\}`\}>\s*\{manager\.email\}/);
  // The drawer keeps both.
  assert.match(board, /<dt className="text-slate-500">\{t\("dashboard\.owner"\)\}<\/dt>[\s\S]*?<dt className="text-slate-500">\{t\("dashboard\.renter"\)\}<\/dt>/);
});

test("the Ops button renders only when an operation is allowed", () => {
  assert.match(menu, /if \(!access\.console && !access\.terminal && !showUpgrade && !access\.takeover\) return upgrade\.dialog;/);
  // Hover would end at once under a modal popover's underlay.
  assert.match(menu, /<Dropdown\.Popover[^>]*isNonModal/);
  // Clicks inside the portaled menu must not collapse the card header.
  assert.match(menu, /<div data-no-row-drawer \{\.\.\.menu\.contentProps\}>/);
  // Touch never hover-opens; it opens by tap.
  assert.match(menu, /if \(event\.pointerType !== "mouse"\) return;/);
  // A second press closes the menu despite React Aria reopening it on press start.
  assert.match(menu, /if \(!current\.current && !closingByPress\.current\) set\("press"\);/);
});
