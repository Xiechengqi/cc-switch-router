"use client";

import { Clock3, HandCoins, Loader2 } from "lucide-react";
import { ReleaseRentalAction } from "@/components/dashboard/client-market/release-rental-action";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_ACCOUNT_BILLING_PATH } from "@/lib/dashboard-nav";
import type { ClientMarketRental } from "@/lib/types";

/** Client lifecycle status plus a single link to supplier-level postpaid billing. */
export function ClientMarketRentalBanner({
  rental,
  onChanged,
  compact = false,
  resumeRelease = true,
  readOnly = false,
  manageHref,
}: {
  rental?: ClientMarketRental;
  onChanged: () => Promise<void> | void;
  compact?: boolean;
  resumeRelease?: boolean;
  readOnly?: boolean;
  manageHref?: string;
}) {
  const { locale, t } = useLocaleText();
  if (!rental || !rental.isClientOwner || rental.status === "released") return null;

  if (rental.status === "releasing") {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <Loader2 className="h-3 w-3 animate-spin" />
        {t("clientMarket.release.releasing")}
        {!readOnly && resumeRelease ? <ReleaseRentalAction rental={rental} onChanged={onChanged} /> : null}
      </span>
    );
  }

  if (rental.status === "release_failed") {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <span className="text-[11px] text-rose-700">{t("clientMarket.release.failedHint")}</span>
        {!readOnly ? (
          <ReleaseRentalAction rental={rental} onChanged={onChanged} label={t("clientMarket.release.retry")} className="h-6 px-2 text-[11px] text-rose-700" />
        ) : manageHref ? (
          <a href={manageHref} className="text-[11px] font-medium text-accent hover:underline">{t("clientMarket.release.manageInMarket")}</a>
        ) : null}
      </span>
    );
  }

  if (!rental.dailyRateMinor) {
    const activated = rental.activatedAt
      ? new Date(rental.activatedAt).toLocaleString(locale)
      : null;
    const expires = rental.expiresAt ? new Date(rental.expiresAt) : null;
    const remainingMs = expires ? expires.getTime() - Date.now() : null;
    const remaining = remainingMs == null
      ? t("clientMarket.freeDuration.permanent")
      : remainingMs <= 0
        ? t("clientMarket.freeDuration.expired")
        : remainingMs < 48 * 60 * 60 * 1000
          ? t("clientMarket.freeDuration.remainingHours", { count: Math.ceil(remainingMs / 3_600_000) })
          : t("clientMarket.freeDuration.remainingDays", { count: Math.ceil(remainingMs / 86_400_000) });
    const detail = activated
      ? expires
        ? `${t("clientMarket.freeDuration.activated")}: ${activated} · ${t("clientMarket.freeDuration.expires")}: ${expires.toLocaleString(locale)} · ${remaining}`
        : `${t("clientMarket.freeDuration.activated")}: ${activated} · ${remaining}`
      : t("clientMarket.freeDuration.pendingActivation");
    return (
      <span
        className={`inline-flex items-center gap-1 text-muted-foreground ${compact ? "text-[11px]" : "text-xs"}`}
        title={detail}
      >
        <Clock3 className="h-3 w-3" />
        {compact ? `${t("clientMarket.free")} · ${remaining}` : detail}
      </span>
    );
  }
  return (
    <a
      href={DASHBOARD_ACCOUNT_BILLING_PATH}
      className={`inline-flex items-center gap-1 rounded-full border border-slate-200 bg-white font-medium text-slate-700 hover:border-slate-300 hover:bg-slate-50 ${compact ? "h-6 px-2 text-[11px]" : "h-7 px-2.5 text-xs"}`}
      data-no-row-drawer
      onClick={(event) => event.stopPropagation()}
    >
      <HandCoins className="h-3 w-3" />
      {t("marketBilling.open")}
    </a>
  );
}
