"use client";

/* A seat may sell a USD-equivalent quota instead of a Token count. These
 * pieces keep the seat editor honest about it: whether this Share's Server
 * can enforce one right now, and what the buyer is actually getting.
 */

import * as React from "react";
import { Loader2, RotateCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  formatUsdLimit,
  getMarketValueQuotaReadiness,
  getRentalValueUsage,
  type MarketValueQuotaReadiness,
  type RentalValueUsage,
} from "@/lib/share-value-quota";
import {
  formatSeatValueDiscount,
  formatSeatValueRatio,
  seatValueDiscount,
} from "@/components/dashboard/share-market/market-utils";
import type { MessageKey } from "@/lib/i18n";
import type { ShareMarketRentQuote } from "@/lib/types";
import { valueReadinessActor } from "@/lib/use-value-quota-readiness";

/** USD requests per buyer never run more than this many at once. */
export const USD_SEAT_PARALLEL_CAP = 4;

const READINESS_TTL_MS = 30_000;
const readinessCache = new Map<string, { at: number; promise: Promise<MarketValueQuotaReadiness> }>();

/** One Server round trip per Share per half minute, however many seats render. */
function loadReadiness(shareId: string, actor: string, force: boolean) {
  const key = `${actor}:${shareId}`;
  const cached = readinessCache.get(key);
  if (!force && cached && Date.now() - cached.at < READINESS_TTL_MS) return cached.promise;
  const promise = getMarketValueQuotaReadiness(shareId).catch(() => ({ ready: false, reason: "quota_server_invalid_response" }));
  if (readinessCache.size >= 64) readinessCache.clear();
  readinessCache.set(key, { at: Date.now(), promise });
  return promise;
}

/** Isolate quota reads and stale responses across logout/login and tabs. */
export function useQuotaActor() {
  const [actor, setActor] = React.useState(valueReadinessActor);
  React.useEffect(() => {
    const update = () => setActor(valueReadinessActor());
    window.addEventListener("router-auth-changed", update);
    window.addEventListener("storage", update);
    return () => { window.removeEventListener("router-auth-changed", update); window.removeEventListener("storage", update); };
  }, []);
  return actor;
}

export type MarketValueReadinessState = {
  readiness?: MarketValueQuotaReadiness;
  loading: boolean;
  retry: () => void;
};

