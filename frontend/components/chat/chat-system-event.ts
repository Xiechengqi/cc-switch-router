import type { MessageKey } from "@/lib/i18n";
import type { ClientChatMessage, ClientChatMessagePreview } from "@/lib/types";

type TFn = (key: MessageKey, values?: Record<string, string | number>) => string;
type StructuredChatMessage = Pick<
  ClientChatMessage | ClientChatMessagePreview,
  "body" | "messageKind" | "eventType" | "eventPayload"
>;

export type ChatSystemEventDetail = {
  key: string;
  label: string;
  value: string;
  href?: string;
};

const DETAIL_LABELS: Record<string, MessageKey> = {
  accountId: "chat.detail.accountId",
  accountStatus: "chat.detail.accountStatus",
  actorEmail: "chat.detail.actorEmail",
  actorUserId: "chat.detail.actorUserId",
  address: "chat.detail.address",
  amountMinor: "chat.detail.amount",
  appType: "chat.detail.appType",
  assetUrl: "chat.detail.assetUrl",
  balanceMinor: "chat.detail.balance",
  billingEventType: "chat.detail.billingEventType",
  buyerEmail: "chat.detail.buyerEmail",
  buyerUserId: "chat.detail.buyerUserId",
  chain: "chat.detail.chain",
  clientLabel: "chat.detail.clientLabel",
  clientOwnerEmail: "chat.detail.clientOwnerEmail",
  clientUserId: "chat.detail.clientUserId",
  contact: "chat.detail.contact",
  createdAt: "chat.detail.createdAt",
  creditKind: "chat.detail.creditKind",
  creditLimitMinor: "chat.detail.creditLimit",
  currency: "chat.detail.currency",
  dailyRateMinor: "chat.detail.dailyRate",
  deadlineAt: "chat.detail.deadlineAt",
  declarationId: "chat.detail.declarationId",
  declaredAt: "chat.detail.declaredAt",
  dispute: "chat.detail.dispute",
  dueAt: "chat.detail.dueAt",
  error: "chat.detail.error",
  evidenceUrl: "chat.detail.evidenceUrl",
  failureCode: "chat.detail.failureCode",
  free: "chat.detail.free",
  futureAccessDenied: "chat.detail.futureAccessDenied",
  hostId: "chat.detail.hostId",
  hostname: "chat.detail.hostname",
  hostStatus: "chat.detail.hostStatus",
  id: "chat.detail.id",
  installationId: "chat.detail.installationId",
  instructions: "chat.detail.instructions",
  invoiceId: "chat.detail.invoiceId",
  invoiceStatus: "chat.detail.invoiceStatus",
  kind: "chat.detail.kind",
  listingId: "chat.detail.listingId",
  marketKind: "chat.detail.marketKind",
  method: "chat.detail.method",
  note: "chat.detail.note",
  offerRevision: "chat.detail.offerRevision",
  ownerEmail: "chat.detail.ownerEmail",
  ownerUserId: "chat.detail.ownerUserId",
  paymentContacts: "chat.detail.paymentContacts",
  paymentDeclaration: "chat.detail.paymentDeclaration",
  paymentMethodKind: "chat.detail.paymentMethodKind",
  paymentMethods: "chat.detail.paymentMethods",
  paymentReference: "chat.detail.paymentReference",
  previousStatus: "chat.detail.previousStatus",
  productKind: "chat.detail.productKind",
  productRef: "chat.detail.productRef",
  providerDeniedClientAccess: "chat.detail.providerDeniedClientAccess",
  providerEmail: "chat.detail.providerEmail",
  qrImageUrl: "chat.detail.qrImageUrl",
  reason: "chat.detail.reason",
  rawError: "chat.detail.rawError",
  rejectedAt: "chat.detail.rejectedAt",
  rejectionReason: "chat.detail.rejectionReason",
  renterEmail: "chat.detail.renterEmail",
  renterUserId: "chat.detail.renterUserId",
  resolution: "chat.detail.resolution",
  resolvedAt: "chat.detail.resolvedAt",
  seatCount: "chat.detail.seatCount",
  seatId: "chat.detail.seatId",
  seatPosition: "chat.detail.seatPosition",
  seatStatus: "chat.detail.seatStatus",
  serviceLabel: "chat.detail.serviceLabel",
  serviceRef: "chat.detail.serviceRef",
  services: "chat.detail.services",
  shareId: "chat.detail.shareId",
  shareName: "chat.detail.shareName",
  status: "chat.detail.status",
  subdomain: "chat.detail.subdomain",
  subscriptionId: "chat.detail.subscriptionId",
  subscriptionStatus: "chat.detail.subscriptionStatus",
  supplierEmail: "chat.detail.supplierEmail",
  supplierUserId: "chat.detail.supplierUserId",
  token: "chat.detail.token",
  tokenLimit: "chat.detail.tokenLimit",
  tokenPeriod: "chat.detail.tokenPeriod",
  parallelLimit: "chat.detail.parallelLimit",
  releaseReason: "chat.detail.releaseReason",
  releasedAt: "chat.detail.releasedAt",
  retiredAt: "chat.detail.retiredAt",
  trialHours: "chat.detail.trialHours",
  utilizationBps: "chat.detail.utilization",
};

