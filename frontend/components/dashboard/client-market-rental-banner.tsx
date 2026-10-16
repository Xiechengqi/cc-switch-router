"use client";

import { CalendarClock, Clock3, HandCoins, Loader2 } from "lucide-react";
import {
  FinalizeFailedRentalAction,
  ReleaseRentalAction,
} from "@/components/dashboard/client-market/release-rental-action";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_ACCOUNT_BILLING_PATH, DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { clientMarketCleanupReasonKey, clientMarketRentalHasBanner } from "@/lib/client-market-rental-state";
import { cleanupFailureGuidanceKey } from "@/components/dashboard/client-market/host-utils";
import type { ClientMarketRental } from "@/lib/types";

/** Client lifecycle status plus a single link to supplier-level postpaid billing. */
export function ClientMarketRentalBanner({
  rental,
  onChanged,
  compact = false,
  resumeRelease = true,
  readOnly = false,
  manageHref,
  showManageLink = true,
  showSchedule = true,
}: {
  rental?: ClientMarketRental;
  onChanged: () => Promise<void> | void;
  compact?: boolean;
  resumeRelease?: boolean;
  readOnly?: boolean;
  manageHref?: string;
  showManageLink?: boolean;
  /** Free-duration schedule is verbose; list rows hide it and surface it in the client drawer instead. */
  showSchedule?: boolean;
}) {
  const { locale, t } = useLocaleText();
  if (!clientMarketRentalHasBanner(rental)) return null;
  if (!rental) return null;
  const nextProbe = rental.nextCleanupProbeAt
    ? new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(new Date(rental.nextCleanupProbeAt))
    : null;
  const probeStopped = !nextProbe && (rental.cleanupProbeAttempts ?? 0) >= 5;

  if (rental.status === "released" && rental.hostCleanupPending) {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5 text-[11px] text-amber-800" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        {rental.activeCleanupJobId ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
        <span>{t(rental.activeCleanupJobId ? "clientMarket.release.hostCleaning" : "clientMarket.release.hostPending")}</span>
        {!rental.activeCleanupJobId && rental.cleanupFailureKind ? <span>{t(clientMarketCleanupReasonKey(rental.cleanupFailureKind))}</span> : null}
        {nextProbe ? <span>{t("clientMarket.release.nextProbe", { time: nextProbe })}</span> : null}
        {probeStopped ? <span>{t("clientMarket.release.probeStopped")}</span> : null}
      </span>
    );
  }

  if (rental.status === "releasing") {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <Loader2 className="h-3 w-3 animate-spin" />
        {t("clientMarket.release.releasing")}
        {!readOnly && resumeRelease ? <ReleaseRentalAction rental={rental} onChanged={onChanged} /> : null}
        {!readOnly ? <FinalizeFailedRentalAction rental={rental} onChanged={onChanged} /> : null}
      </span>
    );
  }

  if (rental.status === "release_failed") {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <span className="text-[11px] text-rose-700">
          {rental.cleanupFailureKind
            ? t(cleanupFailureGuidanceKey(rental.cleanupFailureKind))
            : t("clientMarket.release.failedHint")}
        </span>
        {nextProbe ? <span className="text-[11px] text-muted-foreground">{t("clientMarket.release.nextProbe", { time: nextProbe })}</span> : null}
        {probeStopped ? <span className="text-[11px] text-muted-foreground">{t("clientMarket.release.probeStopped")}</span> : null}
        {!readOnly ? (
          <ReleaseRentalAction rental={rental} onChanged={onChanged} label={t("clientMarket.release.retry")} className="h-6 px-2 text-[11px] text-rose-700" />
        ) : showManageLink && manageHref ? (
          <a href={manageHref} className="text-[11px] font-medium text-accent hover:underline">{t("clientMarket.release.manageInMarket")}</a>
        ) : null}
        {!readOnly ? <FinalizeFailedRentalAction rental={rental} onChanged={onChanged} /> : null}
      </span>
    );
  }

  if (rental.pricingModel === "prepaid_calendar_month" && rental.recurring) {
    const contract = rental.recurring;
    const formatTime = (value?: string) => {
      if (!value) return "—";
      const parsed = Date.parse(value);
      return Number.isFinite(parsed)
        ? new Intl.DateTimeFormat(locale, {
            dateStyle: "medium",
            timeStyle: "short",
          }).format(new Date(parsed))
        : value;
    };
    const lifecycle = contract.cancelAtPeriodEnd
      ? t("marketRecurring.compact.cancelAt", { time: formatTime(contract.currentPeriodEnd) })
      : contract.status === "recovery"
        ? t("marketRecurring.compact.recoveryBy", { time: formatTime(contract.recoveryDeadline) })
        : contract.status === "trial"
          ? t("marketRecurring.compact.trialUntil", { time: formatTime(contract.trialEndsAt) })
          : t("marketRecurring.compact.renewsAt", { time: formatTime(contract.currentPeriodEnd) });
    return (
      <span
        className={`inline-flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-muted-foreground ${compact ? "text-[11px]" : "text-xs"}`}
        data-no-row-drawer
        onClick={(event) => event.stopPropagation()}
      >
        <CalendarClock className="h-3 w-3 shrink-0" />
        <strong className="font-medium text-foreground">
          {formatUsdMoney(contract.cyclePriceMinor, locale)} / {t("marketBilling.month")}
        </strong>
        <span>{lifecycle}</span>
        {showManageLink ? (
          <a
            href={manageHref || DASHBOARD_ACCOUNT_BILLING_PATH}
            className="font-medium text-accent hover:underline"
          >
            {manageHref ? t("clientMarket.release.manageInMarket") : t("marketBilling.open")}
          </a>
        ) : null}
      </span>
    );
  }

  if (rental.pricingModel === "free" || (rental.dailyRateMinor == null && rental.cyclePriceMinor == null)) {
    if (!showSchedule) return null;
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
        {compact
          ? activated
            ? remaining
            : t("clientMarket.freeDuration.pendingActivation")
          : detail}
      </span>
    );
  }
  return (
    <a
      href={rental.walletContractId ? DASHBOARD_ACCOUNT_WALLET_PATH : DASHBOARD_ACCOUNT_BILLING_PATH}
      className={`inline-flex items-center gap-1 rounded-full border border-slate-200 bg-white font-medium text-slate-700 hover:border-slate-300 hover:bg-slate-50 ${compact ? "h-6 px-2 text-[11px]" : "h-7 px-2.5 text-xs"}`}
      data-no-row-drawer
      onClick={(event) => event.stopPropagation()}
    >
      <HandCoins className="h-3 w-3" />
      {t(rental.walletContractId ? "account.nav.wallet" : "marketBilling.open")}
    </a>
  );
}
