import type { MessageKey } from "@/lib/i18n";
import { formatUsdCnyMoney, usdMinorToCnyMinor } from "@/lib/market-money";
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
  activatedAt: "chat.detail.activatedAt",
  address: "chat.detail.address",
  amountMinor: "chat.detail.amount",
  appType: "chat.detail.appType",
  assetUrl: "chat.detail.assetUrl",
  balanceMinor: "chat.detail.balance",
  buyerEmail: "chat.detail.buyerEmail",
  chain: "chat.detail.chain",
  clientOwnerEmail: "chat.detail.clientOwnerEmail",
  contact: "chat.detail.contact",
  createdAt: "chat.detail.createdAt",
  creditKind: "chat.detail.creditKind",
  creditLimitMinor: "chat.detail.creditLimit",
  currency: "chat.detail.currency",
  dailyRateMinor: "chat.detail.dailyRate",
  deadlineAt: "chat.detail.deadlineAt",
  declaredAt: "chat.detail.declaredAt",
  dispute: "chat.detail.dispute",
  dueAt: "chat.detail.dueAt",
  error: "chat.detail.error",
  evidenceUrl: "chat.detail.evidenceUrl",
  expiresAt: "chat.detail.expiresAt",
  failureCode: "chat.detail.failureCode",
  free: "chat.detail.free",
  freeDurationDays: "chat.detail.freeDurationDays",
  futureAccessDenied: "chat.detail.futureAccessDenied",
  hostname: "chat.detail.hostname",
  instructions: "chat.detail.instructions",
  invoiceStatus: "chat.detail.invoiceStatus",
  kind: "chat.detail.kind",
  method: "chat.detail.method",
  note: "chat.detail.note",
  ownerEmail: "chat.detail.ownerEmail",
  paymentContacts: "chat.detail.paymentContacts",
  paymentDeclaration: "chat.detail.paymentDeclaration",
  paymentMethodKind: "chat.detail.paymentMethodKind",
  paymentMethods: "chat.detail.paymentMethods",
  paymentReference: "chat.detail.paymentReference",
  providerDeniedClientAccess: "chat.detail.providerDeniedClientAccess",
  providerEmail: "chat.detail.providerEmail",
  qrImageUrl: "chat.detail.qrImageUrl",
  reason: "chat.detail.reason",
  rejectionReason: "chat.detail.rejectionReason",
  renterEmail: "chat.detail.renterEmail",
  resolution: "chat.detail.resolution",
  resolvedAt: "chat.detail.resolvedAt",
  seatCount: "chat.detail.seatCount",
  seatPosition: "chat.detail.seatPosition",
  seatStatus: "chat.detail.seatStatus",
  subdomain: "chat.detail.subdomain",
  subscriptionStatus: "chat.detail.subscriptionStatus",
  supplierEmail: "chat.detail.supplierEmail",
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

/**
 * User-facing detail keys per market event. Empty = summary only.
 * Technical IDs / internal bookkeeping fields are intentionally omitted.
 */
const DETAIL_FIELDS_BY_EVENT: Record<string, readonly string[]> = {
  client_provisioned: [
    "providerEmail",
    "hostname",
    "dailyRateMinor",
    "currency",
    "trialHours",
    "freeDurationDays",
    "activatedAt",
    "expiresAt",
  ],
  free_period_expiring: ["expiresAt", "freeDurationDays", "hostname", "providerEmail"],
  free_period_expired: ["expiresAt", "hostname", "providerEmail"],
  cleanup_started: ["reason", "hostname", "providerEmail"],
  cleanup_finished: ["hostname", "providerEmail", "providerDeniedClientAccess"],
  subscription_force_released: [
    "hostname",
    "providerEmail",
    "providerDeniedClientAccess",
    "reason",
  ],
  cleanup_failed: ["failureCode", "reason", "hostname", "providerEmail"],

  listing_created: ["seatCount", "dailyRateMinor", "currency"],
  listing_relisted: [],
  listing_closed: [],
  listing_deleted: [],
  seat_added: ["seatPosition", "dailyRateMinor", "currency"],
  seat_updated: [
    "seatPosition",
    "dailyRateMinor",
    "currency",
    "tokenLimit",
    "tokenPeriod",
    "parallelLimit",
  ],
  seat_deleted: ["seatPosition"],
  seat_rented: ["seatPosition", "renterEmail", "dailyRateMinor", "currency", "trialHours"],
  seat_retired: ["seatPosition"],
  entitlement_activated: ["seatPosition", "renterEmail"],
  entitlement_failed: ["seatPosition", "renterEmail", "error", "failureCode"],
  renter_release_requested: ["seatPosition", "renterEmail"],
  owner_revoke_requested: ["seatPosition", "renterEmail"],
  entitlement_revoke_requested: ["seatPosition", "renterEmail"],
  subscription_released: ["seatPosition", "renterEmail"],
  revoke_failed: ["seatPosition", "renterEmail", "error", "failureCode"],
  billing_suspension_requested: ["seatPosition", "renterEmail"],
  billing_suspended: ["seatPosition", "renterEmail"],
  billing_suspension_failed: ["seatPosition", "renterEmail", "error", "failureCode"],
  billing_resume_requested: ["seatPosition", "renterEmail"],
  billing_resumed: ["seatPosition", "renterEmail"],
  billing_resume_failed: ["seatPosition", "renterEmail", "error", "failureCode"],
  share_enabled: [],
  share_disabled: [],
  share_expiration_changed: ["expiresAt"],
  share_expired: ["expiresAt"],
  share_provider_changed: [],
  service_offline: [],
  service_recovered: [],

  payment_due: [
    "amountMinor",
    "currency",
    "dueAt",
    "deadlineAt",
    "supplierEmail",
    "paymentMethods",
    "paymentContacts",
  ],
  payment_declared: [
    "amountMinor",
    "currency",
    "buyerEmail",
    "paymentReference",
    "evidenceUrl",
    "assetUrl",
    "method",
    "note",
  ],
  billing_payment_confirmed: ["amountMinor", "currency", "supplierEmail"],
  billing_payment_rejected: ["amountMinor", "currency", "reason", "rejectionReason"],
  billing_payment_overdue: ["amountMinor", "currency", "deadlineAt"],
  billing_invoice_disputed: ["amountMinor", "currency", "reason"],
  billing_dispute_resolved: ["amountMinor", "currency", "resolution"],
  billing_invoice_voided: ["amountMinor", "currency", "reason"],
  billing_credit_limit_warning: [
    "utilizationBps",
    "balanceMinor",
    "creditLimitMinor",
    "currency",
  ],
  billing_account_closing: ["supplierEmail", "reason"],
};