const SENSITIVE_DETAIL_FIELDS = [
  "apikey",
  "accesstoken",
  "refreshtoken",
  "idtoken",
  "sessiontoken",
  "oauthtoken",
  "authtoken",
  "apitoken",
  "bearertoken",
  "verificationtoken",
  "provisiontoken",
  "resettoken",
  "csrftoken",
  "token",
  "jwt",
  "authorization",
  "cookie",
  "setcookie",
  "password",
  "secret",
  "privatekey",
  "credential",
  "credentials",
  "leasecredential",
  "passphrase",
];

function normalizedField(value: string) {
  return value.replaceAll(/[^a-z0-9]/gi, "").toLocaleLowerCase();
}

function isSensitiveDetailField(value: string, allowPaymentAssetToken = false) {
  const normalized = normalizedField(value);
  if (normalized === "token" && allowPaymentAssetToken) return false;
  return SENSITIVE_DETAIL_FIELDS.some(
    (forbidden) => normalized === forbidden || normalized.endsWith(forbidden),
  );
}

function containsCredentialFragment(value: string) {
  const normalized = value.toLocaleLowerCase();
  return [
    "authorization:",
    "authorization=",
    "proxy-authorization:",
    "proxy-authorization=",
    "bearer ",
    "x-api-key:",
    "x-api-key=",
    "x-goog-api-key:",
    "x-goog-api-key=",
    "api_key=",
    "apikey=",
    "access_token=",
    "refresh_token=",
    "id_token=",
    "session_token=",
    "oauth_token=",
    "client_secret=",
    "cookie:",
    "set-cookie:",
    "control_secret=",
    "ssh_password=",
    "private_key=",
    "password=",
    "secret=",
  ].some((marker) => normalized.includes(marker))
    || normalized.startsWith("sk-")
    || normalized.includes(" sk-");
}

function safePublicUrl(value: string) {
  try {
    const relative = value.startsWith("/") && !value.startsWith("//");
    const url = new URL(value, "https://router.invalid");
    if (!(["http:", "https:"] as string[]).includes(url.protocol)) return undefined;
    if (value.startsWith("//")) return undefined;
    if (url.username || url.password || !url.hostname) return undefined;
    for (const key of url.searchParams.keys()) {
      const normalized = normalizedField(key);
      if (["token", "signature", "secret", "key", "credential", "authorization", "password"]
        .some((forbidden) => normalized.includes(forbidden))) {
        return undefined;
      }
    }
    for (const key of new URLSearchParams(url.hash.slice(1)).keys()) {
      const normalized = normalizedField(key);
      if (["token", "signature", "secret", "key", "credential", "authorization", "password"]
        .some((forbidden) => normalized.includes(forbidden))) {
        return undefined;
      }
    }
    return relative ? `${url.pathname}${url.search}${url.hash}` : url.toString();
  } catch {
    return undefined;
  }
}

