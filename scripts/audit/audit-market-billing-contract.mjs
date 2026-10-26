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
  // A C2C credit reservation has its own offline-payment window. It is not
  // the retired per-Share/per-Client prepaid billing deadline. No other
  // legacy billing symbols are allowed in the new module or its tests.
  if ((relativePath === "src/market_c2c.rs" || relativePath.startsWith("src/market_c2c/")) && ruleName === "product payment deadline") {
    return true;
  }
  if (relativePath === "src/schema.rs" && ruleName === "product payment deadline") {
    for (const declaration of [
      "fn migration_80_requires_explicit_publication_and_preserves_private_quote_evidence()",
      "fn migration_83_preserves_active_reservations_evidence_and_running_deadlines()",
      "fn migration_84_preserves_frozen_orders_and_requires_explicit_automatic_consent()",
    ]) {
      const start = source.indexOf(declaration);
      if (start >= 0 && index >= start && index < start + extractRustBlock(source, declaration).length) return true;
    }
  }
  if (relativePath === "src/market_notifications.rs" && ruleName === "product payment deadline") {
    // C2C delivery rechecks its own live deadline before sending actionable
    // instructions. Do not exempt unrelated notification/payment code.
    for (const declaration of [
      "fn c2c_notice_current_tx(",
      "fn c2c_delivery_discards_stale_instructions_and_revoked_or_conflicting_handlers()",
    ]) {
      const start = source.indexOf(declaration);
      if (start >= 0 && index >= start && index < start + extractRustBlock(source, declaration).length) return true;
    }
  }
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
    "account_email",
    "api_url",
    "accountEmail",
    "apiUrl",
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
        errors.push(relativePath + ":" + lineNumber(source, match.index) + " contains " + ruleName);
      }
    }
  }
  const read = relativePath => fs.readFileSync(path.join(root, relativePath), "utf8");
  for (const retired of [
    "src/binance_settlement/mod.rs", "src/market_billing.rs", "src/market_recurring.rs",
    "frontend/lib/market-funding.ts", "frontend/lib/binance-auto-settlement.ts",
    "frontend/components/dashboard/market-funding-topup-dialog.tsx",
    "frontend/components/dashboard/market-recurring-contract-card.tsx",
    "frontend/components/dashboard/account-billing-page.tsx",
    "frontend/components/dashboard/market-wallet-refund-panel.tsx",
    "frontend/components/dashboard/binance-api-key-guide-dialog.tsx",
    "frontend/components/dashboard/binance-receipt-history-dialog.tsx",
    "frontend/components/common/payment-method-icons.tsx",
    "frontend/app/(dashboard)/account/billing/page.tsx",
    "frontend/app/(dashboard)/account/payments/page.tsx",
  ]) {
    if (fs.existsSync(path.join(root, retired))) errors.push("retired payment implementation returned: " + retired);
  }
  const main = read("src/main.rs"), api = read("frontend/lib/api.ts"), types = read("frontend/lib/types.ts");
  const settings = read("src/admin/settings.rs"), access = read("src/market_access.rs");
  const trade = read("src/client_market_trade.rs"), share = read("src/share_market.rs");
  for (const [label, source, forbidden] of [
    ["runtime", main, /binance_settlement|mod market_billing|mod market_recurring|run_market_billing_worker/],
    ["Settings", settings, /CC_SWITCH_ROUTER_BINANCE|Binance settlement/],
    ["frontend API", api, /\/v1\/(?:admin\/)?market-billing|\/v1\/account\/(?:payment-profile|payment-assets|binance-auto-settlement)|credit-lines|price-changes|termination-quotes/],
    ["frontend DTO", types, /\b(?:MarketFundingSummary|MarketRecurringContract|MarketCreditLine|BinanceFundingIntent|ClientMarketPaymentMethod)\b/],
    ["frontend messages", read("frontend/lib/i18n.ts"), /"(?:account\.binanceAuto\.|marketReadiness\.|marketFunding\.|marketRecurring\.|createClient\.quoteTerms\.postpaid|clientMarket\.offerRequires(?:Payment|Settlement)|shareMarket\.(?:catalog\.postpaidHint|subscription\.activePostpaid))/],
    ["Host offer prerequisites", read("frontend/components/dashboard/client-market/host-utils.ts"), /MARKET_(?:PAYMENT|SUPPLIER_SETTLEMENT)_PROFILE_REQUIRED|isPaymentProfileRequiredError|hostPaidOfferPrerequisiteError/],
    ["contact runtime", trade.split("#[cfg(test)]")[0], /account_payment_profiles|market_payment_assets/],
    ["access runtime", access.split("#[cfg(test)]")[0], /market_credit_accounts|market_credit_grants|market_prepaid_accounts/],
  ]) if (forbidden.test(source)) errors.push(label + " revives retired finance");
  for (const [label, block] of [
    ["Client rental", extractRustBlock(trade, "pub struct RentalView")],
    ["Share subscription", extractRustBlock(share, "pub struct SubscriptionView")],
    ["Share listing", extractRustBlock(share, "pub struct ListingView")],
    ["Client rental DTO", extractTypeBlock(types, "ClientMarketRental")],
    ["Share subscription DTO", extractTypeBlock(types, "ShareMarketSubscription")],
  ]) assertProductDtoIsRedacted(errors, label, block);
  for (const [label, source, required] of [
    ["Client checkout", read("frontend/lib/client-checkout.ts"), ["walletFunding", "fundingRail", "platform_wallet"]],
    ["Share checkout", read("frontend/lib/share-checkout.ts"), ["walletFunding", "unsupported", "MARKET_WALLET_REQUIRED"]],
    ["wallet lifecycle", read("src/market_platform_contracts.rs"), ["cancel_at_period_end_tx", "terminate_after_cleanup_inner_tx", "fail_activation_tx"]],
    ["independent FX", read("src/market_fx.rs"), ["/v1/market/config", "fxRateFresh", "no-store"]],
    ["contact separation", trade, ["FROM market_provider_contacts", "INSERT INTO market_provider_contacts"]],
    ["Client charge history", extractRustBlock(trade, "pub async fn client_market_host_usage_history"), ["market_platform_periods", "market_usage_posting_batches", "refunded_units"]],
    ["trial renewal UI", read("frontend/components/dashboard/share-market/buyer-rentals.tsx"), ["subscription.walletContract?.id", "MarketWalletRenewalCard"]],
  ]) for (const item of required) if (!source.includes(item)) errors.push(label + " is missing " + item);
  const redaction = extractRustBlock(share, "fn redact_public_subscription");
  for (const field of ["wallet_contract = None", "wallet_contract_id = None", "open_wallet_dispute = false"])
    if (!redaction.includes(field)) errors.push("public Share projection exposes wallet detail: " + field);
  if (errors.length) {
    console.error("market wallet contract audit failed:\n" + errors.map(error => "- " + error).join("\n"));
    process.exit(1);
  }
  console.log("market wallet contract audit ok: " + files.length + " source/document files checked");
}

main();