/** Conservative fallback for unknown event types — never dump IDs. */
const FALLBACK_DETAIL_FIELDS = [
  "amountMinor",
  "currency",
  "reason",
  "error",
  "failureCode",
  "deadlineAt",
  "dueAt",
  "expiresAt",
  "activatedAt",
  "renterEmail",
  "ownerEmail",
  "providerEmail",
  "supplierEmail",
  "buyerEmail",
  "seatPosition",
  "hostname",
] as const;

function looksLikeUuid(value: string) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
    value.trim(),
  );
}

function payloadHasMoneyAmount(payload: Record<string, unknown>) {
  return ["amountMinor", "dailyRateMinor", "balanceMinor", "creditLimitMinor"].some(
    (key) => typeof payload[key] === "number" && Number.isFinite(payload[key] as number),
  );
}

function shouldIncludeDetailKey(key: string, payload: Record<string, unknown>) {
  if (!(key in payload) || payload[key] == null) return false;
  if (key === "currency" && !payloadHasMoneyAmount(payload)) return false;
  if (key === "hostname" || key === "clientLabel" || key === "shareName") {
    const value = payload[key];
    return typeof value === "string" && value.trim().length > 0 && !looksLikeUuid(value);
  }
  return true;
}

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
      if (key === "amountMinor") {
        const amountUsdMinor = typeof root.amountUsdMinor === "number" ? root.amountUsdMinor : value;
        const amountCnyMinor = typeof root.amountCnyMinor === "number" ? root.amountCnyMinor : undefined;
        return formatUsdCnyMoney(amountUsdMinor, locale, amountCnyMinor);
      }
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
  const eventType = message.eventType || "";
  const allowed = Object.prototype.hasOwnProperty.call(DETAIL_FIELDS_BY_EVENT, eventType)
    ? DETAIL_FIELDS_BY_EVENT[eventType]
    : FALLBACK_DETAIL_FIELDS;
  if (!allowed.length) return [];
  const output: ChatSystemEventDetail[] = [];
  for (const key of allowed) {
    if (!shouldIncludeDetailKey(key, payload) || isSensitiveDetailField(key)) continue;
    flattenEventDetails(payload[key], [key], payload, locale, t, output);
  }
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

function payloadAmount(payload: Record<string, unknown>, locale: string) {
  const amountUsdMinor = payloadNumber(
    payload,
    "amountUsdMinor",
    payloadNumber(payload, "amountMinor"),
  );
  const amountCnyMinor = payloadNumber(
    payload,
    "amountCnyMinor",
    usdMinorToCnyMinor(amountUsdMinor),
  );
  return formatUsdCnyMoney(amountUsdMinor, locale, amountCnyMinor);
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
        amount: payloadAmount(payload, locale),
        time: payloadTime(payload, "deadlineAt", locale),
      });
    case "payment_declared":
      return t("chat.event.paymentDeclared", { amount: payloadAmount(payload, locale) });
    case "billing_payment_confirmed":
      return t("chat.event.billingPaymentConfirmed", { amount: payloadAmount(payload, locale) });
    case "billing_payment_rejected":
      return t("chat.event.billingPaymentRejected", {
        amount: payloadAmount(payload, locale),
        reason: payloadString(payload, "reason"),
      });
    case "billing_payment_overdue":
      return t("chat.event.billingPaymentOverdue", {
        amount: payloadAmount(payload, locale),
        time: payloadTime(payload, "deadlineAt", locale),
      });
    case "billing_invoice_disputed":
      return t("chat.event.billingInvoiceDisputed", {
        amount: payloadAmount(payload, locale),
        reason: payloadString(payload, "reason"),
      });
    case "billing_dispute_resolved":
      return t(
        payloadString(payload, "resolution", "uphold") === "void"
          ? "chat.event.billingDisputeResolvedVoid"
          : "chat.event.billingDisputeResolvedUpheld",
        { amount: payloadAmount(payload, locale) },
      );
    case "billing_invoice_voided":
      return t("chat.event.billingInvoiceVoided", {
        amount: payloadAmount(payload, locale),
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
    case "free_period_expiring":
      return t("chat.event.freePeriodExpiring", {
        client: payloadString(payload, "clientLabel"),
        time: payloadTime(payload, "expiresAt", locale),
      });
    case "free_period_expired":
      return t("chat.event.freePeriodExpired", {
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
