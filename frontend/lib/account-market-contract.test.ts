import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { accountMarketMessagesEn, accountMarketMessagesZh } from "./account-market-messages";
import { walletPolicyMessagesEn, walletPolicyMessagesZh } from "./wallet-policy-messages";

const source = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("full and quick wallets share one guarded top-up implementation", () => {
  for (const path of ["components/dashboard/platform-wallet-panel.tsx", "components/market/wallet-quick-dialog.tsx"]) {
    const code = source(path);
    assert.match(code, /<WalletTopupForm\b/);
    assert.doesNotMatch(code, /createMarketWalletTopup\s*\(/);
    assert.match(code, /hidden=\{task !== "topup"\}/, "switching tasks keeps the draft and idempotency key mounted");
  }
  const form = source("components/market/wallet-topup-form.tsx");
  assert.match(form, /!active \|\| !ready \|\| busyRef\.current/);
  assert.match(form, /window\.open\("about:blank", "_blank"\)/);
  assert.match(form, /paymentWindow\.opener = null/);
  assert.match(form, /crypto\.randomUUID\(\)/);
  assert.match(form, /createMarketWalletTopup\(channelId, parsedAmount, attempt\.key, attempt\.quoteId/);
  assert.doesNotMatch(form, /selectedChannel\.displayName|\.gatewayKind/);
});

test("termination actions are scoped to Client records, never wallet or income", () => {
  for (const path of ["components/dashboard/platform-wallet-panel.tsx", "components/dashboard/provider-finance-panel.tsx"]) {
    assert.doesNotMatch(source(path), /ClientTerminationsPanel|ClientTerminationRecord|ClientTerminationDialog/);
  }
  const client = source("components/dashboard/account-client-section.tsx");
  assert.match(client, /<ClientTerminationRecord\b/);
  assert.equal(client.match(/<ClientTerminationDialog\b/g)?.length, 1, "refreshes update rows without remounting an open dialog");
  assert.match(client, /key=\{actor \|\| "anonymous"\}/);
  assert.match(client, /getClientMarketHosts\(undefined, controller\.signal\)/);
});

test("record details mount lazily and use stable identity across refreshes", () => {
  const table = source("components/common/expandable-record-table.tsx");
  assert.match(table, /<React\.Fragment key=\{key\}>/);
  assert.match(table, /\{open \? \(/);
  assert.match(table, /\{renderDetail\(row\)\}/);
  assert.match(table, /aria-expanded=\{open\}/);
  assert.match(table, /rootRef\.current\?\.querySelector/);
  assert.match(source("components/dashboard/account-share-rentals.tsx"), /renderDetail=\{\(subscription\) =>/);
  assert.match(source("components/dashboard/account-client-section.tsx"), /renderDetail=\{\(entry\) =>/);
});

test("navbar actions are lazy and refocus the existing wallet instead of duplicating it", () => {
  const navbar = source("components/layout/navbar-wallet-balance.tsx");
  assert.match(navbar, /React\.lazy/);
  assert.match(navbar, /document\.getElementById\(WALLET_ACTIONS_ID\)/);
  assert.match(navbar, /section\.focus\(\{ preventScroll: true \}\)/);
  assert.match(navbar, /<WalletQuickDialog key=\{actor\}/);
  const quick = source("components/market/wallet-quick-dialog.tsx");
  assert.match(quick, /setReady\(false\)/);
  assert.match(quick, /getC2COrders\(\{ role: "buyer", status: "active" \}\)/);
  assert.match(quick, /inlineConfirmation/);
  const c2c = source("components/dashboard/c2c-buy-panel.tsx");
  assert.match(c2c, /inline=\{inlineConfirmation\}/);
  assert.match(c2c, /previewForm\.current\?\.querySelector<HTMLButtonElement>/);
  assert.match(c2c, /<C2CError>\{!quote \|\| pending \? error : ""\}<\/C2CError>/, "confirmation errors appear once, inside the active confirmation");
});

test("account section labels have both languages", async () => {
  const { accountSectionMessagesEn, accountSectionMessagesZh } = await import("./account-section-messages");
  assert.deepEqual(Object.keys(accountSectionMessagesZh).sort(), Object.keys(accountSectionMessagesEn).sort());
  for (const value of [...Object.values(accountSectionMessagesEn), ...Object.values(accountSectionMessagesZh)]) assert.ok(value.trim());
});

test("account labels have both languages and balance sending does not rename external payment", () => {
  assert.deepEqual(Object.keys(accountMarketMessagesZh).sort(), Object.keys(accountMarketMessagesEn).sort());
  for (const value of [...Object.values(accountMarketMessagesEn), ...Object.values(accountMarketMessagesZh)]) assert.ok(value.trim());
  assert.equal(walletPolicyMessagesEn["walletTransfer.confirm"], "Send {amount}");
  assert.equal(walletPolicyMessagesZh["walletTransfer.confirm"], "转出 {amount}");
  assert.match(source("lib/i18n.ts"), /"marketWallet\.reviewInvalidReceipt": "网关报告已付款，但回执未通过核验。请勿重复转账/);
});

test("wallet action copy has both languages", async () => {
  const { walletActionsMessagesEn, walletActionsMessagesZh } = await import("./wallet-actions-messages");
  assert.deepEqual(Object.keys(walletActionsMessagesZh).sort(), Object.keys(walletActionsMessagesEn).sort());
  for (const value of [...Object.values(walletActionsMessagesEn), ...Object.values(walletActionsMessagesZh)]) assert.ok(value.trim());
});

test("short consent labels keep the full terms one tap away", () => {
  assert.match(source("components/dashboard/c2c-buy-panel.tsx"), /hint=\{t\("c2c\.ack"\)\}>\{t\("c2c\.ackShort"\)\}/);
  const orders = source("components/dashboard/c2c-orders-panel.tsx");
  assert.match(orders, /hint=\{t\("c2c\.releaseAck"\)\}>\{t\("c2c\.releaseAckShort"\)\}/);
  assert.match(orders, /hint=\{intent === "paid" \? t\("c2c\.paidAck"\)/);
  assert.match(source("components/dashboard/c2c-seller-panel.tsx"), /\{t\("c2c\.sellerAck"\)\}[\s\S]*\{t\("c2c\.sellerAckShort"\)\}/);
  // Hints render in the top layer so a modal <dialog> cannot hide them.
  assert.match(source("components/common/info-hint.tsx"), /setAttribute\("popover", "manual"\)/);
});
