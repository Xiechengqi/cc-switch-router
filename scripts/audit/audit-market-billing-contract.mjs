#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const root = path.resolve(path.dirname(scriptPath), "../..");
const sourceRoots = ["src", "frontend/app", "frontend/components", "frontend/lib"];
const documentFiles = ["ARCHITECTURE.md", "PROTOCOL.md", "README.md", "UI_TEST_PLAN.md"];
const allowedExtensions = new Set([".rs", ".ts", ".tsx"]);

const legacyRules = [
  ["legacy Share chat route", /\/v1\/chat\/shares\b/g],
  ["legacy Share chat API helper", /\bgetShareChatRoom\b/g],
  ["legacy Share chat model", /\bShareChat(?:Room|Message|Visit)?\b/g],
  ["legacy Share chat membership table", /\bchat_membership_periods\b/g],
  ["legacy Market ready transaction email", /covered by Client Market ready email|Client Market ready email/g],
  ["legacy Market transaction email renderer", /\brender_transactional_card_email\b/g],
  ["Client rental period field", /\brental_period_days\b/g],
  ["Client prepaid price field", /\bprice_cents\b/g],
  ["Client prepaid invoice amount field", /\bamount_cents\b/g],
  ["product billing period unit", /\bperiod_unit\b/g],
  ["product billing period count", /\bperiod_count\b/g],
  ["product prepaid period end", /\bcurrent_period_end\b/g],
  ["product payment deadline", /\bpayment_deadline\b/g],
  ["product payment declaration timestamp", /\blast_declared_at\b/g],
  ["Share prepaid active state", /\bactive_paid\b/g],
  ["Share renewal state", /\brenewal_due\b/g],
  ["Share trial payment state", /\btrial_payment_due\b/g],
  ["Share payment timeout event", /\bpayment_timeout\b/g],
  ["legacy Client invoice table", /\bclient_market_invoices\b/g],
  ["legacy Client payment declaration table", /\bclient_market_payment_declarations\b/g],
  ["legacy Share invoice table", /\bshare_market_invoices\b/g],
  ["legacy Share payment declaration table", /\bshare_market_payment_declarations\b/g],
  ["legacy Client billing model", /\bClientMarketBilling\b|\bBillingView\b/g],
  ["legacy Client billing loader", /\bgetMyClientMarketBilling\b|\bmergeBillingMap\b/g],
  ["legacy Client payment action", /\bdeclareClientMarketPayment\b/g],
  ["legacy Client billing component", /client-market-billing-banner|billing-urgency/g],
  ["legacy Client billing route", /\/v1\/client-market\/(?:my-billing|billing)(?=["'`/?\s]|$)/g],
  ["legacy per-Client billing route", /\/v1\/client-market\/clients\/:installation_id\/(?:billing|declare-paid)/g],
  ["legacy per-invoice declaration route", /\/v1\/client-market\/invoices\/:invoice_id\/declare-paid/g],
  ["legacy Share access table", /\bshare_market_owner_blocks\b/g],
  ["legacy Client Host access table", /\bhost_provider_client_blocks\b/g],
  ["legacy Share block route", /\/v1\/share-market\/blocks\b/g],
  ["legacy Client Host block route", /\/v1\/(?:account|client-market)\/provider-blocks\b/g],
  ["legacy access component", /user-blacklist-panel|provider-blocks-panel/g],
  [
    "legacy access DTO",
    /\b(?:ClientMarketProviderBlock|ShareMarketOwnerBlock|ProviderClientBlockView|CreateProviderBlockRequest)\b/g,
  ],
  [
    "legacy block API helper",
    /\b(?:getClientMarketProviderBlocks|createClientMarketProviderBlock|liftClientMarketProviderBlock|createShareMarketBlock|liftShareMarketBlock)\b/g,
  ],
  ["legacy block request field", /\b(?:block_client_for_provider|blockClientForProvider|blockUser)\b/g],
  ["legacy global credit threshold field", /\b(?:clearing_threshold_minor|clearingThresholdMinor)\b/g],
  ["legacy global credit threshold wording", /clearing threshold|清账阈值|供应商阈值/g],
  [
    "legacy threshold billing event",
    /\b(?:near_threshold|threshold_reached|threshold_warning|billing_threshold_warning|market_threshold_warning)\b/g,
  ],
  [
    "legacy blacklist UI message",
    /"(?:shareMarket\.(?:blockAndRevoke|unblock|blocks|blocksHint|noBlocks|blocksCol\.[A-Za-z]+|blockedAddedToast|blockedAddedCountToast|unblockedToast|confirm\.blockTitle|confirm\.blockDescription)|clientMarket\.(?:unpaidBlockCheckbox|blockedOwners|blockedHint|blockEmailPlaceholder|blockAdd|blockedAddedToast|blockedAddedCountToast|noneBlocked|blockedCol\.[A-Za-z]+|blockReason\.manual)|market\.(?:userBlacklist|blacklistUnblock|blacklistCol\.[A-Za-z]+|blacklistAddTitle|blacklistAddHint|blacklistAddPlaceholder|blacklistAddCount))"/g,
  ],
  [
    "legacy payment UI message",
    /"(?:shareMarket\.dialog\.(?:billingCount|billingUnit|day|week|month)|clientMarket\.(?:rentalPrice|rentalPeriod|periodDays|nextBillCountdown|paymentDueCountdown|observedPeriodRange|activityPaid)|billing\.(?:declaredToast|releaseStartedToast|paymentDue|goPay|billSoon|billSoonCompact|deadlineLabel|deadlineCompact|paidThroughHint|nextBillHint|timeRemaining|threeDays|releaseNow|creationBlocked|releasing|releaseFailed|manageInMarket|retryRelease|nextWindow|payProvider|provider|currentOffer|declareBefore|noPaymentDetails|emailOffline|declarationNotice|confirmPayment|confirmTitle|confirmDescription|yesPaid|releaseTitle|releaseDescription|releaseProgressTitle|releaseStarting|releaseSucceeded|releaseFailedToast|releaseClient))"/g,
  ],
];

function walk(relativeRoot) {
  const absoluteRoot = path.join(root, relativeRoot);
  if (!fs.existsSync(absoluteRoot)) return [];
  const output = [];
  for (const entry of fs.readdirSync(absoluteRoot, { withFileTypes: true })) {
    const relativePath = path.join(relativeRoot, entry.name);
    if (entry.isDirectory()) {
      output.push(...walk(relativePath));
    } else if (allowedExtensions.has(path.extname(entry.name))) {
      output.push(relativePath);
    }
  }
  return output;
}

function lineNumber(source, index) {
  return source.slice(0, index).split("\n").length;
}

function lineAt(source, index) {
  const start = source.lastIndexOf("\n", index - 1) + 1;
  const end = source.indexOf("\n", index);
  return source.slice(start, end === -1 ? source.length : end);
}

function allowedLegacyAssertion(relativePath, source, index, ruleName) {
  if (
    relativePath === "src/client_market.rs"
    && [
      "Client rental period field",
      "Client prepaid price field",
      "product prepaid period end",
      "product payment deadline",
      "product payment declaration timestamp",
      "legacy Client invoice table",
      "legacy Client payment declaration table",
    ].includes(ruleName)
  ) {
    const declaration = "fn schema_excludes_legacy_prepaid_billing_contract()";
    const start = source.indexOf(declaration);
    if (start >= 0 && index >= start && index < start + extractRustBlock(source, declaration).length) {
      return true;
    }
  }
  if (
    relativePath === "src/share_market.rs"
    && (ruleName === "product billing period unit" || ruleName === "product billing period count")
  ) {
    const line = lineAt(source, index);
    return line.includes("assert!(!columns.iter().any") && line.includes("column ==");
  }
  return false;
}

function extractRustBlock(source, declaration) {
  const start = source.indexOf(declaration);
  if (start < 0) throw new Error(`${declaration} not found`);
  const opening = source.indexOf("{", start);
  let depth = 0;
  for (let index = opening; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`${declaration} block is not closed`);
}

function extractTypeBlock(source, typeName) {
  const marker = `export type ${typeName} =`;
  const start = source.indexOf(marker);
  if (start < 0) throw new Error(`${marker} not found`);
  const end = source.indexOf("\n};", start);
  if (end < 0) throw new Error(`${marker} block is not closed`);
  return source.slice(start, end + 3);
}

function assertProductDtoIsRedacted(errors, label, block) {
  for (const forbidden of [
    "payment_methods",
    "payment_profile_updated_at",
    "qr_image_url",
    "asset_url",
    "paymentMethods",
    "paymentProfileUpdatedAt",
    "qrImageUrl",
    "assetUrl",
  ]) {
    if (block.includes(forbidden)) errors.push(`${label} exposes ${forbidden}`);
  }
}

function main() {
  const files = [...new Set(sourceRoots.flatMap(walk).concat(documentFiles))].sort();
  const errors = [];

  for (const relativePath of files) {
    const source = fs.readFileSync(path.join(root, relativePath), "utf8");
    for (const [ruleName, expression] of legacyRules) {
      expression.lastIndex = 0;
      for (const match of source.matchAll(expression)) {
        if (allowedLegacyAssertion(relativePath, source, match.index, ruleName)) continue;
        errors.push(`${relativePath}:${lineNumber(source, match.index)} contains ${ruleName}: ${match[0]}`);
      }
    }
  }

  const clientSource = fs.readFileSync(path.join(root, "src/client_market.rs"), "utf8");
  const tradeSource = fs.readFileSync(path.join(root, "src/client_market_trade.rs"), "utf8");
  const shareSource = fs.readFileSync(path.join(root, "src/share_market.rs"), "utf8");
  const accessSource = fs.readFileSync(path.join(root, "src/market_access.rs"), "utf8");
  const billingSource = fs.readFileSync(path.join(root, "src/market_billing.rs"), "utf8");
  const chatSource = fs.readFileSync(path.join(root, "src/store/client_chat.rs"), "utf8");
  const apiSource = fs.readFileSync(path.join(root, "src/api.rs"), "utf8");
  const typeSource = fs.readFileSync(path.join(root, "frontend/lib/types.ts"), "utf8");

  for (const [label, block] of [
    ["RouterSshHostView", extractRustBlock(clientSource, "struct RouterSshHostView")],
    ["RentalView", extractRustBlock(tradeSource, "pub struct RentalView")],
    ["Share ListingView", extractRustBlock(shareSource, "pub struct ListingView")],
    ["Share SeatView", extractRustBlock(shareSource, "pub struct SeatView")],
    ["Share SubscriptionView", extractRustBlock(shareSource, "pub struct SubscriptionView")],
    ["CreditAccountView", extractRustBlock(billingSource, "pub struct CreditAccountView")],
    ["ClientMarketHost", extractTypeBlock(typeSource, "ClientMarketHost")],
    ["ClientMarketRental", extractTypeBlock(typeSource, "ClientMarketRental")],
    ["ShareMarketListing", extractTypeBlock(typeSource, "ShareMarketListing")],
    ["ShareMarketSubscription", extractTypeBlock(typeSource, "ShareMarketSubscription")],
    ["MarketCreditAccount", extractTypeBlock(typeSource, "MarketCreditAccount")],
  ]) {
    assertProductDtoIsRedacted(errors, label, block);
  }

  const invoiceType = extractTypeBlock(typeSource, "MarketBillingInvoice");
  for (const required of ["paymentMethods", "paymentProfileUpdatedAt", "lines"]) {
    if (!invoiceType.includes(required)) errors.push(`MarketBillingInvoice is missing ${required}`);
  }

  for (const requestType of [
    "UpdatePolicyRequest",
    "UpdateCounterpartyRequest",
    "UpdateCreditLineRequest",
    "UpdatePublicCreditLineRequest",
  ]) {
    const block = extractRustBlock(accessSource, `struct ${requestType}`);
    if (!block.includes("expected_revision: i64")) {
      errors.push(`${requestType} must require expectedRevision`);
    }
  }

  for (const requiredRoute of [
    "/v1/client-market/my-rentals",
    "/v1/client-market/clients/:installation_id/rental",
    "/v1/market-access/dashboard",
    "/v1/market-access/policies/:product_kind",
    "/v1/market-access/counterparties",
    "/v1/market-access/counterparties/:id",
    "/v1/market-access/counterparties/:id/credit-lines/:currency",
    "/v1/market-access/public-credit-lines/:currency",
    "/v1/market-billing/dashboard",
    "/v1/market-billing/accounts/:account_id/request-settlement",
  ]) {
    if (!files.some((relativePath) => {
      if (!relativePath.startsWith("src/")) return false;
      return fs.readFileSync(path.join(root, relativePath), "utf8").includes(requiredRoute);
    })) {
      errors.push(`required route is missing: ${requiredRoute}`);
    }
  }

  for (const required of [
    "/v1/chat/clients/:installation_id/room",
    "client_chat_system_outbox",
    "enqueue_client_system_event_tx",
    "'system', 'market_event'",
    "sanitize_system_event_payload",
  ]) {
    const source = required.startsWith("/v1/chat/") ? apiSource : chatSource;
    if (!source.includes(required)) {
      errors.push(`Client public chat contract is missing ${required}`);
    }
  }
  if (!chatSource.includes("parsed.fragment()")) {
    errors.push("Client chat credential URL guard must inspect fragment parameters");
  }
  const chatRendererSource = fs.readFileSync(
    path.join(root, "frontend/components/chat/chat-system-event.ts"),
    "utf8",
  );
  if (!chatRendererSource.includes("url.hash.slice(1)")) {
    errors.push("Client chat UI credential URL guard must inspect fragment parameters");
  }

  for (const [label, source, required] of [
    ["Share Market chat integration", shareSource, "enqueue_client_system_event_tx"],
    ["Client Market chat integration", tradeSource, "enqueue_client_system_event_tx"],
    ["Market Billing chat integration", billingSource, "enqueue_client_system_event_tx"],
  ]) {
    if (!source.includes(required)) errors.push(`${label} is missing ${required}`);
  }
  for (const [label, block] of [
    ["Share Market source event", extractRustBlock(shareSource, "fn event_tx")],
    ["Client Market source event", extractRustBlock(tradeSource, "fn insert_audit_tx")],
    ["Market Billing source event", extractRustBlock(billingSource, "fn record_event_tx")],
  ]) {
    if (!block.includes("sanitize_system_event_payload")) {
      errors.push(`${label} must sanitize details before source-event persistence`);
    }
  }
  if (!chatSource.includes('normalized == "token" && allows_payment_asset_token')) {
    errors.push("Client chat token exception must be restricted to crypto payment assets");
  }

  const paymentAssetReader = extractRustBlock(
    tradeSource,
    "pub async fn client_market_payment_asset_for_viewer",
  );
  for (const required of [
    "market_invoices",
    "market_credit_accounts",
    "chat_public_payment_assets",
  ]) {
    if (!paymentAssetReader.includes(required)) {
      errors.push(`payment asset authorization is missing ${required}`);
    }
  }
  for (const forbidden of [
    "router_ssh_hosts",
    "client_market_subscriptions",
    "share_market_",
    "market_service_contracts",
  ]) {
    if (paymentAssetReader.includes(forbidden)) {
      errors.push(`payment asset authorization must not depend on ${forbidden}`);
    }
  }

  for (const relativePath of walk("frontend/components")) {
    const source = fs.readFileSync(path.join(root, relativePath), "utf8");
    if (!source.includes("ProviderPaymentMethodsList")) continue;
    if (![
      "frontend/components/common/provider-contacts.tsx",
      "frontend/components/dashboard/account-billing-page.tsx",
    ].includes(relativePath)) {
      errors.push(`${relativePath} renders full payment methods outside Account Billing`);
    }
  }

  if (errors.length) {
    console.error(`market billing contract audit failed:\n${errors.map((error) => `- ${error}`).join("\n")}`);
    process.exit(1);
  }

  console.log(`market billing contract audit ok: ${files.length} source/document files checked`);
}

main();
