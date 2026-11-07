import type { ShareUpstreamProvider } from "@/lib/types";
import { messages, type AppLocale } from "@/lib/i18n";
type QuotaTier = NonNullable<NonNullable<ShareUpstreamProvider["quota"]>["tiers"]>[number];

type Translate = (key: string, options?: Record<string, unknown>) => string;

const ALIASES = new Map([
  ["CodeBuddy个人版拉新权益包", "buddyRegistrationCredits"],
  ["CodeBuddy个人版国内运营裂变包", "buddyCheckinCredits"],
  ["CodeBuddy个人体验版", "buddyBaseCredits"],
]);

// Share descriptors omit internal package names; the reviewed source survives projection.
const isBuddyPackage = (tier: QuotaTier): boolean => /^codebuddy_package_[1-9]\d*$/.test(tier.name ?? "")
  || (!tier.name && tier.source === "codebuddy_billing_resource_v2");

const numberFormat = new Intl.NumberFormat("en-US", { maximumFractionDigits: 2, useGrouping: false });
function number(value: number): string {
  return numberFormat.format(value);
}

function amount(tier: QuotaTier): boolean {
  return typeof tier.used === "number" && Number.isFinite(tier.used) && tier.used >= 0
    && typeof tier.limit === "number" && Number.isFinite(tier.limit) && tier.limit >= tier.used;
}

function time(value: string | null | undefined): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function countdown(at: number, now: number): string {
  const minutes = Math.max(1, Math.floor((at - now) / 60_000));
  const hours = Math.floor(minutes / 60);
  if (hours >= 24) return `${Math.floor(hours / 24)}d${hours % 24}h`;
  return hours > 0 ? `${hours}h${minutes % 60}m` : `${minutes}m`;
}

/** Display-only: keep batches intact; expiry never falls back to a cycle reset. */
export function buildBuddyQuotaSummary(
  tiers: QuotaTier[],
  t: Translate,
  now: number,
  partial = false,
): { text: string; details: string } {
  const groups: { label: string; used: number | null; limit: number | null; unit: string; fallback: string }[] = [];
  const details = [t("subscription.buddyAmountHelp")];
  const candidates: number[] = [];
  let unknownExpiry = partial;
  let pending = false;
  let hasBalance = false;
  let checkinGroup: typeof groups[number] | undefined;

  for (const tier of tiers) {
    const original = tier.label?.trim() || tier.name || "";
    // Exact reviewed CN names only. Unknown products/plans retain vendor labels.
    const alias = isBuddyPackage(tier) ? ALIASES.get(original.replace(/\s+/g, "")) : undefined;
    const label = alias ? t(`subscription.${alias}`) : original;
    const valid = amount(tier);
    const unit = tier.unit?.trim() || "";
    const expiry = time(tier.expiresAt);
    const reset = time(tier.resetsAt);
    const expired = expiry !== null && expiry <= now;
    const amountText = valid ? `${number(tier.used!)}/${number(tier.limit!)}${unit ? ` ${unit}` : ""}` : t("subscription.numericQuotaNotExposed");
    details.push([
      `${original} ${amountText}`,
      valid ? t("subscription.buddyRemaining", { amount: number(tier.limit! - tier.used!), unit }) : null,
      expiry !== null ? t("subscription.buddyExpiryDetail", { time: new Date(expiry).toLocaleString() }) : t("subscription.buddyExpiryUnknown"),
      reset !== null ? t("subscription.buddyResetDetail", { time: new Date(reset).toLocaleString() }) : null,
    ].filter(Boolean).join(" · "));

    if (expired) {
      pending = true;
      continue;
    }
    if (valid && tier.limit! > tier.used!) {
      hasBalance = true;
      if (expiry === null) unknownExpiry = true;
      else candidates.push(expiry);
    } else if (!valid) {
      unknownExpiry = true;
    }

    const canMerge = alias === "buddyCheckinCredits" && valid && unit.toLowerCase() === "credits";
    if (canMerge && checkinGroup
      && Number.isFinite(checkinGroup.used! + tier.used!)
      && Number.isFinite(checkinGroup.limit! + tier.limit!)) {
      checkinGroup.used! += tier.used!;
      checkinGroup.limit! += tier.limit!;
    } else {
      const group = {
        label, used: valid ? tier.used! : null, limit: valid ? tier.limit! : null,
        unit: canMerge ? "credits" : unit,
        fallback: typeof tier.utilization === "number" && Number.isFinite(tier.utilization) && tier.utilization >= 0 && tier.utilization <= 100
          ? `${tier.utilization > 0 && tier.utilization < 0.1 ? "<0.1" : number(tier.utilization)}%`
          : t("subscription.numericQuotaNotExposed"),
      };
      groups.push(group);
      if (canMerge) checkinGroup = group;
    }
  }

  const parts = groups.map((group) => `${group.label} ${group.used !== null && group.limit !== null
    ? `${number(group.used)}/${number(group.limit)}${group.unit ? ` ${group.unit}` : ""}` : group.fallback}`);
  if (candidates.length) {
    const earliest = Math.min(...candidates);
    parts.push(t(unknownExpiry ? "subscription.buddyNextKnownExpiry" : "subscription.buddyNextExpiry", { time: countdown(earliest, now) }));
  } else if (hasBalance || unknownExpiry) {
    parts.push(t("subscription.buddyExpiryUnknown"));
  }
  if (pending) parts.push(t("subscription.buddyExpiryPending"));
  return { text: parts.join(" · "), details: details.join("\n") };
}

/** Local presentation rules; no Server source or runtime dependency. */
export function buddyQuotaPresentation(tiers: QuotaTier[], locale: AppLocale, now = Date.now(), partial = false) {
  const t: Translate = (key, options) => {
    const template = messages[locale][(key === "subscription.numericQuotaNotExposed" ? "quota.buddy.numericNotExposed" : key.replace("subscription.", "quota.buddy.")) as keyof typeof messages[typeof locale]];
    return String(template).replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(options?.[name] ?? ""));
  };
  return buildBuddyQuotaSummary(tiers, t, now, partial);
}