function humanizeField(value: string) {
  return value
    .replaceAll(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replaceAll(/[_-]+/g, " ")
    .replace(/^./, (character) => character.toLocaleUpperCase());
}

function detailPathLabel(path: Array<string | number>, t: TFn) {
  return path
    .map((part) => {
      if (typeof part === "number") return `#${part + 1}`;
      const key = DETAIL_LABELS[part];
      return key ? t(key) : humanizeField(part);
    })
    .join(" / ");
}

function detailScalar(
  value: unknown,
  key: string,
  root: Record<string, unknown>,
  locale: string,
) {
  if (value == null) return undefined;
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "number") {
    if (key.endsWith("Minor")) {
      const currency = typeof root.currency === "string" ? root.currency : "";
      return `${new Intl.NumberFormat(locale, {
        minimumFractionDigits: 2,
        maximumFractionDigits: 2,
      }).format(value / 100)} ${currency}`.trim();
    }
    if (key === "utilizationBps") {
      return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(value / 100)}%`;
    }
    return new Intl.NumberFormat(locale).format(value);
  }
  if (typeof value !== "string") return String(value);
  if (!value.trim()) return undefined;
  if (containsCredentialFragment(value)) return "[credential omitted]";
  if (key.endsWith("At")) {
    const date = new Date(value);
    if (Number.isFinite(date.getTime())) {
      return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
    }
  }
  return value;
}

function flattenEventDetails(
  value: unknown,
  path: Array<string | number>,
  root: Record<string, unknown>,
  locale: string,
  t: TFn,
  output: ChatSystemEventDetail[],
  allowPaymentAssetToken = false,
) {
  const leaf = [...path].reverse().find((part): part is string => typeof part === "string") || "value";
  if (isSensitiveDetailField(leaf, allowPaymentAssetToken)) return;
  if (Array.isArray(value)) {
    value.forEach((entry, index) => flattenEventDetails(
      entry,
      [...path, index],
      root,
      locale,
      t,
      output,
    ));
    return;
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const allowsCryptoAssetToken = record.kind === "crypto"
      && (record.token === "USDT" || record.token === "USDC");
    Object.entries(record).forEach(([key, entry]) => {
      const allowsToken = normalizedField(key) === "token" && allowsCryptoAssetToken;
      if (!isSensitiveDetailField(key, allowsToken)) {
        flattenEventDetails(entry, [...path, key], root, locale, t, output, allowsToken);
      }
    });
    return;
  }
  const displayValue = detailScalar(value, leaf, root, locale);
  if (!displayValue) return;
  const looksLikeUrl = typeof value === "string" && (/^https?:\/\//i.test(value)
    || (normalizedField(leaf).endsWith("url")
      && value.startsWith("/") && !value.startsWith("//")));
  const urlCandidate = looksLikeUrl
    ? safePublicUrl(value)
    : undefined;
  output.push({
    key: path.map(String).join("."),
    label: detailPathLabel(path, t),
    value: urlCandidate || (looksLikeUrl
      ? "[unsafe URL omitted]"
      : displayValue),
    href: urlCandidate,
  });
}

export function chatSystemEventDetails(
  message: StructuredChatMessage,
  t: TFn,
  locale: string,
) {
  if (message.messageKind !== "market_event" || !message.eventPayload) return [];
  const payload = message.eventPayload;
  const output: ChatSystemEventDetail[] = [];
  Object.entries(payload).forEach(([key, value]) => {
    if (key !== "summary" && !isSensitiveDetailField(key)) {
      flattenEventDetails(value, [key], payload, locale, t, output);
    }
  });
  return output;
}

function payloadString(payload: Record<string, unknown>, key: string, fallback = "-") {
  const value = payload[key];
  return typeof value === "string" && value.trim() ? value : fallback;
}

function payloadNumber(payload: Record<string, unknown>, key: string, fallback = 0) {
  const value = payload[key];
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function payloadTime(payload: Record<string, unknown>, key: string, locale: string) {
  const value = payloadString(payload, key, "");
  const date = new Date(value);
  if (!value || !Number.isFinite(date.getTime())) return "-";
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function payloadAmount(payload: Record<string, unknown>) {
  const amountMinor = payloadNumber(payload, "amountMinor");
  const currency = payloadString(payload, "currency", "");
  return `${new Intl.NumberFormat(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(amountMinor / 100)} ${currency}`.trim();
}

function payloadPercent(payload: Record<string, unknown>, key: string, locale: string) {
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(
    payloadNumber(payload, key) / 100,
  )}%`;
}

export function chatSystemEventText(message: StructuredChatMessage, t: TFn, locale: string) {
  if (message.messageKind !== "market_event") return message.body;
  const payload = message.eventPayload || {};
  const seat = payloadNumber(payload, "seatPosition");
  const renter = payloadString(payload, "renterEmail");
  switch (message.eventType) {
    case "listing_created":
      return t("chat.event.listingCreated", { count: payloadNumber(payload, "seatCount") });
    case "listing_relisted":
      return t("chat.event.listingRelisted");
    case "listing_closed":
      return t("chat.event.listingClosed");
    case "listing_deleted":
      return t("chat.event.listingDeleted");
    case "seat_added":
      return t("chat.event.seatAdded", { seat });
    case "seat_updated":
      return t("chat.event.seatUpdated", { seat });
    case "seat_deleted":
      return t("chat.event.seatDeleted", { seat });
    case "seat_rented":
      return t("chat.event.seatRented", { seat, renter });
    case "seat_retired":
      return t("chat.event.seatRetired", { seat });
    case "entitlement_activated":
      return t("chat.event.entitlementActivated", { seat, renter });
    case "entitlement_failed":
      return t("chat.event.entitlementFailed", { seat, renter });
    case "payment_due":
      return t("chat.event.paymentDue", {
        amount: payloadAmount(payload),
        time: payloadTime(payload, "deadlineAt", locale),
      });
    case "payment_declared":
      return t("chat.event.paymentDeclared", { amount: payloadAmount(payload) });
    case "billing_payment_confirmed":
      return t("chat.event.billingPaymentConfirmed", { amount: payloadAmount(payload) });
    case "billing_payment_rejected":
      return t("chat.event.billingPaymentRejected", {
        amount: payloadAmount(payload),
        reason: payloadString(payload, "reason"),
      });
    case "billing_payment_overdue":
      return t("chat.event.billingPaymentOverdue", {
        amount: payloadAmount(payload),
        time: payloadTime(payload, "deadlineAt", locale),
      });
    case "billing_invoice_disputed":
      return t("chat.event.billingInvoiceDisputed", {
        amount: payloadAmount(payload),
        reason: payloadString(payload, "reason"),
      });
    case "billing_dispute_resolved":
      return t(
        payloadString(payload, "resolution", "uphold") === "void"
          ? "chat.event.billingDisputeResolvedVoid"
          : "chat.event.billingDisputeResolvedUpheld",
        { amount: payloadAmount(payload) },
      );
    case "billing_invoice_voided":
      return t("chat.event.billingInvoiceVoided", {
        amount: payloadAmount(payload),
        reason: payloadString(payload, "reason"),
      });
    case "billing_credit_limit_warning":
      return t("chat.event.billingCreditLimitWarning", {
        utilization: payloadPercent(payload, "utilizationBps", locale),
      });
    case "billing_account_closing":
      return t("chat.event.billingAccountClosing");
    case "client_provisioned":
      return t("chat.event.clientProvisioned", {
        client: payloadString(payload, "clientLabel"),
      });
    case "cleanup_started":
      return t("chat.event.cleanupStarted", {
        client: payloadString(payload, "clientLabel"),
        reason: payloadString(payload, "reason"),
      });
    case "cleanup_finished":
    case "subscription_force_released":
      return t("chat.event.cleanupFinished", {
        client: payloadString(payload, "clientLabel"),
      });
    case "cleanup_failed":
      return t("chat.event.cleanupFailed", {
        client: payloadString(payload, "clientLabel"),
        error: payloadString(payload, "failureCode"),
      });
    case "renter_release_requested":
      return t("chat.event.renterReleaseRequested", { seat, renter });
    case "owner_revoke_requested":
    case "entitlement_revoke_requested":
      return t("chat.event.ownerRevokeRequested", { seat, renter });
    case "subscription_released":
      return t("chat.event.subscriptionReleased", { seat, renter });
    case "revoke_failed":
      return t("chat.event.revokeFailed", { seat, renter });
    case "billing_suspension_requested":
      return t("chat.event.billingSuspensionRequested", { seat, renter });
    case "billing_suspended":
      return t("chat.event.billingSuspended", { seat, renter });
    case "billing_suspension_failed":
      return t("chat.event.billingSuspensionFailed", { seat, renter });
    case "billing_resume_requested":
      return t("chat.event.billingResumeRequested", { seat, renter });
    case "billing_resumed":
      return t("chat.event.billingResumed", { seat, renter });
    case "billing_resume_failed":
      return t("chat.event.billingResumeFailed", { seat, renter });
    case "share_enabled":
      return t("chat.event.shareEnabled");
    case "share_disabled":
      return t("chat.event.shareDisabled");
    case "share_expiration_changed":
      return t("chat.event.shareExpirationChanged", {
        time: payloadTime(payload, "expiresAt", locale),
      });
    case "share_expired":
      return t("chat.event.shareExpired", {
        time: payloadTime(payload, "expiresAt", locale),
      });
    case "share_provider_changed":
      return t("chat.event.shareProviderChanged");
    case "service_offline":
      return t("chat.event.serviceOffline");
    case "service_recovered":
      return t("chat.event.serviceRecovered");
    default:
      return t("chat.event.unknown", { event: message.eventType || message.body || "-" });
  }
}

export function chatMessageAuthorLabel(
  message: Pick<ClientChatMessage | ClientChatMessagePreview, "authorKind" | "authorLabel">,
  t: TFn,
) {
  return message.authorKind === "system" ? t("chat.systemMessage") : message.authorLabel;
}