export function useMarketValueQuotaReadiness(shareId: string | undefined): MarketValueReadinessState {
  const actor = useQuotaActor();
  const key = `${actor}:${shareId ?? ""}`;
  const [result, setResult] = React.useState<{ key: string; readiness: MarketValueQuotaReadiness } | null>(null);
  const [attempt, setAttempt] = React.useState(0);
  React.useEffect(() => {
    if (!shareId) return;
    let active = true;
    let pending = false;
    const refresh = (force = false) => {
      if (pending || document.visibilityState === "hidden" || actor !== valueReadinessActor()) return;
      pending = true;
      void loadReadiness(shareId, actor, force).then((readiness) => {
        if (active && actor === valueReadinessActor()) setResult({ key, readiness });
      }).finally(() => { pending = false; });
    };
    refresh(attempt > 0);
    const visible = () => { if (document.visibilityState === "visible") refresh(); };
    document.addEventListener("visibilitychange", visible);
    const timer = window.setInterval(() => refresh(), READINESS_TTL_MS);
    return () => { active = false; window.clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, [shareId, attempt, actor, key]);
  const readiness = result?.key === key && actor === valueReadinessActor() ? result.readiness : undefined;
  return {
    readiness,
    loading: !!shareId && !readiness,
    retry: () => setAttempt((value) => value + 1),
  };
}

const CHECK_REASONS = ["router_protocol", "sqlite_authority", "metering_storage", "tariff_published", "acknowledged_tariff", "text_surface", "model_price_coverage"];
const REQUEST_REASONS: Record<string, string> = {
  quota_server_offline: "offline",
  quota_server_timeout: "timeout",
  quota_server_upgrade_required: "upgrade",
  quota_control_unauthorized: "controlAuth",
  quota_server_invalid_response: "invalidResponse",
  quota_revision_conflict: "conflict",
};

/** Short reason plus, for Server checks, the owner-facing fix. */
export function marketValueReadinessReason(reason: string | undefined, t: (key: MessageKey) => string) {
  if (reason === "server_upgrade_required" || reason === "server_trial_upgrade_required") {
    return { short: t(reason === "server_trial_upgrade_required" ? "shareMarket.quota.usdTrialUpgrade" : "shareMarket.quota.usdServerUpgrade"), detail: undefined };
  }
  if (reason && CHECK_REASONS.includes(reason)) {
    return {
      short: t(`dashboard.userLimit.value.readiness.short.${reason}` as MessageKey),
      detail: t(`dashboard.userLimit.value.readiness.${reason}` as MessageKey),
    };
  }
  const request = reason ? REQUEST_REASONS[reason] : undefined;
  return {
    short: t(`dashboard.userLimit.value.readiness.short.${request ?? "failed"}` as MessageKey),
    detail: request ? t(`dashboard.userLimit.value.readiness.${request}` as MessageKey) : undefined,
  };
}

/**
 * Why `$` is off for this Share, with a way to check again. `quiet` is the
 * one-line form shown while the seat still sells Tokens.
 */
export function MarketValueReadinessLine({ id, state, quiet = false }: {
  id: string;
  state: MarketValueReadinessState;
  quiet?: boolean;
}) {
  const { t } = useLocaleText();
  if (state.loading && quiet) return null;
  if (state.loading) {
    return (
      <p id={id} role="status" className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        {t("shareMarket.quota.usdChecking")}
      </p>
    );
  }
  if (!state.readiness || state.readiness.ready) return null;
  const reason = marketValueReadinessReason(state.readiness.reason, t);
  if (quiet) {
    return (
      <p id={id} className="flex flex-wrap items-center gap-x-2 text-xs leading-5 text-muted-foreground">
        <span>{t("shareMarket.quota.usdUnavailable", { reason: reason.short })}</span>
        <button
          type="button"
          onClick={state.retry}
          className="inline-flex min-h-8 items-center gap-1 rounded-md font-medium text-foreground outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
        >
          <RotateCw className="h-3 w-3" aria-hidden="true" />
          {t("dashboard.userLimit.value.readiness.retry")}
        </button>
      </p>
    );
  }
  return (
    <div id={id} className="grid gap-1 rounded-lg border border-amber-200 bg-amber-50/70 px-3 py-2 text-xs leading-5 text-amber-900">
      <span className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium">{t("shareMarket.quota.usdUnavailable", { reason: reason.short })}</span>
        <button
          type="button"
          onClick={state.retry}
          className="inline-flex min-h-8 items-center gap-1 rounded-md px-2 font-medium text-amber-900 outline-none transition-colors hover:bg-amber-100 focus-visible:ring-2 focus-visible:ring-ring"
        >
          <RotateCw className="h-3.5 w-3.5" aria-hidden="true" />
          {t("dashboard.userLimit.value.readiness.retry")}
        </button>
      </span>
      {reason.detail ? <span className="text-amber-800">{reason.detail}</span> : null}
    </div>
  );
}

/** What a USD seat means, said where the seller sets it. */
export function UsdSeatTerms({ parallelLimit }: { parallelLimit: string }) {
  const { t } = useLocaleText();
  const parallel = Number(parallelLimit);
  const capped = !parallelLimit.trim() || !Number.isFinite(parallel) || parallel > USD_SEAT_PARALLEL_CAP;
  return (
    <div className="grid gap-1 text-xs leading-5 text-muted-foreground">
      <p>{t("shareMarket.quota.usdHint")}</p>
      {capped ? (
        <p className="text-amber-800">{t("shareMarket.quota.usdParallel", { cap: USD_SEAT_PARALLEL_CAP })}</p>
      ) : null}
    </div>
  );
}

/** The USD-equivalent terms a buyer accepts, spelled out before paying. */
type DiscountSeat = Parameters<typeof seatValueDiscount>[0];

/** Rent against full-use API value; always labelled as a 30-day estimate. */
function discountText(seat: DiscountSeat, locale: string, t: ReturnType<typeof useLocaleText>["t"]) {
  const discount = seatValueDiscount(seat);
  if (!discount) return null;
  const ratio = formatSeatValueRatio(discount, locale);
  const label = formatSeatValueDiscount(discount, locale);
  return {
    label,
    text: label
      ? t("shareMarket.discount.rentLine", { ratio, value: formatUsdLimit(discount.monthlyValueMicros) })
      : t("shareMarket.discount.aboveList", { ratio }),
  };
}

/** Compact badge under a seat's terms in the buyer's seat list. */
export function SeatDiscountNote({ seat }: { seat: DiscountSeat }) {
  const { locale, t } = useLocaleText();
  const discount = discountText(seat, locale, t);
  if (!discount?.label) return null;
  return (
    <span className="mt-0.5 block font-medium text-primary" title={discount.text}>
      {t("shareMarket.discount.badge", { discount: discount.label })}
    </span>
  );
}

function SeatDiscountItem({ seat }: { seat: DiscountSeat }) {
  const { locale, t } = useLocaleText();
  const discount = discountText(seat, locale, t);
  return discount ? <li>{discount.text}</li> : null;
}

/** Live seller preview of what the price means against the quota's API value. */
export function SellerDiscountPreview({ seat }: { seat: DiscountSeat | null }) {
  const { locale, t } = useLocaleText();
  if (!seat) return null;
  if (seat.valueQuota && seat.tokenPeriod === "lifetime") {
    return <p className="text-xs leading-5 text-muted-foreground">{t("shareMarket.discount.cumulative")}</p>;
  }
  const discount = seatValueDiscount(seat);
  if (!discount) return null;
  const ratio = formatSeatValueRatio(discount, locale);
  return discount.ratioBps >= 10_000 ? (
    <p className="text-xs leading-5 text-amber-800">{t("shareMarket.discount.sellerAbove", { ratio })}</p>
  ) : (
    <p className="text-xs leading-5 text-primary">
      {t("shareMarket.discount.sellerPreview", { ratio, value: formatUsdLimit(discount.monthlyValueMicros) })}
    </p>
  );
}

export function UsdSeatRentTerms({ offer }: { offer: ShareMarketRentQuote["offer"] }) {
  const { t } = useLocaleText();
  if (!offer.valueQuota) return null;
  return (
    <section className="grid gap-1.5 rounded-lg border border-primary/15 bg-primary/5 px-3 py-3 text-xs leading-5">
      <h3 className="text-sm font-semibold text-foreground">
        {t("shareMarket.rentConfirm.usdTitle", {
          amount: formatUsdLimit(offer.valueQuota.limitUsdMicros),
          period: t(`shareMarket.period.${offer.tokenPeriod}`),
        })}
      </h3>
      <ul className="grid list-disc gap-0.5 pl-4 text-muted-foreground">
        <li>{t("shareMarket.rentConfirm.usdPricing")}</li>
        <li>{t("shareMarket.rentConfirm.usdSoftCap")}</li>
        <li>{t("shareMarket.rentConfirm.usdParallel", { cap: USD_SEAT_PARALLEL_CAP })}</li>
        <SeatDiscountItem seat={{ ...offer, isFree: offer.cyclePriceMinor == null && offer.dailyRateMinor == null }} />
        {offer.trialValueLimitUsdMicros && offer.trialValueLimitUsdMicros !== "0" ? <li>{t("shareMarket.rentConfirm.usdValueTrial")}</li>
          : offer.trialTokenLimit ? <li>{t("shareMarket.rentConfirm.usdTrial")}</li> : null}
      </ul>
    </section>
  );
}

/** The realized value of a paid rental this period; null until known or when not applicable. */
export function useRentalValueUsage(subscriptionId: string | undefined, enabled: boolean, refreshKey?: string) {
  const actor = useQuotaActor();
  const key = `${actor}:${subscriptionId ?? ""}:${refreshKey ?? ""}`;
  const [usage, setUsage] = React.useState<{ subscriptionId: string; usage: RentalValueUsage } | null>(null);
  React.useEffect(() => {
    if (!subscriptionId || !enabled) return;
    const controller = new AbortController();
    void getRentalValueUsage(subscriptionId, controller.signal)
      .then((next) => { if (!controller.signal.aborted && actor === valueReadinessActor()) setUsage({ subscriptionId, usage: next }); })
      .catch(() => undefined);
    return () => controller.abort();
  }, [subscriptionId, enabled, refreshKey, actor, key]);
  return enabled && usage?.subscriptionId === subscriptionId && actor === valueReadinessActor() ? usage.usage : null;
}

/**
 * "US$45 API value used · rent ≈ 44% of API price", with the estimate's
 * limits spelled out. Null when there is nothing honest to say.
 */
export function realizedValueText(usage: RentalValueUsage | null, locale: string, t: ReturnType<typeof useLocaleText>["t"]) {
  if (!usage || usage.reason === "trial" || usage.reason === "no_paid_period") return null;
  if (!usage.valueUsedMicros || usage.ratioBps == null) {
    const text = usage.reason === "unpriced_usage" ? t("shareMarket.realized.unpriced")
      : usage.valueUsedMicros != null ? t("shareMarket.realized.valueOnly", { value: formatUsdLimit(usage.valueUsedMicros) })
      : t("shareMarket.realized.noUsage");
    return usage.partial ? `${text} ${t("shareMarket.realized.partial")}` : text;
  }
  const discount = { ratioBps: usage.ratioBps, monthlyValueMicros: usage.valueUsedMicros, monthlyPriceMinor: 0 };
  const label = formatSeatValueDiscount(discount, locale);
  const text = t(label ? "shareMarket.realized.discount" : "shareMarket.realized.ratio", {
    value: formatUsdLimit(usage.valueUsedMicros),
    ratio: formatSeatValueRatio(discount, locale),
    discount: label ?? "",
  });
  return usage.partial ? `${text} ${t("shareMarket.realized.partial")}` : text;
}

/** One compact line in the seller's seat table for a paid, rented seat. */
export function SellerRealizedValue({ subscriptionId, refreshKey }: { subscriptionId: string; refreshKey?: string }) {
  const { locale, t } = useLocaleText();
  const text = realizedValueText(useRentalValueUsage(subscriptionId, true, refreshKey), locale, t);
  if (!text) return null;
  return <span className="text-[11px] text-slate-500">{t("shareMarket.realized.sellerLine", { text })}</span>;
}
